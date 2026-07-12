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

fn object_kind(tbir: &TbirProgram, name: &str) -> Option<TbirObjectKind> {
    tbir.declarations.iter().find_map(|decl| match decl {
        TbirDeclaration::Object {
            name: object_name,
            kind,
        } if object_name == name => Some(*kind),
        _ => None,
    })
}
