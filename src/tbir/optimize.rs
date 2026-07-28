use crate::{
    ast::{
        AccessPath, AccessSegment, AssignOp, BinaryOp, Declaration, Expr, Function, Place, Program,
        Stmt, Type, UnaryOp,
    },
    compat::prelude::*,
    target::CpuFamily,
};

use super::{
    TbirOptimizationDecision, TbirOptimizationKind, TbirOptimizationOutcome, TbirOptimizationReport,
};

pub fn optimize_program(program: &Program, cpu: CpuFamily) -> (Program, TbirOptimizationReport) {
    let mut program = program.clone();
    let mut report = TbirOptimizationReport::default();
    for declaration in &mut program.declarations {
        optimize_declaration(declaration, &mut report);
    }
    decide_inline_functions(&program, &mut report);
    decide_and_rewrite_tail_calls(&mut program, cpu, &mut report);
    (program, report)
}

fn functions(program: &Program) -> Vec<&Function> {
    fn collect<'a>(declarations: &'a [Declaration], output: &mut Vec<&'a Function>) {
        for declaration in declarations {
            match declaration {
                Declaration::Function(function) => output.push(function),
                Declaration::Cfg { declaration, .. } | Declaration::Bank { declaration, .. } => {
                    collect(core::slice::from_ref(declaration), output)
                }
                _ => {}
            }
        }
    }
    let mut output = Vec::new();
    collect(&program.declarations, &mut output);
    output
}

fn decide_inline_functions(program: &Program, report: &mut TbirOptimizationReport) {
    let functions = functions(program);
    let mut graph = HashMap::new();
    for function in &functions {
        let mut calls = HashSet::new();
        collect_calls(&function.body, &mut calls);
        graph.insert(function.name.clone(), calls);
    }
    for function in functions {
        if !has_attr(function, "inline") {
            continue;
        }
        let reason = if has_attr(function, "naked") {
            Some("naked function")
        } else if has_attr(function, "interrupt") {
            Some("interrupt function")
        } else if graph
            .get(&function.name)
            .is_some_and(|calls| calls.contains(&function.name))
        {
            Some("direct recursion")
        } else if graph.get(&function.name).is_some_and(|calls| {
            calls
                .iter()
                .any(|callee| reachable(callee, &function.name, &graph, &mut HashSet::new()))
        }) {
            Some("mutual recursion")
        } else if inline_return_body(function).is_none() && inline_void_body(function).is_none() {
            Some("unsupported body shape")
        } else {
            None
        };
        decision(
            report,
            TbirOptimizationKind::Inline,
            None,
            &function.name,
            reason,
        );
    }
}

fn reachable(
    current: &str,
    target: &str,
    graph: &HashMap<String, HashSet<String>>,
    visited: &mut HashSet<String>,
) -> bool {
    if current == target {
        return true;
    }
    if !visited.insert(current.to_owned()) {
        return false;
    }
    graph.get(current).is_some_and(|next| {
        next.iter()
            .any(|name| reachable(name, target, graph, visited))
    })
}

fn collect_calls(stmts: &[Stmt], calls: &mut HashSet<String>) {
    for stmt in stmts {
        match stmt {
            Stmt::Let { value, .. }
            | Stmt::Assign { value, .. }
            | Stmt::Return(Some(value))
            | Stmt::Out { value, .. }
            | Stmt::Expr(value) => collect_expr_calls(value, calls),
            Stmt::If {
                condition,
                then_body,
                else_body,
            } => {
                collect_expr_calls(condition, calls);
                collect_calls(then_body, calls);
                collect_calls(else_body, calls);
            }
            Stmt::While { condition, body } => {
                collect_expr_calls(condition, calls);
                collect_calls(body, calls);
            }
            Stmt::Loop { body } => collect_calls(body, calls),
            _ => {}
        }
    }
}

