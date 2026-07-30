use crate::{
    ast::{
        AccessPath, AccessSegment, AssignOp, BinaryOp, Declaration, Expr, Function, Place, Program,
        Stmt, Type, UnaryOp,
    },
    compat::prelude::*,
    target::CpuFamily,
};

use super::{
    TbirAccess, TbirObjectKind, TbirOptimizationDecision, TbirOptimizationKind,
    TbirOptimizationOutcome, TbirOptimizationReport, provenance::OptimizationContext,
};

pub fn optimize_program(program: &Program, cpu: CpuFamily) -> (Program, TbirOptimizationReport) {
    optimize_program_with_context(program, cpu, &OptimizationContext::default())
}

pub fn optimize_program_with_context(
    program: &Program,
    cpu: CpuFamily,
    context: &OptimizationContext,
) -> (Program, TbirOptimizationReport) {
    let mut program = program.clone();
    let mut report = TbirOptimizationReport::default();
    // Keep the stage order visible: later passes rely on the safety facts and
    // normalized expressions produced by earlier stages.
    scalar_simplify_program(&mut program, &mut report, true);
    local_propagation_and_cse_program(&mut program, context, &mut report);
    scalar_simplify_program(&mut program, &mut report, false);
    hoist_pure_loop_invariants_program(&mut program, &mut report);
    hoist_named_memory_reads_program(&mut program, context, &mut report);
    expand_inline_functions(&mut program, context, &mut report);
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

fn expand_inline_functions(
    program: &mut Program,
    context: &OptimizationContext,
    report: &mut TbirOptimizationReport,
) {
    let all_functions = functions(program);
    let mut graph = HashMap::new();
    for function in &all_functions {
        let mut calls = HashSet::new();
        collect_calls(&function.body, &mut calls);
        graph.insert(function.name.clone(), calls);
    }

    let mut approved = HashMap::new();
    let mut rejected = Vec::new();
    for function in all_functions {
        if !has_attr(function, "inline") {
            continue;
        }
        let reason = inline_rejection(function, &graph);
        if let Some(reason) = reason {
            rejected.push((function.name.clone(), reason));
        } else {
            approved.insert(function.name.clone(), function.clone());
        }
    }
    for (name, reason) in rejected {
        decision(
            report,
            TbirOptimizationKind::Inline,
            None,
            &name,
            Some(reason),
        );
    }

    let mut used_names = HashSet::new();
    for function in functions(program) {
        used_names.insert(function.name.clone());
        for param in &function.params {
            used_names.insert(param.name.clone());
        }
        collect_local_names(&function.body, &mut used_names);
    }
    let mut expander = InlineExpander {
        approved: &approved,
        used_names,
        next_temp: 0,
        counts: HashMap::new(),
        active: Vec::new(),
    };
    expand_inline_declarations(&mut program.declarations, &mut expander);

    let expanded_any = !expander.counts.is_empty();
    let mut counts: Vec<_> = expander.counts.into_iter().collect();
    counts.sort_by(|left, right| left.0.cmp(&right.0));
    for (name, count) in counts {
        decision(report, TbirOptimizationKind::Inline, None, &name, None);
        if let Some(last) = report.decisions.last_mut() {
            last.reason = format!("approved; transformed {count} call(s)");
        }
    }

    if expanded_any {
        let mut cleanup_report = TbirOptimizationReport::default();
        scalar_simplify_program(program, &mut cleanup_report, false);
        local_propagation_and_cse_program(program, context, &mut cleanup_report);
        scalar_simplify_program(program, &mut cleanup_report, false);
    }
}

fn inline_rejection<'a>(
    function: &Function,
    graph: &HashMap<String, HashSet<String>>,
) -> Option<&'a str> {
    if has_attr(function, "naked") {
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
            .any(|callee| reachable(callee, &function.name, graph, &mut HashSet::new()))
    }) {
        Some("mutual recursion")
    } else if function.body.iter().any(stmt_has_inline_forbidden_control) {
        Some("inline assembly or control exit")
    } else if inline_return_body(function).is_none() && inline_void_body(function).is_none() {
        Some("unsupported body shape")
    } else {
        None
    }
}

fn stmt_has_inline_forbidden_control(stmt: &Stmt) -> bool {
    match stmt {
        Stmt::Break | Stmt::Continue => true,
        Stmt::If {
            then_body,
            else_body,
            ..
        } => then_body
            .iter()
            .chain(else_body)
            .any(stmt_has_inline_forbidden_control),
        Stmt::While { body, .. } | Stmt::Loop { body } => {
            body.iter().any(stmt_has_inline_forbidden_control)
        }
        _ => false,
    }
}

struct InlineExpander<'a> {
    approved: &'a HashMap<String, Function>,
    used_names: HashSet<String>,
    next_temp: usize,
    counts: HashMap<String, usize>,
    active: Vec<String>,
}

fn expand_inline_declarations(declarations: &mut [Declaration], expander: &mut InlineExpander<'_>) {
    for declaration in declarations {
        match declaration {
            Declaration::Function(function) => {
                function.body = expander.expand_stmts(core::mem::take(&mut function.body));
            }
            Declaration::Cfg { declaration, .. } | Declaration::Bank { declaration, .. } => {
                expand_inline_declarations(core::slice::from_mut(declaration), expander);
            }
            _ => {}
        }
    }
}

