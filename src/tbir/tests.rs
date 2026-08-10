use std::path::Path;

use crate::{asm::AssemblyOptions, ast::BinaryOp, hir::HirProgram, parser::parse_program};

use crate::tbir::model::SemanticModel;
use crate::tbir::*;

#[test]
fn semantic_model_uses_four_byte_u32_and_i32_widths() {
    let program = parse_program(Path::new("test.ezra"), "fn main() {}").unwrap();
    let model = SemanticModel::from_program(&program, 24, 0x040000, 0x020000, 0x100000).unwrap();

    assert_eq!(model.type_width(&Type::Named("u32".to_owned())).unwrap(), 4);
    assert_eq!(model.type_width(&Type::Named("i32".to_owned())).unwrap(), 4);
}

#[test]
fn semantic_model_uses_target_width_for_raw_ptr() {
    let program = parse_program(Path::new("test.ezra"), "fn main() {}").unwrap();
    for (pointer_width_bits, expected_bytes) in [(16, 2), (24, 3)] {
        let model =
            SemanticModel::from_program(&program, pointer_width_bits, 0x040000, 0x020000, 0x100000)
                .unwrap();
        assert_eq!(
            model.type_width(&Type::Named("ptr".to_owned())).unwrap(),
            expected_bytes
        );
    }
}

#[test]
fn tbir_binds_ez80_memory_model() {
    let program = parse_program(Path::new("test.ezra"), "fn main() {}").unwrap();
    let hir = HirProgram::from_ast(&program).unwrap();
    let tbir = TbirProgram::for_ez80(&hir, &program, &AssemblyOptions::default()).unwrap();
    assert_eq!(tbir.target.pointer_width_bits, 24);
    assert!(
        tbir.memory
            .regions
            .iter()
            .any(|region| region.name == "vram")
    );
}

#[test]
fn tbir_uses_cpu_capabilities_for_target_metadata() {
    let program = parse_program(Path::new("test.ezra"), "fn main() {}").unwrap();
    let hir = HirProgram::from_ast(&program).unwrap();
    for (cpu, name, pointer_width) in [
        (crate::target::CpuFamily::Ez80, "ez80-adl", 24),
        (crate::target::CpuFamily::Z80, "z80", 16),
        (crate::target::CpuFamily::Z80N, "z80n", 16),
        (crate::target::CpuFamily::Z180, "z180", 16),
        (crate::target::CpuFamily::I8080, "i8080", 16),
        (crate::target::CpuFamily::I8085, "i8085", 16),
    ] {
        let tbir = TbirProgram::lower(
            &hir,
            &program,
            &AssemblyOptions {
                cpu,
                ..AssemblyOptions::default()
            },
        )
        .unwrap();
        assert_eq!(tbir.target.name, name);
        assert_eq!(tbir.target.pointer_width_bits, pointer_width);
        assert!(tbir.target.supports_port_io);
    }
}

#[test]
fn tbir_lowers_port_read_and_write_for_each_supported_cpu() {
    let program = parse_program(
        Path::new("test.ezra"),
        r#"
                port INPUT: u8 = 0x10
                port OUTPUT: u8 = 0x11
                fn main() { out OUTPUT, in INPUT }
            "#,
    )
    .unwrap();
    let hir = HirProgram::from_ast(&program).unwrap();

    for cpu in [
        crate::target::CpuFamily::Ez80,
        crate::target::CpuFamily::Z80,
        crate::target::CpuFamily::Z80N,
        crate::target::CpuFamily::Z180,
        crate::target::CpuFamily::I8080,
        crate::target::CpuFamily::I8085,
    ] {
        let tbir = TbirProgram::lower(
            &hir,
            &program,
            &AssemblyOptions {
                cpu,
                ..AssemblyOptions::default()
            },
        )
        .unwrap();
        let TbirDeclaration::Function { body, effects, .. } = tbir
            .declarations
            .iter()
            .find(|declaration| matches!(declaration, TbirDeclaration::Function { .. }))
            .unwrap()
        else {
            unreachable!();
        };

        assert!(matches!(
            body.as_slice(),
            [TbirStmt::PortWrite {
                port,
                value: Expr::In(input),
            }] if port == "OUTPUT" && input == "INPUT"
        ));
        assert!(
            effects.contains(&TbirEffect::PortIo),
            "{cpu:?}: {effects:?}"
        );
    }
}

