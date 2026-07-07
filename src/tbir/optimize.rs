use crate::ast::{BinaryOp, Declaration, Expr, Function, Program, Stmt, UnaryOp};

use super::TbirOptimizationReport;

pub fn optimize_program(program: &Program) -> (Program, TbirOptimizationReport) {
    let mut report = TbirOptimizationReport::default();
    for declaration in &program.declarations {
        analyze_declaration(declaration, &mut report);
    }
    (program.clone(), report)
}

fn analyze_declaration(declaration: &Declaration, report: &mut TbirOptimizationReport) {
    match declaration {
        Declaration::Function(function) => analyze_function(function, report),
        Declaration::Const(decl) => count_foldable_expr(&decl.value, report),
        Declaration::Port(decl) => count_foldable_expr(&decl.value, report),
        Declaration::Mmio(decl) => count_foldable_expr(&decl.value, report),
        Declaration::Global(decl) => count_foldable_expr(&decl.value, report),
        Declaration::Embed(_)
        | Declaration::Import(_)
        | Declaration::Alias(_)
        | Declaration::Struct(_)
        | Declaration::ExternAsmFunction(_) => {}
    }
}

fn analyze_function(function: &Function, report: &mut TbirOptimizationReport) {
    if function.attrs.iter().any(|attr| attr == "inline") {
        report.inline_candidates.push(function.name.clone());
    }
    analyze_stmts(&function.body, report);
}

fn analyze_stmts(stmts: &[Stmt], report: &mut TbirOptimizationReport) {
    let mut terminated = false;
    for stmt in stmts {
        if terminated {
            report.dead_statements_marked += 1;
            continue;
        }
        analyze_stmt(stmt, report);
        terminated = matches!(stmt, Stmt::Return(_) | Stmt::Break | Stmt::Continue);
    }
}

fn analyze_stmt(stmt: &Stmt, report: &mut TbirOptimizationReport) {
    match stmt {
        Stmt::Let { value, .. } | Stmt::Assign { value, .. } => count_foldable_expr(value, report),
        Stmt::If {
            condition,
            then_body,
            else_body,
        } => {
            count_foldable_expr(condition, report);
            analyze_stmts(then_body, report);
            analyze_stmts(else_body, report);
        }
        Stmt::While { condition, body } => {
            count_foldable_expr(condition, report);
            analyze_stmts(body, report);
        }
        Stmt::Loop { body } => analyze_stmts(body, report),
        Stmt::Return(Some(value)) | Stmt::Out { value, .. } | Stmt::Expr(value) => {
            count_foldable_expr(value, report)
        }
        Stmt::Return(None) | Stmt::Break | Stmt::Continue | Stmt::Asm { .. } => {}
    }
}

fn count_foldable_expr(expr: &Expr, report: &mut TbirOptimizationReport) {
    match expr {
        Expr::Unary { op, expr } => {
            count_foldable_expr(expr, report);
            if fold_unary(*op, expr).is_some() {
                report.constant_folds += 1;
            }
        }
        Expr::Binary { left, op, right } => {
            count_foldable_expr(left, report);
            count_foldable_expr(right, report);
            if fold_binary(left, *op, right).is_some() {
                report.constant_folds += 1;
            }
        }
        Expr::Array(values) => values
            .iter()
            .for_each(|value| count_foldable_expr(value, report)),
        Expr::Index { index, .. } | Expr::AddressOfIndex { index, .. } | Expr::Deref(index) => {
            count_foldable_expr(index, report)
        }
        Expr::Access(path) | Expr::AddressOfAccess(path) => {
            for segment in &path.segments {
                if let crate::ast::AccessSegment::Index(index) = segment {
                    count_foldable_expr(index, report);
                }
            }
        }
        Expr::StructInit { fields, .. } => fields
            .iter()
            .for_each(|(_, value)| count_foldable_expr(value, report)),
        Expr::Call { args, .. } => args.iter().for_each(|arg| count_foldable_expr(arg, report)),
        Expr::Cast { expr, .. } => count_foldable_expr(expr, report),
        Expr::Int(_)
        | Expr::TypedInt(_, _)
        | Expr::Bool(_)
        | Expr::Char(_)
        | Expr::String(_)
        | Expr::Ident(_)
        | Expr::In(_)
        | Expr::Field { .. }
        | Expr::AddressOfField { .. }
        | Expr::AddressOf(_) => {}
    }
}

fn fold_unary(op: UnaryOp, expr: &Expr) -> Option<Expr> {
    match (op, expr) {
        (UnaryOp::Neg, Expr::Int(value)) => value.checked_neg().map(Expr::Int),
        (UnaryOp::BitNot, Expr::Int(value)) => Some(Expr::Int(!value)),
        (UnaryOp::Not, Expr::Bool(value)) => Some(Expr::Bool(!value)),
        _ => None,
    }
}

fn fold_binary(left: &Expr, op: BinaryOp, right: &Expr) -> Option<Expr> {
    match (left, right) {
        (Expr::Int(left), Expr::Int(right)) => fold_int_binary(*left, op, *right),
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

fn fold_int_binary(left: i64, op: BinaryOp, right: i64) -> Option<Expr> {
    match op {
        BinaryOp::Mul => left.checked_mul(right).map(Expr::Int),
        BinaryOp::Div if right != 0 => left.checked_div(right).map(Expr::Int),
        BinaryOp::Mod if right != 0 => left.checked_rem(right).map(Expr::Int),
        BinaryOp::Add => left.checked_add(right).map(Expr::Int),
        BinaryOp::Sub => left.checked_sub(right).map(Expr::Int),
        BinaryOp::Shl if (0..64).contains(&right) => left.checked_shl(right as u32).map(Expr::Int),
        BinaryOp::Shr if (0..64).contains(&right) => left.checked_shr(right as u32).map(Expr::Int),
        BinaryOp::Lt => Some(Expr::Bool(left < right)),
        BinaryOp::Le => Some(Expr::Bool(left <= right)),
        BinaryOp::Gt => Some(Expr::Bool(left > right)),
        BinaryOp::Ge => Some(Expr::Bool(left >= right)),
        BinaryOp::Eq => Some(Expr::Bool(left == right)),
        BinaryOp::Ne => Some(Expr::Bool(left != right)),
        BinaryOp::BitAnd => Some(Expr::Int(left & right)),
        BinaryOp::BitXor => Some(Expr::Int(left ^ right)),
        BinaryOp::BitOr => Some(Expr::Int(left | right)),
        BinaryOp::And
        | BinaryOp::Or
        | BinaryOp::Div
        | BinaryOp::Mod
        | BinaryOp::Shl
        | BinaryOp::Shr => None,
    }
}