impl InlineExpander<'_> {
    fn unique(&mut self, kind: &str) -> String {
        loop {
            let name = format!("__tbir_inline_{kind}_{}", self.next_temp);
            self.next_temp += 1;
            if self.used_names.insert(name.clone()) {
                return name;
            }
        }
    }

    fn expand_stmts(&mut self, stmts: Vec<Stmt>) -> Vec<Stmt> {
        let mut output = Vec::new();
        for stmt in stmts {
            self.expand_stmt(stmt, &mut output);
        }
        output
    }

    fn expand_stmt(&mut self, stmt: Stmt, output: &mut Vec<Stmt>) {
        match stmt {
            Stmt::Let { name, ty, value } => {
                let value = self.expand_expr(value, output);
                output.push(Stmt::Let { name, ty, value });
            }
            Stmt::Assign { target, op, value } => {
                let target = self.expand_place(target, output);
                let value = self.expand_expr(value, output);
                output.push(Stmt::Assign { target, op, value });
            }
            Stmt::Return(value) => {
                let value = value.map(|value| self.expand_expr(value, output));
                output.push(Stmt::Return(value));
            }
            Stmt::Out { port, value } => {
                let value = self.expand_expr(value, output);
                output.push(Stmt::Out { port, value });
            }
            Stmt::Expr(Expr::Call { path, args }) if self.is_approved_void(&path) => {
                self.expand_void_call(path, args, output);
            }
            Stmt::Expr(value) => {
                let value = self.expand_expr(value, output);
                output.push(Stmt::Expr(value));
            }
            Stmt::If {
                condition,
                then_body,
                else_body,
            } => {
                let condition = self.expand_expr(condition, output);
                output.push(Stmt::If {
                    condition,
                    then_body: self.expand_stmts(then_body),
                    else_body: self.expand_stmts(else_body),
                });
            }
            Stmt::While { condition, body } => output.push(Stmt::While {
                condition,
                body: self.expand_stmts(body),
            }),
            Stmt::Loop { body } => output.push(Stmt::Loop {
                body: self.expand_stmts(body),
            }),
            stmt => output.push(stmt),
        }
    }

    fn is_approved_void(&self, path: &[String]) -> bool {
        path.last()
            .and_then(|name| self.approved.get(name))
            .is_some_and(|f| f.return_type.is_none())
    }

    fn expand_void_call(&mut self, path: Vec<String>, args: Vec<Expr>, output: &mut Vec<Stmt>) {
        let Some(name) = path.last().cloned() else {
            return;
        };
        let Some(function) = self.approved.get(&name).cloned() else {
            output.push(Stmt::Expr(Expr::Call { path, args }));
            return;
        };
        if self.active.contains(&name) || args.len() != function.params.len() {
            output.push(Stmt::Expr(Expr::Call { path, args }));
            return;
        }
        let (bindings, rename) = self.bind_arguments(&function, args, output);
        output.extend(bindings);
        let body = inline_void_body(&function).unwrap_or(&[]).to_vec();
        self.active.push(name.clone());
        output.extend(self.expand_stmts(rename_stmts(body, &rename)));
        self.active.pop();
        *self.counts.entry(name).or_default() += 1;
    }

    fn expand_expr(&mut self, expr: Expr, prefix: &mut Vec<Stmt>) -> Expr {
        match expr {
            Expr::Call { path, args } => {
                let mut expanded_args = Vec::new();
                for arg in args {
                    expanded_args.push(self.expand_expr(arg, prefix));
                }
                let Some(name) = path.last().cloned() else {
                    return Expr::Call {
                        path,
                        args: expanded_args,
                    };
                };
                let Some(function) = self.approved.get(&name).cloned() else {
                    return Expr::Call {
                        path,
                        args: expanded_args,
                    };
                };
                let Some(return_ty) = function.return_type.clone() else {
                    return Expr::Call {
                        path,
                        args: expanded_args,
                    };
                };
                if self.active.contains(&name) || expanded_args.len() != function.params.len() {
                    return Expr::Call {
                        path,
                        args: expanded_args,
                    };
                }
                let (bindings, rename) = self.bind_arguments(&function, expanded_args, prefix);
                prefix.extend(bindings);
                let (body, result) = inline_return_body(&function).expect("approved value inline");
                self.active.push(name.clone());
                prefix.extend(self.expand_stmts(rename_stmts(body.to_vec(), &rename)));
                let result = self.expand_expr(rename_expr(result, &rename), prefix);
                self.active.pop();
                let result_name = self.unique("result");
                prefix.push(Stmt::Let {
                    name: result_name.clone(),
                    ty: return_ty,
                    value: result,
                });
                *self.counts.entry(name).or_default() += 1;
                Expr::Ident(result_name)
            }
            Expr::Array(values) => Expr::Array(
                values
                    .into_iter()
                    .map(|v| self.expand_expr(v, prefix))
                    .collect(),
            ),
            Expr::Index { name, index } => Expr::Index {
                name,
                index: Box::new(self.expand_expr(*index, prefix)),
            },
            Expr::AddressOfIndex { name, index } => Expr::AddressOfIndex {
                name,
                index: Box::new(self.expand_expr(*index, prefix)),
            },
            Expr::Deref(expr) => Expr::Deref(Box::new(self.expand_expr(*expr, prefix))),
            Expr::BankedPointer { pointer, bank } => Expr::BankedPointer {
                pointer: Box::new(self.expand_expr(*pointer, prefix)),
                bank,
            },
            Expr::Unary { op, expr } => Expr::Unary {
                op,
                expr: Box::new(self.expand_expr(*expr, prefix)),
            },
            Expr::Cast { ty, expr } => Expr::Cast {
                ty,
                expr: Box::new(self.expand_expr(*expr, prefix)),
            },
            Expr::Binary { left, op, right } => {
                let left = Box::new(self.expand_expr(*left, prefix));
                let right = if matches!(op, BinaryOp::And | BinaryOp::Or) {
                    right
                } else {
                    Box::new(self.expand_expr(*right, prefix))
                };
                Expr::Binary { left, op, right }
            }
            Expr::Access(path) => Expr::Access(self.expand_access(path, prefix)),
            Expr::AddressOfAccess(path) => Expr::AddressOfAccess(self.expand_access(path, prefix)),
            Expr::StructInit { ty, fields } => Expr::StructInit {
                ty,
                fields: fields
                    .into_iter()
                    .map(|(n, v)| (n, self.expand_expr(v, prefix)))
                    .collect(),
            },
            expr => expr,
        }
    }

    fn bind_arguments(
        &mut self,
        function: &Function,
        args: Vec<Expr>,
        prefix: &mut Vec<Stmt>,
    ) -> (Vec<Stmt>, HashMap<String, String>) {
        let mut bindings = Vec::new();
        let mut rename = HashMap::new();
        for (param, arg) in function.params.iter().zip(args) {
            let arg_name = self.unique("arg");
            prefix.push(Stmt::Let {
                name: arg_name.clone(),
                ty: param.ty.clone(),
                value: arg,
            });
            let param_name = self.unique("param");
            bindings.push(Stmt::Let {
                name: param_name.clone(),
                ty: param.ty.clone(),
                value: Expr::Ident(arg_name),
            });
            rename.insert(param.name.clone(), param_name);
        }
        let mut locals = HashSet::new();
        collect_local_names(&function.body, &mut locals);
        for local in locals {
            rename.entry(local).or_insert_with(|| self.unique("local"));
        }
        (bindings, rename)
    }

    fn expand_place(&mut self, place: Place, prefix: &mut Vec<Stmt>) -> Place {
        match place {
            Place::Index { name, index } => Place::Index {
                name,
                index: Box::new(self.expand_expr(*index, prefix)),
            },
            Place::Access(path) => Place::Access(self.expand_access(path, prefix)),
            Place::Deref(expr) => Place::Deref(Box::new(self.expand_expr(*expr, prefix))),
            place => place,
        }
    }

    fn expand_access(&mut self, mut path: AccessPath, prefix: &mut Vec<Stmt>) -> AccessPath {
        for segment in &mut path.segments {
            if let AccessSegment::Index(index) = segment {
                **index = self.expand_expr((**index).clone(), prefix);
            }
        }
        path
    }
}