#[test]
fn tbir_lowers_declaration_kinds() {
    let program = parse_program(
        Path::new("test.ezra"),
        r#"
                const LIMIT: u8 = 10
                alias Byte = u8
                port DEBUG_CHAR: u8 = 0x0C
                volatile mmio FRAMEBUFFER: ptr<u8> = 0x080000
                embed palette: bytes = bytes [0x11, 0x22]
                global counter: u8 = 0
                struct Point { x: u8 y: u8 }
                extern asm fn read_status() -> u8
                fn main() {}
            "#,
    )
    .unwrap();
    let hir = HirProgram::from_ast(&program).unwrap();
    let tbir = TbirProgram::for_ez80(&hir, &program, &AssemblyOptions::default()).unwrap();

    assert_eq!(object_kind(&tbir, "LIMIT"), Some(TbirObjectKind::Const));
    assert_eq!(object_kind(&tbir, "Byte"), Some(TbirObjectKind::Alias));
    assert_eq!(object_kind(&tbir, "DEBUG_CHAR"), Some(TbirObjectKind::Port));
    assert_eq!(
        object_kind(&tbir, "FRAMEBUFFER"),
        Some(TbirObjectKind::Mmio)
    );
    assert_eq!(object_kind(&tbir, "palette"), Some(TbirObjectKind::Embed));
    assert_eq!(object_kind(&tbir, "counter"), Some(TbirObjectKind::Global));
    assert_eq!(object_kind(&tbir, "Point"), Some(TbirObjectKind::Struct));
    assert_eq!(
        object_kind(&tbir, "read_status"),
        Some(TbirObjectKind::ExternFunction)
    );
}

#[test]
fn tbir_keeps_inline_comments_attached_to_source_statements() {
    let program = parse_program(
        Path::new("test.ezra"),
        "fn main() {\n    let value: u8 = 1 // initialize value\n    value += 1 // increment value\n}\n",
    )
    .unwrap();
    let hir = HirProgram::from_ast(&program).unwrap();
    let tbir = TbirProgram::for_ez80(&hir, &program, &AssemblyOptions::default()).unwrap();

    assert_eq!(tbir.source_comments.len(), 2);
    assert_eq!(tbir.source_comments[0].text, "initialize value");
    assert_eq!(tbir.source_comments[0].statement_text, "let value: u8 = 1");
    assert_eq!(tbir.source_comments[0].statement_span.start.line, 2);
    assert_eq!(tbir.source_comments[1].text, "increment value");
    assert_eq!(tbir.source_comments[1].statement_span.start.line, 3);
    assert!(
        tbir.dump_text().contains("text=initialize value"),
        "{}",
        tbir.dump_text()
    );
}

#[test]
fn tbir_attaches_standalone_comments_to_the_next_statement() {
    let program = parse_program(
        Path::new("test.ezra"),
        "fn main() {\n    // initialize value\n    let value: u8 = 1\n    // increment value\n    value += 1\n}\n",
    )
    .unwrap();
    let hir = HirProgram::from_ast(&program).unwrap();
    let tbir = TbirProgram::for_ez80(&hir, &program, &AssemblyOptions::default()).unwrap();

    assert_eq!(tbir.source_comments.len(), 2);
    assert_eq!(tbir.source_comments[0].text, "initialize value");
    assert_eq!(tbir.source_comments[0].statement_text, "let value: u8 = 1");
    assert_eq!(tbir.source_comments[0].statement_span.start.line, 3);
    assert_eq!(tbir.source_comments[1].text, "increment value");
    assert_eq!(tbir.source_comments[1].statement_text, "value += 1");
    assert_eq!(tbir.source_comments[1].statement_span.start.line, 5);
}

#[test]
fn tbir_preserves_two_result_nodes_and_signature() {
    let program = parse_program(
        Path::new("multi.ezra"),
        r#"
                fn pair() -> u8, bool { return 1, true }
                fn caller() -> u8, bool {
                    let value: u8, flag: bool = pair()
                    return value, flag
                }
                fn main() {}
            "#,
    )
    .unwrap();
    let hir = HirProgram::from_ast(&program).unwrap();
    let tbir = TbirProgram::for_ez80(&hir, &program, &AssemblyOptions::default()).unwrap();
    let caller = tbir
        .declarations
        .iter()
        .find_map(|declaration| match declaration {
            TbirDeclaration::Function {
                name,
                second_return_type,
                body,
                ..
            } if name == "caller" => Some((second_return_type, body)),
            _ => None,
        })
        .unwrap();

    assert_eq!(caller.0, &Some(Type::Named("bool".to_owned())));
    assert!(matches!(caller.1[0], TbirStmt::LetTwo { .. }));
    assert!(matches!(caller.1[1], TbirStmt::ReturnTwo { .. }));
}

