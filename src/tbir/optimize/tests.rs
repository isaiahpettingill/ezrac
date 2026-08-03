use std::path::Path;

use crate::{
    asm::AssemblyOptions,
    hir::HirProgram,
    parser::parse_program,
    target::{Address24, CpuFamily},
    tbir::{TbirDeclaration, TbirProgram},
};

use super::*;

#[test]
fn folds_simplifies_and_removes_dead_statements() {
    let program = parse_program(
        Path::new("test.ezra"),
        "fn main() { let n: bool = !false let answer: bool = n return test.pass() test.fail(1) }",
    )
    .unwrap();
    let (program, report) = optimize_program(&program, CpuFamily::Ez80);
    let main = program.main_function().unwrap();

    assert_eq!(report.constant_folds, 1);
    assert_eq!(report.constant_propagations, 1);
    assert_eq!(report.dead_statements_marked, 1);
    assert_eq!(main.body.len(), 1);
    assert!(matches!(
        main.body[0],
        Stmt::Return(Some(Expr::Call { .. }))
    ));
}

#[test]
fn does_not_propagate_an_address_taken_local_past_a_call() {
    let program = parse_program(
        Path::new("test.ezra"),
        "fn set(value: ptr<u8>) { *value = 7 } fn main() { let choice: u8 = 0 set(&choice) let result: u8 = choice }",
    )
    .unwrap();
    let (program, _report) = optimize_program(&program, CpuFamily::I8086);
    let main = program.main_function().unwrap();

    assert!(matches!(&main.body[0], Stmt::Let { name, .. } if name == "choice"));
    assert!(!format!("{:?}", main.body).contains("result"));
}

