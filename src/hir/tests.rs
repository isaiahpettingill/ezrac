use std::path::Path;

use crate::parser::parse_program;

use super::*;

#[test]
fn hir_marks_tail_recursion_and_loops() {
    let program = parse_program(
        Path::new("test.ezra"),
        r#"
                fn count(n: u8) -> u8 {
                    while n > 0 {
                        return count(n - 1)
                    }
                    return 0
                }
            "#,
    )
    .unwrap();
    let hir = HirProgram::from_ast(&program).unwrap();
    let function = hir
        .declarations
        .iter()
        .find_map(|decl| match decl {
            HirDeclaration::Function(function) => Some(function),
            _ => None,
        })
        .unwrap();
    assert!(function.analysis.recursive);
    assert_eq!(function.analysis.loop_candidates, 1);
}

#[test]
fn hir_drops_imports_and_marks_shared_library_candidates() {
    let program = parse_program(
        Path::new("lib.ezra"),
        r#"
                import sdk.mos
                pub const ANSWER: u8 = 42
                pub fn answer() -> u8 { return ANSWER }
            "#,
    )
    .unwrap();
    let hir = HirProgram::from_ast(&program).unwrap();

    assert_eq!(hir.analysis.function_count, 1);
    assert!(hir.analysis.shared_library_candidate);
    assert!(
        !hir.declarations
            .iter()
            .any(|decl| matches!(decl, HirDeclaration::Const(object) if object.name == "sdk"))
    );
    assert_eq!(
        hir.declarations
            .iter()
            .filter(|decl| matches!(decl, HirDeclaration::Function(_)))
            .count(),
        1
    );
}

#[test]
fn hir_records_tail_call_candidates_without_marking_non_recursive_calls_recursive() {
    let program = parse_program(
        Path::new("test.ezra"),
        r#"
                fn callee(v: u8) -> u8 { return v }
                fn caller(v: u8) -> u8 { return callee(v) }
            "#,
    )
    .unwrap();
    let hir = HirProgram::from_ast(&program).unwrap();
    let caller = hir
        .declarations
        .iter()
        .find_map(|decl| match decl {
            HirDeclaration::Function(function) if function.sig.name == "caller" => Some(function),
            _ => None,
        })
        .unwrap();

    assert!(!caller.analysis.recursive);
    assert!(!caller.analysis.tail_recursive);
    assert_eq!(caller.analysis.tail_call_candidates, ["callee"]);
}

#[test]
fn hir_dump_exposes_analysis_summary() {
    let program = parse_program(
        Path::new("lib.ezra"),
        "pub inline fn helper() -> u8 { return 1 }",
    )
    .unwrap();
    let hir = HirProgram::from_ast(&program).unwrap();
    let dump = hir.dump_text();

    assert!(dump.contains("HIR"), "{dump}");
    assert!(dump.contains("shared_library_candidate=true"), "{dump}");
    assert!(dump.contains("fn helper"), "{dump}");
}
