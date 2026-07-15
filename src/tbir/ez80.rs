use crate::{
    asm::AssemblyOptions,
    ast::{Declaration, Program, Stmt},
    diagnostic::Diagnostic,
    hir::{HirDeclaration, HirProgram},
    target::{Address24, CpuFamily, memory_model_for_cpu},
};

use super::{
    TbirAccess, TbirDeclaration, TbirEffect, TbirMemoryModel, TbirMemoryRegion, TbirObjectKind,
    TbirParam, TbirProgram, TbirStmt, TbirTarget, diagnostics, optimize,
};

pub fn lower(
    hir: &HirProgram,
    lowered_program: &Program,
    options: &AssemblyOptions,
) -> Result<TbirProgram, Diagnostic> {
    diagnostics::validate_program(lowered_program, options.cpu)?;
    let memory = memory_model(options)?;
    let pointer_width_bits = memory_model_for_cpu(options.cpu)
        .map(|model| model.pointer_width_bits as u8)
        .unwrap_or(24);
    let (lowered_program, mut optimizations) = optimize::optimize_program(lowered_program);
    let declarations = hir
        .declarations
        .iter()
        .map(|declaration| lower_declaration(declaration, &lowered_program))
        .collect();
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
            name: match options.cpu {
                CpuFamily::Z80 => "z80".to_owned(),
                CpuFamily::Z80N => "z80n".to_owned(),
                CpuFamily::Z180 => "z180".to_owned(),
                CpuFamily::Avr => "avr".to_owned(),
                _ => "ez80-adl".to_owned(),
            },
            pointer_width_bits,
            native_int_widths: if pointer_width_bits == 16 {
                vec![8, 16]
            } else {
                vec![8, 16, 24]
            },
            prefer_code_size: true,
            has_cache: false,
            supports_port_io: supports_port_io(options.cpu),
        },
        memory,
        declarations,
        optimizations,
        lowered_program,
    })
}