#[test]
fn inline_asm_outputs_invalidate_propagated_locals() {
    let program = parse_program(
        Path::new("test.ezra"),
        r#"
            fn read() -> u8 {
                let result: u8 = 0
                asm volatile(out result: u8 as mem, clobber memory) { "mov {result},al" }
                return result
            }
            fn main() {}
        "#,
    )
    .unwrap();
    let (program, _report) = optimize_program(&program, CpuFamily::I8086);
    let read = program
        .declarations
        .iter()
        .find_map(|declaration| match declaration {
            Declaration::Function(function) if function.name == "read" => Some(function),
            _ => None,
        })
        .unwrap();

    assert!(
        matches!(read.body.last(), Some(Stmt::Return(Some(Expr::Ident(name)))) if name == "result")
    );
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
fn strength_reduces_unsigned_power_of_two_division_and_modulo() {
    let program = parse_program(
        Path::new("test.ezra"),
        "fn half(value: u16) -> u16 { return value / 8u16 } fn remainder(value: u8) -> u8 { return value % 8u8 } fn signed(value: i16) -> i16 { return value / 8i16 }",
    )
    .unwrap();
    let (program, report) = optimize_program(&program, CpuFamily::Ez80);

    assert!(matches!(
        function_named(&program, "half").body.last(),
        Some(Stmt::Return(Some(Expr::Binary {
            op: BinaryOp::Shr,
            right,
            ..
        }))) if matches!(right.as_ref(), Expr::TypedInt(3, Type::Named(ty)) if ty == "u16")
    ));
    assert!(matches!(
        function_named(&program, "remainder").body.last(),
        Some(Stmt::Return(Some(Expr::Binary {
            op: BinaryOp::BitAnd,
            right,
            ..
        }))) if matches!(right.as_ref(), Expr::TypedInt(7, Type::Named(ty)) if ty == "u8")
    ));
    assert!(matches!(
        function_named(&program, "signed").body.last(),
        Some(Stmt::Return(Some(Expr::Binary {
            op: BinaryOp::Div,
            ..
        })))
    ));
    assert_eq!(report.strength_reductions, 2);
}

#[test]
fn keeps_reused_nontrivial_binary_search_locals() {
    let program = parse_program(
        Path::new("test.ezra"),
        "fn search(start: u8, length: u8, input: ptr<u8>) -> u8 { let offset: u8 = length / 2u8 let mid: u8 = start + offset let value: u8 = *(input + mid) if value == 0 { return mid } return offset }",
    )
    .unwrap();
    let (program, _) = optimize_program(&program, CpuFamily::Mos6502);
    let search = function_named(&program, "search");

    assert!(matches!(
        &search.body[0],
        Stmt::Let {
            name,
            value: Expr::Binary { left, op: BinaryOp::Shr, .. },
            ..
        } if name == "offset" && matches!(left.as_ref(), Expr::Ident(value) if value == "length")
    ));
    assert!(matches!(
        &search.body[1],
        Stmt::Let {
            name,
            value: Expr::Binary { left, op: BinaryOp::Add, right },
            ..
        } if name == "mid"
            && matches!(left.as_ref(), Expr::Ident(value) if value == "start")
            && matches!(right.as_ref(), Expr::Ident(value) if value == "offset")
    ));
    let body = format!("{:?}", search.body);
    assert_eq!(body.matches("Ident(\"offset\")").count(), 2, "{body}");
    assert_eq!(body.matches("Ident(\"mid\")").count(), 2, "{body}");
}

#[test]
fn removes_unused_pure_lets_but_keeps_effectful_lets() {
    let program = parse_program(
        Path::new("test.ezra"),
        "fn side() -> u8 { return 1 } fn test(pointer: ptr<u8>) -> u8 { let unused: u8 = 1 + 2 let called: u8 = side() let loaded: u8 = *pointer return 7 side() }",
    )
    .unwrap();
    let (program, report) = optimize_program(&program, CpuFamily::Ez80);
    let test = function_named(&program, "test");

    assert_eq!(report.dead_statements_marked, 1);
    assert_eq!(test.body.len(), 3);
    assert!(
        matches!(&test.body[0], Stmt::Let { name, value: Expr::Call { .. }, .. } if name == "called")
    );
    assert!(
        matches!(&test.body[1], Stmt::Let { name, value: Expr::Deref(_), .. } if name == "loaded")
    );
    assert!(matches!(
        test.body[2],
        Stmt::Return(Some(Expr::TypedInt(7, _)))
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
        Stmt::Return(Some(Expr::Cast { ref expr, .. }))
            if matches!(expr.as_ref(), Expr::Ident(name) if name == "value")
    ));
}

#[test]
fn propagates_immutable_scalars_and_rejects_assigned_or_effectful_values() {
    let program = parse_program(
        Path::new("test.ezra"),
        "fn helper() -> u8 { return 7 } fn test(p: ptr<u8>, a: u8) -> u8 { let copy: u8 = a let constant: u8 = 3 let changed: u8 = a changed = 4 let call: u8 = helper() let memory: u8 = *p return copy + constant + changed + call + memory }",
    ).unwrap();
    let (program, report) = optimize_program(&program, CpuFamily::Ez80);
    let test = program
        .declarations
        .iter()
        .find_map(|d| match d {
            Declaration::Function(f) if f.name == "test" => Some(f),
            _ => None,
        })
        .unwrap();
    let Stmt::Return(Some(value)) = test.body.last().unwrap() else {
        panic!("missing return")
    };
    let text = format!("{value:?}");
    assert!(!text.contains("copy"));
    assert!(!text.contains("constant"));
    assert!(text.contains("changed"));
    assert!(text.contains("call"));
    assert!(text.contains("memory"));
    assert!(report.copy_propagations >= 2);
}

#[test]
fn simplifies_expressions_exposed_by_propagation() {
    let program = parse_program(
        Path::new("test.ezra"),
        "fn test() -> u8 { let one: u8 = 1 return one + 0 }",
    )
    .unwrap();
    let (program, report) = optimize_program(&program, CpuFamily::Ez80);
    let test = program
        .declarations
        .iter()
        .find_map(|declaration| match declaration {
            Declaration::Function(function) if function.name == "test" => Some(function),
            _ => None,
        })
        .unwrap();

    assert!(matches!(
        test.body[0],
        Stmt::Return(Some(Expr::TypedInt(1, Type::Named(ref name)))) if name == "u8"
    ));
    assert!(report.copy_propagations >= 1);
    assert!(report.algebraic_simplifications >= 1);
}

#[test]
fn performs_local_cse_and_clears_it_at_barriers() {
    let program = parse_program(Path::new("test.ezra"), "fn helper() {} fn consume(value: u8) {} fn test(a: u8, b: u8) { let first: u8 = a + b let second: u8 = a + b consume(second) helper() let third: u8 = a + b consume(third) }").unwrap();
    let (program, report) = optimize_program(&program, CpuFamily::Ez80);
    let test = program
        .declarations
        .iter()
        .find_map(|d| match d {
            Declaration::Function(f) if f.name == "test" => Some(f),
            _ => None,
        })
        .unwrap();
    assert!(matches!(&test.body[0], Stmt::Let { name, .. } if name == "first"));
    assert!(matches!(
        &test.body[1],
        Stmt::Expr(Expr::Call { args, .. })
            if format!("{:?}", args).contains("first")
    ));
    assert!(matches!(
        &test.body[3],
        Stmt::Expr(Expr::Call { args, .. })
            if format!("{:?}", args).contains("Binary")
    ));
    assert_eq!(report.common_subexpressions, 1);
}

#[test]
fn folds_typed_bitnot_and_the_integer_expression_it_exposes() {
    let program = parse_program(
        Path::new("test.ezra"),
        "fn test() -> u24 { let mask: u24 = ~1u24 return mask & 0xffu24 }",
    )
    .unwrap();
    let (program, report) = optimize_program(&program, CpuFamily::Ez80);
    let test = function_named(&program, "test");

    assert!(matches!(
        test.body[0],
        Stmt::Return(Some(Expr::TypedInt(254, Type::Named(ref name)))) if name == "u24"
    ));
    assert!(report.constant_folds >= 2);
}

#[test]
fn simplifies_width_aware_bitwise_masks_and_constant_chains() {
    let program = parse_program(
        Path::new("test.ezra"),
        "fn test(x: u24) -> u24 { let unchanged: u24 = x & 0xFFFFFFu24 let inverted: u24 = x ^ 0xFFFFFFu24 let narrowed: u24 = (x & 0x00FFFFu24) & 0x0000FFu24 return ~~narrowed }",
    )
    .unwrap();
    let (program, report) = optimize_program(&program, CpuFamily::Ez80);
    let test = function_named(&program, "test");

    assert!(matches!(
        test.body[0],
        Stmt::Return(Some(Expr::Cast { ref expr, .. }))
            if matches!(expr.as_ref(), Expr::Binary { op: BinaryOp::BitAnd, right, .. }
                if matches!(right.as_ref(), Expr::TypedInt(0xFF, Type::Named(name)) if name == "u24"))
    ));
    assert!(report.algebraic_simplifications >= 4);
}

#[test]
fn removes_masks_that_do_not_affect_explicit_unsigned_narrowing() {
    let program = parse_program(
        Path::new("test.ezra"),
        "fn test(word: u16, wide: u24, signed: i16, mask: u16) -> u16 { let low_word: u8 = cast<u8>(word & 0x00ffu16) let low_wide: u8 = cast<u8>(wide & 0x0000ffu24) let narrow_wide: u16 = cast<u16>(wide & 0x00ffffu24) let keep_signed: u8 = cast<u8>(signed & 0x00ffi16) let keep_bits: u8 = cast<u8>(word & 0x007fu16) let keep_dynamic: u8 = cast<u8>(word & mask) return narrow_wide }",
    )
    .unwrap();
    let (program, _) = optimize_program(&program, CpuFamily::Ez80);
    let test = function_named(&program, "test");

    assert_eq!(test.body.len(), 1);
    assert!(matches!(test.body[0], Stmt::Return(Some(_))));
    assert!(!format!("{:?}", test.body).contains("BitAnd"));
}

#[test]
fn folds_fully_known_one_bits_for_supported_local_integer_widths() {
    let program = parse_program(
        Path::new("test.ezra"),
        "fn byte(x: u8) -> u8 { let low: u8 = x | 0x0f return low | 0xf0 } fn word(x: u16) -> u16 { let low: u16 = x | 0x00ff return low | 0xff00 } fn triple(x: u24) -> u24 { let low: u24 = x | 0x0000ff return low | 0xffff00 } fn zero(x: u8) -> u8 { let low: u8 = x & 0x0f return low & 0xf0 }",
    )
    .unwrap();
    let (program, _) = optimize_program(&program, CpuFamily::Ez80);

    for (name, ty, value) in [
        ("byte", "u8", 0xff),
        ("word", "u16", 0xffff),
        ("triple", "u24", 0xffffff),
    ] {
        let function = function_named(&program, name);
        assert!(
            matches!(function.body.last(), Some(Stmt::Return(Some(Expr::TypedInt(actual, Type::Named(actual_ty))))) if *actual == value && actual_ty == ty),
            "{name}: {:?}",
            function.body
        );
    }

    let zero = function_named(&program, "zero");
    assert!(matches!(
        zero.body.last(),
        Some(Stmt::Return(Some(Expr::TypedInt(0, Type::Named(ty))))) if ty == "u8"
    ));
}

#[test]
fn range_facts_fold_pure_overshifts_and_extend_known_bits_to_u32() {
    let program = parse_program(
        Path::new("test.ezra"),
        "fn unsigned(value: u8) -> u8 { let count: u8 = 8 return value << count } fn signed() -> i8 { let value: i8 = -1i8 return value >> 8 } fn wide(value: u32) -> u32 { let high: u32 = value | 0xffff0000u32 return high | 0x0000ffffu32 }",
    )
    .unwrap();
    let (program, _) = optimize_program(&program, CpuFamily::Ez80);

    let unsigned = function_named(&program, "unsigned");
    assert!(matches!(
        unsigned.body.last(),
        Some(Stmt::Return(Some(Expr::TypedInt(0, Type::Named(ty))))) if ty == "u8"
    ));

    let signed = function_named(&program, "signed");
    assert!(matches!(
        signed.body.last(),
        Some(Stmt::Return(Some(Expr::TypedInt(-1, Type::Named(ty))))) if ty == "i8"
    ));

    let wide = function_named(&program, "wide");
    assert!(matches!(
        wide.body.last(),
        Some(Stmt::Return(Some(Expr::TypedInt(0xffff_ffff, Type::Named(ty))))) if ty == "u32"
    ));
}

#[test]
fn range_facts_do_not_narrow_volatile_memory_reads() {
    let program = parse_program(
        Path::new("test.ezra"),
        "volatile mmio STATUS: ptr<u16> = 0x080000 fn test() -> u8 { return cast<u8>(*STATUS + 1) }",
    )
    .unwrap();
    let (program, _) = optimize_program(&program, CpuFamily::Ez80);
    let test = function_named(&program, "test");

    assert!(matches!(
        test.body.last(),
        Some(Stmt::Return(Some(Expr::Cast { expr, .. })))
            if matches!(expr.as_ref(), Expr::Binary { op: BinaryOp::Add, left, .. }
                if matches!(left.as_ref(), Expr::Deref(pointer)
                    if matches!(pointer.as_ref(), Expr::Ident(name) if name == "STATUS")))
    ));
}

#[test]
fn known_bits_keeps_address_taken_locals_and_branch_facts_local() {
    let program = parse_program(
        Path::new("test.ezra"),
        "fn mutate(value: ptr<u8>) { *value = 0 } fn addressed(x: u8) -> u8 { let low: u8 = x | 0x0f mutate(&low) return low | 0xf0 } fn branches(x: u8, flag: bool) -> u8 { let result: u8 = x if flag { let low: u8 = result & 0x0f result = low } return result | 0xf0 }",
    )
    .unwrap();
    let (program, _) = optimize_program(&program, CpuFamily::Ez80);

    for name in ["addressed", "branches"] {
        let function = function_named(&program, name);
        assert!(
            matches!(
                function.body.last(),
                Some(Stmt::Return(Some(Expr::Binary {
                    op: BinaryOp::BitOr,
                    ..
                })))
            ),
            "{name}: {:?}",
            function.body
        );
    }
}

#[test]
fn simplifies_safe_short_circuit_boolean_and_control_expressions() {
    let program = parse_program(
        Path::new("test.ezra"),
        "fn side() -> bool { return true } fn test(flag: bool) { let skipped: bool = false && side() let selected: bool = true && flag if true || side() { return } while false && side() {} }",
    )
    .unwrap();
    let (program, report) = optimize_program(&program, CpuFamily::Ez80);
    let test = function_named(&program, "test");

    assert!(matches!(
        test.body[0],
        Stmt::If {
            condition: Expr::Bool(true),
            ..
        }
    ));
    assert!(matches!(
        test.body[1],
        Stmt::While {
            condition: Expr::Bool(false),
            ..
        }
    ));
    assert!(report.algebraic_simplifications >= 4);
}

#[test]
fn reuses_pure_subexpressions_inside_later_expressions() {
    let program = parse_program(
        Path::new("test.ezra"),
        "fn test(a: u8, b: u8) -> u8 { let sum: u8 = a + b let doubled: u8 = (a + b) + (a + b) return doubled }",
    )
    .unwrap();
    let (program, report) = optimize_program(&program, CpuFamily::Ez80);
    let test = function_named(&program, "test");

    assert!(matches!(
        test.body[1],
        Stmt::Return(Some(Expr::Cast { ref expr, .. }))
            if matches!(expr.as_ref(), Expr::Binary { left, right, .. }
                if matches!(left.as_ref(), Expr::Ident(name) if name == "sum")
                    && matches!(right.as_ref(), Expr::Ident(name) if name == "sum"))
    ));
    assert!(report.common_subexpressions >= 2);
}

#[test]
fn hoists_pure_loop_invariants_but_not_assigned_dependencies_or_effects() {
    let program = parse_program(Path::new("test.ezra"), "fn helper() -> u8 { return 1 } fn test(p: ptr<u8>, limit: u8) { let i: u8 = 0 while i < limit { let invariant: u8 = limit + 1 let changing: u8 = i + 1 let call: u8 = helper() let memory: u8 = *p i += 1 } }").unwrap();
    let (program, report) = optimize_program(&program, CpuFamily::Ez80);
    let test = program
        .declarations
        .iter()
        .find_map(|d| match d {
            Declaration::Function(f) if f.name == "test" => Some(f),
            _ => None,
        })
        .unwrap();
    let body = format!("{:?}", test.body);
    assert!(body.contains("Call"), "{body}");
    assert!(body.contains("Deref"), "{body}");
    assert_eq!(report.loop_invariants_hoisted, 1);
    assert!(
        report
            .decisions
            .iter()
            .any(|d| d.kind == TbirOptimizationKind::LoopInvariantCodeMotion
                && d.outcome == TbirOptimizationOutcome::Rejected
                && d.reason == "dependency assigned in loop")
    );
}

#[test]
fn licm_temp_names_do_not_collide() {
    let program = parse_program(
        Path::new("test.ezra"),
        "fn test(a: u8) { let __tbir_licm_0: u8 = 0 loop { let invariant: u8 = a + 1 } }",
    )
    .unwrap();
    let (program, report) = optimize_program(&program, CpuFamily::Ez80);
    let _test = program.main_function().unwrap_or_else(|| {
        program
            .declarations
            .iter()
            .find_map(|d| match d {
                Declaration::Function(f) => Some(f),
                _ => None,
            })
            .unwrap()
    });
    assert_eq!(report.loop_invariants_hoisted, 1);
}

fn memory_context() -> OptimizationContext {
    use crate::tbir::{TbirAccess, TbirMemoryObject, TbirObjectKind};
    let array_type = Type::Array {
        element: Box::new(Type::Named("u8".to_owned())),
        len: Box::new(Expr::Int(8)),
    };
    let mut objects = vec![
        TbirMemoryObject {
            name: "first".to_owned(),
            kind: TbirObjectKind::Global,
            ty: array_type.clone(),
            address: 0x40000,
            size: 8,
            region: Some("ram".to_owned()),
            access: TbirAccess::ReadWrite,
            volatile: false,
        },
        TbirMemoryObject {
            name: "second".to_owned(),
            kind: TbirObjectKind::Global,
            ty: array_type.clone(),
            address: 0x40008,
            size: 8,
            region: Some("ram".to_owned()),
            access: TbirAccess::ReadWrite,
            volatile: false,
        },
    ];
    for (name, kind, access, volatile, region) in [
        (
            "STATUS",
            TbirObjectKind::Mmio,
            TbirAccess::ReadWrite,
            true,
            Some("vram"),
        ),
        (
            "PLAIN_MMIO",
            TbirObjectKind::Mmio,
            TbirAccess::ReadWrite,
            false,
            Some("ram"),
        ),
        (
            "blob",
            TbirObjectKind::Embed,
            TbirAccess::ReadOnly,
            false,
            Some("assets"),
        ),
        (
            "readonly",
            TbirObjectKind::Global,
            TbirAccess::ReadOnly,
            false,
            Some("rom"),
        ),
        (
            "volatile_global",
            TbirObjectKind::Global,
            TbirAccess::ReadWrite,
            true,
            Some("vram"),
        ),
        (
            "unplaced",
            TbirObjectKind::Global,
            TbirAccess::ReadWrite,
            false,
            None,
        ),
    ] {
        objects.push(TbirMemoryObject {
            name: name.to_owned(),
            kind,
            ty: array_type.clone(),
            address: 0,
            size: 8,
            region: region.map(str::to_owned),
            access,
            volatile,
        });
    }
    OptimizationContext::from_objects(&objects)
}

fn optimize_memory(source: &str) -> (Program, TbirOptimizationReport) {
    let program = parse_program(Path::new("test.ezra"), source).unwrap();
    optimize_program_with_context(&program, CpuFamily::Ez80, &memory_context())
}

#[test]
fn hoists_named_global_reads_and_preserves_order_and_temp_names() {
    let (program, report) = optimize_memory(
        "fn test(i: u8) { let __tbir_mem_licm_0: u8 = 0 loop { let scalar: u8 = first let indexed: u8 = first[2] let arithmetic: u8 = first[i] + 1 } }",
    );
    let function = program
        .declarations
        .iter()
        .find_map(|decl| match decl {
            Declaration::Function(value) => Some(value),
            _ => None,
        })
        .unwrap();
    assert!(matches!(function.body.last(), Some(Stmt::Loop { .. })));
    assert_eq!(report.named_memory_reads_hoisted, 3);
}

#[test]
fn named_write_blocks_only_reads_from_the_same_global() {
    let (same, same_report) =
        optimize_memory("fn test() { loop { let value: u8 = first[0] first[1] = 2 } }");
    assert_eq!(same_report.named_memory_reads_hoisted, 0);
    assert!(same_report.decisions.iter().any(|decision| decision.kind
        == TbirOptimizationKind::MemoryReadLicm
        && decision.reason == "same named object written in loop"));
    let (_, different_report) =
        optimize_memory("fn test() { loop { let value: u8 = first[0] second[1] = 2 } }");
    assert_eq!(different_report.named_memory_reads_hoisted, 1);
    let _ = same;
}

#[test]
fn rejects_loop_varying_indexes_and_memory_barriers() {
    let cases = [
        (
            "fn helper() {} fn test() { loop { let value: u8 = first[0] helper() } }",
            "loop contains call",
        ),
        (
            "port P: u8 = 1 fn test() { loop { let value: u8 = first[0] out P, 1 } }",
            "loop contains port I/O",
        ),
        (
            "fn test(p: ptr<u8>) { loop { let value: u8 = first[0] let other: u8 = *p } }",
            "loop contains pointer dereference",
        ),
        (
            "fn test() { loop { let value: u8 = first[0] let address: ptr<u8> = &first } }",
            "loop contains address-taking",
        ),
    ];
    for (source, reason) in cases {
        let (_, report) = optimize_memory(source);
        assert_eq!(report.named_memory_reads_hoisted, 0, "{source}");
        assert!(
            report
                .decisions
                .iter()
                .any(|decision| decision.reason == reason),
            "{source}"
        );
    }
    let (_, report) =
        optimize_memory("fn test() { let i: u8 = 0 loop { let value: u8 = first[i] i += 1 } }");
    assert_eq!(report.named_memory_reads_hoisted, 0);
    assert!(
        report
            .decisions
            .iter()
            .any(|decision| decision.reason == "scalar dependency assigned in loop")
    );
}

#[test]
fn rejects_unsafe_memory_object_classes_and_alias_barriers() {
    let cases = [
        (
            "fn test() { loop { let value: u8 = STATUS[0] } }",
            "MMIO object",
        ),
        (
            "fn test() { loop { let value: u8 = PLAIN_MMIO[0] } }",
            "MMIO object",
        ),
        (
            "fn test() { loop { let value: u8 = blob[0] } }",
            "embedded or read-only object",
        ),
        (
            "fn test() { loop { let value: u8 = readonly[0] } }",
            "object or region is read-only",
        ),
        (
            "fn test() { loop { let value: u8 = volatile_global[0] } }",
            "object or region is volatile",
        ),
        (
            "fn test() { loop { let value: u8 = unplaced[0] } }",
            "object has no known region",
        ),
        (
            "fn helper() {} fn test() { loop { let value: u8 = first[0] helper() } }",
            "loop contains call",
        ),
        (
            "fn test(p: ptr<u8>) { loop { let value: u8 = first[0]; *p = 1 } }",
            "loop contains pointer dereference",
        ),
    ];

    for (source, reason) in cases {
        let (_, report) = optimize_memory(source);
        assert_eq!(report.named_memory_reads_hoisted, 0, "{source}");
        assert!(
            report.decisions.iter().any(|decision| decision.kind
                == TbirOptimizationKind::MemoryReadLicm
                && decision.outcome == TbirOptimizationOutcome::Rejected
                && decision.reason == reason),
            "{source}: {report:?}"
        );
    }
}

#[test]
fn records_inline_approval_and_mutual_recursion_rejection() {
    let program = parse_program(
        Path::new("test.ezra"),
        "inline fn leaf() -> u8 { return 1 } inline fn left() -> u8 { return right() } fn right() -> u8 { return left() } fn use_leaf() -> u8 { return leaf() }",
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

fn function_named<'a>(program: &'a Program, name: &str) -> &'a Function {
    program
        .declarations
        .iter()
        .find_map(|declaration| match declaration {
            Declaration::Function(function) if function.name == name => Some(function),
            _ => None,
        })
        .unwrap()
}

#[test]
fn expands_value_void_nested_calls_and_alpha_renames_locals() {
    let program = parse_program(
        Path::new("test.ezra"),
        "inline fn one(x: u8) -> u8 { let local: u8 = x + 1 return local } inline fn two(x: u8) -> u8 { return one(x) + one(x) } inline fn sink(x: u8) { let local: u8 = x } fn test(local: u8) -> u8 { sink(local) return two(local) }",
    )
    .unwrap();
    let (program, report) = optimize_program(&program, CpuFamily::Dcpu);
    let test = function_named(&program, "test");
    let text = format!("{:?}", test.body);

    assert!(!text.contains("Call"), "{text}");
    assert!(text.contains("__tbir_inline_"), "{text}");
    assert!(report.decisions.iter().any(|decision| {
        decision.kind == TbirOptimizationKind::Inline
            && decision.callee == "one"
            && decision.reason.contains("transformed")
    }));
    assert!(report.inline_function_names().contains("sink"));
}

#[test]
fn evaluates_inline_arguments_once_in_left_to_right_typed_temporaries() {
    let program = parse_program(
        Path::new("test.ezra"),
        "fn first() -> u8 { return 1 } fn second() -> u8 { return 2 } inline fn pair(a: u8, b: u8) -> u8 { return a + b } fn test() -> u8 { return pair(first(), second()) }",
    )
    .unwrap();
    let (program, _) = optimize_program(&program, CpuFamily::Dcpu);
    let text = format!("{:?}", function_named(&program, "test").body);

    assert_eq!(text.matches("first").count(), 1, "{text}");
    assert_eq!(text.matches("second").count(), 1, "{text}");
    assert!(
        text.find("first").unwrap() < text.find("second").unwrap(),
        "{text}"
    );
    assert!(text.contains("Named(\"u8\")"), "{text}");
}

#[test]
fn preserves_inline_calls_in_short_circuit_rhs_and_while_conditions() {
    let program = parse_program(
        Path::new("test.ezra"),
        "inline fn yes() -> bool { return true } fn test(flag: bool) { let value: bool = flag && yes() while yes() { return } }",
    )
    .unwrap();
    let (program, report) = optimize_program(&program, CpuFamily::Dcpu);
    let text = format!("{:?}", function_named(&program, "test").body);

    assert_eq!(text.matches("yes").count(), 2, "{text}");
    assert!(!report.inline_function_names().contains("yes"));
}

#[test]
fn rejects_recursive_asm_and_control_exit_inline_functions() {
    let program = parse_program(
        Path::new("test.ezra"),
        "inline fn recurse() -> u8 { return recurse() } inline fn exits() -> u8 { loop { break } return 1 } inline fn raw() { asm { \"nop\" } } fn test() -> u8 { return recurse() }",
    )
    .unwrap();
    let (program, report) = optimize_program(&program, CpuFamily::Dcpu);

    assert!(format!("{:?}", function_named(&program, "test").body).contains("recurse"));
    assert!(
        report
            .decisions
            .iter()
            .any(|d| d.callee == "recurse" && d.reason == "direct recursion")
    );
    assert!(
        report
            .decisions
            .iter()
            .any(|d| d.callee == "exits" && d.reason == "inline assembly or control exit")
    );
    assert!(
        report
            .decisions
            .iter()
            .any(|d| d.callee == "raw" && d.reason == "inline assembly or control exit")
    );
}

#[test]
fn leaves_global_initializer_calls_unchanged() {
    let program = parse_program(
        Path::new("test.ezra"),
        "inline fn value() -> u8 { return 7 } global answer: u8 = value() fn test() -> u8 { return value() }",
    )
    .unwrap();
    let (program, _) = optimize_program(&program, CpuFamily::Dcpu);
    let text = format!("{:?}", program.declarations);

    assert!(text.contains("Global"));
    assert!(text.contains("Call { path: [\"value\"]"), "{text}");
    assert!(!format!("{:?}", function_named(&program, "test").body).contains("Call"));
}

#[test]
fn rebuilds_dcpu_tbir_declarations_from_the_transformed_program() {
    let program = parse_program(
        Path::new("test.ezra"),
        "inline fn value(x: u16) -> u16 { return x + 1 } fn test() -> u16 { return value(2) }",
    )
    .unwrap();
    let hir = HirProgram::from_ast(&program).unwrap();
    let options = AssemblyOptions {
        cpu: CpuFamily::Dcpu,
        load_addr: Address24::new(0x0100),
        entry_addr: Address24::new(0x0100),
        code_base: Address24::new(0x0100),
        rodata_base: Address24::new(0x1000),
        ram_base: Address24::new(0x2000),
        vram_base: Address24::new(0x3000),
        audio_base: Address24::new(0x4000),
        asset_base: Address24::new(0x5000),
        stack_top: Address24::new(0xff00),
        ..AssemblyOptions::default()
    };
    let tbir = TbirProgram::lower(&hir, &program, &options).unwrap();
    let lowered = format!("{:?}", function_named(&tbir.lowered_program, "test").body);
    let declaration = tbir
        .declarations
        .iter()
        .find(|declaration| matches!(declaration, TbirDeclaration::Function { name, .. } if name == "test"))
        .unwrap();

    assert!(!lowered.contains("Call"), "{lowered}");
    assert!(!format!("{declaration:?}").contains("Call"));
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
fn rewrites_void_self_tail_calls_after_if_with_ordered_temporaries() {
    let program = parse_program(
        Path::new("test.ezra"),
        "fn walk(first: u8, second: u8) { if first == 0 { return } walk(second, first) }",
    )
    .unwrap();
    let (program, report) = optimize_program(&program, CpuFamily::I8086);
    let walk = function_named(&program, "walk");
    let Stmt::Loop { body } = &walk.body[0] else {
        panic!("void tail recursion was not rewritten");
    };

    assert!(matches!(body[0], Stmt::If { .. }));
    assert!(matches!(
        &body[1],
        Stmt::Let {
            name,
            value: Expr::Ident(value),
            ..
        } if name == "__tbir_tail_arg_0" && value == "second"
    ));
    assert!(matches!(
        &body[2],
        Stmt::Let {
            name,
            value: Expr::Ident(value),
            ..
        } if name == "__tbir_tail_arg_1" && value == "first"
    ));
    assert!(matches!(
        &body[3],
        Stmt::Assign {
            target: Place::Ident(name),
            value: Expr::Ident(value),
            ..
        } if name == "first" && value == "__tbir_tail_arg_0"
    ));
    assert!(matches!(
        &body[4],
        Stmt::Assign {
            target: Place::Ident(name),
            value: Expr::Ident(value),
            ..
        } if name == "second" && value == "__tbir_tail_arg_1"
    ));
    assert!(matches!(body[5], Stmt::Continue));
    assert!(report.decisions.iter().any(|decision| {
        decision.kind == TbirOptimizationKind::TailRecursion
            && decision.caller.as_deref() == Some("walk")
            && decision.outcome == TbirOptimizationOutcome::Applied
    }));
}

#[test]
fn keeps_void_self_calls_out_of_non_tail_positions_and_loops() {
    let program = parse_program(
        Path::new("test.ezra"),
        "fn after(value: u8) { after(value) test.pass() } fn inside(value: u8) { while value > 0 { inside(value - 1) } } fn unreachable(value: u8) { loop { return } unreachable(value) }",
    )
    .unwrap();
    let (program, report) = optimize_program(&program, CpuFamily::I8086);

    let after = function_named(&program, "after");
    assert!(
        !after
            .body
            .iter()
            .any(|stmt| matches!(stmt, Stmt::Loop { .. }))
    );
    assert!(matches!(
        after.body.first(),
        Some(Stmt::Expr(Expr::Call { path, .. })) if path.last().is_some_and(|name| name == "after")
    ));
    assert!(matches!(
        after.body.get(1),
        Some(Stmt::Expr(Expr::Call { .. }))
    ));

    let inside = function_named(&program, "inside");
    assert!(matches!(inside.body.first(), Some(Stmt::While { body, .. })
        if matches!(body.first(), Some(Stmt::Expr(Expr::Call { path, .. }))
            if path.last().is_some_and(|name| name == "inside"))));

    let unreachable = function_named(&program, "unreachable");
    assert!(matches!(
        unreachable.body.get(1),
        Some(Stmt::Expr(Expr::Call { path, .. }))
            if path.last().is_some_and(|name| name == "unreachable")
    ));
    assert!(!report.decisions.iter().any(|decision| {
        decision.kind == TbirOptimizationKind::TailRecursion
            && decision
                .caller
                .as_deref()
                .is_some_and(|caller| caller == "after" || caller == "inside")
    }));
}

#[test]
fn preserves_void_tail_fallthrough_after_a_final_if_branch() {
    let program = parse_program(
        Path::new("test.ezra"),
        "fn maybe(value: u8) { if value > 0 { maybe(value - 1) } }",
    )
    .unwrap();
    let (program, _) = optimize_program(&program, CpuFamily::I8086);
    let maybe = function_named(&program, "maybe");
    let Stmt::Loop { body } = &maybe.body[0] else {
        panic!("void tail recursion was not rewritten");
    };

    assert!(matches!(body.last(), Some(Stmt::Break)));
    assert!(matches!(
        body.first(),
        Some(Stmt::If {
            then_body,
            else_body,
            ..
        }) if matches!(then_body.last(), Some(Stmt::Continue)) && else_body.is_empty()
    ));
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
