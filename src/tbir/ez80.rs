use crate::{
    asm::AssemblyOptions,
    ast::{AccessPath, AccessSegment, Declaration, Expr, Place, Program, Stmt, Type},
    compat::prelude::*,
    diagnostic::Diagnostic,
    hir::{HirDeclaration, HirProgram},
    target::{Address24, memory_model_for_cpu},
};

use super::{
    TbirAccess, TbirDeclaration, TbirEffect, TbirMemoryModel, TbirMemoryRegion, TbirObjectKind,
    TbirParam, TbirProgram, TbirStmt, TbirTarget, diagnostics, model::SemanticModel, optimize,
    provenance,
};

pub fn lower(
    hir: &HirProgram,
    lowered_program: &Program,
    options: &AssemblyOptions,
) -> Result<TbirProgram, Diagnostic> {
    diagnostics::validate_program(lowered_program, options.cpu)?;
    let memory = memory_model(options)?;
    let capabilities = options.cpu.capabilities();
    let pointer_width_bits = capabilities.memory.pointer_width_bits as u8;
    let semantic = SemanticModel::from_program(
        lowered_program,
        u16::from(pointer_width_bits),
        options.ram_base.get(),
        options.rodata_base.get(),
        options.asset_base.get(),
    )?;
    validate_constant_array_indices(&semantic, lowered_program)?;
    let objects = provenance::memory_objects(lowered_program, &semantic, &memory);
    let context = provenance::OptimizationContext::from_objects(&objects);
    let (lowered_program, optimizations) =
        optimize::optimize_program_with_context(lowered_program, options.cpu, &context);
    let declarations = hir
        .declarations
        .iter()
        .map(|declaration| lower_declaration(declaration, &lowered_program))
        .collect();

    Ok(TbirProgram {
        source: hir.source_path.clone(),
        target: TbirTarget {
            name: capabilities.name.to_owned(),
            pointer_width_bits,
            native_int_widths: capabilities.native_int_widths.to_vec(),
            prefer_code_size: capabilities.prefer_code_size,
            has_cache: capabilities.has_cache,
            supports_port_io: capabilities.supports_port_io,
        },
        memory,
        objects,
        declarations,
        optimizations,
        lowered_program,
    })
}