#[test]
fn tbir_accepts_catalog_paired_intrinsics() {
    let program = parse_program(
        Path::new("intrinsics_two_result.ezra"),
        r#"
            global bytes: [u8; 4] = [1, 2, 3, 4]
            fn main() {
                let quotient: u8, remainder: u8 = ezra.int.divmod(7u8, 3u8)
                let sum: u8, carry: bool = ezra.int.add_carry(0xFFu8, 1u8, false)
                let difference: u8, borrow: bool = ezra.int.sub_borrow(0u8, 1u8, false)
                let low: u8, high: u8 = ezra.int.full_mul(3u8, 4u8)
                let found: ptr<u8>, present: bool = ezra.mem.find_byte(&bytes[0], 4u24, 2u8)
            }
        "#,
    )
    .unwrap();
    let hir = HirProgram::from_ast(&program).unwrap();
    let tbir = TbirProgram::for_ez80(&hir, &program, &AssemblyOptions::default()).unwrap();
    let main = tbir
        .declarations
        .iter()
        .find_map(|declaration| match declaration {
            TbirDeclaration::Function { name, body, .. } if name == "main" => Some(body),
            _ => None,
        })
        .unwrap();
    assert_eq!(
        main.iter()
            .filter(|statement| matches!(statement, TbirStmt::LetTwo { .. }))
            .count(),
        5
    );
}

#[test]
fn tbir_preserves_function_analysis() {
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
    let tbir = TbirProgram::for_ez80(&hir, &program, &AssemblyOptions::default()).unwrap();
    let count = tbir
        .declarations
        .iter()
        .find_map(|decl| match decl {
            TbirDeclaration::Function {
                name,
                effects,
                recursive,
                tail_recursive,
                loop_candidates,
                ..
            } if name == "count" => Some((effects, *recursive, *tail_recursive, *loop_candidates)),
            _ => None,
        })
        .unwrap();

    assert_eq!(count, (&vec![TbirEffect::Call], true, false, 1));
}

#[test]
fn tbir_reports_optimization_markers_and_dump() {
    let program = parse_program(
        Path::new("test.ezra"),
        r#"
                inline fn helper() -> bool { return !false }
                fn main() {
                    return
                    helper()
                }
            "#,
    )
    .unwrap();
    let hir = HirProgram::from_ast(&program).unwrap();
    let tbir = TbirProgram::for_ez80(&hir, &program, &AssemblyOptions::default()).unwrap();
    let dump = tbir.dump_text();

    assert!(tbir.optimizations.constant_folds >= 1);
    assert!(
        tbir.optimizations
            .inline_function_names()
            .contains("helper")
    );
    assert!(dump.contains("kind=Inline outcome=Applied"), "{dump}");
    assert!(dump.contains("callee=helper reason=approved"), "{dump}");
    assert!(dump.contains("TBIR"), "{dump}");
    assert!(dump.contains("target: ez80-adl"), "{dump}");
    assert!(dump.contains("optimizations:"), "{dump}");
    assert!(dump.contains("comptime_evaluations="), "{dump}");
    assert!(dump.contains("comptime_rejections="), "{dump}");
    assert!(dump.contains("strength_reductions="), "{dump}");
    assert!(dump.contains("copy_propagations="), "{dump}");
    assert!(dump.contains("common_subexpressions="), "{dump}");
    assert!(dump.contains("loop_invariants_hoisted="), "{dump}");
}