fn collect_expr_calls(expr: &Expr, calls: &mut HashSet<String>) {
    match expr {
        Expr::Call { path, args } => {
            if let Some(name) = path.last() {
                calls.insert(name.clone());
            }
            for arg in args {
                collect_expr_calls(arg, calls);
            }
        }
        Expr::Array(values) => {
            for value in values {
                collect_expr_calls(value, calls);
            }
        }
        Expr::Index { index, .. }
        | Expr::AddressOfIndex { index, .. }
        | Expr::Deref(index)
        | Expr::BankedPointer { pointer: index, .. }
        | Expr::Unary { expr: index, .. }
        | Expr::Cast { expr: index, .. } => collect_expr_calls(index, calls),
        Expr::Access(path) | Expr::AddressOfAccess(path) => {
            for segment in &path.segments {
                if let AccessSegment::Index(index) = segment {
                    collect_expr_calls(index, calls);
                }
            }
        }
        Expr::StructInit { fields, .. } => {
            for (_, value) in fields {
                collect_expr_calls(value, calls);
            }
        }
        Expr::Binary { left, right, .. } => {
            collect_expr_calls(left, calls);
            collect_expr_calls(right, calls);
        }
        _ => {}
    }
}

#[derive(Clone)]
struct FunctionFacts {
    attrs: Vec<String>,
    params: Vec<crate::ast::Param>,
    return_type: Option<Type>,
}

fn decide_and_rewrite_tail_calls(
    program: &mut Program,
    cpu: CpuFamily,
    report: &mut TbirOptimizationReport,
) {
    let facts: HashMap<String, FunctionFacts> = functions(program)
        .into_iter()
        .map(|function| {
            (
                function.name.clone(),
                FunctionFacts {
                    attrs: function.attrs.clone(),
                    params: function.params.clone(),
                    return_type: function.return_type.clone(),
                },
            )
        })
        .collect();
    decide_tail_calls_in_declarations(&mut program.declarations, cpu, &facts, report);
}

fn decide_tail_calls_in_declarations(
    declarations: &mut [Declaration],
    cpu: CpuFamily,
    facts: &HashMap<String, FunctionFacts>,
    report: &mut TbirOptimizationReport,
) {
    for declaration in declarations {
        match declaration {
            Declaration::Cfg { declaration, .. } | Declaration::Bank { declaration, .. } => {
                decide_tail_calls_in_declarations(
                    core::slice::from_mut(declaration),
                    cpu,
                    facts,
                    report,
                )
            }
            Declaration::Function(function) => {
                let mut targets = Vec::new();
                collect_tail_targets(&function.body, &mut targets);
                let mut seen = HashSet::new();
                for callee in targets {
                    if !seen.insert(callee.clone()) {
                        continue;
                    }
                    if callee == function.name {
                        let reason = tail_recursion_rejection(function);
                        decision(
                            report,
                            TbirOptimizationKind::TailRecursion,
                            Some(&function.name),
                            &callee,
                            reason,
                        );
                        if reason.is_none() {
                            rewrite_tail_recursion(function);
                        }
                    } else {
                        let reason = sibling_tail_rejection(function, &callee, cpu, facts);
                        decision(
                            report,
                            TbirOptimizationKind::TailCall,
                            Some(&function.name),
                            &callee,
                            reason,
                        );
                    }
                }
            }
            _ => {}
        }
    }
}

fn collect_tail_targets(stmts: &[Stmt], targets: &mut Vec<String>) {
    for stmt in stmts {
        match stmt {
            Stmt::Return(Some(Expr::Call { path, .. })) => {
                if let Some(name) = path.last() {
                    targets.push(name.clone());
                }
            }
            Stmt::If {
                then_body,
                else_body,
                ..
            } => {
                collect_tail_targets(then_body, targets);
                collect_tail_targets(else_body, targets);
            }
            Stmt::While { .. } | Stmt::Loop { .. } => {}
            _ => {}
        }
    }
}

fn tail_recursion_rejection(function: &Function) -> Option<&'static str> {
    if has_attr(function, "naked") {
        Some("naked function")
    } else if has_attr(function, "interrupt") {
        Some("interrupt function")
    } else if function.params.len() > 3 {
        Some("too many parameters")
    } else if uses_argument_slots(&function.params) {
        Some("argument slots required")
    } else {
        None
    }
}

