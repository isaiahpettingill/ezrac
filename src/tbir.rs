use crate::{
    asm::AssemblyOptions,
    diagnostic::Diagnostic,
    hir::{HirDeclaration, HirProgram},
    target::Address24,
};

#[derive(Clone, Debug, PartialEq)]
pub struct TbirProgram {
    pub source: std::path::PathBuf,
    pub target: TbirTarget,
    pub memory: TbirMemoryModel,
    pub declarations: Vec<TbirDeclaration>,
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
    pub fn for_ez80(hir: &HirProgram, options: &AssemblyOptions) -> Result<Self, Diagnostic> {
        let memory = ez80_memory_model(options)?;
        let declarations = hir.declarations.iter().map(lower_declaration).collect();
        Ok(Self {
            source: hir.source_path.clone(),
            target: TbirTarget {
                name: "ez80-adl".to_owned(),
                pointer_width_bits: 24,
                native_int_widths: vec![8, 16, 24],
                prefer_code_size: true,
                has_cache: false,
            },
            memory,
            declarations,
        })
    }
}

fn ez80_memory_model(options: &AssemblyOptions) -> Result<TbirMemoryModel, Diagnostic> {
    let regions = vec![
        region(
            "code",
            options.code_base,
            0x01_0000,
            TbirAccess::ReadOnly,
            false,
            true,
        ),
        region(
            "rodata",
            options.rodata_base,
            0x02_0000,
            TbirAccess::ReadOnly,
            false,
            false,
        ),
        region(
            "ram",
            options.ram_base,
            0x04_0000,
            TbirAccess::ReadWrite,
            false,
            false,
        ),
        region(
            "vram",
            options.vram_base,
            0x04_0000,
            TbirAccess::ReadWrite,
            true,
            false,
        ),
        region(
            "audio",
            options.audio_base,
            0x04_0000,
            TbirAccess::ReadWrite,
            true,
            false,
        ),
        region(
            "assets",
            options.asset_base,
            0x30_0000,
            TbirAccess::ReadOnly,
            false,
            false,
        ),
    ];
    for region in &regions {
        let end = region
            .start
            .checked_add(region.size)
            .ok_or_else(|| Diagnostic::new(format!("TBIR region `{}` overflows", region.name)))?;
        if end > Address24::MAX + 1 {
            return Err(Diagnostic::new(format!(
                "TBIR region `{}` exceeds the 24-bit address space",
                region.name
            )));
        }
    }
    Ok(TbirMemoryModel {
        address_width_bits: 24,
        regions,
    })
}

fn region(
    name: &str,
    start: Address24,
    size: u32,
    access: TbirAccess,
    volatile: bool,
    executable: bool,
) -> TbirMemoryRegion {
    TbirMemoryRegion {
        name: name.to_owned(),
        start: start.get(),
        size,
        access,
        volatile,
        executable,
    }
}

fn lower_declaration(declaration: &HirDeclaration) -> TbirDeclaration {
    match declaration {
        HirDeclaration::Function(function) => TbirDeclaration::Function {
            name: function.sig.name.clone(),
            effects: vec![TbirEffect::Call],
            recursive: function.analysis.recursive,
            tail_recursive: function.analysis.tail_recursive,
            loop_candidates: function.analysis.loop_candidates,
        },
        HirDeclaration::Const(object) => object_decl(&object.name, TbirObjectKind::Const),
        HirDeclaration::Alias { name, .. } => object_decl(name, TbirObjectKind::Alias),
        HirDeclaration::Port(object) => object_decl(&object.name, TbirObjectKind::Port),
        HirDeclaration::Mmio { object, .. } => object_decl(&object.name, TbirObjectKind::Mmio),
        HirDeclaration::Embed { name, .. } => object_decl(name, TbirObjectKind::Embed),
        HirDeclaration::Global(object) => object_decl(&object.name, TbirObjectKind::Global),
        HirDeclaration::Struct { name, .. } => object_decl(name, TbirObjectKind::Struct),
        HirDeclaration::ExternFunction(sig) => {
            object_decl(&sig.name, TbirObjectKind::ExternFunction)
        }
    }
}

fn object_decl(name: &str, kind: TbirObjectKind) -> TbirDeclaration {
    TbirDeclaration::Object {
        name: name.to_owned(),
        kind,
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
        let tbir = TbirProgram::for_ez80(&hir, &AssemblyOptions::default()).unwrap();
        assert_eq!(tbir.target.pointer_width_bits, 24);
        assert!(
            tbir.memory
                .regions
                .iter()
                .any(|region| region.name == "vram")
        );
    }
}
