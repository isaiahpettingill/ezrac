use std::path::Path;

use crate::{parser::parse_program, target::CpuFamily};

use super::*;

#[test]
fn folds_simplifies_and_marks_dead_statements_without_skipping_validation() {
    let program = parse_program(
        Path::new("test.ezra"),
        "fn main() { let n: bool = !false let answer: bool = n return test.pass() test.fail(1) }",
    )
    .unwrap();
    let (program, report) = optimize_program(&program, CpuFamily::Ez80);
    let main = program.main_function().unwrap();

    assert_eq!(report.constant_folds, 1);
    assert_eq!(report.constant_propagations, 0);
    assert_eq!(report.dead_statements_marked, 1);
    assert!(matches!(
        main.body[0],
        Stmt::Let {
            value: Expr::Bool(true),
            ..
        }
    ));
    assert!(matches!(
        main.body[1],
        Stmt::Let {
            value: Expr::Ident(_),
            ..
        }
    ));
    assert_eq!(main.body.len(), 4);
}

#[test]
fn folds_constant_branches_without_hiding_validation() {
    let program = parse_program(
        Path::new("test.ezra"),
        "fn main() { if false { test.fail(1) } else { test.pass() } while false { test.fail(2) } }",
    )
    .unwrap();
    let (program, _report) = optimize_program(&program, CpuFamily::Ez80);
    let main = program.main_function().unwrap();

    assert_eq!(main.body.len(), 2);
    assert!(matches!(
        main.body[0],
        Stmt::If {
            condition: Expr::Bool(false),
            ..
        }
    ));
    assert!(matches!(
        main.body[1],
        Stmt::While {
            condition: Expr::Bool(false),
            ..
        }
    ));
}

#[test]
fn simplifies_identity_operations_on_runtime_values() {
    let program = parse_program(
        Path::new("test.ezra"),
        "fn helper(value: u8) -> u8 { let answer: u8 = value * 1 return answer } fn main() {}",
    )
    .unwrap();
    let (program, report) = optimize_program(&program, CpuFamily::Ez80);
    let helper = program
        .declarations
        .iter()
        .find_map(|declaration| match declaration {
            Declaration::Function(function) if function.name == "helper" => Some(function),
            _ => None,
        })
        .unwrap();

    assert!(report.algebraic_simplifications >= 1);
    assert!(matches!(
        helper.body[0],
        Stmt::Let {
            value: Expr::Ident(_),
            ..
        }
    ));
}

#[test]
fn records_inline_approval_and_mutual_recursion_rejection() {
    let program = parse_program(
        Path::new("test.ezra"),
        "inline fn leaf() -> u8 { return 1 } inline fn left() -> u8 { return right() } fn right() -> u8 { return left() }",
    )
    .unwrap();
    let (_, report) = optimize_program(&program, CpuFamily::Ez80);

    assert!(report.inline_function_names().contains("leaf"));
    assert!(report.decisions.iter().any(|decision| {
        decision.kind == TbirOptimizationKind::Inline
            && decision.callee == "left"
            && decision.outcome == TbirOptimizationOutcome::Rejected
            && decision.reason == "mutual recursion"
    }));
}

#[test]
fn rewrites_self_tail_recursion_with_simultaneous_temporaries() {
    let program = parse_program(
        Path::new("test.ezra"),
        "fn swap(a: u8, b: u8) -> u8 { if a == 0 { return b } return swap(b, a) }",
    )
    .unwrap();
    let (program, report) = optimize_program(&program, CpuFamily::Ez80);
    let swap = program
        .declarations
        .iter()
        .find_map(|declaration| match declaration {
            Declaration::Function(function) if function.name == "swap" => Some(function),
            _ => None,
        })
        .unwrap();
    let Stmt::Loop { body } = &swap.body[0] else {
        panic!("tail-recursive body was not wrapped in a loop");
    };

    assert!(matches!(body[0], Stmt::If { .. }));
    assert!(
        matches!(&body[1], Stmt::Let { name, value: Expr::Ident(value), .. }
        if name == "__tbir_tail_arg_0" && value == "b")
    );
    assert!(
        matches!(&body[2], Stmt::Let { name, value: Expr::Ident(value), .. }
        if name == "__tbir_tail_arg_1" && value == "a")
    );
    assert!(
        matches!(&body[3], Stmt::Assign { target: Place::Ident(name), value: Expr::Ident(value), .. }
        if name == "a" && value == "__tbir_tail_arg_0")
    );
    assert!(
        matches!(&body[4], Stmt::Assign { target: Place::Ident(name), value: Expr::Ident(value), .. }
        if name == "b" && value == "__tbir_tail_arg_1")
    );
    assert!(matches!(body[5], Stmt::Continue));
    assert!(report.decisions.iter().any(|decision| {
        decision.kind == TbirOptimizationKind::TailRecursion
            && decision.outcome == TbirOptimizationOutcome::Applied
            && decision.reason == "approved"
    }));
}

#[test]
fn approves_and_rejects_sibling_tail_edges() {
    let program = parse_program(
        Path::new("test.ezra"),
        "fn byte(x: u8) -> u8 { return x } fn word(x: u8) -> u16 { return x } fn good(x: u8) -> u8 { return byte(x) } fn bad(x: u8) -> u8 { return word(x) }",
    )
    .unwrap();
    let (_, report) = optimize_program(&program, CpuFamily::Ez80);

    assert!(
        report
            .tail_call_edges()
            .contains(&("good".to_owned(), "byte".to_owned()))
    );
    assert!(report.decisions.iter().any(|decision| {
        decision.kind == TbirOptimizationKind::TailCall
            && decision.caller.as_deref() == Some("bad")
            && decision.callee == "word"
            && decision.outcome == TbirOptimizationOutcome::Rejected
            && decision.reason == "return type mismatch"
    }));
}