fn sibling_tail_rejection(
    caller: &Function,
    callee: &str,
    cpu: CpuFamily,
    facts: &HashMap<String, FunctionFacts>,
) -> Option<&'static str> {
    if !matches!(
        cpu,
        CpuFamily::Ez80 | CpuFamily::Z80 | CpuFamily::Z80N | CpuFamily::Z180
    ) {
        return Some("target does not support sibling tail calls");
    }
    if has_attr(caller, "naked") {
        return Some("naked caller");
    }
    if has_attr(caller, "interrupt") {
        return Some("interrupt caller");
    }
    let Some(callee) = facts.get(callee) else {
        return Some("callee is not a function");
    };
    if callee.attrs.iter().any(|attr| attr == "naked") {
        return Some("naked callee");
    }
    if callee.attrs.iter().any(|attr| attr == "interrupt") {
        return Some("interrupt callee");
    }
    if caller.return_type != callee.return_type {
        return Some("return type mismatch");
    }
    if caller.params.len() > 3 || callee.params.len() > 3 {
        return Some("too many parameters");
    }
    if uses_argument_slots(&caller.params) || uses_argument_slots(&callee.params) {
        return Some("argument slots required");
    }
    None
}

fn uses_argument_slots(params: &[crate::ast::Param]) -> bool {
    params.len() >= 3 && is_byte_type(&params[1].ty) && !is_byte_type(&params[2].ty)
}

fn is_byte_type(ty: &Type) -> bool {
    matches!(
        ty,
        Type::Named(name) if matches!(name.as_str(), "u8" | "i8" | "bool" | "char")
    )
}

fn rewrite_tail_recursion(function: &mut Function) {
    let mut used_names = HashSet::new();
    for param in &function.params {
        used_names.insert(param.name.clone());
    }
    collect_local_names(&function.body, &mut used_names);
    let mut next_temp = 0usize;
    let (body, rewritten) = rewrite_tail_stmts(
        core::mem::take(&mut function.body),
        &function.name,
        &function.params,
        &mut used_names,
        &mut next_temp,
    );
    function.body = if rewritten {
        vec![Stmt::Loop { body }]
    } else {
        body
    };
}

fn rewrite_tail_stmts(
    stmts: Vec<Stmt>,
    function_name: &str,
    params: &[crate::ast::Param],
    used_names: &mut HashSet<String>,
    next_temp: &mut usize,
) -> (Vec<Stmt>, bool) {
    let mut output = Vec::new();
    let mut any_rewritten = false;
    for stmt in stmts {
        match stmt {
            Stmt::Return(Some(Expr::Call { path, args }))
                if path.last().is_some_and(|name| name == function_name) =>
            {
                let mut temps = Vec::new();
                for (arg, param) in args.into_iter().zip(params) {
                    let name = unique_temp_name(used_names, next_temp);
                    output.push(Stmt::Let {
                        name: name.clone(),
                        ty: param.ty.clone(),
                        value: arg,
                    });
                    temps.push(name);
                }
                for (param, temp) in params.iter().zip(temps) {
                    output.push(Stmt::Assign {
                        target: Place::Ident(param.name.clone()),
                        op: AssignOp::Set,
                        value: Expr::Ident(temp),
                    });
                }
                output.push(Stmt::Continue);
                any_rewritten = true;
            }
            Stmt::If {
                condition,
                then_body,
                else_body,
            } => {
                let (then_body, then_rewritten) =
                    rewrite_tail_stmts(then_body, function_name, params, used_names, next_temp);
                let (else_body, else_rewritten) =
                    rewrite_tail_stmts(else_body, function_name, params, used_names, next_temp);
                any_rewritten |= then_rewritten || else_rewritten;
                output.push(Stmt::If {
                    condition,
                    then_body,
                    else_body,
                });
            }
            stmt => output.push(stmt),
        }
    }
    (output, any_rewritten)
}

fn unique_temp_name(used_names: &mut HashSet<String>, next_temp: &mut usize) -> String {
    loop {
        let name = format!("__tbir_tail_arg_{}", *next_temp);
        *next_temp += 1;
        if used_names.insert(name.clone()) {
            return name;
        }
    }
}

fn collect_local_names(stmts: &[Stmt], names: &mut HashSet<String>) {
    for stmt in stmts {
        match stmt {
            Stmt::Let { name, .. } => {
                names.insert(name.clone());
            }
            Stmt::If {
                then_body,
                else_body,
                ..
            } => {
                collect_local_names(then_body, names);
                collect_local_names(else_body, names);
            }
            Stmt::While { body, .. } | Stmt::Loop { body } => collect_local_names(body, names),
            _ => {}
        }
    }
}