#[test]
fn tbir_simplifies_power_of_two_multiplication_and_zero_division() {
    let program = parse_program(
        Path::new("test.ezra"),
        r#"
                fn scale(x: u8) -> u8 { return x * 8 }
                fn unsigned_division(x: u8) -> u8 { let result: u8 = x / 4 return result }
                fn remainder(x: u16) -> u16 { let result: u16 = x % 8 return result }
                fn signed_division(x: i8) -> i8 { return x / 4 }
                fn zero(x: u8) -> u8 { return x / 0 }
            "#,
    )
    .unwrap();
    let hir = HirProgram::from_ast(&program).unwrap();
    let tbir = TbirProgram::for_ez80(&hir, &program, &AssemblyOptions::default()).unwrap();

    assert!(matches!(
        return_expr(&tbir, "scale"),
        Some(Expr::Binary {
            op: BinaryOp::Shl,
            ..
        })
    ));
    assert!(matches!(
        return_expr(&tbir, "unsigned_division"),
        Some(Expr::Cast { expr, .. })
            if matches!(expr.as_ref(), Expr::Binary { op: BinaryOp::Shr, right, .. }
                if matches!(right.as_ref(), Expr::TypedInt(2, Type::Named(ty)) if ty == "u8"))
    ));
    assert!(matches!(
        return_expr(&tbir, "remainder"),
        Some(Expr::Cast { expr, .. })
            if matches!(expr.as_ref(), Expr::Binary { op: BinaryOp::BitAnd, right, .. }
                if matches!(right.as_ref(), Expr::TypedInt(7, Type::Named(ty)) if ty == "u16"))
    ));
    assert!(matches!(
        return_expr(&tbir, "signed_division"),
        Some(Expr::Binary {
            op: BinaryOp::Div,
            ..
        })
    ));
    assert!(matches!(
        return_expr(&tbir, "zero"),
        Some(Expr::Int(0) | Expr::TypedInt(0, _))
    ));
    assert!(tbir.optimizations.algebraic_simplifications >= 1);
    assert!(tbir.optimizations.strength_reductions >= 1);
}

fn return_expr<'a>(tbir: &'a TbirProgram, name: &str) -> Option<&'a Expr> {
    tbir.declarations
        .iter()
        .find_map(|declaration| match declaration {
            TbirDeclaration::Function {
                name: function_name,
                body,
                ..
            } if function_name == name => match body.as_slice() {
                [TbirStmt::Return(Some(expr))] => Some(expr),
                _ => None,
            },
            _ => None,
        })
}

#[test]
fn tbir_rejects_ez80_port_outside_8_bit_range() {
    let program = parse_program(
        Path::new("test.ezra"),
        "port BAD: u16 = 0x0100\nfn main() {}",
    )
    .unwrap();
    let hir = HirProgram::from_ast(&program).unwrap();
    let error = TbirProgram::for_ez80(&hir, &program, &AssemblyOptions::default()).unwrap_err();

    assert_eq!(
        error.message,
        "port `BAD` value 0x100 is outside the 8-bit port range for target CPU `ez80`"
    );
}

#[test]
fn tbir_rejects_ez80_mmio_outside_24_bit_range() {
    let program = parse_program(
        Path::new("test.ezra"),
        "volatile mmio BAD: ptr<u8> = 0x01000000\nfn main() {}",
    )
    .unwrap();
    let hir = HirProgram::from_ast(&program).unwrap();
    let error = TbirProgram::for_ez80(&hir, &program, &AssemblyOptions::default()).unwrap_err();

    assert_eq!(
        error.message,
        "mmio `BAD` address 0x1000000 is outside the eZ80 24-bit address space"
    );
}

#[test]
fn tbir_rejects_ports_for_mmio_only_cpus() {
    let program = parse_program(
        Path::new("test.ezra"),
        "port PPU: u8 = 0x2000\nfn main() {}",
    )
    .unwrap();
    let hir = HirProgram::from_ast(&program).unwrap();
    let options = AssemblyOptions {
        cpu: crate::target::CpuFamily::Mos6502,
        ..AssemblyOptions::default()
    };
    let error = TbirProgram::lower(&hir, &program, &options).unwrap_err();

    assert_eq!(
        error.message,
        "target CPU `6502` does not support separate port I/O; declare `PPU` as mmio instead"
    );
}

#[test]
fn tbir_rejects_port_operations_for_mmio_only_cpus() {
    let program = parse_program(
        Path::new("test.ezra"),
        "fn main() { let status: u8 = in STATUS }",
    )
    .unwrap();
    let hir = HirProgram::from_ast(&program).unwrap();
    let options = AssemblyOptions {
        cpu: crate::target::CpuFamily::Mos6502,
        ..AssemblyOptions::default()
    };
    let error = TbirProgram::lower(&hir, &program, &options).unwrap_err();

    assert_eq!(
        error.message,
        "target CPU `6502` does not support separate port I/O `STATUS`; use mmio instead"
    );
}

