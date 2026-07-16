use std::collections::HashMap;

use crate::ast::{
    AccessPath, AccessSegment, BinaryOp, Declaration, Expr, Function, Place, Program, Stmt, UnaryOp,
};

use super::TbirOptimizationReport;

pub fn optimize_program(program: &Program) -> (Program, TbirOptimizationReport) {
    let mut program = program.clone();
    let mut report = TbirOptimizationReport::default();
    for declaration in &mut program.declarations {
        optimize_declaration(declaration, &mut report);
    }
    (program, report)
}

fn optimize_declaration(declaration: &mut Declaration, report: &mut TbirOptimizationReport) {
    match declaration {
        Declaration::Cfg { declaration, .. } => optimize_declaration(declaration, report),
        Declaration::Function(function) => optimize_function(function, report),
        Declaration::Const(decl) => {
            decl.value = optimize_expr(
                std::mem::replace(&mut decl.value, Expr::Int(0)),
                &HashMap::new(),
                report,
            )
        }
        Declaration::Port(decl) => {
            decl.value = optimize_expr(
                std::mem::replace(&mut decl.value, Expr::Int(0)),
                &HashMap::new(),
                report,
            )
        }
        Declaration::Mmio(decl) => {
            decl.value = optimize_expr(
                std::mem::replace(&mut decl.value, Expr::Int(0)),
                &HashMap::new(),
                report,
            )
        }
        Declaration::Global(decl) => {
            decl.value = optimize_expr(
                std::mem::replace(&mut decl.value, Expr::Int(0)),
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
    if function.attrs.iter().any(|attr| attr == "inline") {
        report.inline_candidates.push(function.name.clone());
    }
    let mut constants = HashMap::new();
    function.body = optimize_stmts(std::mem::take(&mut function.body), &mut constants, report);
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
        (value, BinaryOp::Div, value_expr) => power_of_two_shift(value_expr)
            .map(|shift| shift_expr(value.clone(), BinaryOp::Shr, shift)),
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