fn decision(
    report: &mut TbirOptimizationReport,
    kind: TbirOptimizationKind,
    caller: Option<&str>,
    callee: &str,
    rejection: Option<&str>,
) {
    report.decisions.push(TbirOptimizationDecision {
        kind,
        caller: caller.map(str::to_owned),
        callee: callee.to_owned(),
        outcome: if rejection.is_some() {
            TbirOptimizationOutcome::Rejected
        } else {
            TbirOptimizationOutcome::Applied
        },
        reason: rejection.unwrap_or("approved").to_owned(),
    });
}

fn has_attr(function: &Function, attr: &str) -> bool {
    function.attrs.iter().any(|candidate| candidate == attr)
}

fn inline_return_body(function: &Function) -> Option<(&[Stmt], Expr)> {
    let (last, prefix) = function.body.split_last()?;
    if prefix.iter().any(stmt_contains_return) {
        return None;
    }
    match last {
        Stmt::Return(Some(expr)) => Some((prefix, expr.clone())),
        _ => None,
    }
}

fn inline_void_body(function: &Function) -> Option<&[Stmt]> {
    if function.return_type.is_some() {
        return None;
    }
    match function.body.split_last() {
        Some((Stmt::Return(None), prefix)) if !prefix.iter().any(stmt_contains_return) => {
            Some(prefix)
        }
        _ if !function.body.iter().any(stmt_contains_return) => Some(&function.body),
        _ => None,
    }
}

fn stmt_contains_return(stmt: &Stmt) -> bool {
    match stmt {
        Stmt::Return(_) => true,
        Stmt::If {
            then_body,
            else_body,
            ..
        } => {
            then_body.iter().any(stmt_contains_return) || else_body.iter().any(stmt_contains_return)
        }
        Stmt::While { body, .. } | Stmt::Loop { body } => body.iter().any(stmt_contains_return),
        _ => false,
    }
}

fn optimize_declaration(declaration: &mut Declaration, report: &mut TbirOptimizationReport) {
    match declaration {
        Declaration::Cfg { declaration, .. } | Declaration::Bank { declaration, .. } => {
            optimize_declaration(declaration, report)
        }
        Declaration::Function(function) => optimize_function(function, report),
        Declaration::Const(decl) => {
            decl.value = optimize_expr(
                core::mem::replace(&mut decl.value, Expr::Int(0)),
                &HashMap::new(),
                report,
            )
        }
        Declaration::Port(decl) => {
            decl.value = optimize_expr(
                core::mem::replace(&mut decl.value, Expr::Int(0)),
                &HashMap::new(),
                report,
            )
        }
        Declaration::Mmio(decl) => {
            decl.value = optimize_expr(
                core::mem::replace(&mut decl.value, Expr::Int(0)),
                &HashMap::new(),
                report,
            )
        }
        Declaration::Global(decl) => {
            decl.value = optimize_expr(
                core::mem::replace(&mut decl.value, Expr::Int(0)),
                &HashMap::new(),
                report,
            )
        }
        Declaration::Embed(_)
        | Declaration::Import(_)
        | Declaration::Alias(_)
        | Declaration::Struct(_)
        | Declaration::ExternAsmFunction(_) => {}
    }
}

fn optimize_function(function: &mut Function, report: &mut TbirOptimizationReport) {
    let mut constants = HashMap::new();
    function.body = optimize_stmts(core::mem::take(&mut function.body), &mut constants, report);
}

fn optimize_stmts(
    stmts: Vec<Stmt>,
    constants: &mut HashMap<String, Expr>,
    report: &mut TbirOptimizationReport,
) -> Vec<Stmt> {
    let mut output = Vec::with_capacity(stmts.len());
    let mut terminated = false;
    for stmt in stmts {
        if terminated {
            report.dead_statements_marked += 1;
        }
        let stmt = optimize_stmt(stmt, constants, report);
        terminated |= terminates(&stmt);
        output.push(stmt);
    }
    output
}

