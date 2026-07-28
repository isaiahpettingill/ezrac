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
    // Keep the stage order visible: later passes rely on the safety facts and
    // normalized expressions produced by earlier stages.
    scalar_simplify_program(&mut program, &mut report, true);
    local_propagation_and_cse_program(&mut program, &mut report);
    scalar_simplify_program(&mut program, &mut report, false);
    hoist_pure_loop_invariants_program(&mut program, &mut report);
    decide_inline_functions(&program, &mut report);
    run_tail_passes(&mut program, cpu, &mut report);
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

fn run_tail_passes(program: &mut Program, cpu: CpuFamily, report: &mut TbirOptimizationReport) {
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

fn local_propagation_and_cse_program(program: &mut Program, report: &mut TbirOptimizationReport) {
    fn visit(declarations: &mut [Declaration], report: &mut TbirOptimizationReport) {
        for declaration in declarations {
            match declaration {
                Declaration::Function(function) => {
                    let mut assigned = HashSet::new();
                    collect_assigned_names(&function.body, &mut assigned);
                    let mut values = HashMap::new();
                    let mut available = Vec::new();
                    function.body = propagate_block(
                        core::mem::take(&mut function.body),
                        &assigned,
                        &mut values,
                        &mut available,
                        report,
                    );
                }
                Declaration::Cfg { declaration, .. } | Declaration::Bank { declaration, .. } => {
                    visit(core::slice::from_mut(declaration), report)
                }
                _ => {}
            }
        }
    }
    visit(&mut program.declarations, report);
}

fn collect_assigned_names(stmts: &[Stmt], assigned: &mut HashSet<String>) {
    for stmt in stmts {
        match stmt {
            Stmt::Assign {
                target: Place::Ident(name),
                ..
            } => {
                assigned.insert(name.clone());
            }
            Stmt::If {
                then_body,
                else_body,
                ..
            } => {
                collect_assigned_names(then_body, assigned);
                collect_assigned_names(else_body, assigned);
            }
            Stmt::While { body, .. } | Stmt::Loop { body } => {
                collect_assigned_names(body, assigned)
            }
            _ => {}
        }
    }
}

fn propagate_block(
    stmts: Vec<Stmt>,
    assigned: &HashSet<String>,
    values: &mut HashMap<String, Expr>,
    available: &mut Vec<(Expr, String)>,
    report: &mut TbirOptimizationReport,
) -> Vec<Stmt> {
    let mut output = Vec::with_capacity(stmts.len());
    for stmt in stmts {
        let stmt = match stmt {
            Stmt::Let { name, ty, value } => {
                let mut value = substitute_expr(value, values, report);
                if is_cse_candidate(&value) {
                    if let Some((_, prior)) = available.iter().find(|(expr, _)| expr == &value) {
                        value = Expr::Ident(prior.clone());
                        report.common_subexpressions += 1;
                        decision(
                            report,
                            TbirOptimizationKind::CommonSubexpression,
                            None,
                            &name,
                            None,
                        );
                    } else if !assigned.contains(&name) {
                        available.push((value.clone(), name.clone()));
                    }
                }
                if !assigned.contains(&name) && is_pure_scalar(&value) {
                    values.insert(name.clone(), value.clone());
                }
                Stmt::Let { name, ty, value }
            }
            Stmt::Assign { target, op, value } => {
                let target = substitute_place(target, values, report);
                let value = substitute_expr(value, values, report);
                available.clear();
                Stmt::Assign { target, op, value }
            }
            Stmt::If {
                condition,
                then_body,
                else_body,
            } => {
                let condition = substitute_expr(condition, values, report);
                let mut then_values = values.clone();
                let mut else_values = values.clone();
                let then_body = propagate_block(
                    then_body,
                    assigned,
                    &mut then_values,
                    &mut Vec::new(),
                    report,
                );
                let else_body = propagate_block(
                    else_body,
                    assigned,
                    &mut else_values,
                    &mut Vec::new(),
                    report,
                );
                available.clear();
                Stmt::If {
                    condition,
                    then_body,
                    else_body,
                }
            }
            Stmt::While { condition, body } => {
                let condition = substitute_expr(condition, values, report);
                let mut body_values = values.clone();
                let body =
                    propagate_block(body, assigned, &mut body_values, &mut Vec::new(), report);
                available.clear();
                Stmt::While { condition, body }
            }
            Stmt::Loop { body } => {
                let mut body_values = values.clone();
                let body =
                    propagate_block(body, assigned, &mut body_values, &mut Vec::new(), report);
                available.clear();
                Stmt::Loop { body }
            }
            Stmt::Return(value) => Stmt::Return(value.map(|v| substitute_expr(v, values, report))),
            Stmt::Out { port, value } => {
                available.clear();
                Stmt::Out {
                    port,
                    value: substitute_expr(value, values, report),
                }
            }
            Stmt::Expr(value) => {
                let value = substitute_expr(value, values, report);
                if !is_pure_scalar(&value) {
                    available.clear();
                }
                Stmt::Expr(value)
            }
            Stmt::Asm { .. } => {
                available.clear();
                stmt
            }
            Stmt::Break | Stmt::Continue => stmt,
        };
        output.push(stmt);
    }
    output
}

fn substitute_place(
    place: Place,
    values: &HashMap<String, Expr>,
    report: &mut TbirOptimizationReport,
) -> Place {
    match place {
        Place::Index { name, index } => Place::Index {
            name,
            index: Box::new(substitute_expr(*index, values, report)),
        },
        Place::Access(mut path) => {
            for segment in &mut path.segments {
                if let AccessSegment::Index(index) = segment {
                    **index = substitute_expr((**index).clone(), values, report);
                }
            }
            Place::Access(path)
        }
        Place::Deref(expr) => Place::Deref(Box::new(substitute_expr(*expr, values, report))),
        other => other,
    }
}

fn substitute_expr(
    expr: Expr,
    values: &HashMap<String, Expr>,
    report: &mut TbirOptimizationReport,
) -> Expr {
    match expr {
        Expr::Ident(name) => {
            if let Some(value) = values.get(&name) {
                report.copy_propagations += 1;
                if matches!(
                    value,
                    Expr::Int(_) | Expr::TypedInt(_, _) | Expr::Bool(_) | Expr::Char(_)
                ) {
                    report.constant_propagations += 1;
                }
                decision(
                    report,
                    TbirOptimizationKind::CopyPropagation,
                    None,
                    &name,
                    None,
                );
                value.clone()
            } else {
                Expr::Ident(name)
            }
        }
        Expr::Unary { op, expr } => Expr::Unary {
            op,
            expr: Box::new(substitute_expr(*expr, values, report)),
        },
        Expr::Binary { left, op, right } => Expr::Binary {
            left: Box::new(substitute_expr(*left, values, report)),
            op,
            right: Box::new(substitute_expr(*right, values, report)),
        },
        Expr::Cast { ty, expr } => Expr::Cast {
            ty,
            expr: Box::new(substitute_expr(*expr, values, report)),
        },
        Expr::Array(values_) => Expr::Array(
            values_
                .into_iter()
                .map(|v| substitute_expr(v, values, report))
                .collect(),
        ),
        Expr::StructInit { ty, fields } => Expr::StructInit {
            ty,
            fields: fields
                .into_iter()
                .map(|(n, v)| (n, substitute_expr(v, values, report)))
                .collect(),
        },
        Expr::Index { name, index } => Expr::Index {
            name,
            index: Box::new(substitute_expr(*index, values, report)),
        },
        Expr::AddressOfIndex { name, index } => Expr::AddressOfIndex {
            name,
            index: Box::new(substitute_expr(*index, values, report)),
        },
        Expr::Deref(expr) => Expr::Deref(Box::new(substitute_expr(*expr, values, report))),
        Expr::BankedPointer { pointer, bank } => Expr::BankedPointer {
            pointer: Box::new(substitute_expr(*pointer, values, report)),
            bank,
        },
        Expr::Call { path, args } => Expr::Call {
            path,
            args: args
                .into_iter()
                .map(|v| substitute_expr(v, values, report))
                .collect(),
        },
        other => other,
    }
}

fn is_pure_scalar(expr: &Expr) -> bool {
    match expr {
        Expr::Int(_) | Expr::TypedInt(_, _) | Expr::Bool(_) | Expr::Char(_) | Expr::Ident(_) => {
            true
        }
        Expr::Unary { expr, .. } | Expr::Cast { expr, .. } => is_pure_scalar(expr),
        Expr::Binary { left, right, .. } => is_pure_scalar(left) && is_pure_scalar(right),
        _ => false,
    }
}

fn is_cse_candidate(expr: &Expr) -> bool {
    is_pure_scalar(expr)
        && !matches!(
            expr,
            Expr::Int(_) | Expr::TypedInt(_, _) | Expr::Bool(_) | Expr::Char(_) | Expr::Ident(_)
        )
}

fn hoist_pure_loop_invariants_program(program: &mut Program, report: &mut TbirOptimizationReport) {
    fn visit(declarations: &mut [Declaration], report: &mut TbirOptimizationReport) {
        for declaration in declarations {
            match declaration {
                Declaration::Function(function) => {
                    let mut names: HashSet<String> =
                        function.params.iter().map(|p| p.name.clone()).collect();
                    collect_local_names(&function.body, &mut names);
                    let mut next_temp = 0;
                    let external: HashSet<String> =
                        function.params.iter().map(|p| p.name.clone()).collect();
                    function.body = licm_block(
                        core::mem::take(&mut function.body),
                        &external,
                        &mut names,
                        &mut next_temp,
                        report,
                    );
                }
                Declaration::Cfg { declaration, .. } | Declaration::Bank { declaration, .. } => {
                    visit(core::slice::from_mut(declaration), report)
                }
                _ => {}
            }
        }
    }
    visit(&mut program.declarations, report);
}

fn licm_block(
    stmts: Vec<Stmt>,
    external: &HashSet<String>,
    used_names: &mut HashSet<String>,
    next_temp: &mut usize,
    report: &mut TbirOptimizationReport,
) -> Vec<Stmt> {
    let mut output = Vec::new();
    let mut visible = external.clone();
    for stmt in stmts {
        match stmt {
            Stmt::Let { name, ty, value } => {
                visible.insert(name.clone());
                output.push(Stmt::Let { name, ty, value });
            }
            Stmt::If {
                condition,
                then_body,
                else_body,
            } => {
                let then_body = licm_block(then_body, &visible, used_names, next_temp, report);
                let else_body = licm_block(else_body, &visible, used_names, next_temp, report);
                output.push(Stmt::If {
                    condition,
                    then_body,
                    else_body,
                });
            }
            Stmt::While { condition, body } => {
                let (preheader, body) =
                    hoist_loop_body(body, &visible, used_names, next_temp, report);
                output.extend(preheader);
                output.push(Stmt::While { condition, body });
            }
            Stmt::Loop { body } => {
                let (preheader, body) =
                    hoist_loop_body(body, &visible, used_names, next_temp, report);
                output.extend(preheader);
                output.push(Stmt::Loop { body });
            }
            other => output.push(other),
        }
    }
    output
}

fn hoist_loop_body(
    mut body: Vec<Stmt>,
    external: &HashSet<String>,
    used_names: &mut HashSet<String>,
    next_temp: &mut usize,
    report: &mut TbirOptimizationReport,
) -> (Vec<Stmt>, Vec<Stmt>) {
    let has_exit = body.iter().any(stmt_contains_loop_exit);
    let mut assigned = HashSet::new();
    collect_assigned_names(&body, &mut assigned);
    let mut declared = HashSet::new();
    collect_local_names(&body, &mut declared);
    let mut preheader = Vec::new();
    for stmt in &mut body {
        let Stmt::Let { name, ty, value } = stmt else {
            continue;
        };
        if !is_cse_candidate(value) {
            continue;
        }
        let mut deps = HashSet::new();
        collect_scalar_dependencies(value, &mut deps);
        let rejection = if has_exit {
            Some("loop contains return, break, or continue")
        } else if deps.iter().any(|dep| assigned.contains(dep)) {
            Some("dependency assigned in loop")
        } else if deps
            .iter()
            .any(|dep| declared.contains(dep) || !external.contains(dep))
        {
            Some("dependency declared in loop")
        } else {
            None
        };
        if let Some(reason) = rejection {
            decision(
                report,
                TbirOptimizationKind::LoopInvariantCodeMotion,
                None,
                name,
                Some(reason),
            );
            continue;
        }
        let temp = unique_licm_temp_name(used_names, next_temp);
        preheader.push(Stmt::Let {
            name: temp.clone(),
            ty: ty.clone(),
            value: value.clone(),
        });
        *value = Expr::Ident(temp);
        report.loop_invariants_hoisted += 1;
        decision(
            report,
            TbirOptimizationKind::LoopInvariantCodeMotion,
            None,
            name,
            None,
        );
    }
    let mut nested_external = external.clone();
    nested_external.extend(declared);
    body = licm_block(body, &nested_external, used_names, next_temp, report);
    (preheader, body)
}

fn stmt_contains_loop_exit(stmt: &Stmt) -> bool {
    match stmt {
        Stmt::Return(_) | Stmt::Break | Stmt::Continue => true,
        Stmt::If {
            then_body,
            else_body,
            ..
        } => {
            then_body.iter().any(stmt_contains_loop_exit)
                || else_body.iter().any(stmt_contains_loop_exit)
        }
        Stmt::While { body, .. } | Stmt::Loop { body } => body.iter().any(stmt_contains_loop_exit),
        _ => false,
    }
}

fn collect_scalar_dependencies(expr: &Expr, deps: &mut HashSet<String>) {
    match expr {
        Expr::Ident(name) => {
            deps.insert(name.clone());
        }
        Expr::Unary { expr, .. } | Expr::Cast { expr, .. } => {
            collect_scalar_dependencies(expr, deps)
        }
        Expr::Binary { left, right, .. } => {
            collect_scalar_dependencies(left, deps);
            collect_scalar_dependencies(right, deps);
        }
        _ => {}
    }
}

fn unique_licm_temp_name(used_names: &mut HashSet<String>, next_temp: &mut usize) -> String {
    loop {
        let name = format!("__tbir_licm_{}", *next_temp);
        *next_temp += 1;
        if used_names.insert(name.clone()) {
            return name;
        }
    }
}

fn scalar_simplify_program(
    program: &mut Program,
    report: &mut TbirOptimizationReport,
    count_dead_statements: bool,
) {
    for declaration in &mut program.declarations {
        optimize_declaration(declaration, report, count_dead_statements);
    }
}

fn optimize_declaration(
    declaration: &mut Declaration,
    report: &mut TbirOptimizationReport,
    count_dead_statements: bool,
) {
    match declaration {
        Declaration::Cfg { declaration, .. } | Declaration::Bank { declaration, .. } => {
            optimize_declaration(declaration, report, count_dead_statements)
        }
        Declaration::Function(function) => {
            optimize_function(function, report, count_dead_statements)
        }
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

fn optimize_function(
    function: &mut Function,
    report: &mut TbirOptimizationReport,
    count_dead_statements: bool,
) {
    let mut constants = HashMap::new();
    function.body = optimize_stmts(
        core::mem::take(&mut function.body),
        &mut constants,
        report,
        count_dead_statements,
    );
}

fn optimize_stmts(
    stmts: Vec<Stmt>,
    constants: &mut HashMap<String, Expr>,
    report: &mut TbirOptimizationReport,
    count_dead_statements: bool,
) -> Vec<Stmt> {
    let mut output = Vec::with_capacity(stmts.len());
    let mut terminated = false;
    for stmt in stmts {
        if terminated && count_dead_statements {
            report.dead_statements_marked += 1;
        }
        let stmt = optimize_stmt(stmt, constants, report, count_dead_statements);
        terminated |= terminates(&stmt);
        output.push(stmt);
    }
    output
}

fn optimize_stmt(
    stmt: Stmt,
    constants: &mut HashMap<String, Expr>,
    report: &mut TbirOptimizationReport,
    count_dead_statements: bool,
) -> Stmt {
    match stmt {
        Stmt::Let { name, ty, value } => {
            let value = optimize_expr(value, constants, report);
            Stmt::Let { name, ty, value }
        }
        Stmt::Assign { target, op, value } => {
            let target = optimize_place(target, constants, report);
            let value = optimize_expr(value, constants, report);
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
            let then_body = optimize_stmts(
                then_body,
                &mut then_constants,
                report,
                count_dead_statements,
            );
            let else_body = optimize_stmts(
                else_body,
                &mut else_constants,
                report,
                count_dead_statements,
            );
            Stmt::If {
                condition,
                then_body,
                else_body,
            }
        }
        Stmt::While { condition, body } => {
            let condition = optimize_expr(condition, constants, report);
            let mut body_constants = constants.clone();
            let body = optimize_stmts(body, &mut body_constants, report, count_dead_statements);
            Stmt::While { condition, body }
        }
        Stmt::Loop { body } => {
            let mut body_constants = constants.clone();
            let body = optimize_stmts(body, &mut body_constants, report, count_dead_statements);
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
            Stmt::Expr(value)
        }
        Stmt::Asm { .. } => stmt,
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
            } else if let Some((value, strength_reduction)) = simplify_binary(&left, op, &right) {
                if strength_reduction {
                    report.strength_reductions += 1;
                    decision(
                        report,
                        TbirOptimizationKind::StrengthReduction,
                        None,
                        "binary expression",
                        None,
                    );
                } else {
                    report.algebraic_simplifications += 1;
                }
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

fn simplify_binary(left: &Expr, op: BinaryOp, right: &Expr) -> Option<(Expr, bool)> {
    let algebraic = |expr| Some((expr, false));
    let strength = |expr| Some((expr, true));
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
        ) if int_value(value_expr) == Some(0) => algebraic(value.clone()),
        (value, BinaryOp::Mul | BinaryOp::Div, value_expr) if int_value(value_expr) == Some(1) => {
            algebraic(value.clone())
        }
        (_, BinaryOp::Div | BinaryOp::Mod, value_expr) if int_value(value_expr) == Some(0) => {
            algebraic(Expr::Int(0))
        }
        (_, BinaryOp::Mul | BinaryOp::BitAnd, value)
            if int_value(value) == Some(0) && is_pure_scalar(left) =>
        {
            algebraic(Expr::Int(0))
        }
        (value, BinaryOp::Mul | BinaryOp::BitAnd, _)
            if int_value(value) == Some(0) && is_pure_scalar(right) =>
        {
            algebraic(Expr::Int(0))
        }
        (value_expr, BinaryOp::Mul, value) if power_of_two_shift(value_expr).is_some() => {
            power_of_two_shift(value_expr)
                .and_then(|shift| strength(shift_expr(value.clone(), BinaryOp::Shl, shift)))
        }
        (value, BinaryOp::Mul, value_expr) => power_of_two_shift(value_expr)
            .and_then(|shift| strength(shift_expr(value.clone(), BinaryOp::Shl, shift))),
        (value_expr, BinaryOp::Add | BinaryOp::BitOr | BinaryOp::BitXor, value)
            if int_value(value_expr) == Some(0) =>
        {
            algebraic(value.clone())
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

fn terminates(stmt: &Stmt) -> bool {
    matches!(stmt, Stmt::Return(_) | Stmt::Break | Stmt::Continue)
}

#[cfg(test)]
mod tests;
