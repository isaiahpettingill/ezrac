use std::path::Path;

use crate::{asm::AssemblyOptions, hir::HirProgram, parser::parse_program};

use super::*;

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
    assert_eq!(tbir.optimizations.inline_candidates, ["helper"]);
    assert!(dump.contains("TBIR"), "{dump}");
    assert!(dump.contains("target: ez80-adl"), "{dump}");
    assert!(dump.contains("optimizations:"), "{dump}");
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
        "port `BAD` value 0x100 is outside the eZ80 8-bit port range"
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

fn object_kind(tbir: &TbirProgram, name: &str) -> Option<TbirObjectKind> {
    tbir.declarations.iter().find_map(|decl| match decl {
        TbirDeclaration::Object {
            name: object_name,
            kind,
        } if object_name == name => Some(*kind),
        _ => None,
    })
}