fn optimize_stmt(
    stmt: Stmt,
    constants: &mut HashMap<String, Expr>,
    report: &mut TbirOptimizationReport,
) -> Stmt {
    match stmt {
        Stmt::Let { name, ty, value } => {
            let value = optimize_expr(value, constants, report);
            // Locals can be mutated indirectly or by a loop body. Substitution needs
            // alias and control-flow analysis, so only fold the initializer for now.
            constants.remove(&name);
            Stmt::Let { name, ty, value }
        }
        Stmt::Assign { target, op, value } => {
            let target = optimize_place(target, constants, report);
            let value = optimize_expr(value, constants, report);
            constants.clear();
            Stmt::Assign { target, op, value }
        }
        Stmt::If {
            condition,
            then_body,
            else_body,
        } => {
            let condition = optimize_expr(condition, constants, report);
            let mut then_constants = constants.clone();
            let mut else_constants = constants.clone();
            let then_body = optimize_stmts(then_body, &mut then_constants, report);
            let else_body = optimize_stmts(else_body, &mut else_constants, report);
            constants.clear();
            Stmt::If {
                condition,
                then_body,
                else_body,
            }
        }
        Stmt::While { condition, body } => {
            let condition = optimize_expr(condition, constants, report);
            let mut body_constants = constants.clone();
            let body = optimize_stmts(body, &mut body_constants, report);
            constants.clear();
            Stmt::While { condition, body }
        }
        Stmt::Loop { body } => {
            let mut body_constants = constants.clone();
            let body = optimize_stmts(body, &mut body_constants, report);
            constants.clear();
            Stmt::Loop { body }
        }
        Stmt::Return(value) => {
            Stmt::Return(value.map(|value| optimize_expr(value, constants, report)))
        }
        Stmt::Out { port, value } => Stmt::Out {
            port,
            value: optimize_expr(value, constants, report),
        },
        Stmt::Expr(value) => {
            let value = optimize_expr(value, constants, report);
            if expr_can_mutate(&value) {
                constants.clear();
            }
            Stmt::Expr(value)
        }
        Stmt::Asm { .. } => {
            constants.clear();
            stmt
        }
        Stmt::Break | Stmt::Continue => stmt,
    }
}

fn optimize_place(
    place: Place,
    constants: &HashMap<String, Expr>,
    report: &mut TbirOptimizationReport,
) -> Place {
    match place {
        Place::Index { name, index } => Place::Index {
            name,
            index: Box::new(optimize_expr(*index, constants, report)),
        },
        Place::Access(path) => Place::Access(optimize_access(path, constants, report)),
        Place::Deref(expr) => Place::Deref(Box::new(optimize_expr(*expr, constants, report))),
        Place::Ident(_) | Place::Field { .. } => place,
    }
}

fn optimize_expr(
    mut expr: Expr,
    constants: &HashMap<String, Expr>,
    report: &mut TbirOptimizationReport,
) -> Expr {
    expr = match expr {
        Expr::Ident(name) => Expr::Ident(name),
        Expr::Array(values) => Expr::Array(
            values
                .into_iter()
                .map(|value| optimize_expr(value, constants, report))
                .collect(),
        ),
        Expr::Index { name, index } => Expr::Index {
            name,
            index: Box::new(optimize_expr(*index, constants, report)),
        },
        Expr::AddressOfIndex { name, index } => Expr::AddressOfIndex {
            name,
            index: Box::new(optimize_expr(*index, constants, report)),
        },
        Expr::Access(path) => Expr::Access(optimize_access(path, constants, report)),
        Expr::AddressOfAccess(path) => {
            Expr::AddressOfAccess(optimize_access(path, constants, report))
        }
        Expr::StructInit { ty, fields } => Expr::StructInit {
            ty,
            fields: fields
                .into_iter()
                .map(|(name, value)| (name, optimize_expr(value, constants, report)))
                .collect(),
        },
        Expr::Deref(value) => Expr::Deref(Box::new(optimize_expr(*value, constants, report))),
        Expr::BankedPointer { pointer, bank } => Expr::BankedPointer {
            pointer: Box::new(optimize_expr(*pointer, constants, report)),
            bank,
        },
        Expr::Call { path, args } => Expr::Call {
            path,
            args: args
                .into_iter()
                .map(|arg| optimize_expr(arg, constants, report))
                .collect(),
        },
        Expr::Unary { op, expr } => {
            let expr = optimize_expr(*expr, constants, report);
            if let Some(value) = fold_unary(op, &expr) {
                report.constant_folds += 1;
                value
            } else {
                Expr::Unary {
                    op,
                    expr: Box::new(expr),
                }
            }
        }
        Expr::Binary { left, op, right } => {
            let left = optimize_expr(*left, constants, report);
            let right = optimize_expr(*right, constants, report);
            if let Some(value) = fold_binary(&left, op, &right) {
                report.constant_folds += 1;
                value
            } else if let Some(value) = simplify_binary(&left, op, &right) {
                report.algebraic_simplifications += 1;
                value
            } else {
                Expr::Binary {
                    left: Box::new(left),
                    op,
                    right: Box::new(right),
                }
            }
        }
        Expr::Cast { ty, expr } => Expr::Cast {
            ty,
            expr: Box::new(optimize_expr(*expr, constants, report)),
        },
        Expr::Int(_)
        | Expr::TypedInt(_, _)
        | Expr::Bool(_)
        | Expr::Char(_)
        | Expr::String(_)
        | Expr::In(_)
        | Expr::Field { .. }
        | Expr::AddressOfField { .. }
        | Expr::AddressOf(_) => expr,
    };
    expr
}