fn renamed(name: String, names: &HashMap<String, String>) -> String {
    names.get(&name).cloned().unwrap_or(name)
}

fn rename_stmts(stmts: Vec<Stmt>, names: &HashMap<String, String>) -> Vec<Stmt> {
    stmts
        .into_iter()
        .map(|stmt| match stmt {
            Stmt::Let { name, ty, value } => Stmt::Let {
                name: renamed(name, names),
                ty,
                value: rename_expr(value, names),
            },
            Stmt::Assign { target, op, value } => Stmt::Assign {
                target: rename_place(target, names),
                op,
                value: rename_expr(value, names),
            },
            Stmt::If {
                condition,
                then_body,
                else_body,
            } => Stmt::If {
                condition: rename_expr(condition, names),
                then_body: rename_stmts(then_body, names),
                else_body: rename_stmts(else_body, names),
            },
            Stmt::While { condition, body } => Stmt::While {
                condition: rename_expr(condition, names),
                body: rename_stmts(body, names),
            },
            Stmt::Loop { body } => Stmt::Loop {
                body: rename_stmts(body, names),
            },
            Stmt::Return(value) => Stmt::Return(value.map(|v| rename_expr(v, names))),
            Stmt::Out { port, value } => Stmt::Out {
                port,
                value: rename_expr(value, names),
            },
            Stmt::Expr(value) => Stmt::Expr(rename_expr(value, names)),
            Stmt::Asm {
                volatile,
                inputs,
                outputs,
                clobbers,
                lines,
            } => {
                let inputs = inputs
                    .into_iter()
                    .map(|mut input| {
                        input.name = renamed(input.name, names);
                        input
                    })
                    .collect();
                let outputs = outputs
                    .into_iter()
                    .map(|mut output| {
                        output.name = renamed(output.name, names);
                        output
                    })
                    .collect();
                let lines = lines
                    .into_iter()
                    .map(|mut line| {
                        for (old, new) in names {
                            line = line.replace(&format!("{{{old}}}"), &format!("{{{new}}}"));
                        }
                        line
                    })
                    .collect();
                Stmt::Asm {
                    volatile,
                    inputs,
                    outputs,
                    clobbers,
                    lines,
                }
            }
            stmt => stmt,
        })
        .collect()
}

fn rename_place(place: Place, names: &HashMap<String, String>) -> Place {
    match place {
        Place::Ident(name) => Place::Ident(renamed(name, names)),
        Place::Index { name, index } => Place::Index {
            name: renamed(name, names),
            index: Box::new(rename_expr(*index, names)),
        },
        Place::Field { base, field } => Place::Field {
            base: renamed(base, names),
            field,
        },
        Place::Access(path) => Place::Access(rename_access(path, names)),
        Place::Deref(expr) => Place::Deref(Box::new(rename_expr(*expr, names))),
    }
}

fn rename_access(mut path: AccessPath, names: &HashMap<String, String>) -> AccessPath {
    path.root = renamed(path.root, names);
    for segment in &mut path.segments {
        if let AccessSegment::Index(index) = segment {
            **index = rename_expr((**index).clone(), names);
        }
    }
    path
}