fn validate_constant_array_indices(
    model: &SemanticModel,
    program: &Program,
) -> Result<(), Diagnostic> {
    fn root_type(
        model: &SemanticModel,
        locals: &HashMap<String, Type>,
        name: &str,
    ) -> Option<Type> {
        locals
            .get(name)
            .cloned()
            .or_else(|| model.global_types.get(name).cloned())
            .or_else(|| model.constant_types.get(name).cloned())
            .or_else(|| model.mmio.get(name).map(|(_, ty, _)| ty.clone()))
            .or_else(|| model.embeds.get(name).map(|embed| embed.ty.clone()))
    }

    fn validate_index(
        model: &SemanticModel,
        index: &Expr,
        ty: &Type,
        root: &str,
    ) -> Result<Option<Type>, Diagnostic> {
        let resolved = model.resolved_type(ty)?;
        match resolved {
            Type::Array { element, len } => {
                if let Ok(index_value) = model.const_value(index) {
                    let len_value = model.const_value(&len)?;
                    if index_value < 0 || index_value >= len_value {
                        return Err(Diagnostic::new(format!(
                            "array index {index_value} is out of bounds for `{root}` with length {len_value}"
                        )));
                    }
                }
                Ok(Some(*element))
            }
            Type::Ptr(element) => Ok(Some(*element)),
            _ => Ok(None),
        }
    }

    fn validate_access(
        model: &SemanticModel,
        path: &AccessPath,
        locals: &HashMap<String, Type>,
    ) -> Result<(), Diagnostic> {
        let mut current = root_type(model, locals, &path.root);
        for segment in &path.segments {
            match segment {
                AccessSegment::Field(field) => {
                    let Some(ty) = current.take() else {
                        continue;
                    };
                    current = model.field(&ty, field).ok().map(|layout| layout.ty.clone());
                }
                AccessSegment::Index(index) => {
                    if let Some(ty) = current.take() {
                        current = validate_index(model, index, &ty, &path.root)?;
                    }
                    validate_expr(model, index, locals)?;
                }
            }
        }
        Ok(())
    }

    fn validate_place(
        model: &SemanticModel,
        place: &Place,
        locals: &HashMap<String, Type>,
    ) -> Result<(), Diagnostic> {
        match place {
            Place::Index { name, index } => {
                if let Some(ty) = root_type(model, locals, name) {
                    let _ = validate_index(model, index, &ty, name)?;
                }
                validate_expr(model, index, locals)
            }
            Place::Access(path) => validate_access(model, path, locals),
            Place::Deref(pointer) => validate_expr(model, pointer, locals),
            Place::Ident(_) | Place::Field { .. } => Ok(()),
        }
    }

    fn validate_expr(
        model: &SemanticModel,
        expr: &Expr,
        locals: &HashMap<String, Type>,
    ) -> Result<(), Diagnostic> {
        match expr {
            Expr::Index { name, index } | Expr::AddressOfIndex { name, index } => {
                if let Some(ty) = root_type(model, locals, name) {
                    let _ = validate_index(model, index, &ty, name)?;
                }
                validate_expr(model, index, locals)?;
            }
            Expr::Access(path) | Expr::AddressOfAccess(path) => {
                validate_access(model, path, locals)?;
            }
            Expr::Array(values) => {
                for value in values {
                    validate_expr(model, value, locals)?;
                }
            }
            Expr::StructInit { fields, .. } => {
                for (_, value) in fields {
                    validate_expr(model, value, locals)?;
                }
            }
            Expr::Deref(value)
            | Expr::BankedPointer { pointer: value, .. }
            | Expr::Unary { expr: value, .. }
            | Expr::Cast { expr: value, .. } => validate_expr(model, value, locals)?,
            Expr::Call { args, .. } => {
                for arg in args {
                    validate_expr(model, arg, locals)?;
                }
            }
            Expr::Binary { left, right, .. } => {
                validate_expr(model, left, locals)?;
                validate_expr(model, right, locals)?;
            }
            Expr::Int(_)
            | Expr::TypedInt(_, _)
            | Expr::Bool(_)
            | Expr::Char(_)
            | Expr::String(_)
            | Expr::Ident(_)
            | Expr::In(_)
            | Expr::Field { .. }
            | Expr::AddressOf(_)
            | Expr::AddressOfField { .. } => {}
        }
        Ok(())
    }

    fn validate_stmts(
        model: &SemanticModel,
        stmts: &[Stmt],
        locals: &HashMap<String, Type>,
    ) -> Result<(), Diagnostic> {
        let mut locals = locals.clone();
        for stmt in stmts {
            match stmt {
                Stmt::Let { name, ty, value } => {
                    validate_expr(model, value, &locals)?;
                    locals.insert(name.clone(), ty.clone());
                }
                Stmt::Assign { target, value, .. } => {
                    validate_place(model, target, &locals)?;
                    validate_expr(model, value, &locals)?;
                }
                Stmt::If {
                    condition,
                    then_body,
                    else_body,
                } => {
                    validate_expr(model, condition, &locals)?;
                    validate_stmts(model, then_body, &locals)?;
                    validate_stmts(model, else_body, &locals)?;
                }
                Stmt::While { condition, body } => {
                    validate_expr(model, condition, &locals)?;
                    validate_stmts(model, body, &locals)?;
                }
                Stmt::Loop { body } => validate_stmts(model, body, &locals)?,
                Stmt::Return(Some(value)) | Stmt::Expr(value) => {
                    validate_expr(model, value, &locals)?;
                }
                Stmt::Out { value, .. } => validate_expr(model, value, &locals)?,
                Stmt::Asm { .. } | Stmt::Return(None) | Stmt::Break | Stmt::Continue => {}
            }
        }
        Ok(())
    }

    fn validate_declarations(
        model: &SemanticModel,
        declarations: &[Declaration],
    ) -> Result<(), Diagnostic> {
        for declaration in declarations {
            match declaration {
                Declaration::Cfg { declaration, .. } | Declaration::Bank { declaration, .. } => {
                    validate_declarations(model, core::slice::from_ref(declaration))?;
                }
                Declaration::Const(declaration) => {
                    validate_expr(model, &declaration.value, &HashMap::new())?;
                }
                Declaration::Global(declaration) => {
                    validate_expr(model, &declaration.value, &HashMap::new())?;
                }
                Declaration::Mmio(declaration) => {
                    validate_expr(model, &declaration.value, &HashMap::new())?;
                }
                Declaration::Function(function) => {
                    let locals = function
                        .params
                        .iter()
                        .map(|param| (param.name.clone(), param.ty.clone()))
                        .collect();
                    validate_stmts(model, &function.body, &locals)?;
                }
                Declaration::Embed(_)
                | Declaration::Import(_)
                | Declaration::Alias(_)
                | Declaration::Port(_)
                | Declaration::Struct(_)
                | Declaration::ExternAsmFunction(_) => {}
            }
        }
        Ok(())
    }

    validate_declarations(model, &program.declarations)
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
            Stmt::Assign {
                target: crate::ast::Place::Deref(_),
                ..
            } => Some(TbirEffect::VolatileMemory),
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