#[test]
fn tbir_accepts_16_bit_mmio_for_6502() {
    let program = parse_program(
        Path::new("test.ezra"),
        "volatile mmio PPU: ptr<u8> = 0x2000\nfn main() {}",
    )
    .unwrap();
    let hir = HirProgram::from_ast(&program).unwrap();
    let options = AssemblyOptions {
        cpu: crate::target::CpuFamily::Mos6502,
        ..AssemblyOptions::default()
    };
    let tbir = TbirProgram::lower(&hir, &program, &options).unwrap();

    assert!(!tbir.target.supports_port_io);
    assert_eq!(tbir.target.pointer_width_bits, 16);
}

#[test]
fn semantic_model_uses_target_pointer_width() {
    let program = parse_program(
        Path::new("test.ezra"),
        "global cursor: ptr<u8> = 0\nfn main() {}",
    )
    .unwrap();
    let model = model::SemanticModel::from_program(&program, 16, 0xA000, 0x8000, 0xC000).unwrap();

    assert_eq!(model.pointer_bytes(), 2);
    assert_eq!(model.globals["cursor"].size, 2);
}

#[test]
fn semantic_model_uses_target_storage_for_twenty_bit_integers() {
    let program = parse_program(
        Path::new("test.ezra"),
        "global value: u20 = 0\nfn main() {}",
    )
    .unwrap();
    let msp430x = model::SemanticModel::from_program_with_native_int_widths(
        &program,
        20,
        0x1000,
        0x2000,
        0x3000,
        &[8, 16, 20],
    )
    .unwrap();
    let generic = model::SemanticModel::from_program_with_native_int_widths(
        &program,
        16,
        0x1000,
        0x2000,
        0x3000,
        &[8, 16],
    )
    .unwrap();
    let m68k = model::SemanticModel::from_program_with_native_int_widths(
        &program,
        24,
        0x1000,
        0x2000,
        0x3000,
        &[8, 16, 24, 32],
    )
    .unwrap();

    assert_eq!(msp430x.globals["value"].size, 4);
    assert_eq!(generic.globals["value"].size, 3);
    assert_eq!(m68k.globals["value"].size, 4);
}

#[test]
fn semantic_model_layouts_aggregates_and_function_slots() {
    let program = parse_program(
        Path::new("test.ezra"),
        r#"
            const COUNT: u8 = 3
            struct Pixel { x: u8 y: u16 }
            global pixels: [Pixel; COUNT] = [Pixel { x: 0, y: 0 }]
            fn draw(pixel: ptr<Pixel>, color: u8) {}
            fn main() {}
        "#,
    )
    .unwrap();
    let model = model::SemanticModel::from_program(&program, 16, 0xA000, 0x8000, 0xC000).unwrap();

    assert_eq!(model.structs["Pixel"].size, 3);
    assert_eq!(model.globals["pixels"].size, 9);
    assert_eq!(model.functions["draw"].argument_slots.len(), 2);
    assert_eq!(model.functions["draw"].argument_slots[0].size, 2);
}

#[test]
fn semantic_model_allocates_aggregate_constants_in_rodata() {
    let program = parse_program(
        Path::new("test.ezra"),
        "const VALUES: [u16; 2] = [1, 2]\nfn main() { let p: ptr<u16> = &VALUES }",
    )
    .unwrap();
    let model = model::SemanticModel::from_program(&program, 16, 0xA000, 0x8000, 0xC000).unwrap();

    assert_eq!(model.globals["VALUES"].address, 0x8000);
    assert_eq!(model.globals["VALUES"].size, 4);
    assert_eq!(
        model.global_types["VALUES"],
        Type::Array {
            element: Box::new(Type::Named("u16".to_owned())),
            len: Box::new(Expr::Int(2)),
        }
    );
}

#[test]
fn semantic_model_rejects_circular_constants() {
    let program = parse_program(
        Path::new("test.ezra"),
        "const FIRST: u8 = SECOND + 1\nconst SECOND: u8 = FIRST + 1\nfn main() {}",
    )
    .unwrap();
    let error = SemanticModel::from_program(&program, 16, 0xA000, 0x8000, 0xC000).unwrap_err();

    assert!(
        error.message.contains("circular constant reference"),
        "{error}"
    );
}

