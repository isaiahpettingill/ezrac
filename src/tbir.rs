use crate::{asm::AssemblyOptions, ast::Program, diagnostic::Diagnostic, hir::HirProgram};

pub mod ez80;

#[derive(Clone, Debug, PartialEq)]
pub struct TbirProgram {
    pub source: std::path::PathBuf,
    pub target: TbirTarget,
    pub memory: TbirMemoryModel,
    pub declarations: Vec<TbirDeclaration>,
    pub lowered_program: Program,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TbirTarget {
    pub name: String,
    pub pointer_width_bits: u8,
    pub native_int_widths: Vec<u8>,
    pub prefer_code_size: bool,
    pub has_cache: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TbirMemoryModel {
    pub address_width_bits: u8,
    pub regions: Vec<TbirMemoryRegion>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TbirMemoryRegion {
    pub name: String,
    pub start: u32,
    pub size: u32,
    pub access: TbirAccess,
    pub volatile: bool,
    pub executable: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TbirAccess {
    ReadOnly,
    ReadWrite,
}

#[derive(Clone, Debug, PartialEq)]
pub enum TbirDeclaration {
    Function {
        name: String,
        effects: Vec<TbirEffect>,
        recursive: bool,
        tail_recursive: bool,
        loop_candidates: usize,
    },
    Object {
        name: String,
        kind: TbirObjectKind,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TbirObjectKind {
    Const,
    Port,
    Mmio,
    Embed,
    Global,
    Alias,
    Struct,
    ExternFunction,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TbirEffect {
    Pure,
    VolatileMemory,
    PortIo,
    InlineAsm,
    Call,
}

impl TbirProgram {
    pub fn for_ez80(
        hir: &HirProgram,
        lowered_program: &Program,
        options: &AssemblyOptions,
    ) -> Result<Self, Diagnostic> {
        ez80::lower(hir, lowered_program, options)
    }
}

#[cfg(test)]
mod tests {
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
                } if name == "count" => {
                    Some((effects, *recursive, *tail_recursive, *loop_candidates))
                }
                _ => None,
            })
            .unwrap();

        assert_eq!(count, (&vec![TbirEffect::Call], true, false, 1));
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
}