fn memory_model(options: &AssemblyOptions) -> Result<TbirMemoryModel, Diagnostic> {
    let address_width_bits = memory_model_for_cpu(options.cpu)
        .map(|model| model.address_width_bits as u8)
        .unwrap_or(24);
    if address_width_bits == 16 {
        return Ok(TbirMemoryModel {
            address_width_bits,
            regions: vec![
                region(
                    "code",
                    options.code_base,
                    0x1_0000u32.saturating_sub(options.code_base.get()),
                    TbirAccess::ReadOnly,
                    false,
                    true,
                ),
                region(
                    "rodata",
                    options.rodata_base,
                    0x1_0000u32.saturating_sub(options.rodata_base.get()),
                    TbirAccess::ReadOnly,
                    false,
                    false,
                ),
                region(
                    "ram",
                    options.ram_base,
                    0x1_0000u32.saturating_sub(options.ram_base.get()),
                    TbirAccess::ReadWrite,
                    false,
                    false,
                ),
                region(
                    "assets",
                    options.asset_base,
                    0x1_0000u32.saturating_sub(options.asset_base.get()),
                    TbirAccess::ReadOnly,
                    false,
                    false,
                ),
            ],
        });
    }
    let regions = vec![
        region(
            "code",
            options.code_base,
            region_size(options.code_base, 0x01_0000),
            TbirAccess::ReadOnly,
            false,
            true,
        ),
        region(
            "rodata",
            options.rodata_base,
            region_size(options.rodata_base, 0x02_0000),
            TbirAccess::ReadOnly,
            false,
            false,
        ),
        region(
            "ram",
            options.ram_base,
            region_size(options.ram_base, 0x04_0000),
            TbirAccess::ReadWrite,
            false,
            false,
        ),
        region(
            "vram",
            options.vram_base,
            region_size(options.vram_base, 0x04_0000),
            TbirAccess::ReadWrite,
            true,
            false,
        ),
        region(
            "audio",
            options.audio_base,
            region_size(options.audio_base, 0x04_0000),
            TbirAccess::ReadWrite,
            true,
            false,
        ),
        region(
            "assets",
            options.asset_base,
            region_size(options.asset_base, 0x30_0000),
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

fn supports_port_io(cpu: CpuFamily) -> bool {
    matches!(
        cpu,
        CpuFamily::Ez80
            | CpuFamily::Z80
            | CpuFamily::Z80N
            | CpuFamily::Z180
            | CpuFamily::I8080
            | CpuFamily::I8085
    )
}

fn region_size(start: Address24, preferred: u32) -> u32 {
    let remaining = Address24::MAX + 1 - start.get();
    preferred.min(remaining)
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

fn lower_declaration(declaration: &HirDeclaration, program: &Program) -> TbirDeclaration {
    match declaration {
        HirDeclaration::Function(function) => {
            let source = program
                .declarations
                .iter()
                .find_map(|declaration| match declaration {
                    Declaration::Function(source) if source.name == function.sig.name => {
                        Some(source)
                    }
                    _ => None,
                });
            TbirDeclaration::Function {
                name: function.sig.name.clone(),
                public: function.sig.public,
                attrs: function.attrs.clone(),
                params: function
                    .sig
                    .params
                    .iter()
                    .map(|param| TbirParam {
                        name: param.name.clone(),
                        ty: param.ty.clone(),
                    })
                    .collect(),
                return_type: function.sig.return_type.clone(),
                body: source
                    .map(|source| lower_stmts(&source.body))
                    .unwrap_or_default(),
                effects: function_effects(source.map_or(&function.body, |source| &source.body)),
                recursive: function.analysis.recursive,
                tail_recursive: function.analysis.tail_recursive,
                loop_candidates: function.analysis.loop_candidates,
            }
        }
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

fn lower_stmts(stmts: &[Stmt]) -> Vec<TbirStmt> {
    stmts
        .iter()
        .map(|stmt| match stmt {
            Stmt::Let { name, ty, value } => TbirStmt::Let {
                name: name.clone(),
                ty: ty.clone(),
                value: value.clone(),
            },
            Stmt::Assign { target, op, value } => TbirStmt::Assign {
                target: target.clone(),
                op: *op,
                value: value.clone(),
            },
            Stmt::If {
                condition,
                then_body,
                else_body,
            } => TbirStmt::If {
                condition: condition.clone(),
                then_body: lower_stmts(then_body),
                else_body: lower_stmts(else_body),
            },
            Stmt::While { condition, body } => TbirStmt::While {
                condition: condition.clone(),
                body: lower_stmts(body),
            },
            Stmt::Loop { body } => TbirStmt::Loop {
                body: lower_stmts(body),
            },
            Stmt::Break => TbirStmt::Break,
            Stmt::Continue => TbirStmt::Continue,
            Stmt::Return(value) => TbirStmt::Return(value.clone()),
            Stmt::Asm {
                volatile,
                inputs,
                outputs,
                clobbers,
                lines,
            } => TbirStmt::Asm {
                volatile: *volatile,
                inputs: inputs.clone(),
                outputs: outputs.clone(),
                clobbers: clobbers.clone(),
                lines: lines.clone(),
            },
            Stmt::Out { port, value } => TbirStmt::PortWrite {
                port: port.clone(),
                value: value.clone(),
            },
            Stmt::Expr(expr) => TbirStmt::Eval(expr.clone()),
        })
        .collect()
}

fn function_effects(stmts: &[Stmt]) -> Vec<TbirEffect> {
    let mut effects = Vec::new();
    collect_effects(stmts, &mut effects);
    if effects.is_empty() {
        effects.push(TbirEffect::Pure);
    }
    effects
}

fn collect_effects(stmts: &[Stmt], effects: &mut Vec<TbirEffect>) {
    for stmt in stmts {
        let effect = match stmt {
            Stmt::Out { .. } => Some(TbirEffect::PortIo),
            Stmt::Asm { .. } => Some(TbirEffect::InlineAsm),
            Stmt::Expr(crate::ast::Expr::Call { .. }) => Some(TbirEffect::Call),
            Stmt::Assign { target, .. } if matches!(target, crate::ast::Place::Deref(_)) => {
                Some(TbirEffect::VolatileMemory)
            }
            _ => None,
        };
        if let Some(effect) = effect
            && !effects.contains(&effect)
        {
            effects.push(effect);
        }
        match stmt {
            Stmt::Let { value, .. }
            | Stmt::Assign { value, .. }
            | Stmt::Return(Some(value))
            | Stmt::Out { value, .. }
            | Stmt::Expr(value) => collect_expr_effects(value, effects),
            Stmt::If { condition, .. } | Stmt::While { condition, .. } => {
                collect_expr_effects(condition, effects)
            }
            _ => {}
        }
        match stmt {
            Stmt::If {
                then_body,
                else_body,
                ..
            } => {
                collect_effects(then_body, effects);
                collect_effects(else_body, effects);
            }
            Stmt::While { body, .. } | Stmt::Loop { body } => collect_effects(body, effects),
            _ => {}
        }
    }
}

fn collect_expr_effects(expr: &crate::ast::Expr, effects: &mut Vec<TbirEffect>) {
    use crate::ast::{AccessSegment, Expr};
    match expr {
        Expr::Call { args, .. } => {
            if !effects.contains(&TbirEffect::Call) {
                effects.push(TbirEffect::Call);
            }
            for arg in args {
                collect_expr_effects(arg, effects);
            }
        }
        Expr::In(_) => {
            if !effects.contains(&TbirEffect::PortIo) {
                effects.push(TbirEffect::PortIo);
            }
        }
        Expr::Array(values) => {
            for value in values {
                collect_expr_effects(value, effects);
            }
        }
        Expr::Index { index, .. }
        | Expr::AddressOfIndex { index, .. }
        | Expr::Deref(index)
        | Expr::Unary { expr: index, .. }
        | Expr::Cast { expr: index, .. } => collect_expr_effects(index, effects),
        Expr::Access(path) | Expr::AddressOfAccess(path) => {
            for segment in &path.segments {
                if let AccessSegment::Index(index) = segment {
                    collect_expr_effects(index, effects);
                }
            }
        }
        Expr::StructInit { fields, .. } => {
            for (_, value) in fields {
                collect_expr_effects(value, effects);
            }
        }
        Expr::Binary { left, right, .. } => {
            collect_expr_effects(left, effects);
            collect_expr_effects(right, effects);
        }
        _ => {}
    }
}

fn object_decl(name: &str, kind: TbirObjectKind) -> TbirDeclaration {
    TbirDeclaration::Object {
        name: name.to_owned(),
        kind,
    }
}