fn optimize_access(
    mut path: AccessPath,
    constants: &HashMap<String, Expr>,
    report: &mut TbirOptimizationReport,
) -> AccessPath {
    path.segments = path
        .segments
        .into_iter()
        .map(|segment| match segment {
            AccessSegment::Index(index) => {
                AccessSegment::Index(Box::new(optimize_expr(*index, constants, report)))
            }
            AccessSegment::Field(field) => AccessSegment::Field(field),
        })
        .collect();
    path
}

fn simplify_binary(left: &Expr, op: BinaryOp, right: &Expr) -> Option<Expr> {
    match (left, op, right) {
        (
            value,
            BinaryOp::Add
            | BinaryOp::Sub
            | BinaryOp::BitOr
            | BinaryOp::BitXor
            | BinaryOp::Shl
            | BinaryOp::Shr,
            value_expr,
        ) if int_value(value_expr) == Some(0) => Some(value.clone()),
        (value, BinaryOp::Mul | BinaryOp::Div, value_expr) if int_value(value_expr) == Some(1) => {
            Some(value.clone())
        }
        (_, BinaryOp::Div | BinaryOp::Mod, value_expr) if int_value(value_expr) == Some(0) => {
            Some(Expr::Int(0))
        }
        (value_expr, BinaryOp::Mul, value) if power_of_two_shift(value_expr).is_some() => {
            power_of_two_shift(value_expr)
                .map(|shift| shift_expr(value.clone(), BinaryOp::Shl, shift))
        }
        (value, BinaryOp::Mul, value_expr) => power_of_two_shift(value_expr)
            .map(|shift| shift_expr(value.clone(), BinaryOp::Shl, shift)),
        (value_expr, BinaryOp::Add | BinaryOp::BitOr | BinaryOp::BitXor, value)
            if int_value(value_expr) == Some(0) =>
        {
            Some(value.clone())
        }
        _ => None,
    }
}

fn int_value(expr: &Expr) -> Option<i64> {
    match expr {
        Expr::Int(value) | Expr::TypedInt(value, _) => Some(*value),
        _ => None,
    }
}

fn power_of_two_shift(expr: &Expr) -> Option<u32> {
    let value = int_value(expr)?;
    if value > 1 && (value as u64).is_power_of_two() {
        Some((value as u64).trailing_zeros())
    } else {
        None
    }
}

fn shift_expr(value: Expr, op: BinaryOp, shift: u32) -> Expr {
    Expr::Binary {
        left: Box::new(value),
        op,
        right: Box::new(Expr::Int(i64::from(shift))),
    }
}

fn fold_unary(op: UnaryOp, expr: &Expr) -> Option<Expr> {
    match (op, expr) {
        (UnaryOp::Not, Expr::Bool(value)) => Some(Expr::Bool(!value)),
        _ => None,
    }
}

fn fold_binary(left: &Expr, op: BinaryOp, right: &Expr) -> Option<Expr> {
    match (left, right) {
        (Expr::Bool(left), Expr::Bool(right)) => match op {
            BinaryOp::And => Some(Expr::Bool(*left && *right)),
            BinaryOp::Or => Some(Expr::Bool(*left || *right)),
            BinaryOp::Eq => Some(Expr::Bool(left == right)),
            BinaryOp::Ne => Some(Expr::Bool(left != right)),
            _ => None,
        },
        _ => None,
    }
}

fn expr_can_mutate(expr: &Expr) -> bool {
    matches!(expr, Expr::Call { .. })
}

fn terminates(stmt: &Stmt) -> bool {
    matches!(stmt, Stmt::Return(_) | Stmt::Break | Stmt::Continue)
}

#[cfg(test)]
mod tests;