#[test]
fn semantic_model_resolves_forward_constants_in_layouts() {
    let program = parse_program(
        Path::new("test.ezra"),
        "const COUNT: u8 = BASE + 1\nconst BASE: u8 = 2\nglobal values: [u8; COUNT] = [1, 2, 3]\nfn main() {}",
    )
    .unwrap();
    let model = model::SemanticModel::from_program(&program, 16, 0xA000, 0x8000, 0xC000).unwrap();

    assert_eq!(model.constants["COUNT"], 3);
    assert_eq!(model.globals["values"].size, 3);
}

#[test]
fn tbir_retains_typed_memory_object_facts() {
    let program = parse_program(
        Path::new("test.ezra"),
        "global counter: u16 = 0\nvolatile mmio VIDEO: ptr<u8> = 0x080000\nembed blob: bytes = bytes [1, 2, 3]\nfn main() {}",
    )
    .unwrap();
    let hir = HirProgram::from_ast(&program).unwrap();
    let tbir = TbirProgram::for_ez80(&hir, &program, &AssemblyOptions::default()).unwrap();

    let global = tbir
        .objects
        .iter()
        .find(|object| object.name == "counter")
        .unwrap();
    assert_eq!(global.kind, TbirObjectKind::Global);
    assert_eq!(global.region.as_deref(), Some("ram"));
    assert_eq!(global.access, TbirAccess::ReadWrite);
    assert!(!global.volatile);

    let mmio = tbir
        .objects
        .iter()
        .find(|object| object.name == "VIDEO")
        .unwrap();
    assert_eq!(mmio.kind, TbirObjectKind::Mmio);
    assert_eq!(mmio.region.as_deref(), Some("vram"));
    assert!(mmio.volatile);
    assert_eq!(mmio.size, 1);

    let embed = tbir
        .objects
        .iter()
        .find(|object| object.name == "blob")
        .unwrap();
    assert_eq!(embed.kind, TbirObjectKind::Embed);
    assert_eq!(embed.region.as_deref(), Some("assets"));
    assert_eq!(embed.access, TbirAccess::ReadOnly);
    assert!(tbir.dump_text().contains("named_memory_reads_hoisted="));
    assert!(tbir.dump_text().contains("memory_object counter"));
}

#[test]
fn mmio_uses_pointee_size_without_inheriting_read_only_region_access() {
    let program = parse_program(
        Path::new("test.ezra"),
        "volatile mmio CONTROL: ptr<u16> = 0x020040\nfn main() {}",
    )
    .unwrap();
    let hir = HirProgram::from_ast(&program).unwrap();
    let tbir = TbirProgram::for_ez80(&hir, &program, &AssemblyOptions::default()).unwrap();
    let control = tbir
        .objects
        .iter()
        .find(|object| object.name == "CONTROL")
        .unwrap();

    assert_eq!(control.region.as_deref(), Some("rodata"));
    assert_eq!(control.size, 2);
    assert_eq!(control.access, TbirAccess::ReadWrite);
    assert!(control.volatile);
}

#[test]
fn memory_objects_inherit_read_only_region_access() {
    let program =
        parse_program(Path::new("test.ezra"), "global fixed: u8 = 1\nfn main() {}").unwrap();
    let hir = HirProgram::from_ast(&program).unwrap();
    let options = AssemblyOptions {
        ram_base: crate::target::Address24::new(0x020040),
        ..AssemblyOptions::default()
    };
    let tbir = TbirProgram::for_ez80(&hir, &program, &options).unwrap();
    let fixed = tbir
        .objects
        .iter()
        .find(|object| object.name == "fixed")
        .unwrap();

    assert_eq!(fixed.region.as_deref(), Some("rodata"));
    assert_eq!(fixed.access, TbirAccess::ReadOnly);
    assert_eq!(tbir.optimizations.named_memory_reads_hoisted, 0);
}

fn object_kind(tbir: &TbirProgram, name: &str) -> Option<TbirObjectKind> {
    tbir.declarations.iter().find_map(|decl| match decl {
        TbirDeclaration::Object {
            name: object_name,
            kind,
        } if object_name == name => Some(*kind),
        _ => None,
    })
}