fn rename_expr(expr: Expr, names: &HashMap<String, String>) -> Expr {
    match expr {
        Expr::Ident(name) => Expr::Ident(renamed(name, names)),
        Expr::Index { name, index } => Expr::Index {
            name: renamed(name, names),
            index: Box::new(rename_expr(*index, names)),
        },
        Expr::Field { base, field } => Expr::Field {
            base: renamed(base, names),
            field,
        },
        Expr::AddressOfIndex { name, index } => Expr::AddressOfIndex {
            name: renamed(name, names),
            index: Box::new(rename_expr(*index, names)),
        },
        Expr::AddressOfField { base, field } => Expr::AddressOfField {
            base: renamed(base, names),
            field,
        },
        Expr::Access(path) => Expr::Access(rename_access(path, names)),
        Expr::AddressOfAccess(path) => Expr::AddressOfAccess(rename_access(path, names)),
        Expr::AddressOf(name) => Expr::AddressOf(renamed(name, names)),
        Expr::Array(values) => {
            Expr::Array(values.into_iter().map(|v| rename_expr(v, names)).collect())
        }
        Expr::StructInit { ty, fields } => Expr::StructInit {
            ty,
            fields: fields
                .into_iter()
                .map(|(n, v)| (n, rename_expr(v, names)))
                .collect(),
        },
        Expr::Deref(expr) => Expr::Deref(Box::new(rename_expr(*expr, names))),
        Expr::BankedPointer { pointer, bank } => Expr::BankedPointer {
            pointer: Box::new(rename_expr(*pointer, names)),
            bank,
        },
        Expr::Call { path, args } => Expr::Call {
            path,
            args: args.into_iter().map(|v| rename_expr(v, names)).collect(),
        },
        Expr::Unary { op, expr } => Expr::Unary {
            op,
            expr: Box::new(rename_expr(*expr, names)),
        },
        Expr::Binary { left, op, right } => Expr::Binary {
            left: Box::new(rename_expr(*left, names)),
            op,
            right: Box::new(rename_expr(*right, names)),
        },
        Expr::Cast { ty, expr } => Expr::Cast {
            ty,
            expr: Box::new(rename_expr(*expr, names)),
        },
        expr => expr,
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

fn local_propagation_and_cse_program(
    program: &mut Program,
    context: &OptimizationContext,
    report: &mut TbirOptimizationReport,
) {
    fn visit(
        declarations: &mut [Declaration],
        context: &OptimizationContext,
        report: &mut TbirOptimizationReport,
    ) {
        for declaration in declarations {
            match declaration {
                Declaration::Function(function) => {
                    let mut assigned = HashSet::new();
                    collect_assigned_names(&function.body, &mut assigned);
                    collect_address_taken_names(&function.body, &mut assigned);
                    let mut values = HashMap::new();
                    let mut available = Vec::new();
                    function.body = propagate_block(
                        core::mem::take(&mut function.body),
                        &assigned,
                        &mut values,
                        &mut available,
                        context,
                        report,
                    );
                }
                Declaration::Cfg { declaration, .. } | Declaration::Bank { declaration, .. } => {
                    visit(core::slice::from_mut(declaration), context, report)
                }
                _ => {}
            }
        }
    }
    visit(&mut program.declarations, context, report);
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

fn collect_address_taken_names(stmts: &[Stmt], names: &mut HashSet<String>) {
    for stmt in stmts {
        match stmt {
            Stmt::Let { value, .. }
            | Stmt::Return(Some(value))
            | Stmt::Out { value, .. }
            | Stmt::Expr(value) => collect_address_taken_names_in_expr(value, names),
            Stmt::Assign { target, value, .. } => {
                collect_address_taken_names_in_place(target, names);
                collect_address_taken_names_in_expr(value, names);
            }
            Stmt::If {
                condition,
                then_body,
                else_body,
            } => {
                collect_address_taken_names_in_expr(condition, names);
                collect_address_taken_names(then_body, names);
                collect_address_taken_names(else_body, names);
            }
            Stmt::While { condition, body } => {
                collect_address_taken_names_in_expr(condition, names);
                collect_address_taken_names(body, names);
            }
            Stmt::Loop { body } => collect_address_taken_names(body, names),
            Stmt::Asm { .. } | Stmt::Return(None) | Stmt::Break | Stmt::Continue => {}
        }
    }
}

fn collect_address_taken_names_in_place(place: &Place, names: &mut HashSet<String>) {
    match place {
        Place::Index { index, .. } | Place::Deref(index) => {
            collect_address_taken_names_in_expr(index, names)
        }
        Place::Access(path) => {
            for segment in &path.segments {
                if let AccessSegment::Index(index) = segment {
                    collect_address_taken_names_in_expr(index, names);
                }
            }
        }
        Place::Ident(_) | Place::Field { .. } => {}
    }
}

fn collect_address_taken_names_in_expr(expr: &Expr, names: &mut HashSet<String>) {
    match expr {
        Expr::AddressOf(name) => {
            names.insert(name.clone());
        }
        Expr::AddressOfIndex { name, index } => {
            names.insert(name.clone());
            collect_address_taken_names_in_expr(index, names);
        }
        Expr::AddressOfField { base, .. } => {
            names.insert(base.clone());
        }
        Expr::AddressOfAccess(path) => {
            names.insert(path.root.clone());
            for segment in &path.segments {
                if let AccessSegment::Index(index) = segment {
                    collect_address_taken_names_in_expr(index, names);
                }
            }
        }
        Expr::Array(values) => {
            for value in values {
                collect_address_taken_names_in_expr(value, names);
            }
        }
        Expr::StructInit { fields, .. } => {
            for (_, value) in fields {
                collect_address_taken_names_in_expr(value, names);
            }
        }
        Expr::Deref(value)
        | Expr::BankedPointer { pointer: value, .. }
        | Expr::Unary { expr: value, .. }
        | Expr::Cast { expr: value, .. } => collect_address_taken_names_in_expr(value, names),
        Expr::Binary { left, right, .. } => {
            collect_address_taken_names_in_expr(left, names);
            collect_address_taken_names_in_expr(right, names);
        }
        Expr::Call { args, .. } => {
            for argument in args {
                collect_address_taken_names_in_expr(argument, names);
            }
        }
        Expr::Access(path) => {
            for segment in &path.segments {
                if let AccessSegment::Index(index) = segment {
                    collect_address_taken_names_in_expr(index, names);
                }
            }
        }
        Expr::Index { index, .. } => collect_address_taken_names_in_expr(index, names),
        Expr::Int(_)
        | Expr::TypedInt(_, _)
        | Expr::Bool(_)
        | Expr::Char(_)
        | Expr::String(_)
        | Expr::Ident(_)
        | Expr::In(_)
        | Expr::Field { .. } => {}
    }
}

#[derive(Clone)]
struct PropagatedValue {
    ty: Type,
    expr: Expr,
}

fn propagate_block(
    stmts: Vec<Stmt>,
    assigned: &HashSet<String>,
    values: &mut HashMap<String, PropagatedValue>,
    available: &mut Vec<(Expr, String)>,
    context: &OptimizationContext,
    report: &mut TbirOptimizationReport,
) -> Vec<Stmt> {
    let mut output = Vec::with_capacity(stmts.len());
    for stmt in stmts {
        let stmt = match stmt {
            Stmt::Let { name, ty, value } => {
                let mut value = substitute_expr(value, values, report);
                value = reuse_available_expr(value, available, &name, context, report);
                let references_memory = expr_references_memory_object(&value, context);
                if is_cse_candidate(&value) && !references_memory {
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
                if !assigned.contains(&name) && is_pure_scalar(&value) && !references_memory {
                    values.insert(
                        name.clone(),
                        PropagatedValue {
                            ty: ty.clone(),
                            expr: value.clone(),
                        },
                    );
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
                    context,
                    report,
                );
                let else_body = propagate_block(
                    else_body,
                    assigned,
                    &mut else_values,
                    &mut Vec::new(),
                    context,
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
                let body = propagate_block(
                    body,
                    assigned,
                    &mut body_values,
                    &mut Vec::new(),
                    context,
                    report,
                );
                available.clear();
                Stmt::While { condition, body }
            }
            Stmt::Loop { body } => {
                let mut body_values = values.clone();
                let body = propagate_block(
                    body,
                    assigned,
                    &mut body_values,
                    &mut Vec::new(),
                    context,
                    report,
                );
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
            Stmt::Asm { ref outputs, .. } => {
                available.clear();
                for output in outputs {
                    values.remove(&output.name);
                }
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
    values: &HashMap<String, PropagatedValue>,
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
    values: &HashMap<String, PropagatedValue>,
    report: &mut TbirOptimizationReport,
) -> Expr {
    match expr {
        Expr::Ident(name) => {
            if let Some(value) = values.get(&name) {
                report.copy_propagations += 1;
                if matches!(
                    value.expr,
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
                match &value.expr {
                    Expr::Int(integer) | Expr::TypedInt(integer, _) => {
                        Expr::TypedInt(*integer, value.ty.clone())
                    }
                    Expr::Unary {
                        op: UnaryOp::Neg,
                        expr,
                    } => match expr.as_ref() {
                        Expr::Int(integer) | Expr::TypedInt(integer, _) => {
                            Expr::TypedInt(-*integer, value.ty.clone())
                        }
                        _ => Expr::Cast {
                            ty: value.ty.clone(),
                            expr: Box::new(value.expr.clone()),
                        },
                    },
                    Expr::Bool(boolean) if value.ty == Type::Named("bool".to_owned()) => {
                        Expr::Bool(*boolean)
                    }
                    Expr::Char(character) if value.ty == Type::Named("char".to_owned()) => {
                        Expr::Char(*character)
                    }
                    _ => Expr::Cast {
                        ty: value.ty.clone(),
                        expr: Box::new(value.expr.clone()),
                    },
                }
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

fn reuse_available_expr(
    expr: Expr,
    available: &[(Expr, String)],
    name: &str,
    context: &OptimizationContext,
    report: &mut TbirOptimizationReport,
) -> Expr {
    let expr = match expr {
        Expr::Unary { op, expr } => Expr::Unary {
            op,
            expr: Box::new(reuse_available_expr(
                *expr, available, name, context, report,
            )),
        },
        Expr::Binary { left, op, right } => Expr::Binary {
            left: Box::new(reuse_available_expr(
                *left, available, name, context, report,
            )),
            op,
            right: Box::new(reuse_available_expr(
                *right, available, name, context, report,
            )),
        },
        Expr::Cast { ty, expr } => Expr::Cast {
            ty,
            expr: Box::new(reuse_available_expr(
                *expr, available, name, context, report,
            )),
        },
        other => other,
    };

    if is_cse_candidate(&expr) && !expr_references_memory_object(&expr, context) {
        if let Some((_, prior)) = available.iter().find(|(prior, _)| prior == &expr) {
            report.common_subexpressions += 1;
            decision(
                report,
                TbirOptimizationKind::CommonSubexpression,
                None,
                name,
                None,
            );
            return Expr::Ident(prior.clone());
        }
    }
    expr
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

fn expr_references_memory_object(expr: &Expr, context: &OptimizationContext) -> bool {
    match expr {
        Expr::Ident(name)
        | Expr::Index { name, .. }
        | Expr::AddressOfIndex { name, .. }
        | Expr::AddressOf(name) => context.objects.contains_key(name),
        Expr::Field { base, .. } | Expr::AddressOfField { base, .. } => {
            context.objects.contains_key(base)
        }
        Expr::Access(path) | Expr::AddressOfAccess(path) => {
            context.objects.contains_key(&path.root)
                || path.segments.iter().any(|segment| match segment {
                    AccessSegment::Index(index) => expr_references_memory_object(index, context),
                    AccessSegment::Field(_) => false,
                })
        }
        Expr::Unary { expr, .. }
        | Expr::Cast { expr, .. }
        | Expr::Deref(expr)
        | Expr::BankedPointer { pointer: expr, .. } => expr_references_memory_object(expr, context),
        Expr::Binary { left, right, .. } => {
            expr_references_memory_object(left, context)
                || expr_references_memory_object(right, context)
        }
        Expr::Array(values) => values
            .iter()
            .any(|value| expr_references_memory_object(value, context)),
        Expr::StructInit { fields, .. } => fields
            .iter()
            .any(|(_, value)| expr_references_memory_object(value, context)),
        Expr::Call { args, .. } => args
            .iter()
            .any(|arg| expr_references_memory_object(arg, context)),
        Expr::Int(_)
        | Expr::TypedInt(_, _)
        | Expr::Bool(_)
        | Expr::Char(_)
        | Expr::String(_)
        | Expr::In(_) => false,
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

fn hoist_named_memory_reads_program(
    program: &mut Program,
    context: &OptimizationContext,
    report: &mut TbirOptimizationReport,
) {
    fn visit(
        declarations: &mut [Declaration],
        context: &OptimizationContext,
        report: &mut TbirOptimizationReport,
    ) {
        for declaration in declarations {
            match declaration {
                Declaration::Function(function) => {
                    let mut names: HashSet<String> = function
                        .params
                        .iter()
                        .map(|param| param.name.clone())
                        .collect();
                    collect_local_names(&function.body, &mut names);
                    let mut next_temp = 0;
                    let external = function
                        .params
                        .iter()
                        .map(|param| param.name.clone())
                        .collect();
                    function.body = memory_licm_block(
                        core::mem::take(&mut function.body),
                        &external,
                        &mut names,
                        &mut next_temp,
                        context,
                        report,
                    );
                }
                Declaration::Cfg { declaration, .. } | Declaration::Bank { declaration, .. } => {
                    visit(core::slice::from_mut(declaration), context, report)
                }
                _ => {}
            }
        }
    }
    visit(&mut program.declarations, context, report);
}

fn memory_licm_block(
    stmts: Vec<Stmt>,
    external: &HashSet<String>,
    used_names: &mut HashSet<String>,
    next_temp: &mut usize,
    context: &OptimizationContext,
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
            } => output.push(Stmt::If {
                condition,
                then_body: memory_licm_block(
                    then_body, &visible, used_names, next_temp, context, report,
                ),
                else_body: memory_licm_block(
                    else_body, &visible, used_names, next_temp, context, report,
                ),
            }),
            Stmt::While { condition, body } => {
                let (preheader, body) =
                    hoist_memory_loop_body(body, &visible, used_names, next_temp, context, report);
                output.extend(preheader);
                output.push(Stmt::While { condition, body });
            }
            Stmt::Loop { body } => {
                let (preheader, body) =
                    hoist_memory_loop_body(body, &visible, used_names, next_temp, context, report);
                output.extend(preheader);
                output.push(Stmt::Loop { body });
            }
            other => output.push(other),
        }
    }
    output
}

fn hoist_memory_loop_body(
    mut body: Vec<Stmt>,
    external: &HashSet<String>,
    used_names: &mut HashSet<String>,
    next_temp: &mut usize,
    context: &OptimizationContext,
    report: &mut TbirOptimizationReport,
) -> (Vec<Stmt>, Vec<Stmt>) {
    let mut assigned = HashSet::new();
    collect_assigned_names(&body, &mut assigned);
    let mut declared = HashSet::new();
    collect_local_names(&body, &mut declared);
    let barrier = memory_loop_barrier(&body, context);
    let mut named_writes = HashSet::new();
    collect_named_writes(&body, &mut named_writes);
    let mut preheader = Vec::new();
    for stmt in &mut body {
        let Stmt::Let { name, ty, value } = stmt else {
            continue;
        };
        let analysis = analyze_memory_initializer(value, context);
        if !analysis.memory_like {
            continue;
        }
        let reason = analysis
            .rejection
            .or_else(|| barrier.clone())
            .or_else(|| {
                analysis
                    .scalar_deps
                    .iter()
                    .any(|dep| assigned.contains(dep))
                    .then(|| "scalar dependency assigned in loop".to_owned())
            })
            .or_else(|| {
                analysis
                    .scalar_deps
                    .iter()
                    .any(|dep| declared.contains(dep) || !external.contains(dep))
                    .then(|| "scalar dependency declared in loop".to_owned())
            })
            .or_else(|| {
                analysis
                    .roots
                    .iter()
                    .any(|root| named_writes.contains(root))
                    .then(|| "same named object written in loop".to_owned())
            });
        if let Some(reason) = reason {
            decision(
                report,
                TbirOptimizationKind::MemoryReadLicm,
                None,
                name,
                Some(&reason),
            );
            continue;
        }
        let temp = unique_memory_licm_temp_name(used_names, next_temp);
        preheader.push(Stmt::Let {
            name: temp.clone(),
            ty: ty.clone(),
            value: value.clone(),
        });
        *value = Expr::Ident(temp);
        report.named_memory_reads_hoisted += 1;
        decision(
            report,
            TbirOptimizationKind::MemoryReadLicm,
            None,
            name,
            None,
        );
    }
    let mut nested_external = external.clone();
    nested_external.extend(declared);
    body = memory_licm_block(
        body,
        &nested_external,
        used_names,
        next_temp,
        context,
        report,
    );
    (preheader, body)
}

#[derive(Default)]
struct MemoryInitializerAnalysis {
    memory_like: bool,
    roots: HashSet<String>,
    scalar_deps: HashSet<String>,
    rejection: Option<String>,
}

fn analyze_memory_initializer(
    expr: &Expr,
    context: &OptimizationContext,
) -> MemoryInitializerAnalysis {
    fn reject(analysis: &mut MemoryInitializerAnalysis, reason: &str) {
        if analysis.rejection.is_none() {
            analysis.rejection = Some(reason.to_owned());
        }
    }
    fn root(name: &str, analysis: &mut MemoryInitializerAnalysis, context: &OptimizationContext) {
        analysis.memory_like = true;
        let Some(object) = context.objects.get(name) else {
            reject(analysis, "unknown named memory object");
            return;
        };
        if object.kind != TbirObjectKind::Global {
            reject(
                analysis,
                match object.kind {
                    TbirObjectKind::Mmio => "MMIO object",
                    TbirObjectKind::Embed => "embedded or read-only object",
                    _ => "non-global named object",
                },
            );
        } else if object.region.is_none() {
            reject(analysis, "object has no known region");
        } else if object.access != TbirAccess::ReadWrite {
            reject(analysis, "object or region is read-only");
        } else if object.volatile {
            reject(analysis, "object or region is volatile");
        } else {
            analysis.roots.insert(name.to_owned());
        }
    }
    fn walk(expr: &Expr, analysis: &mut MemoryInitializerAnalysis, context: &OptimizationContext) {
        match expr {
            Expr::Int(_) | Expr::TypedInt(_, _) | Expr::Bool(_) | Expr::Char(_) => {}
            Expr::Ident(name) if context.objects.contains_key(name) => {
                root(name, analysis, context)
            }
            Expr::Ident(name) => {
                analysis.scalar_deps.insert(name.clone());
            }
            Expr::Index { name, index } => {
                root(name, analysis, context);
                walk(index, analysis, context);
            }
            Expr::Field { base, .. } => root(base, analysis, context),
            Expr::Access(path) => {
                root(&path.root, analysis, context);
                for segment in &path.segments {
                    if let AccessSegment::Index(index) = segment {
                        walk(index, analysis, context);
                    }
                }
            }
            Expr::Unary { expr, .. } | Expr::Cast { expr, .. } => walk(expr, analysis, context),
            Expr::Binary { left, right, .. } => {
                walk(left, analysis, context);
                walk(right, analysis, context);
            }
            Expr::AddressOf(_)
            | Expr::AddressOfIndex { .. }
            | Expr::AddressOfField { .. }
            | Expr::AddressOfAccess(_) => {
                analysis.memory_like = true;
                reject(analysis, "address-taking expression");
            }
            Expr::Deref(_) => {
                analysis.memory_like = true;
                reject(analysis, "pointer dereference");
            }
            Expr::BankedPointer { .. } => {
                analysis.memory_like = true;
                reject(analysis, "banked pointer expression");
            }
            Expr::Call { .. } | Expr::In(_) => {
                analysis.memory_like = true;
                reject(analysis, "effectful expression");
            }
            Expr::String(_) | Expr::Array(_) | Expr::StructInit { .. } => {
                analysis.memory_like = true;
                reject(analysis, "aggregate expression");
            }
        }
    }
    let mut analysis = MemoryInitializerAnalysis::default();
    walk(expr, &mut analysis, context);
    analysis
}

fn unique_memory_licm_temp_name(used_names: &mut HashSet<String>, next: &mut usize) -> String {
    loop {
        let name = format!("__tbir_mem_licm_{}", *next);
        *next += 1;
        if used_names.insert(name.clone()) {
            return name;
        }
    }
}

fn collect_named_writes(stmts: &[Stmt], roots: &mut HashSet<String>) {
    for stmt in stmts {
        match stmt {
            Stmt::Assign { target, .. } => {
                let root = match target {
                    Place::Ident(name) | Place::Index { name, .. } => Some(name),
                    Place::Field { base, .. } => Some(base),
                    Place::Access(path) => Some(&path.root),
                    Place::Deref(_) => None,
                };
                if let Some(root) = root {
                    roots.insert(root.clone());
                }
            }
            Stmt::If {
                then_body,
                else_body,
                ..
            } => {
                collect_named_writes(then_body, roots);
                collect_named_writes(else_body, roots);
            }
            Stmt::While { body, .. } | Stmt::Loop { body } => collect_named_writes(body, roots),
            _ => {}
        }
    }
}

fn memory_loop_barrier(stmts: &[Stmt], context: &OptimizationContext) -> Option<String> {
    for stmt in stmts {
        let reason = match stmt {
            Stmt::Return(_) | Stmt::Break | Stmt::Continue => Some("loop contains explicit exit"),
            Stmt::Asm { .. } => Some("loop contains inline assembly"),
            Stmt::Out { .. } => Some("loop contains port I/O"),
            Stmt::Assign {
                target: Place::Deref(_),
                ..
            } => Some("loop contains pointer dereference"),
            Stmt::Assign { target, .. } if place_is_unknown_write(target, context) => {
                Some("loop contains unknown write")
            }
            _ => None,
        };
        if let Some(reason) = reason {
            return Some(reason.to_owned());
        }
        let expressions: Vec<&Expr> = match stmt {
            Stmt::Let { value, .. }
            | Stmt::Assign { value, .. }
            | Stmt::Expr(value)
            | Stmt::Out { value, .. } => vec![value],
            Stmt::Return(Some(value)) => vec![value],
            Stmt::If { condition, .. } | Stmt::While { condition, .. } => vec![condition],
            _ => Vec::new(),
        };
        for expr in expressions {
            if let Some(reason) = memory_expr_barrier(expr, context) {
                return Some(reason);
            }
        }
        match stmt {
            Stmt::If {
                then_body,
                else_body,
                ..
            } => {
                if let Some(reason) = memory_loop_barrier(then_body, context)
                    .or_else(|| memory_loop_barrier(else_body, context))
                {
                    return Some(reason);
                }
            }
            Stmt::While { body, .. } | Stmt::Loop { body } => {
                if let Some(reason) = memory_loop_barrier(body, context) {
                    return Some(reason);
                }
            }
            _ => {}
        }
    }
    None
}

fn place_is_unknown_write(place: &Place, context: &OptimizationContext) -> bool {
    match place {
        Place::Ident(_) => false,
        Place::Index { name, .. } | Place::Field { base: name, .. } => {
            !context.objects.contains_key(name)
        }
        Place::Access(path) => !context.objects.contains_key(&path.root),
        Place::Deref(_) => true,
    }
}

fn memory_expr_barrier(expr: &Expr, context: &OptimizationContext) -> Option<String> {
    match expr {
        Expr::Call { .. } => Some("loop contains call".to_owned()),
        Expr::In(_) => Some("loop contains port I/O".to_owned()),
        Expr::Deref(_) => Some("loop contains pointer dereference".to_owned()),
        Expr::AddressOf(_)
        | Expr::AddressOfIndex { .. }
        | Expr::AddressOfField { .. }
        | Expr::AddressOfAccess(_) => Some("loop contains address-taking".to_owned()),
        Expr::BankedPointer { .. } => Some("loop contains banked pointer".to_owned()),
        Expr::Ident(name) | Expr::Index { name, .. } => object_access_barrier(name, context),
        Expr::Field { base, .. } => object_access_barrier(base, context),
        Expr::Access(path) => object_access_barrier(&path.root, context),
        Expr::Unary { expr, .. } | Expr::Cast { expr, .. } => memory_expr_barrier(expr, context),
        Expr::Binary { left, right, .. } => {
            memory_expr_barrier(left, context).or_else(|| memory_expr_barrier(right, context))
        }
        Expr::Array(values) => values
            .iter()
            .find_map(|value| memory_expr_barrier(value, context)),
        Expr::StructInit { fields, .. } => fields
            .iter()
            .find_map(|(_, value)| memory_expr_barrier(value, context)),
        _ => None,
    }
}

fn object_access_barrier(name: &str, context: &OptimizationContext) -> Option<String> {
    context
        .objects
        .get(name)
        .and_then(|object| match object.kind {
            TbirObjectKind::Mmio => Some("loop accesses MMIO".to_owned()),
            TbirObjectKind::Embed => Some("loop accesses embedded data".to_owned()),
            _ => None,
        })
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
        (Expr::Bool(true), BinaryOp::And, value)
        | (Expr::Bool(false), BinaryOp::Or, value)
        | (value, BinaryOp::And, Expr::Bool(true))
        | (value, BinaryOp::Or, Expr::Bool(false)) => algebraic(value.clone()),
        (Expr::Bool(false), BinaryOp::And, _) | (Expr::Bool(true), BinaryOp::Or, _) => {
            algebraic(Expr::Bool(matches!(op, BinaryOp::Or)))
        }
        (value, BinaryOp::And, Expr::Bool(false)) if is_pure_scalar(value) => {
            algebraic(Expr::Bool(false))
        }
        (value, BinaryOp::Or, Expr::Bool(true)) if is_pure_scalar(value) => {
            algebraic(Expr::Bool(true))
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
        (UnaryOp::BitNot, Expr::TypedInt(value, ty)) => {
            let value = if type_is_unsigned_integer(ty) {
                (!value) & typed_integer_mask(ty)?
            } else {
                !value
            };
            Some(Expr::TypedInt(value, ty.clone()))
        }
        _ => None,
    }
}

fn type_is_unsigned_integer(ty: &Type) -> bool {
    matches!(ty, Type::Named(name) if matches!(name.as_str(), "u8" | "u16" | "u24" | "u32"))
}

fn typed_integer_mask(ty: &Type) -> Option<i64> {
    let bits = match ty {
        Type::Named(name) if matches!(name.as_str(), "u8" | "i8") => 8,
        Type::Named(name) if matches!(name.as_str(), "u16" | "i16") => 16,
        Type::Named(name) if matches!(name.as_str(), "u24" | "i24") => 24,
        Type::Named(name) if matches!(name.as_str(), "u32" | "i32") => 32,
        _ => return None,
    };
    Some((1_i64 << bits) - 1)
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
        (Expr::Int(left), Expr::Int(right)) => fold_integer_binary(*left, *right, op, None),
        (Expr::TypedInt(left, ty), Expr::TypedInt(right, _)) => {
            fold_integer_binary(*left, *right, op, Some(ty))
        }
        _ => None,
    }
}

fn fold_integer_binary(left: i64, right: i64, op: BinaryOp, ty: Option<&Type>) -> Option<Expr> {
    let value = match op {
        BinaryOp::BitAnd => left & right,
        BinaryOp::BitXor => left ^ right,
        BinaryOp::BitOr => left | right,
        _ => return None,
    };
    Some(match ty {
        Some(ty) => Expr::TypedInt(value, ty.clone()),
        None => Expr::Int(value),
    })
}

fn terminates(stmt: &Stmt) -> bool {
    matches!(stmt, Stmt::Return(_) | Stmt::Break | Stmt::Continue)
}

#[cfg(test)]
mod tests;
