use std::path::Path;

use crate::parser::parse_program;

use super::*;

#[test]
fn folds_simplifies_and_marks_dead_statements_without_skipping_validation() {
    let program = parse_program(
        Path::new("test.ezra"),
        "fn main() { let n: bool = !false let answer: bool = n return test.pass() test.fail(1) }",
    )
    .unwrap();
    let (program, report) = optimize_program(&program);
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
    let (program, _report) = optimize_program(&program);
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
    let (program, report) = optimize_program(&program);
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
