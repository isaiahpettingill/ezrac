use crate::{
    asm::AssemblyOptions,
    ast::Program,
    diagnostic::Diagnostic,
    hir::{HirDeclaration, HirProgram},
    target::Address24,
};

use super::{
    TbirAccess, TbirDeclaration, TbirEffect, TbirMemoryModel, TbirMemoryRegion, TbirObjectKind,
    TbirProgram, TbirTarget, diagnostics, optimize,
};

pub fn lower(
    hir: &HirProgram,
    lowered_program: &Program,
    options: &AssemblyOptions,
) -> Result<TbirProgram, Diagnostic> {
    diagnostics::validate_ez80_program(lowered_program)?;
    let memory = memory_model(options)?;
    let declarations = hir.declarations.iter().map(lower_declaration).collect();
    let (lowered_program, mut optimizations) = optimize::optimize_program(lowered_program);
    optimizations.tail_call_candidates = hir
        .declarations
        .iter()
        .filter_map(|declaration| match declaration {
            HirDeclaration::Function(function) => Some(&function.analysis.tail_call_candidates),
            _ => None,
        })
        .flatten()
        .cloned()
        .collect();
    Ok(TbirProgram {
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
        optimizations,
        lowered_program,
    })
}

fn memory_model(options: &AssemblyOptions) -> Result<TbirMemoryModel, Diagnostic> {
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
