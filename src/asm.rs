use std::{
    collections::{HashMap, HashSet},
    fs,
    path::Path,
};

use crate::{
    ast::{
        AccessPath, AccessSegment, AssignOp, BinaryOp, Declaration, EmbedSource, Expr, FieldDecl,
        Function, Place, Program, Stmt, Type, UnaryOp,
    },
    diagnostic::Diagnostic,
    target::{
        Address24, EZRA_ASSET_BASE, EZRA_AUDIO_BASE, EZRA_CODE_BASE, EZRA_ENTRY_ADDR,
        EZRA_LOAD_ADDR, EZRA_RAM_BASE, EZRA_RODATA_BASE, EZRA_STACK_TOP, EZRA_VRAM_BASE,
    },
};

pub fn emit_ez80_assembly(program: &Program) -> Result<String, Diagnostic> {
    emit_ez80_assembly_with_options(program, AssemblyOptions::default())
}

pub fn emit_ez80_assembly_with_debug_comments(
    program: &Program,
    debug_comments: bool,
) -> Result<String, Diagnostic> {
    emit_ez80_assembly_with_options(
        program,
        AssemblyOptions {
            debug_comments,
            ..AssemblyOptions::default()
        },
    )
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AssemblyOptions {
    pub debug_comments: bool,
    pub default_sdk_symbols: bool,
    pub load_addr: Address24,
    pub entry_addr: Address24,
    pub code_base: Address24,
    pub stack_top: Address24,
    pub ram_base: Address24,
    pub vram_base: Address24,
    pub audio_base: Address24,
    pub asset_base: Address24,
    pub rodata_base: Address24,
    pub section_bases: Vec<(String, Address24)>,
}

impl Default for AssemblyOptions {
    fn default() -> Self {
        Self {
            debug_comments: false,
            default_sdk_symbols: true,
            load_addr: EZRA_LOAD_ADDR,
            entry_addr: EZRA_ENTRY_ADDR,
            code_base: EZRA_CODE_BASE,
            stack_top: EZRA_STACK_TOP,
            ram_base: EZRA_RAM_BASE,
            vram_base: EZRA_VRAM_BASE,
            audio_base: EZRA_AUDIO_BASE,
            asset_base: EZRA_ASSET_BASE,
            rodata_base: EZRA_RODATA_BASE,
            section_bases: vec![
                (".rodata".to_owned(), EZRA_RODATA_BASE),
                (".assets".to_owned(), EZRA_ASSET_BASE),
            ],
        }
    }
}

pub fn emit_ez80_assembly_with_options(
    program: &Program,
    options: AssemblyOptions,
) -> Result<String, Diagnostic> {
    let symbols = Symbols::from_program(program, options.clone())?;
    let main = program
        .main_function()
        .ok_or_else(|| Diagnostic::new("missing required `fn main()`"))?;
    validate_main_signature(main)?;
    validate_all_function_calls(program, &symbols.functions)?;
    let recursive_call_edges = recursive_call_edges(program, &symbols.functions);
    validate_all_function_bodies(program, symbols.clone(), recursive_call_edges.clone())?;
    let emitted_functions = reachable_function_names(program, &symbols);

    let mut emitter = Emitter::new(symbols, options, recursive_call_edges);
    emitter.emit_prelude();
    emitter.emit_embed_initializers();
    emitter.emit_string_literal_initializers();
    emitter.emit_global_initializers(program)?;
    emitter.emit_start_tail();
    emitter.emit_function(main)?;
    for declaration in &program.declarations {
        let Declaration::Function(function) = declaration else {
            continue;
        };
        if function.name != "main" {
            if emitted_functions.contains(&function.name) {
                emitter.emit_function(function)?;
            }
        }
    }
    emitter.emit_required_sections();
    Ok(peephole_cleanup(&emitter.out))
}

fn peephole_cleanup(assembly: &str) -> String {
    let mut out = String::new();
    let mut previous_redundant_load = None;

    for line in assembly.lines() {
        let redundant_load = redundant_load_key(line);
        if redundant_load.is_some() && redundant_load == previous_redundant_load {
            continue;
        }
        out.push_str(line);
        out.push('\n');
        previous_redundant_load = redundant_load;
    }

    out
}

fn redundant_load_key(line: &str) -> Option<&str> {
    let trimmed = line.trim();
    if !trimmed.starts_with("ld ") || trimmed.contains('(') {
        return None;
    }
    let (target, value) = trimmed.strip_prefix("ld ")?.split_once(',')?;
    let target = target.trim();
    if !matches!(
        target,
        "a" | "b" | "c" | "d" | "e" | "h" | "l" | "hl" | "de" | "bc" | "ix" | "iy" | "sp"
    ) {
        return None;
    }
    if value.trim().is_empty() {
        return None;
    }
    Some(trimmed)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct Variable {
    addr: u32,
    size: u32,
    element_size: Option<u32>,
    len: Option<u32>,
}

impl Variable {
    fn width(self) -> Result<ValueWidth, Diagnostic> {
        if self.element_size.is_some() {
            return Err(Diagnostic::new("array value cannot be used as a scalar"));
        }
        match self.size {
            1 => Ok(ValueWidth::U8),
            2 => Ok(ValueWidth::U16),
            3 => Ok(ValueWidth::U24),
            size => Err(Diagnostic::new(format!(
                "unsupported variable size {size} in codegen"
            ))),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
enum ValueWidth {
    U8,
    U16,
    U24,
}

impl ValueWidth {
    fn bytes(self) -> u8 {
        match self {
            Self::U8 => 1,
            Self::U16 => 2,
            Self::U24 => 3,
        }
    }

    fn max(self, other: Self) -> Self {
        match (self, other) {
            (Self::U24, _) | (_, Self::U24) => Self::U24,
            (Self::U16, _) | (_, Self::U16) => Self::U16,
            (Self::U8, Self::U8) => Self::U8,
        }
    }
}

#[derive(Clone)]
struct Symbols {
    constants: HashMap<String, i64>,
    constant_types: HashMap<String, Type>,
    evaluating_constants: HashSet<String>,
    aliases: HashMap<String, Type>,
    structs: HashMap<String, StructLayout>,
    embeds: HashMap<String, EmbedObject>,
    string_literals: HashMap<String, Variable>,
    ports: HashMap<String, u8>,
    globals: HashMap<String, Variable>,
    global_types: HashMap<String, Type>,
    readonly_global_pointer_aliases: HashMap<String, u32>,
    functions: HashMap<String, FunctionSig>,
    inline_functions: HashMap<String, Function>,
    next_addr: u32,
    asset_next_addr: u32,
    rodata_next_addr: u32,
    section_next_addrs: Vec<(String, u32)>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct EmbedObject {
    variable: Variable,
    bytes: Vec<u8>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct StructLayout {
    size: u32,
    fields: HashMap<String, StructField>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct StructField {
    offset: u32,
    ty: Type,
    size: u32,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct FunctionSig {
    arity: usize,
    params: Vec<ValueWidth>,
    param_types: Vec<Type>,
    arg_slots: Vec<Variable>,
    uses_arg_slots: bool,
    stack_arg_offsets: Vec<Option<u8>>,
    stack_arg_bytes: u8,
    return_width: ValueWidth,
    return_type: Option<Type>,
}

impl Symbols {
    fn from_program(program: &Program, options: AssemblyOptions) -> Result<Self, Diagnostic> {
        let mut symbols = Self {
            constants: sdk_constants(&options),
            constant_types: sdk_constant_types(&options),
            evaluating_constants: HashSet::new(),
            aliases: HashMap::new(),
            structs: HashMap::new(),
            embeds: HashMap::new(),
            string_literals: HashMap::new(),
            ports: sdk_ports(&options),
            globals: HashMap::new(),
            global_types: HashMap::new(),
            readonly_global_pointer_aliases: HashMap::new(),
            functions: HashMap::new(),
            inline_functions: HashMap::new(),
            next_addr: options.ram_base.get(),
            asset_next_addr: options.asset_base.get(),
            rodata_next_addr: options.rodata_base.get(),
            section_next_addrs: options
                .section_bases
                .iter()
                .filter(|(name, _)| !matches!(name.as_str(), ".assets" | ".rodata"))
                .map(|(name, base)| (name.clone(), base.get()))
                .collect(),
        };

        let mut declared_names = HashSet::new();
        for declaration in &program.declarations {
            let Some(name) = declaration_name(declaration) else {
                continue;
            };
            if !declared_names.insert(name.to_owned()) {
                return Err(Diagnostic::new(format!("duplicate declaration `{name}`")));
            }
        }

        for declaration in &program.declarations {
            if let Declaration::Alias(decl) = declaration {
                symbols.aliases.insert(decl.name.clone(), decl.ty.clone());
            }
        }

        for declaration in &program.declarations {
            if let Declaration::Struct(decl) = declaration {
                let layout = symbols.build_struct_layout(&decl.fields, program)?;
                symbols.structs.insert(decl.name.clone(), layout);
            }
        }

        for declaration in &program.declarations {
            if let Declaration::Alias(decl) = declaration {
                symbols.validate_type_names(&decl.ty)?;
            }
        }

        for declaration in &program.declarations {
            let (name, params, return_type, extern_asm) = match declaration {
                Declaration::Function(function) => (
                    &function.name,
                    &function.params,
                    &function.return_type,
                    false,
                ),
                Declaration::ExternAsmFunction(function) => (
                    &function.name,
                    &function.params,
                    &function.return_type,
                    true,
                ),
                _ => continue,
            };
            let mut param_names = HashSet::new();
            for param in params {
                if !param_names.insert(param.name.as_str()) {
                    return Err(Diagnostic::new(format!(
                        "function `{name}` has duplicate parameter `{}`",
                        param.name
                    )));
                }
                symbols.validate_signature_value_type(
                    name,
                    &param.ty,
                    "parameter",
                    Some(&param.name),
                )?;
                symbols.type_width(&param.ty)?;
            }
            if let Some(return_type) = return_type {
                symbols.validate_signature_value_type(name, return_type, "return type", None)?;
            }
            let param_widths = params
                .iter()
                .map(|param| symbols.type_width(&param.ty))
                .collect::<Result<Vec<_>, _>>()?;
            let uses_arg_slots = param_widths.get(2).is_some_and(|third| third.bytes() != 1)
                && param_widths
                    .get(1)
                    .is_some_and(|second| second.bytes() == 1);
            if extern_asm && uses_arg_slots {
                return Err(Diagnostic::new(format!(
                    "extern asm function `{name}` cannot use a byte second argument followed by a wide third argument"
                )));
            }
            let mut stack_arg_offsets = vec![None; params.len()];
            let mut stack_arg_bytes = 0u8;
            if params.len() > 3 {
                let mut offset = 6u8;
                for (index, width) in param_widths.iter().enumerate().skip(3) {
                    let bytes = width.bytes();
                    if offset as u16 + bytes as u16 > 0x80 {
                        return Err(Diagnostic::new(format!(
                            "function `{name}` stack arguments exceed IX displacement range"
                        )));
                    }
                    stack_arg_offsets[index] = Some(offset);
                    offset += bytes;
                    stack_arg_bytes += bytes;
                }
            }
            let arg_slots = if uses_arg_slots {
                param_widths
                    .iter()
                    .map(|width| symbols.alloc_var(width.bytes()))
                    .collect()
            } else {
                Vec::new()
            };
            symbols.functions.insert(
                name.clone(),
                FunctionSig {
                    arity: params.len(),
                    params: param_widths,
                    param_types: params.iter().map(|param| param.ty.clone()).collect(),
                    arg_slots,
                    uses_arg_slots,
                    stack_arg_offsets,
                    stack_arg_bytes,
                    return_width: return_type
                        .as_ref()
                        .map(|ty| symbols.type_width(ty))
                        .transpose()?
                        .unwrap_or(ValueWidth::U8),
                    return_type: return_type.clone(),
                },
            );
            if let Declaration::Function(function) = declaration {
                if is_inlinable_function(function) {
                    symbols
                        .inline_functions
                        .insert(function.name.clone(), function.clone());
                }
            }
        }

        for declaration in &program.declarations {
            match declaration {
                Declaration::Const(decl) => {
                    symbols.evaluate_const_declaration(decl, program)?;
                }
                Declaration::Port(decl) => {
                    let resolved = symbols.resolved_type(&decl.ty)?;
                    if resolved != Type::Named("u8".to_owned()) {
                        return Err(Diagnostic::new(format!(
                            "port `{}` type `{}` must be u8",
                            decl.name,
                            type_display(&decl.ty)
                        )));
                    }
                    symbols.ensure_const_dependencies_evaluated(&decl.value, program)?;
                    let value = symbols.eval_i64(&decl.value)?;
                    if !(0..=0xFF).contains(&value) {
                        return Err(Diagnostic::new(format!(
                            "port `{}` value {value} is outside u8 range",
                            decl.name
                        )));
                    }
                    symbols.ports.insert(decl.name.clone(), value as u8);
                }
                Declaration::Mmio(decl) => {
                    symbols.type_width(&decl.ty)?;
                    let resolved = symbols.resolved_type(&decl.ty)?;
                    let is_pointer = match &resolved {
                        Type::Ptr(_) => true,
                        Type::Named(name) if name == "ptr24" => true,
                        _ => false,
                    };
                    if !is_pointer {
                        return Err(Diagnostic::new(format!(
                            "mmio `{}` type `{}` must be a pointer type",
                            decl.name,
                            type_display(&decl.ty)
                        )));
                    }
                    symbols.ensure_const_dependencies_evaluated(&decl.value, program)?;
                    let value = symbols.eval_i64(&decl.value)?;
                    if !(0..=0xFF_FFFF).contains(&value) {
                        return Err(Diagnostic::new(format!(
                            "mmio `{}` value {value} is outside 24-bit address range",
                            decl.name
                        )));
                    }
                    symbols.constants.insert(decl.name.clone(), value);
                    symbols
                        .constant_types
                        .insert(decl.name.clone(), decl.ty.clone());
                }
                Declaration::Embed(decl) => {
                    if !symbols.embeds.contains_key(&decl.name) {
                        symbols.allocate_embed_declaration(decl, program)?;
                    }
                }
                Declaration::Global(decl) => {
                    if !symbols.globals.contains_key(&decl.name) {
                        symbols.allocate_global_declaration(decl, program)?;
                    }
                }
                Declaration::Struct(_) => {}
                _ => {}
            }
        }

        symbols.record_readonly_global_pointer_aliases(program);

        Ok(symbols)
    }

    fn alloc_var<S: Into<u32>>(&mut self, size: S) -> Variable {
        let size = size.into();
        let variable = Variable {
            addr: self.next_addr,
            size,
            element_size: None,
            len: None,
        };
        self.next_addr += size;
        variable
    }

    fn alloc_array(&mut self, element_size: u32, len: u32) -> Variable {
        let size = element_size * len;
        let variable = Variable {
            addr: self.next_addr,
            size,
            element_size: Some(element_size),
            len: Some(len),
        };
        self.next_addr += size;
        variable
    }

    fn intern_string_literal(&mut self, value: &str) -> Result<Variable, Diagnostic> {
        if let Some(variable) = self.string_literals.get(value).copied() {
            return Ok(variable);
        }
        let len = value
            .len()
            .checked_add(1)
            .ok_or_else(|| Diagnostic::new("string literal is too large"))?;
        if len > u32::MAX as usize {
            return Err(Diagnostic::new("string literal is too large"));
        }
        let size =
            u32::try_from(len).map_err(|_| Diagnostic::new("string literal is too large"))?;
        let variable = Variable {
            addr: self.rodata_next_addr,
            size,
            element_size: Some(u32::from(ValueWidth::U8.bytes())),
            len: Some(size),
        };
        self.rodata_next_addr = self
            .rodata_next_addr
            .checked_add(size)
            .ok_or_else(|| Diagnostic::new("string literal exceeds 24-bit address space"))?;
        if self.rodata_next_addr > Address24::MAX + 1 {
            return Err(Diagnostic::new(
                "string literal exceeds 24-bit address space",
            ));
        }
        self.string_literals.insert(value.to_owned(), variable);
        Ok(variable)
    }

    fn alloc_section_bytes(
        &mut self,
        section: &str,
        align: u32,
        len: u32,
    ) -> Result<Variable, Diagnostic> {
        match section {
            ".assets" => {
                let variable = alloc_from_cursor(&mut self.asset_next_addr, align, len)?;
                Ok(variable)
            }
            ".rodata" => {
                let variable = alloc_from_cursor(&mut self.rodata_next_addr, align, len)?;
                Ok(variable)
            }
            _ if self
                .section_next_addrs
                .iter()
                .any(|(name, _)| name == section) =>
            {
                let cursor = section_cursor(&mut self.section_next_addrs, section);
                let variable = alloc_from_cursor(cursor, align, len)?;
                Ok(variable)
            }
            _ => {
                self.align_next_addr(align);
                Ok(self.alloc_array(u32::from(ValueWidth::U8.bytes()), len))
            }
        }
    }

    fn align_next_addr(&mut self, align: u32) {
        if align <= 1 {
            return;
        }
        self.next_addr = (self.next_addr + align - 1) & !(align - 1);
    }

    fn register_embed_properties(&mut self, name: &str, variable: Variable, len: u32) {
        let ptr_ty = Type::Ptr(Box::new(Type::Named("u8".to_owned())));
        for (property, value, ty) in [
            ("ptr", variable.addr as i64, ptr_ty.clone()),
            ("len", len as i64, Type::Named("u24".to_owned())),
            ("end", (variable.addr + len) as i64, ptr_ty),
        ] {
            let key = format!("{name}.{property}");
            self.constants.insert(key.clone(), value);
            self.constant_types.insert(key, ty);
        }
    }

    fn evaluate_const_declaration(
        &mut self,
        decl: &crate::ast::ConstDecl,
        program: &Program,
    ) -> Result<(), Diagnostic> {
        if !self.evaluating_constants.insert(decl.name.clone()) {
            return Err(Diagnostic::new(format!(
                "circular constant reference involving `{}`",
                decl.name
            )));
        }

        let result = (|| {
            self.ensure_const_dependencies_evaluated(&decl.value, program)?;
            let mut address_roots = Vec::new();
            collect_const_address_roots(&decl.value, &mut address_roots);
            for root in address_roots {
                if !self.globals.contains_key(&root) {
                    self.ensure_global_storage_allocated_through(&root, program)?;
                }
            }
            self.validate_const_expr_arithmetic_compatibility(&decl.value)?;
            let mut value = if let Expr::String(text) = &decl.value {
                if self.resolved_type(&decl.ty)? != ptr_u8_type() {
                    return Err(Diagnostic::new("type mismatch"));
                }
                self.intern_string_literal(text)?.addr as i64
            } else {
                self.eval_i64(&decl.value)?
            };
            if !matches!(decl.value, Expr::String(_))
                && self.const_expr_uses_wrapping_arithmetic(&decl.value)
            {
                value = self.wrap_value_for_type(value, &decl.ty)?;
            }
            self.validate_value_for_type(value, &decl.ty)?;
            self.constants.insert(decl.name.clone(), value);
            self.constant_types
                .insert(decl.name.clone(), decl.ty.clone());
            Ok(())
        })();

        self.evaluating_constants.remove(&decl.name);
        result
    }

    fn ensure_const_dependencies_evaluated(
        &mut self,
        expr: &Expr,
        program: &Program,
    ) -> Result<(), Diagnostic> {
        let mut names = Vec::new();
        collect_const_dependency_names(expr, &mut names);
        for name in names {
            if let Some(decl) = find_const_declaration(program, &name) {
                self.evaluate_const_declaration(decl, program)?;
                continue;
            }
            if self.constant_types.contains_key(&name) {
                continue;
            }
            if self.constants.contains_key(&name) || self.embed_property_value(&name).is_some() {
                continue;
            }
        }
        Ok(())
    }

    fn ensure_global_storage_allocated_through(
        &mut self,
        target: &str,
        program: &Program,
    ) -> Result<(), Diagnostic> {
        let Some(target_index) = program.declarations.iter().position(
            |declaration| matches!(declaration, Declaration::Global(decl) if decl.name == target),
        ) else {
            return Ok(());
        };

        for declaration in &program.declarations[..=target_index] {
            match declaration {
                Declaration::Const(decl)
                    if !self.constant_types.contains_key(&decl.name)
                        && !self.evaluating_constants.contains(&decl.name) =>
                {
                    self.evaluate_const_declaration(decl, program)?;
                }
                Declaration::Embed(decl) if !self.embeds.contains_key(&decl.name) => {
                    self.allocate_embed_declaration(decl, program)?;
                }
                Declaration::Global(decl) if !self.globals.contains_key(&decl.name) => {
                    self.allocate_global_declaration(decl, program)?;
                }
                _ => {}
            }
        }

        Ok(())
    }

    fn allocate_embed_declaration(
        &mut self,
        decl: &crate::ast::EmbedDecl,
        program: &Program,
    ) -> Result<(), Diagnostic> {
        if let Some(align) = &decl.align {
            self.ensure_const_dependencies_evaluated(align, program)?;
            self.validate_embed_alignment_expr(&decl.name, align)?;
        }
        self.ensure_embed_source_const_dependencies_evaluated(&decl.source, program)?;
        let align = decl
            .align
            .as_ref()
            .map(|expr| self.eval_i64(expr))
            .transpose()?
            .unwrap_or(1);
        if align <= 0 || (align & (align - 1)) != 0 {
            return Err(Diagnostic::new(format!(
                "embed `{}` alignment {align} is not a positive power of two",
                decl.name
            )));
        }
        let align = u32::try_from(align).map_err(|_| {
            Diagnostic::new(format!(
                "embed `{}` alignment {align} exceeds 24-bit address space",
                decl.name
            ))
        })?;
        if let Some(original) = module_alias_original_name(&decl.name) {
            if let Some(embed) = self.embeds.get(original).cloned() {
                self.register_embed_properties(
                    &decl.name,
                    embed.variable,
                    embed.variable.len.unwrap_or(0),
                );
                return Ok(());
            }
        }
        let bytes = self.embed_bytes(&decl.source, &program.source_path)?;
        let len = u32::try_from(bytes.len())
            .map_err(|_| Diagnostic::new("embedded asset exceeds 24-bit address space"))?;
        let section = decl.section.as_deref().unwrap_or(".assets");
        let variable = self.alloc_section_bytes(section, align, len)?;
        self.register_embed_properties(&decl.name, variable, len);
        self.embeds
            .insert(decl.name.clone(), EmbedObject { variable, bytes });
        Ok(())
    }

    fn validate_embed_alignment_expr(&self, name: &str, expr: &Expr) -> Result<(), Diagnostic> {
        let ty = self.resolved_type(&self.const_expr_type(expr)?)?;
        if type_is_bool(&ty) || matches!(ty, Type::Ptr(_)) {
            return Err(Diagnostic::new(format!(
                "embed `{name}` alignment must be an integer constant"
            )));
        }
        self.type_width(&ty)?;
        self.validate_const_expr_arithmetic_compatibility(expr)
    }

    fn allocate_global_declaration(
        &mut self,
        decl: &crate::ast::GlobalDecl,
        program: &Program,
    ) -> Result<(), Diagnostic> {
        if let Some(original) = module_alias_original_name(&decl.name) {
            if let Some(variable) = self.globals.get(original).copied() {
                self.globals.insert(decl.name.clone(), variable);
                if let Some(ty) = self.global_types.get(original).cloned() {
                    self.global_types.insert(decl.name.clone(), ty);
                }
                return Ok(());
            }
        }
        self.ensure_type_const_dependencies_evaluated(&decl.ty, program)?;
        let variable = self.alloc_storage(&decl.ty)?;
        self.globals.insert(decl.name.clone(), variable);
        self.global_types.insert(decl.name.clone(), decl.ty.clone());
        Ok(())
    }

    fn record_readonly_global_pointer_aliases(&mut self, program: &Program) {
        let assigned = assigned_names_in_program(program);
        for declaration in &program.declarations {
            let Declaration::Global(decl) = declaration else {
                continue;
            };
            if assigned.contains(&decl.name) {
                continue;
            }
            if let Some(addr) = self.readonly_initializer_addr(&decl.value) {
                self.readonly_global_pointer_aliases
                    .insert(decl.name.clone(), addr);
            }
        }
    }

    fn readonly_initializer_addr(&mut self, expr: &Expr) -> Option<u32> {
        let addr = match expr {
            Expr::String(value) => self.intern_string_literal(value).ok()?.addr,
            _ => addr24(self.eval_i64(expr).ok()?)?,
        };
        if self.readonly_embed_name_for_addr(addr).is_some()
            || self.readonly_string_literal_for_addr(addr).is_some()
        {
            Some(addr)
        } else {
            None
        }
    }

    fn ensure_type_const_dependencies_evaluated(
        &mut self,
        ty: &Type,
        program: &Program,
    ) -> Result<(), Diagnostic> {
        match ty {
            Type::Ptr(inner) => self.ensure_type_const_dependencies_evaluated(inner, program),
            Type::Array { element, len } => {
                self.ensure_type_const_dependencies_evaluated(element, program)?;
                self.ensure_const_dependencies_evaluated(len, program)
            }
            Type::Named(name) => {
                if let Some(alias) = self.aliases.get(name).cloned() {
                    self.ensure_type_const_dependencies_evaluated(&alias, program)
                } else {
                    Ok(())
                }
            }
        }
    }

    fn ensure_embed_source_const_dependencies_evaluated(
        &mut self,
        source: &EmbedSource,
        program: &Program,
    ) -> Result<(), Diagnostic> {
        match source {
            EmbedSource::Bytes(values) => {
                for value in values {
                    self.ensure_const_dependencies_evaluated(value, program)?;
                }
                Ok(())
            }
            EmbedSource::Repeat { value, len } => {
                self.ensure_const_dependencies_evaluated(value, program)?;
                self.ensure_const_dependencies_evaluated(len, program)
            }
            EmbedSource::File(_) | EmbedSource::Text(_) | EmbedSource::CStr(_) => Ok(()),
        }
    }

    fn embed_bytes(&self, source: &EmbedSource, source_path: &Path) -> Result<Vec<u8>, Diagnostic> {
        match source {
            EmbedSource::File(path) => {
                let path = Path::new(path);
                let resolved = if path.is_absolute() {
                    path.to_path_buf()
                } else {
                    source_path
                        .parent()
                        .unwrap_or_else(|| Path::new("."))
                        .join(path)
                };
                fs::read(&resolved).map_err(|error| {
                    if error.kind() == std::io::ErrorKind::NotFound {
                        Diagnostic::new(format!("embedded file `{}` not found", resolved.display()))
                    } else {
                        Diagnostic::new(format!(
                            "failed to read embedded file `{}`: {error}",
                            resolved.display()
                        ))
                    }
                })
            }
            EmbedSource::Bytes(values) => values
                .iter()
                .map(|value| {
                    let byte = self.eval_i64(value)?;
                    if !(0..=0xFF).contains(&byte) {
                        return Err(Diagnostic::new(format!(
                            "embedded byte value {byte} is outside u8 range"
                        )));
                    }
                    Ok(byte as u8)
                })
                .collect(),
            EmbedSource::Text(text) => Ok(text.as_bytes().to_vec()),
            EmbedSource::CStr(text) => {
                let mut bytes = text.as_bytes().to_vec();
                bytes.push(0);
                Ok(bytes)
            }
            EmbedSource::Repeat { value, len } => {
                let byte = self.eval_i64(value)?;
                if !(0..=0xFF).contains(&byte) {
                    return Err(Diagnostic::new(format!(
                        "embedded repeat byte value {byte} is outside u8 range"
                    )));
                }
                let len = self.eval_i64(len)?;
                if !(0..=0xFF_FFFF).contains(&len) {
                    return Err(Diagnostic::new(format!(
                        "embedded repeat length {len} is outside u24 range"
                    )));
                }
                Ok(vec![byte as u8; len as usize])
            }
        }
    }

    fn build_struct_layout(
        &mut self,
        fields: &[FieldDecl],
        program: &Program,
    ) -> Result<StructLayout, Diagnostic> {
        let mut offset = 0u32;
        let mut layout_fields = HashMap::new();
        for field in fields {
            self.ensure_type_const_dependencies_evaluated(&field.ty, program)?;
            let size = self.type_size(&field.ty)?;
            if layout_fields
                .insert(
                    field.name.clone(),
                    StructField {
                        offset,
                        ty: field.ty.clone(),
                        size,
                    },
                )
                .is_some()
            {
                return Err(Diagnostic::new(format!(
                    "duplicate struct field `{}`",
                    field.name
                )));
            }
            offset += u32::from(size);
        }
        Ok(StructLayout {
            size: offset,
            fields: layout_fields,
        })
    }

    fn type_width(&self, ty: &Type) -> Result<ValueWidth, Diagnostic> {
        match ty {
            Type::Named(name) if name == "u8" || name == "i8" || name == "bool" => {
                Ok(ValueWidth::U8)
            }
            Type::Named(name) if name == "u16" || name == "i16" => Ok(ValueWidth::U16),
            Type::Named(name) if name == "u24" || name == "i24" || name == "ptr24" => {
                Ok(ValueWidth::U24)
            }
            Type::Named(name) if matches!(name.as_str(), "u32" | "i32" | "u64" | "i64") => {
                Err(Diagnostic::new(format!(
                    "type `{name}` is not supported; use explicit u8/u16/u24 or i8/i16/i24"
                )))
            }
            Type::Named(name) => {
                if self.structs.contains_key(name) {
                    return Err(Diagnostic::new(format!(
                        "struct `{name}` cannot be used as a scalar value"
                    )));
                }
                let Some(alias) = self.aliases.get(name) else {
                    return Err(Diagnostic::new(format!("unknown type `{name}`")));
                };
                self.type_width(alias)
            }
            Type::Ptr(_) => Ok(ValueWidth::U24),
            Type::Array { .. } => Err(Diagnostic::new(
                "array storage codegen is not implemented yet",
            )),
        }
    }

    fn validate_type_names(&self, ty: &Type) -> Result<(), Diagnostic> {
        match ty {
            Type::Named(name)
                if matches!(
                    name.as_str(),
                    "u8" | "i8" | "u16" | "i16" | "u24" | "i24" | "bool" | "ptr24"
                ) =>
            {
                Ok(())
            }
            Type::Named(name) if matches!(name.as_str(), "u32" | "i32" | "u64" | "i64") => {
                Err(Diagnostic::new(format!(
                    "type `{name}` is not supported; use explicit u8/u16/u24 or i8/i16/i24"
                )))
            }
            Type::Named(name) => {
                if self.structs.contains_key(name) || self.aliases.contains_key(name) {
                    Ok(())
                } else {
                    Err(Diagnostic::new(format!("unknown type `{name}`")))
                }
            }
            Type::Ptr(inner) => self.validate_type_names(inner),
            Type::Array { element, .. } => self.validate_type_names(element),
        }
    }

    fn validate_signature_value_type(
        &self,
        function: &str,
        ty: &Type,
        role: &str,
        name: Option<&str>,
    ) -> Result<(), Diagnostic> {
        let resolved = self.resolved_type(ty)?;
        let invalid = match &resolved {
            Type::Array { .. } => Some("an array"),
            Type::Named(name) if self.structs.contains_key(name) => Some("a struct"),
            _ => None,
        };
        if let Some(kind) = invalid {
            let subject = name
                .map(|name| format!("{role} `{name}` type"))
                .unwrap_or_else(|| role.to_owned());
            return Err(Diagnostic::new(format!(
                "function `{function}` {subject} `{}` is {kind}; pass it by pointer",
                type_display(ty)
            )));
        }
        Ok(())
    }

    fn validate_value_for_type(&self, value: i64, ty: &Type) -> Result<(), Diagnostic> {
        let resolved = self.resolved_type(ty)?;
        if matches!(&resolved, Type::Named(name) if name == "bool") {
            if !(0..=1).contains(&value) {
                return Err(Diagnostic::new(format!(
                    "value {value} is outside bool range"
                )));
            }
            return Ok(());
        }

        let width = self.type_width(&resolved)?;
        let bits = u32::from(width.bytes()) * 8;
        if type_is_signed(&resolved) {
            let min = -(1_i64 << (bits - 1));
            let max = (1_i64 << (bits - 1)) - 1;
            if !(min..=max).contains(&value) {
                return Err(Diagnostic::new(format!(
                    "value {value} is outside {} range",
                    type_display(&resolved)
                )));
            }
        } else {
            let max = (1_i64 << bits) - 1;
            if !(0..=max).contains(&value) {
                return Err(Diagnostic::new(format!(
                    "value {value} is outside {} range",
                    type_display(&resolved)
                )));
            }
        }
        Ok(())
    }

    fn resolved_type(&self, ty: &Type) -> Result<Type, Diagnostic> {
        match ty {
            Type::Named(name) => {
                if let Some(alias) = self.aliases.get(name) {
                    self.resolved_type(alias)
                } else {
                    Ok(ty.clone())
                }
            }
            Type::Ptr(inner) => Ok(Type::Ptr(Box::new(self.resolved_type(inner)?))),
            Type::Array { element, len } => Ok(Type::Array {
                element: Box::new(self.resolved_type(element)?),
                len: len.clone(),
            }),
        }
    }

    fn type_size(&self, ty: &Type) -> Result<u32, Diagnostic> {
        match self.resolved_type(ty)? {
            Type::Array { element, len } => {
                let element_size = self.type_size(&element)?;
                let len = self.array_len(&len)?;
                let size = element_size
                    .checked_mul(len)
                    .ok_or_else(|| Diagnostic::new("array size exceeds 24-bit address space"))?;
                if size > 0xFF_FFFF {
                    return Err(Diagnostic::new(format!(
                        "array size {size} exceeds 24-bit address space"
                    )));
                }
                Ok(size)
            }
            Type::Named(name) if self.structs.contains_key(&name) => {
                let size = self.structs[&name].size;
                if size > 0xFF_FFFF {
                    return Err(Diagnostic::new(format!(
                        "struct `{name}` size {size} exceeds 24-bit address space"
                    )));
                }
                Ok(size)
            }
            scalar => Ok(u32::from(self.type_width(&scalar)?.bytes())),
        }
    }

    fn alloc_storage(&mut self, ty: &Type) -> Result<Variable, Diagnostic> {
        match self.resolved_type(ty)? {
            Type::Array { element, len } => {
                let element_size = self.type_size(&element)?;
                let len = self.array_len(&len)?;
                Ok(self.alloc_array(element_size, len))
            }
            Type::Named(name) if self.structs.contains_key(&name) => {
                let size = self.type_size(&Type::Named(name))?;
                Ok(self.alloc_var(size))
            }
            scalar => Ok(self.alloc_var(self.type_width(&scalar)?.bytes())),
        }
    }

    fn storage_at(&self, addr: u32, ty: &Type) -> Result<Variable, Diagnostic> {
        match self.resolved_type(ty)? {
            Type::Array { element, len } => {
                let element_size = self.type_size(&element)?;
                let len = self.array_len(&len)?;
                Ok(Variable {
                    addr,
                    size: element_size * len,
                    element_size: Some(element_size),
                    len: Some(len),
                })
            }
            resolved => Ok(scalar_var(addr, self.type_size(&resolved)?)),
        }
    }

    fn const_address_value(&self, expr: &Expr) -> Result<i64, Diagnostic> {
        let variable = match expr {
            Expr::AddressOf(name) => self
                .globals
                .get(name)
                .copied()
                .ok_or_else(|| Diagnostic::new(format!("unknown variable `{name}`")))?,
            Expr::AddressOfIndex { name, index } => {
                self.const_array_element_variable(name, index)?
            }
            Expr::AddressOfField { base, field } => self.const_field_variable(base, field)?,
            Expr::AddressOfAccess(path) => self.const_access_variable(path)?,
            _ => unreachable!("not an address-of expression"),
        };
        Ok(variable.addr as i64)
    }

    fn const_address_type(&self, expr: &Expr) -> Result<Type, Diagnostic> {
        let ty = match expr {
            Expr::AddressOf(name) => self
                .global_types
                .get(name)
                .cloned()
                .ok_or_else(|| Diagnostic::new(format!("unknown variable `{name}`")))?,
            Expr::AddressOfIndex { name, .. } => self.array_element_type(name)?,
            Expr::AddressOfField { base, field } => self.field_type(base, field)?,
            Expr::AddressOfAccess(path) => self.access_type(path)?,
            _ => unreachable!("not an address-of expression"),
        };
        Ok(Type::Ptr(Box::new(self.resolved_type(&ty)?)))
    }

    fn const_array_element_variable(
        &self,
        name: &str,
        index: &Expr,
    ) -> Result<Variable, Diagnostic> {
        let array = self
            .globals
            .get(name)
            .copied()
            .ok_or_else(|| Diagnostic::new(format!("unknown array `{name}`")))?;
        let Type::Array { element, len } = self
            .global_types
            .get(name)
            .ok_or_else(|| Diagnostic::new(format!("unknown array `{name}`")))
            .and_then(|ty| self.resolved_type(ty))?
        else {
            return Err(Diagnostic::new(format!("`{name}` is not an array")));
        };
        let index_value = self.eval_i64(index)?;
        let len = self.array_len(&len)?;
        if index_value < 0 || index_value as u32 >= len {
            return Err(Diagnostic::new(format!(
                "array index {index_value} is out of bounds for `{name}` length {len}"
            )));
        }
        let element_size = self.type_size(&element)?;
        self.storage_at(array.addr + index_value as u32 * element_size, &element)
    }

    fn const_field_variable(&self, base: &str, field: &str) -> Result<Variable, Diagnostic> {
        let base_variable = self
            .globals
            .get(base)
            .copied()
            .ok_or_else(|| Diagnostic::new(format!("unknown variable `{base}`")))?;
        let field = self.const_field_info(
            self.global_types
                .get(base)
                .ok_or_else(|| Diagnostic::new(format!("unknown variable `{base}`")))?,
            field,
        )?;
        self.storage_at(base_variable.addr + field.offset, &field.ty)
    }

    fn const_field_info(&self, ty: &Type, field: &str) -> Result<StructField, Diagnostic> {
        let Type::Named(struct_name) = self.resolved_type(ty)? else {
            return Err(Diagnostic::new(format!(
                "type `{}` is not a struct type",
                type_display(ty)
            )));
        };
        let layout = self
            .structs
            .get(&struct_name)
            .ok_or_else(|| Diagnostic::new(format!("unknown struct `{struct_name}`")))?;
        layout.fields.get(field).cloned().ok_or_else(|| {
            Diagnostic::new(format!("struct `{struct_name}` has no field `{field}`"))
        })
    }

    fn const_access_variable(&self, path: &AccessPath) -> Result<Variable, Diagnostic> {
        let mut variable = self
            .globals
            .get(&path.root)
            .copied()
            .ok_or_else(|| Diagnostic::new(format!("unknown variable `{}`", path.root)))?;
        let mut ty = self
            .global_types
            .get(&path.root)
            .cloned()
            .ok_or_else(|| Diagnostic::new(format!("unknown variable `{}`", path.root)))?;

        for segment in &path.segments {
            match segment {
                AccessSegment::Field(field) => {
                    let field = self.const_field_info(&ty, field)?;
                    variable = self.storage_at(variable.addr + field.offset, &field.ty)?;
                    ty = field.ty;
                }
                AccessSegment::Index(index) => {
                    let Type::Array { element, len } = self.resolved_type(&ty)? else {
                        return Err(Diagnostic::new(format!(
                            "value `{}` is not an array",
                            access_path_summary(path)
                        )));
                    };
                    let index_value = self.eval_i64(index)?;
                    let len = self.array_len(&len)?;
                    if index_value < 0 || index_value as u32 >= len {
                        return Err(Diagnostic::new(format!(
                            "array index {index_value} is out of bounds for `{}` length {len}",
                            access_path_summary(path)
                        )));
                    }
                    let element_size = self.type_size(&element)?;
                    variable = self
                        .storage_at(variable.addr + index_value as u32 * element_size, &element)?;
                    ty = *element;
                }
            }
        }

        Ok(variable)
    }

    fn array_element_type(&self, name: &str) -> Result<Type, Diagnostic> {
        let Some(ty) = self.global_types.get(name) else {
            return Err(Diagnostic::new(format!("unknown array `{name}`")));
        };
        match self.resolved_type(ty)? {
            Type::Array { element, .. } => Ok(*element),
            _ => Err(Diagnostic::new(format!("`{name}` is not an array"))),
        }
    }

    fn field_type(&self, base: &str, field: &str) -> Result<Type, Diagnostic> {
        let Some(ty) = self.global_types.get(base) else {
            return Err(Diagnostic::new(format!("unknown variable `{base}`")));
        };
        self.const_field_info(ty, field).map(|field| field.ty)
    }

    fn access_type(&self, path: &AccessPath) -> Result<Type, Diagnostic> {
        let mut ty = self
            .global_types
            .get(&path.root)
            .cloned()
            .ok_or_else(|| Diagnostic::new(format!("unknown variable `{}`", path.root)))?;
        for segment in &path.segments {
            ty = match segment {
                AccessSegment::Field(field) => self.const_field_info(&ty, field)?.ty,
                AccessSegment::Index(_) => match self.resolved_type(&ty)? {
                    Type::Array { element, .. } => *element,
                    _ => {
                        return Err(Diagnostic::new(format!(
                            "value `{}` is not an array",
                            access_path_summary(path)
                        )));
                    }
                },
            };
        }
        Ok(ty)
    }

    fn array_len(&self, expr: &Expr) -> Result<u32, Diagnostic> {
        let ty = self.resolved_type(&self.const_expr_type(expr)?)?;
        if type_is_bool(&ty) || matches!(ty, Type::Ptr(_)) {
            return Err(Diagnostic::new("array length must be an integer constant"));
        }
        self.validate_const_expr_arithmetic_compatibility(expr)?;
        let value = self.eval_i64(expr)?;
        if value < 0 {
            return Err(Diagnostic::new(format!("array length {value} is negative")));
        }
        if value > 0xFF_FFFF {
            return Err(Diagnostic::new(format!(
                "array length {value} exceeds 24-bit address space"
            )));
        }
        Ok(value as u32)
    }

    fn eval_i64(&self, expr: &Expr) -> Result<i64, Diagnostic> {
        match expr {
            Expr::Int(value) | Expr::TypedInt(value, _) => Ok(*value),
            Expr::Char(value) => Ok(*value as i64),
            Expr::Bool(value) => Ok(i64::from(*value)),
            Expr::Ident(name) => self.const_value(name),
            Expr::Field { base, field } => self.const_value(&format!("{base}.{field}")),
            Expr::Access(path) => {
                let name = const_access_name(path)?;
                self.const_value(&name)
            }
            Expr::Unary { op, expr } => {
                let value = self.eval_i64(expr)?;
                Ok(match op {
                    UnaryOp::Neg => value.wrapping_neg(),
                    UnaryOp::BitNot => !value,
                    UnaryOp::Not => i64::from(value == 0),
                })
            }
            Expr::Binary { left, op, right } => {
                let left_signed =
                    type_is_signed(&self.resolved_type(&self.const_expr_type(left)?)?);
                let left = self.eval_i64(left)?;
                let right = self.eval_i64(right)?;
                Ok(match op {
                    BinaryOp::Mul => left.wrapping_mul(right),
                    BinaryOp::Div => trunc_div_or_zero(left, right),
                    BinaryOp::Mod => trunc_mod_or_zero(left, right),
                    BinaryOp::Add => left.wrapping_add(right),
                    BinaryOp::Sub => left.wrapping_sub(right),
                    BinaryOp::Shl => const_shl_or_zero(left, right),
                    BinaryOp::Shr => const_shr_or_zero(left, right, left_signed),
                    BinaryOp::Lt => i64::from(left < right),
                    BinaryOp::Le => i64::from(left <= right),
                    BinaryOp::Gt => i64::from(left > right),
                    BinaryOp::Ge => i64::from(left >= right),
                    BinaryOp::Eq => i64::from(left == right),
                    BinaryOp::Ne => i64::from(left != right),
                    BinaryOp::BitAnd => left & right,
                    BinaryOp::BitXor => left ^ right,
                    BinaryOp::BitOr => left | right,
                    BinaryOp::And => i64::from(left != 0 && right != 0),
                    BinaryOp::Or => i64::from(left != 0 || right != 0),
                })
            }
            Expr::Cast { expr, ty } => {
                let value = self.eval_i64(expr)?;
                self.const_cast_value(value, ty)
            }
            Expr::AddressOfIndex { .. }
            | Expr::AddressOfField { .. }
            | Expr::AddressOfAccess(_)
            | Expr::AddressOf(_) => self.const_address_value(expr),
            Expr::Array(_)
            | Expr::Index { .. }
            | Expr::StructInit { .. }
            | Expr::Deref(_)
            | Expr::In(_)
            | Expr::Call { .. }
            | Expr::String(_) => Err(Diagnostic::new(format!(
                "expression `{expr:?}` is not a compile-time integer"
            ))),
        }
    }

    fn const_value(&self, name: &str) -> Result<i64, Diagnostic> {
        self.constants
            .get(name)
            .copied()
            .or_else(|| self.embed_property_value(name))
            .ok_or_else(|| Diagnostic::new(format!("unknown constant `{name}`")))
    }

    fn const_cast_value(&self, value: i64, ty: &Type) -> Result<i64, Diagnostic> {
        self.wrap_value_for_type(value, ty)
    }

    fn wrap_value_for_type(&self, value: i64, ty: &Type) -> Result<i64, Diagnostic> {
        let resolved = self.resolved_type(ty)?;
        if type_is_bool(&resolved) {
            return Ok(i64::from(value != 0));
        }
        let width = self.type_width(&resolved)?;
        let bits = u32::from(width.bytes()) * 8;
        let mask = (1_i128 << bits) - 1;
        let unsigned = (value as i128) & mask;
        if type_is_signed(&resolved) {
            let sign_bit = 1_i128 << (bits - 1);
            if unsigned & sign_bit != 0 {
                Ok((unsigned - (1_i128 << bits)) as i64)
            } else {
                Ok(unsigned as i64)
            }
        } else {
            Ok(unsigned as i64)
        }
    }

    fn embed_property_value(&self, name: &str) -> Option<i64> {
        let (embed_name, property) = name.rsplit_once('.')?;
        let embed = self.embeds.get(embed_name).or_else(|| {
            module_alias_original_name(embed_name).and_then(|original| self.embeds.get(original))
        })?;
        match property {
            "ptr" => Some(embed.variable.addr as i64),
            "len" => Some(embed.variable.len.unwrap_or(0) as i64),
            "end" => Some((embed.variable.addr + embed.variable.len.unwrap_or(0)) as i64),
            _ => None,
        }
    }

    fn readonly_embed_name_for_addr(&self, addr: u32) -> Option<&str> {
        let addr = u64::from(addr);
        for (name, embed) in &self.embeds {
            let Some(len) = embed.variable.len else {
                continue;
            };
            let start = u64::from(embed.variable.addr);
            let end = start + u64::from(len);
            if addr >= start && addr < end {
                return Some(name.as_str());
            }
        }
        None
    }

    fn readonly_string_literal_for_addr(&self, addr: u32) -> Option<&str> {
        self.readonly_string_literal_for_range(u64::from(addr), u64::from(addr) + 1)
    }

    fn readonly_string_literal_for_range(&self, start: u64, end: u64) -> Option<&str> {
        for (value, variable) in &self.string_literals {
            let Some(len) = variable.len else {
                continue;
            };
            if len == 0 {
                continue;
            }
            let literal_start = u64::from(variable.addr);
            let literal_end = literal_start + u64::from(len);
            if start < literal_end && end > literal_start {
                return Some(value.as_str());
            }
        }
        None
    }

    fn const_expr_uses_wrapping_arithmetic(&self, expr: &Expr) -> bool {
        match expr {
            Expr::Binary { .. } => true,
            Expr::Unary {
                op: UnaryOp::BitNot,
                ..
            } => true,
            Expr::Unary { expr, .. } | Expr::Cast { expr, .. } | Expr::Deref(expr) => {
                self.const_expr_uses_wrapping_arithmetic(expr)
            }
            Expr::Array(values) => values
                .iter()
                .any(|value| self.const_expr_uses_wrapping_arithmetic(value)),
            Expr::Index { index, .. } | Expr::AddressOfIndex { index, .. } => {
                self.const_expr_uses_wrapping_arithmetic(index)
            }
            Expr::Access(path) | Expr::AddressOfAccess(path) => {
                path.segments.iter().any(|segment| match segment {
                    AccessSegment::Field(_) => false,
                    AccessSegment::Index(index) => self.const_expr_uses_wrapping_arithmetic(index),
                })
            }
            Expr::StructInit { fields, .. } => fields
                .iter()
                .any(|(_, value)| self.const_expr_uses_wrapping_arithmetic(value)),
            Expr::Call { args, .. } => args
                .iter()
                .any(|arg| self.const_expr_uses_wrapping_arithmetic(arg)),
            Expr::Int(_)
            | Expr::TypedInt(_, _)
            | Expr::Char(_)
            | Expr::Bool(_)
            | Expr::String(_)
            | Expr::Ident(_)
            | Expr::Field { .. }
            | Expr::AddressOf(_)
            | Expr::AddressOfField { .. }
            | Expr::In(_) => false,
        }
    }

    fn validate_const_expr_arithmetic_compatibility(&self, expr: &Expr) -> Result<(), Diagnostic> {
        match expr {
            Expr::Binary { left, op, right } => {
                self.validate_const_expr_arithmetic_compatibility(left)?;
                self.validate_const_expr_arithmetic_compatibility(right)?;
                if matches!(op, BinaryOp::And | BinaryOp::Or) {
                    self.ensure_const_expr_is_bool(left, "logical operand")?;
                    self.ensure_const_expr_is_bool(right, "logical operand")?;
                } else if is_comparison(*op) {
                    self.validate_const_comparison_operand_types(left, *op, right)?;
                } else if matches!(op, BinaryOp::Shl | BinaryOp::Shr) {
                    self.validate_const_shift_operand_types(left, right)?;
                } else {
                    self.validate_const_binary_operand_types(left, right)?;
                }
            }
            Expr::Unary { expr, op } => {
                self.validate_const_expr_arithmetic_compatibility(expr)?;
                match op {
                    UnaryOp::Not => self.ensure_const_expr_is_bool(expr, "logical operand")?,
                    UnaryOp::Neg | UnaryOp::BitNot => {
                        let ty = self.resolved_type(&self.const_expr_type(expr)?)?;
                        validate_integer_unary_operand_type(&ty)?;
                    }
                }
            }
            Expr::Cast { expr, ty } => {
                self.validate_const_expr_arithmetic_compatibility(expr)?;
                self.validate_const_cast(expr, ty)?;
            }
            Expr::Deref(expr) => {
                self.validate_const_expr_arithmetic_compatibility(expr)?;
            }
            Expr::Array(values) => {
                for value in values {
                    self.validate_const_expr_arithmetic_compatibility(value)?;
                }
            }
            Expr::Index { index, .. } | Expr::AddressOfIndex { index, .. } => {
                self.validate_const_expr_arithmetic_compatibility(index)?;
            }
            Expr::Access(path) | Expr::AddressOfAccess(path) => {
                for segment in &path.segments {
                    if let AccessSegment::Index(index) = segment {
                        self.validate_const_expr_arithmetic_compatibility(index)?;
                    }
                }
            }
            Expr::StructInit { fields, .. } => {
                for (_, value) in fields {
                    self.validate_const_expr_arithmetic_compatibility(value)?;
                }
            }
            Expr::Call { args, .. } => {
                for arg in args {
                    self.validate_const_expr_arithmetic_compatibility(arg)?;
                }
            }
            Expr::Int(_)
            | Expr::TypedInt(_, _)
            | Expr::Char(_)
            | Expr::Bool(_)
            | Expr::String(_)
            | Expr::Ident(_)
            | Expr::AddressOf(_)
            | Expr::AddressOfField { .. }
            | Expr::Field { .. }
            | Expr::In(_) => {}
        }
        Ok(())
    }

    fn ensure_const_expr_is_bool(&self, expr: &Expr, context: &str) -> Result<(), Diagnostic> {
        let ty = self.resolved_type(&self.const_expr_type(expr)?)?;
        if type_is_bool(&ty) {
            Ok(())
        } else {
            Err(Diagnostic::new(format!("{context} must be bool")))
        }
    }

    fn validate_const_binary_operand_types(
        &self,
        left: &Expr,
        right: &Expr,
    ) -> Result<(), Diagnostic> {
        let left_is_literal = expr_is_untyped_literal(left);
        let right_is_literal = expr_is_untyped_literal(right);
        if left_is_literal && right_is_literal {
            return Ok(());
        }

        let left_type = self.resolved_type(&self.const_expr_type(left)?)?;
        let right_type = self.resolved_type(&self.const_expr_type(right)?)?;
        if type_is_bool(&left_type) || type_is_bool(&right_type) {
            return Err(Diagnostic::new("type mismatch"));
        }
        if left_is_literal {
            let value = self.eval_i64(left)?;
            return self.validate_value_for_type(value, &right_type);
        }
        if right_is_literal {
            let value = self.eval_i64(right)?;
            return self.validate_value_for_type(value, &left_type);
        }
        if type_is_signed(&left_type) != type_is_signed(&right_type) {
            return Err(Diagnostic::new("signed/unsigned mix without cast"));
        }
        if self.type_width(&left_type)? != self.type_width(&right_type)? {
            return Err(Diagnostic::new(
                "arithmetic operands must have same width without cast",
            ));
        }
        Ok(())
    }

    fn validate_const_shift_operand_types(
        &self,
        left: &Expr,
        right: &Expr,
    ) -> Result<(), Diagnostic> {
        let left_type = self.resolved_type(&self.const_expr_type(left)?)?;
        validate_shift_operand_type(&left_type)?;

        let right_type = self.resolved_type(&self.const_expr_type(right)?)?;
        validate_shift_count_integer_type(&right_type)?;
        let value = self.eval_i64(right)?;
        if !(0..=u8::MAX as i64).contains(&value) {
            return Err(Diagnostic::new(format!(
                "shift count {value} is outside supported range 0..=255"
            )));
        }
        Ok(())
    }

    fn validate_const_comparison_operand_types(
        &self,
        left: &Expr,
        op: BinaryOp,
        right: &Expr,
    ) -> Result<(), Diagnostic> {
        let left_type = self.resolved_type(&self.const_expr_type(left)?)?;
        let right_type = self.resolved_type(&self.const_expr_type(right)?)?;
        if !type_is_bool(&left_type)
            && !type_is_bool(&right_type)
            && !matches!(left_type, Type::Ptr(_))
            && !matches!(right_type, Type::Ptr(_))
        {
            if expr_is_untyped_literal(left) && expr_is_untyped_literal(right) {
                return Ok(());
            }
            if expr_is_untyped_literal(left) {
                let value = self.eval_i64(left)?;
                return self.validate_value_for_type(value, &right_type);
            }
            if expr_is_untyped_literal(right) {
                let value = self.eval_i64(right)?;
                return self.validate_value_for_type(value, &left_type);
            }
        }
        validate_comparison_types(&left_type, op, &right_type, || {
            if expr_is_untyped_literal(left) || expr_is_untyped_literal(right) {
                None
            } else {
                Some((
                    self.type_width(&left_type).ok()?,
                    self.type_width(&right_type).ok()?,
                ))
            }
        })
    }

    fn const_expr_type(&self, expr: &Expr) -> Result<Type, Diagnostic> {
        match expr {
            Expr::Ident(name) => self.const_name_type(name),
            Expr::Field { base, field } => self.const_name_type(&format!("{base}.{field}")),
            Expr::Access(path) => {
                let name = const_access_name(path)?;
                self.const_name_type(&name)
            }
            Expr::Int(value) => Ok(int_value_type(*value)),
            Expr::TypedInt(_, ty) => Ok(ty.clone()),
            Expr::Char(_) => Ok(Type::Named("u8".to_owned())),
            Expr::Bool(_) => Ok(Type::Named("bool".to_owned())),
            Expr::Unary { expr, op } => match op {
                UnaryOp::Not => {
                    self.ensure_const_expr_is_bool(expr, "logical operand")?;
                    Ok(Type::Named("bool".to_owned()))
                }
                UnaryOp::Neg | UnaryOp::BitNot => self.const_expr_type(expr),
            },
            Expr::Binary { left, op, right } => {
                if is_comparison(*op) || matches!(op, BinaryOp::And | BinaryOp::Or) {
                    Ok(Type::Named("bool".to_owned()))
                } else if self.type_width(&self.const_expr_type(left)?)?
                    >= self.type_width(&self.const_expr_type(right)?)?
                {
                    self.const_expr_type(left)
                } else {
                    self.const_expr_type(right)
                }
            }
            Expr::Cast { ty, .. } => Ok(ty.clone()),
            Expr::String(_) => Ok(Type::Ptr(Box::new(Type::Named("u8".to_owned())))),
            Expr::AddressOfIndex { .. }
            | Expr::AddressOfField { .. }
            | Expr::AddressOfAccess(_)
            | Expr::AddressOf(_) => self.const_address_type(expr),
            Expr::Array(_)
            | Expr::Index { .. }
            | Expr::StructInit { .. }
            | Expr::Deref(_)
            | Expr::In(_)
            | Expr::Call { .. } => Err(Diagnostic::new(
                "expression is not supported in a constant declaration",
            )),
        }
    }

    fn const_name_type(&self, name: &str) -> Result<Type, Diagnostic> {
        if let Some(ty) = self.constant_types.get(name) {
            Ok(ty.clone())
        } else if let Some(value) = self.constants.get(name).copied() {
            Ok(int_value_type(value))
        } else if self.embed_property_value(name).is_some() {
            Ok(Type::Named("u24".to_owned()))
        } else {
            Err(Diagnostic::new(format!("unknown constant `{name}`")))
        }
    }

    fn validate_const_cast(&self, expr: &Expr, target: &Type) -> Result<(), Diagnostic> {
        let source_type = self.resolved_type(&self.const_expr_type(expr)?)?;
        let target_type = self.resolved_type(target)?;
        match (&source_type, &target_type) {
            (Type::Ptr(_), Type::Ptr(_)) => Ok(()),
            (Type::Ptr(_), Type::Named(name)) if name == "bool" => Ok(()),
            (Type::Ptr(_), Type::Named(name)) if is_raw_address_type(name) => Ok(()),
            (Type::Ptr(_), Type::Named(_)) => Err(Diagnostic::new(
                "pointer-to-integer casts produce u24 or ptr24",
            )),
            (Type::Named(name), Type::Ptr(_)) if is_raw_address_type(name) => Ok(()),
            (Type::Named(_), Type::Ptr(_)) => Err(Diagnostic::new(
                "integer-to-pointer casts require u24 or ptr24",
            )),
            _ => Ok(()),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct LoopLabels {
    continue_label: String,
    break_label: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct LocalConstant {
    value: i64,
    ty: Type,
}

struct Emitter {
    symbols: Symbols,
    out: String,
    label_counter: usize,
    scopes: Vec<HashMap<String, Variable>>,
    scope_types: Vec<HashMap<String, Type>>,
    local_constants: Vec<HashMap<String, LocalConstant>>,
    readonly_pointer_aliases: Vec<HashMap<String, u32>>,
    string_literals: HashMap<String, Variable>,
    loop_stack: Vec<LoopLabels>,
    return_type_stack: Vec<Option<Type>>,
    return_value_stack: Vec<bool>,
    function_name_stack: Vec<String>,
    function_frame_stack: Vec<bool>,
    function_interrupt_stack: Vec<bool>,
    function_naked_stack: Vec<bool>,
    function_storage_stack: Vec<Vec<Variable>>,
    assigned_names_stack: Vec<HashSet<String>>,
    recursive_call_edges: HashSet<(String, String)>,
    inline_expansion_stack: Vec<String>,
    debug_comments: bool,
    stack_top: Address24,
    eliminate_dead_code: bool,
}

impl Emitter {
    fn new(
        symbols: Symbols,
        options: AssemblyOptions,
        recursive_call_edges: HashSet<(String, String)>,
    ) -> Self {
        let string_literals = symbols.string_literals.clone();
        Self {
            symbols,
            out: String::new(),
            label_counter: 0,
            scopes: Vec::new(),
            scope_types: Vec::new(),
            local_constants: Vec::new(),
            readonly_pointer_aliases: Vec::new(),
            string_literals,
            loop_stack: Vec::new(),
            return_type_stack: Vec::new(),
            return_value_stack: Vec::new(),
            function_name_stack: Vec::new(),
            function_frame_stack: Vec::new(),
            function_interrupt_stack: Vec::new(),
            function_naked_stack: Vec::new(),
            function_storage_stack: Vec::new(),
            assigned_names_stack: Vec::new(),
            recursive_call_edges,
            inline_expansion_stack: Vec::new(),
            debug_comments: options.debug_comments,
            stack_top: options.stack_top,
            eliminate_dead_code: true,
        }
    }

    fn disable_dead_code_elimination(&mut self) {
        self.eliminate_dead_code = false;
    }

    fn emit_prelude(&mut self) {
        self.line("; generated by ezrac");
        self.line("; target: eZ80 ADL mode");
        self.line("section .text");
        self.line("__ezra_start:");
        self.line("    di");
        self.line(&format!("    ld sp, {:06X}h", self.stack_top.get()));
    }

    fn alloc_var<S: Into<u32>>(&mut self, size: S) -> Variable {
        let variable = self.symbols.alloc_var(size);
        self.track_function_storage(variable);
        variable
    }

    fn alloc_storage(&mut self, ty: &Type) -> Result<Variable, Diagnostic> {
        let variable = self.symbols.alloc_storage(ty)?;
        self.track_function_storage(variable);
        Ok(variable)
    }

    fn track_function_storage(&mut self, variable: Variable) {
        if let Some(storage) = self.function_storage_stack.last_mut() {
            storage.push(variable);
        }
    }

    fn emit_required_sections(&mut self) {
        for section in [".header", ".rodata", ".data", ".bss", ".assets", ".scratch"] {
            self.line(&format!("section {section}"));
        }
    }

    fn emit_start_tail(&mut self) {
        self.line("    call _main");
        self.line("__ezra_exit:");
        self.line("    jp __ezra_exit");
        self.emit_runtime_helpers();
        self.line("");
    }

    fn emit_runtime_helpers(&mut self) {
        self.line("__ezra_pass:");
        self.emit_out(0x0D, 0);
        self.emit_out(0x0E, 1);
        self.line("    ret");
        self.line("__ezra_fail:");
        self.emit_out_a(0x0D);
        self.emit_out(0x0E, 1);
        self.line("    ret");
        self.line("__ezra_memcpy:");
        self.line("    push de");
        self.line("    push hl");
        self.line("    push bc");
        self.line("    pop hl");
        self.line("    ld de, 000000h");
        self.line("    or a");
        self.line("    sbc hl, de");
        self.line("    pop hl");
        self.line("    pop de");
        self.line("    ret z");
        self.line("    ex de, hl");
        self.line("    ldir");
        self.line("    ret");
        self.line("__ezra_memset:");
        self.line("    push hl");
        self.line("    push bc");
        self.line("    pop hl");
        self.line("    ld de, 000000h");
        self.line("    or a");
        self.line("    sbc hl, de");
        self.line("    pop hl");
        self.line("    ret z");
        self.line("    ld (hl), a");
        self.line("    dec bc");
        self.line("    push hl");
        self.line("    push bc");
        self.line("    pop hl");
        self.line("    ld de, 000000h");
        self.line("    or a");
        self.line("    sbc hl, de");
        self.line("    pop hl");
        self.line("    ret z");
        self.line("    push hl");
        self.line("    inc hl");
        self.line("    ex de, hl");
        self.line("    pop hl");
        self.line("    ldir");
        self.line("    ret");
        self.line("__ezra_mul_u8:");
        self.line("    ld b, a");
        self.line("    mlt bc");
        self.line("    ld a, c");
        self.line("    ret");
        self.line("__ezra_mul_u16:");
        self.line("    ld d, h");
        self.line("    ld e, l");
        self.line("    ld h, c");
        self.line("    mlt hl");
        self.line("    push hl");
        self.line("    ld h, d");
        self.line("    ld l, c");
        self.line("    mlt hl");
        self.line("    ld a, l");
        self.line("    ld h, e");
        self.line("    ld l, b");
        self.line("    mlt hl");
        self.line("    add a, l");
        self.line("    pop de");
        self.line("    add a, d");
        self.line("    ld hl, 000000h");
        self.line("    ld h, a");
        self.line("    ld l, e");
        self.line("    ret");
        self.line("__ezra_mul_u24:");
        self.line("    ex de, hl");
        self.line("    ld hl, 000000h");
        self.line(".L_mul_u24_loop:");
        self.line("    push hl");
        self.line("    ld hl, 000000h");
        self.line("    or a");
        self.line("    sbc hl, bc");
        self.line("    jp z, .L_mul_u24_done");
        self.line("    pop hl");
        self.line("    add hl, de");
        self.line("    dec bc");
        self.line("    jp .L_mul_u24_loop");
        self.line(".L_mul_u24_done:");
        self.line("    pop hl");
        self.line("    ret");
        self.line("__ezra_mul_i24:");
        self.line("    jp __ezra_mul_u24");
        self.line("__ezra_div_u8:");
        self.line("    ld d, a");
        self.line("    xor a");
        self.line("    ld b, a");
        self.line("    ld a, c");
        self.line("    or a");
        self.line("    jp z, .L_div_u8_zero");
        self.line(".L_div_u8_loop:");
        self.line("    ld a, d");
        self.line("    cp c");
        self.line("    jp c, .L_div_u8_done");
        self.line("    sub c");
        self.line("    ld d, a");
        self.line("    inc b");
        self.line("    jp .L_div_u8_loop");
        self.line(".L_div_u8_zero:");
        self.line("    xor a");
        self.line("    ret");
        self.line(".L_div_u8_done:");
        self.line("    ld a, b");
        self.line("    ret");
        self.line("__ezra_div_u16:");
        self.line("    ld a, b");
        self.line("    or c");
        self.line("    jp z, .L_div_u16_zero");
        self.line("    ex de, hl");
        self.line("    ld hl, 000000h");
        self.line(".L_div_u16_loop:");
        self.line("    push hl");
        self.line("    push de");
        self.line("    pop hl");
        self.line("    or a");
        self.line("    sbc hl, bc");
        self.line("    jp c, .L_div_u16_done");
        self.line("    ex de, hl");
        self.line("    pop hl");
        self.line("    inc hl");
        self.line("    jp .L_div_u16_loop");
        self.line(".L_div_u16_zero:");
        self.line("    ld hl, 000000h");
        self.line("    ret");
        self.line(".L_div_u16_done:");
        self.line("    pop hl");
        self.line("    ret");
        self.line("__ezra_div_u24:");
        self.line("    push hl");
        self.line("    ld hl, 000000h");
        self.line("    or a");
        self.line("    sbc hl, bc");
        self.line("    jp z, .L_div_u24_zero");
        self.line("    pop de");
        self.line("    ld hl, 000000h");
        self.line(".L_div_u24_loop:");
        self.line("    push hl");
        self.line("    push de");
        self.line("    pop hl");
        self.line("    or a");
        self.line("    sbc hl, bc");
        self.line("    jp c, .L_div_u24_done");
        self.line("    ex de, hl");
        self.line("    pop hl");
        self.line("    inc hl");
        self.line("    jp .L_div_u24_loop");
        self.line(".L_div_u24_zero:");
        self.line("    pop hl");
        self.line("    ld hl, 000000h");
        self.line("    ret");
        self.line(".L_div_u24_done:");
        self.line("    pop hl");
        self.line("    ret");
        self.line("__ezra_mod_u8:");
        self.line("    ld d, a");
        self.line("    ld a, c");
        self.line("    or a");
        self.line("    jp z, .L_mod_u8_zero");
        self.line(".L_mod_u8_loop:");
        self.line("    ld a, d");
        self.line("    cp c");
        self.line("    jp c, .L_mod_u8_done");
        self.line("    sub c");
        self.line("    ld d, a");
        self.line("    jp .L_mod_u8_loop");
        self.line(".L_mod_u8_zero:");
        self.line("    xor a");
        self.line("    ret");
        self.line(".L_mod_u8_done:");
        self.line("    ld a, d");
        self.line("    ret");
        self.line("__ezra_mod_u16:");
        self.line("    ld a, b");
        self.line("    or c");
        self.line("    jp z, .L_mod_u16_zero");
        self.line("    ex de, hl");
        self.line(".L_mod_u16_loop:");
        self.line("    push de");
        self.line("    pop hl");
        self.line("    or a");
        self.line("    sbc hl, bc");
        self.line("    jp c, .L_mod_u16_done");
        self.line("    ex de, hl");
        self.line("    jp .L_mod_u16_loop");
        self.line(".L_mod_u16_zero:");
        self.line("    ld hl, 000000h");
        self.line("    ret");
        self.line(".L_mod_u16_done:");
        self.line("    push de");
        self.line("    pop hl");
        self.line("    ret");
        self.line("__ezra_mod_u24:");
        self.line("    push hl");
        self.line("    ld hl, 000000h");
        self.line("    or a");
        self.line("    sbc hl, bc");
        self.line("    jp z, .L_mod_u24_zero");
        self.line("    pop de");
        self.line(".L_mod_u24_loop:");
        self.line("    push de");
        self.line("    pop hl");
        self.line("    or a");
        self.line("    sbc hl, bc");
        self.line("    jp c, .L_mod_u24_done");
        self.line("    ex de, hl");
        self.line("    jp .L_mod_u24_loop");
        self.line(".L_mod_u24_zero:");
        self.line("    pop hl");
        self.line("    ld hl, 000000h");
        self.line("    ret");
        self.line(".L_mod_u24_done:");
        self.line("    push de");
        self.line("    pop hl");
        self.line("    ret");
        self.emit_signed_i24_div_mod_helper("__ezra_div_i24", BinaryOp::Div);
        self.emit_signed_i24_div_mod_helper("__ezra_mod_i24", BinaryOp::Mod);
    }

    fn emit_signed_i24_div_mod_helper(&mut self, label: &str, op: BinaryOp) {
        let dividend = self.alloc_var(ValueWidth::U24.bytes());
        let divisor = self.alloc_var(ValueWidth::U24.bytes());
        let quotient = self.alloc_var(ValueWidth::U24.bytes());
        let quotient_negative = self.alloc_var(ValueWidth::U8.bytes());
        let remainder_negative = self.alloc_var(ValueWidth::U8.bytes());
        let loop_label = self.next_label("sdiv_i24_loop");
        let zero_label = self.next_label("sdiv_i24_zero");
        let done_label = self.next_label("sdiv_i24_done");
        let quotient_positive_label = self.next_label("sdiv_i24_q_positive");
        let remainder_positive_label = self.next_label("sdiv_i24_r_positive");
        let not_overflow_label = self.next_label("sdiv_i24_not_overflow");

        self.line(&format!("{label}:"));
        self.emit_store_width(dividend);
        self.line("    push bc");
        self.line("    pop hl");
        self.emit_store_width(divisor);
        self.emit_zero_variable(quotient);
        self.emit_zero_variable(quotient_negative);
        self.emit_zero_variable(remainder_negative);
        self.emit_jump_if_memory_zero(divisor, &zero_label);
        self.emit_jump_if_memory_not_equals(
            dividend,
            signed_min_bytes(ValueWidth::U24),
            &not_overflow_label,
        );
        self.emit_jump_if_memory_not_equals(
            divisor,
            signed_negative_one_bytes(ValueWidth::U24),
            &not_overflow_label,
        );
        match op {
            BinaryOp::Div => self.emit_load_width(dividend),
            BinaryOp::Mod => {
                self.emit_zero_variable(dividend);
                self.emit_load_width(dividend);
            }
            _ => unreachable!("not a division op"),
        }
        self.line("    ret");
        self.line(&format!("{not_overflow_label}:"));

        self.emit_abs_signed_variable(dividend, Some(quotient_negative), Some(remainder_negative));
        self.emit_abs_signed_variable(divisor, Some(quotient_negative), None);

        self.line(&format!("{loop_label}:"));
        self.emit_load_width(dividend);
        self.line("    push hl");
        self.emit_load_width(divisor);
        self.line("    ex de, hl");
        self.line("    pop hl");
        self.line("    or a");
        self.line("    sbc hl, de");
        self.line(&format!("    jp c, {done_label}"));
        self.emit_store_width(dividend);
        self.emit_increment_memory(quotient);
        self.line(&format!("    jp {loop_label}"));

        self.line(&format!("{zero_label}:"));
        self.emit_zero_variable(dividend);
        self.emit_zero_variable(quotient);
        self.line(&format!("{done_label}:"));
        self.emit_load_a(quotient_negative);
        self.line("    or a");
        self.line(&format!("    jp z, {quotient_positive_label}"));
        self.emit_negate_memory(quotient);
        self.line(&format!("{quotient_positive_label}:"));
        self.emit_load_a(remainder_negative);
        self.line("    or a");
        self.line(&format!("    jp z, {remainder_positive_label}"));
        self.emit_negate_memory(dividend);
        self.line(&format!("{remainder_positive_label}:"));

        match op {
            BinaryOp::Div => self.emit_load_width(quotient),
            BinaryOp::Mod => self.emit_load_width(dividend),
            _ => unreachable!("not a division op"),
        }
        self.line("    ret");
    }

    fn emit_global_initializers(&mut self, program: &Program) -> Result<(), Diagnostic> {
        for declaration in &program.declarations {
            let Declaration::Global(decl) = declaration else {
                continue;
            };
            let variable = self
                .symbols
                .globals
                .get(&decl.name)
                .copied()
                .expect("global allocation exists");
            self.emit_storage_initializer(variable, &decl.ty, &decl.value)?;
        }
        Ok(())
    }

    fn emit_embed_initializers(&mut self) {
        let embeds = self.symbols.embeds.values().cloned().collect::<Vec<_>>();
        for embed in embeds {
            for (offset, byte) in embed.bytes.into_iter().enumerate() {
                self.line(&format!("    ld a, {byte:02X}h"));
                self.emit_store_a(scalar_var(
                    embed.variable.addr + offset as u32,
                    u32::from(ValueWidth::U8.bytes()),
                ));
            }
        }
    }

    fn emit_string_literal_initializers(&mut self) {
        let mut literals = self
            .string_literals
            .iter()
            .map(|(value, variable)| (variable.addr, value.clone(), *variable))
            .collect::<Vec<_>>();
        literals.sort_by_key(|(addr, _, _)| *addr);
        for (_, value, variable) in literals {
            self.emit_string_literal_initializer(&value, variable);
        }
    }

    fn emit_string_literal_initializer(&mut self, value: &str, variable: Variable) {
        for (offset, byte) in value.bytes().chain(std::iter::once(0)).enumerate() {
            self.line(&format!("    ld a, {byte:02X}h"));
            self.emit_store_a(scalar_var(
                variable.addr + offset as u32,
                u32::from(ValueWidth::U8.bytes()),
            ));
        }
    }

    fn emit_function(&mut self, function: &Function) -> Result<(), Diagnostic> {
        validate_function_attrs(function)?;
        let naked = has_attr(function, "naked");
        let interrupt = has_attr(function, "interrupt");
        if naked {
            for stmt in &function.body {
                let Stmt::Asm {
                    inputs, outputs, ..
                } = stmt
                else {
                    return Err(Diagnostic::new(format!(
                        "naked function `{}` may contain only asm blocks",
                        function.name
                    )));
                };
                if !inputs.is_empty() || !outputs.is_empty() {
                    return Err(Diagnostic::new(format!(
                        "naked function `{}` asm blocks cannot use operands",
                        function.name
                    )));
                }
            }
        }
        if !naked
            && function.return_type.is_some()
            && !block_guarantees_value_return(&function.body, &self.symbols)
        {
            return Err(Diagnostic::new(format!(
                "missing return value in function `{}`",
                function.name
            )));
        }
        self.line(&format!("{}:", function_label(&function.name)));
        self.scopes.push(HashMap::new());
        self.scope_types.push(HashMap::new());
        self.local_constants.push(HashMap::new());
        self.readonly_pointer_aliases.push(HashMap::new());
        self.assigned_names_stack
            .push(assigned_names_in_block(&function.body));
        if let Some(return_type) = &function.return_type {
            self.symbols.type_width(return_type)?;
        }
        let uses_stack_frame = self
            .symbols
            .functions
            .get(&function.name)
            .is_some_and(|sig| sig.stack_arg_bytes > 0);
        self.return_type_stack.push(function.return_type.clone());
        self.return_value_stack.push(function.return_type.is_some());
        self.function_name_stack.push(function.name.clone());
        self.function_frame_stack.push(uses_stack_frame);
        self.function_interrupt_stack.push(interrupt);
        self.function_naked_stack.push(naked);
        self.function_storage_stack.push(Vec::new());
        if !naked {
            if interrupt {
                if !function.params.is_empty() {
                    return Err(Diagnostic::new(format!(
                        "interrupt function `{}` cannot take parameters",
                        function.name
                    )));
                }
                if function.return_type.is_some() {
                    return Err(Diagnostic::new(format!(
                        "interrupt function `{}` cannot return a value",
                        function.name
                    )));
                }
                self.emit_interrupt_prologue();
            }
            if uses_stack_frame {
                self.emit_frame_prologue();
            }
            self.bind_params(function)?;
        }
        self.emit_block(&function.body)?;
        self.function_naked_stack.pop();
        self.function_interrupt_stack.pop();
        self.function_frame_stack.pop();
        self.function_name_stack.pop();
        self.function_storage_stack.pop();
        self.return_value_stack.pop();
        self.return_type_stack.pop();
        self.assigned_names_stack.pop();
        self.readonly_pointer_aliases.pop();
        self.local_constants.pop();
        self.scope_types.pop();
        self.scopes.pop();
        if naked {
            return Ok(());
        }
        if interrupt {
            self.emit_interrupt_epilogue();
            return Ok(());
        }
        if function.name == "main" {
            self.line("    jp __ezra_exit");
        } else {
            if uses_stack_frame {
                self.emit_frame_epilogue();
            }
            self.line("    ret");
        }
        Ok(())
    }

    fn emit_frame_prologue(&mut self) {
        self.line("    push ix");
        self.line("    ld ix, 000000h");
        self.line("    add ix, sp");
    }

    fn emit_frame_epilogue(&mut self) {
        self.line("    pop ix");
    }

    fn emit_interrupt_prologue(&mut self) {
        self.line("    push af");
        self.line("    push bc");
        self.line("    push de");
        self.line("    push hl");
        self.line("    push ix");
        self.line("    push iy");
    }

    fn emit_interrupt_epilogue(&mut self) {
        self.line("    pop iy");
        self.line("    pop ix");
        self.line("    pop hl");
        self.line("    pop de");
        self.line("    pop bc");
        self.line("    pop af");
        self.line("    reti");
    }

    fn bind_params(&mut self, function: &Function) -> Result<(), Diagnostic> {
        let sig = self
            .symbols
            .functions
            .get(&function.name)
            .cloned()
            .ok_or_else(|| Diagnostic::new(format!("unknown function `{}`", function.name)))?;

        for (index, param) in function.params.iter().enumerate() {
            if self.name_in_current_function(&param.name) {
                return Err(Diagnostic::new(format!(
                    "parameter `{}` shadows an existing name",
                    param.name
                )));
            }
            let width = self.symbols.type_width(&param.ty)?;
            let variable = self.alloc_var(width.bytes());
            self.current_scope_mut()
                .insert(param.name.clone(), variable);
            self.current_scope_types_mut()
                .insert(param.name.clone(), param.ty.clone());
            if sig.uses_arg_slots {
                let slot = sig.arg_slots[index];
                self.emit_load_width(slot);
                self.emit_store_width(variable);
                continue;
            }
            if let Some(offset) = sig.stack_arg_offsets[index] {
                self.emit_load_ix_offset_width_into(offset, variable)?;
                continue;
            }
            match width {
                ValueWidth::U8 => {
                    match index {
                        0 => {}
                        1 => self.line("    ld a, b"),
                        2 => self.line("    ld a, c"),
                        _ => unreachable!("param count checked"),
                    }
                    self.emit_store_a(variable);
                }
                ValueWidth::U16 | ValueWidth::U24 => {
                    match index {
                        0 => {}
                        1 => self.line("    ex de, hl"),
                        2 => {
                            self.line("    push bc");
                            self.line("    pop hl");
                        }
                        _ => unreachable!("param count checked"),
                    }
                    self.emit_store_width(variable);
                }
            }
        }
        Ok(())
    }

    fn emit_block(&mut self, body: &[Stmt]) -> Result<(), Diagnostic> {
        for stmt in body {
            self.emit_stmt(stmt)?;
            if self.eliminate_dead_code && self.stmt_terminates_current_block(stmt) {
                break;
            }
        }
        Ok(())
    }

    fn block_terminates_current_block(&self, body: &[Stmt]) -> bool {
        body.iter()
            .any(|stmt| self.stmt_terminates_current_block(stmt))
    }

    fn stmt_terminates_current_block(&self, stmt: &Stmt) -> bool {
        match stmt {
            Stmt::Return(_) | Stmt::Break | Stmt::Continue => true,
            Stmt::If {
                condition,
                then_body,
                else_body,
            } => {
                if let Ok(value) = self.eval_i64_with_local_constants(condition) {
                    if value == 0 {
                        return self.block_terminates_current_block(else_body);
                    }
                    return self.block_terminates_current_block(then_body);
                }
                !else_body.is_empty()
                    && self.block_terminates_current_block(then_body)
                    && self.block_terminates_current_block(else_body)
            }
            Stmt::Loop { body } => {
                !block_can_break_current_loop(body) && self.block_terminates_current_block(body)
            }
            Stmt::While { condition, body } => {
                self.eval_i64_with_local_constants(condition)
                    .is_ok_and(|value| value != 0)
                    && !block_can_break_current_loop(body)
                    && self.block_terminates_current_block(body)
            }
            _ => false,
        }
    }

    fn emit_stmt(&mut self, stmt: &Stmt) -> Result<(), Diagnostic> {
        if self.debug_comments {
            self.line(&format!("    ; source: {}", stmt_summary(stmt)));
        }
        match stmt {
            Stmt::Let { name, ty, value } => {
                if self.name_in_current_function(name) {
                    return Err(Diagnostic::new(format!(
                        "local `{name}` shadows an existing name"
                    )));
                }
                let variable = self.alloc_storage(ty)?;
                self.current_scope_mut().insert(name.clone(), variable);
                self.current_scope_types_mut()
                    .insert(name.clone(), ty.clone());
                self.emit_storage_initializer(variable, ty, value)?;
                self.record_local_constant(name, ty, value);
                self.record_readonly_pointer_alias(name, value);
            }
            Stmt::Assign { target, op, value } => {
                self.emit_assignment(target, *op, value)?;
            }
            Stmt::Out { port, value } => {
                let port = self.port(port)?;
                self.validate_expr_assignable_to_type(value, &Type::Named("u8".to_owned()))?;
                self.emit_expr_to_a(value)?;
                self.emit_out_a(port);
            }
            Stmt::Expr(Expr::Call { path, args }) => self.emit_call(path, args)?,
            Stmt::Expr(expr) => {
                let width = self.expr_width(expr)?;
                self.emit_expr_to_width(expr, width)?;
            }
            Stmt::If {
                condition,
                then_body,
                else_body,
            } => {
                self.ensure_expr_is_bool(condition, "if condition")?;
                if self.eliminate_dead_code {
                    if let Ok(value) = self.eval_i64_with_local_constants(condition) {
                        if value == 0 {
                            self.emit_block(else_body)?;
                        } else {
                            self.emit_block(then_body)?;
                        }
                        return Ok(());
                    }
                }
                let else_label = self.next_label("else");
                let end_label = self.next_label("endif");
                self.emit_expr_to_a(condition)?;
                self.line("    or a");
                self.line(&format!("    jp z, {else_label}"));
                self.emit_block(then_body)?;
                self.line(&format!("    jp {end_label}"));
                self.line(&format!("{else_label}:"));
                self.emit_block(else_body)?;
                self.line(&format!("{end_label}:"));
            }
            Stmt::While { condition, body } => {
                self.ensure_expr_is_bool(condition, "while condition")?;
                let mut condition_is_always_true = false;
                if self.eliminate_dead_code {
                    if let Ok(value) = self.eval_i64_with_local_constants(condition) {
                        if value == 0 {
                            return Ok(());
                        }
                        condition_is_always_true = true;
                    }
                }
                let start_label = self.next_label("while");
                let end_label = self.next_label("endwhile");
                self.loop_stack.push(LoopLabels {
                    continue_label: start_label.clone(),
                    break_label: end_label.clone(),
                });
                self.line(&format!("{start_label}:"));
                if !condition_is_always_true {
                    self.emit_expr_to_a(condition)?;
                    self.line("    or a");
                    self.line(&format!("    jp z, {end_label}"));
                }
                self.emit_block(body)?;
                self.line(&format!("    jp {start_label}"));
                self.line(&format!("{end_label}:"));
                self.loop_stack.pop();
            }
            Stmt::Loop { body } => {
                let start_label = self.next_label("loop");
                let end_label = self.next_label("endloop");
                self.loop_stack.push(LoopLabels {
                    continue_label: start_label.clone(),
                    break_label: end_label.clone(),
                });
                self.line(&format!("{start_label}:"));
                self.emit_block(body)?;
                self.line(&format!("    jp {start_label}"));
                self.line(&format!("{end_label}:"));
                self.loop_stack.pop();
            }
            Stmt::Break => {
                let Some(labels) = self.loop_stack.last() else {
                    return Err(Diagnostic::new("`break` outside loop"));
                };
                self.line(&format!("    jp {}", labels.break_label));
            }
            Stmt::Continue => {
                let Some(labels) = self.loop_stack.last() else {
                    return Err(Diagnostic::new("`continue` outside loop"));
                };
                self.line(&format!("    jp {}", labels.continue_label));
            }
            Stmt::Return(None) => {
                if self.current_function_requires_return_value() {
                    return Err(Diagnostic::new(format!(
                        "missing return value in function `{}`",
                        self.current_function_name()
                    )));
                }
                if self.current_function_uses_frame() {
                    self.emit_frame_epilogue();
                }
                if self.current_function_is_interrupt() {
                    self.emit_interrupt_epilogue();
                } else {
                    self.line("    ret");
                }
            }
            Stmt::Return(Some(expr)) => {
                if !self.current_function_requires_return_value() {
                    return Err(Diagnostic::new(format!(
                        "void function `{}` cannot return a value",
                        self.current_function_name()
                    )));
                }
                let return_type = self.current_return_type().clone();
                self.emit_expr_to_type(expr, &return_type)?;
                if self.current_function_uses_frame() {
                    self.emit_frame_epilogue();
                }
                if self.current_function_is_interrupt() {
                    self.emit_interrupt_epilogue();
                } else {
                    self.line("    ret");
                }
            }
            Stmt::Asm {
                volatile,
                inputs,
                outputs,
                clobbers,
                lines,
            } => self.emit_inline_asm(*volatile, inputs, outputs, clobbers, lines)?,
        }
        Ok(())
    }

    fn emit_inline_asm(
        &mut self,
        volatile: bool,
        inputs: &[crate::ast::AsmInput],
        outputs: &[crate::ast::AsmOutput],
        clobbers: &[String],
        lines: &[String],
    ) -> Result<(), Diagnostic> {
        let mut operands = HashMap::new();

        if volatile {
            self.line("    ; asm volatile");
        } else {
            self.line("    ; asm");
        }
        for input in inputs {
            if operands.contains_key(&input.name) {
                return Err(Diagnostic::new(format!(
                    "duplicate inline asm operand `{}`",
                    input.name
                )));
            }
            self.validate_inline_asm_input_type(input)?;
            let binding = self.inline_asm_input_binding(input)?;
            self.line(&format!(
                "    ; in {}: {} as {}",
                input.name,
                type_display(&input.ty),
                input.class
            ));
            operands.insert(input.name.clone(), binding);
        }
        for output in outputs {
            if operands.contains_key(&output.name) {
                return Err(Diagnostic::new(format!(
                    "duplicate inline asm operand `{}`",
                    output.name
                )));
            }
            self.validate_inline_asm_output_type(output)?;
            let binding = self.inline_asm_output_binding(output)?;
            self.line(&format!(
                "    ; out {}: {} as {}",
                output.name,
                type_display(&output.ty),
                output.class
            ));
            operands.insert(output.name.clone(), binding);
        }
        if !clobbers.is_empty() {
            self.line(&format!("    ; clobber {}", clobbers.join(", ")));
        }
        validate_inline_asm_clobbers(clobbers, lines, self.current_function_is_naked())?;

        for input in inputs {
            self.emit_inline_asm_input_load(input)?;
        }
        for line in lines {
            self.line(&format!(
                "    {}",
                substitute_inline_asm_operands(line, &operands)?
            ));
        }
        for output in outputs {
            self.emit_inline_asm_output_store(output)?;
            self.invalidate_local_constant(&output.name);
            self.invalidate_readonly_pointer_alias(&output.name);
        }
        Ok(())
    }

    fn validate_inline_asm_input_type(
        &self,
        input: &crate::ast::AsmInput,
    ) -> Result<(), Diagnostic> {
        let Some(bound) = self.named_value_type(&input.name) else {
            return Err(Diagnostic::new(format!("unknown value `{}`", input.name)));
        };
        let declared = self.symbols.resolved_type(&input.ty)?;
        let bound = self.symbols.resolved_type(bound)?;
        if declared != bound {
            return Err(Diagnostic::new(format!(
                "inline asm input `{}` declared type `{}` does not match bound type `{}`",
                input.name,
                type_display(&declared),
                type_display(&bound)
            )));
        }
        Ok(())
    }

    fn validate_inline_asm_output_type(
        &self,
        output: &crate::ast::AsmOutput,
    ) -> Result<(), Diagnostic> {
        let Some(bound) = self.variable_type(&output.name) else {
            return Err(Diagnostic::new(format!(
                "unknown variable `{}`",
                output.name
            )));
        };
        let declared = self.symbols.resolved_type(&output.ty)?;
        let bound = self.symbols.resolved_type(bound)?;
        if declared != bound {
            return Err(Diagnostic::new(format!(
                "inline asm output `{}` declared type `{}` does not match bound type `{}`",
                output.name,
                type_display(&declared),
                type_display(&bound)
            )));
        }
        Ok(())
    }

    fn inline_asm_input_binding(&self, input: &crate::ast::AsmInput) -> Result<String, Diagnostic> {
        match input.class.as_str() {
            "reg8" => Ok("a".to_owned()),
            "reg16" | "reg24" => Ok("hl".to_owned()),
            "mem" => {
                let variable = self.variable(&input.name)?;
                Ok(format!("({:06X}h)", variable.addr))
            }
            "imm" => {
                let width = self.symbols.type_width(&input.ty)?;
                let value = self.eval_i64_with_local_constants(&Expr::Ident(input.name.clone()))?;
                Ok(format_immediate(value, width))
            }
            _ => Err(Diagnostic::new(format!(
                "unsupported inline asm operand class `{}`",
                input.class
            ))),
        }
    }

    fn inline_asm_output_binding(
        &self,
        output: &crate::ast::AsmOutput,
    ) -> Result<String, Diagnostic> {
        match output.class.as_str() {
            "reg8" => Ok("a".to_owned()),
            "reg16" | "reg24" => Ok("hl".to_owned()),
            "mem" => {
                let variable = self.variable(&output.name)?;
                Ok(format!("({:06X}h)", variable.addr))
            }
            "imm" => Err(Diagnostic::new(format!(
                "inline asm output `{}` cannot use imm class",
                output.name
            ))),
            _ => Err(Diagnostic::new(format!(
                "unsupported inline asm operand class `{}`",
                output.class
            ))),
        }
    }

    fn emit_inline_asm_input_load(
        &mut self,
        input: &crate::ast::AsmInput,
    ) -> Result<(), Diagnostic> {
        match input.class.as_str() {
            "reg8" => {
                if let Some(variable) = self.variable_opt(&input.name) {
                    self.emit_load_a(variable);
                } else {
                    let value = self.u8(&Expr::Ident(input.name.clone()))?;
                    self.line(&format!("    ld a, {value:02X}h"));
                }
            }
            "reg16" | "reg24" => {
                let width = self.symbols.type_width(&input.ty)?;
                if let Some(variable) = self.variable_opt(&input.name) {
                    self.emit_load_width(variable);
                } else {
                    let value = self.symbols.eval_i64(&Expr::Ident(input.name.clone()))?;
                    self.line(&format!("    ld hl, {}", format_immediate(value, width)));
                }
            }
            "mem" | "imm" => {}
            _ => {
                return Err(Diagnostic::new(format!(
                    "unsupported inline asm operand class `{}`",
                    input.class
                )));
            }
        }
        Ok(())
    }

    fn emit_inline_asm_output_store(
        &mut self,
        output: &crate::ast::AsmOutput,
    ) -> Result<(), Diagnostic> {
        match output.class.as_str() {
            "reg8" | "reg16" | "reg24" => {
                let variable = self.variable(&output.name)?;
                self.emit_store_width(variable);
            }
            "mem" => {}
            "imm" => {
                return Err(Diagnostic::new(format!(
                    "inline asm output `{}` cannot use imm class",
                    output.name
                )));
            }
            _ => {
                return Err(Diagnostic::new(format!(
                    "unsupported inline asm operand class `{}`",
                    output.class
                )));
            }
        }
        Ok(())
    }

    fn emit_assignment_value(
        &mut self,
        variable: Variable,
        op: AssignOp,
        value: &Expr,
        signed: bool,
    ) -> Result<(), Diagnostic> {
        if variable.size == 2 {
            match op {
                AssignOp::Set => self.emit_expr_to_hl(value, variable.width()?)?,
                AssignOp::Add => self.emit_wide_assignment_op(variable, BinaryOp::Add, value)?,
                AssignOp::Sub => self.emit_wide_assignment_op(variable, BinaryOp::Sub, value)?,
                AssignOp::BitAnd => {
                    self.emit_wide_assignment_op(variable, BinaryOp::BitAnd, value)?
                }
                AssignOp::BitOr => {
                    self.emit_wide_assignment_op(variable, BinaryOp::BitOr, value)?
                }
                AssignOp::BitXor => {
                    self.emit_wide_assignment_op(variable, BinaryOp::BitXor, value)?
                }
                AssignOp::Shl => {
                    self.emit_wide_assignment_shift(variable, BinaryOp::Shl, value, signed)?
                }
                AssignOp::Shr => {
                    self.emit_wide_assignment_shift(variable, BinaryOp::Shr, value, signed)?
                }
            }
            return Ok(());
        }
        if variable.size == 3 {
            match op {
                AssignOp::Set => self.emit_expr_to_hl(value, ValueWidth::U24)?,
                AssignOp::Add => self.emit_wide_assignment_op(variable, BinaryOp::Add, value)?,
                AssignOp::Sub => self.emit_wide_assignment_op(variable, BinaryOp::Sub, value)?,
                AssignOp::BitAnd => {
                    self.emit_wide_assignment_op(variable, BinaryOp::BitAnd, value)?
                }
                AssignOp::BitOr => {
                    self.emit_wide_assignment_op(variable, BinaryOp::BitOr, value)?
                }
                AssignOp::BitXor => {
                    self.emit_wide_assignment_op(variable, BinaryOp::BitXor, value)?
                }
                AssignOp::Shl => {
                    self.emit_wide_assignment_shift(variable, BinaryOp::Shl, value, signed)?
                }
                AssignOp::Shr => {
                    self.emit_wide_assignment_shift(variable, BinaryOp::Shr, value, signed)?
                }
            }
            return Ok(());
        }

        match op {
            AssignOp::Set => self.emit_expr_to_a(value)?,
            AssignOp::Add => {
                self.emit_load_a(variable);
                self.line("    ld b, a");
                self.emit_expr_to_a(value)?;
                self.line("    add a, b");
            }
            AssignOp::Sub => {
                self.emit_load_a(variable);
                self.line("    ld b, a");
                self.emit_expr_to_a(value)?;
                self.line("    ld c, a");
                self.line("    ld a, b");
                self.line("    sub c");
            }
            AssignOp::BitAnd => {
                self.emit_load_a(variable);
                self.line("    ld b, a");
                self.emit_expr_to_a(value)?;
                self.line("    and b");
            }
            AssignOp::BitOr => {
                self.emit_load_a(variable);
                self.line("    ld b, a");
                self.emit_expr_to_a(value)?;
                self.line("    or b");
            }
            AssignOp::BitXor => {
                self.emit_load_a(variable);
                self.line("    ld b, a");
                self.emit_expr_to_a(value)?;
                self.line("    xor b");
            }
            AssignOp::Shl => {
                self.ensure_shift_count_compatible(value)?;
                self.emit_load_a(variable);
                self.emit_shift_a_by_expr(BinaryOp::Shl, value, signed)?;
            }
            AssignOp::Shr => {
                self.ensure_shift_count_compatible(value)?;
                self.emit_load_a(variable);
                self.emit_shift_a_by_expr(BinaryOp::Shr, value, signed)?;
            }
        }
        Ok(())
    }

    fn emit_assignment(
        &mut self,
        target: &Place,
        op: AssignOp,
        value: &Expr,
    ) -> Result<(), Diagnostic> {
        match target {
            Place::Ident(name) => {
                self.invalidate_local_constant(name);
                self.invalidate_readonly_pointer_alias(name);
                let variable = self.variable(name)?;
                let ty = self.variable_type(name).cloned();
                if op == AssignOp::Set {
                    if let Some(ty) = ty.as_ref() {
                        self.emit_storage_initializer(variable, ty, value)?;
                        self.record_local_constant(name, ty, value);
                        self.record_readonly_pointer_alias(name, value);
                        return Ok(());
                    }
                }
                let signed = self
                    .variable_type(name)
                    .map(|ty| self.type_is_signed(ty))
                    .transpose()?
                    .unwrap_or(false);
                self.emit_assignment_value(variable, op, value, signed)?;
                self.emit_store_width(variable);
            }
            Place::Index { name, index } => {
                self.emit_index_assignment(name, index, op, value)?;
            }
            Place::Field { base, field } => {
                let variable = self.field_variable(base, field)?;
                if op == AssignOp::Set {
                    let ty = self.field_type(base, field)?;
                    self.validate_expr_assignable_to_type(value, &ty)?;
                    self.emit_storage_initializer(variable, &ty, value)?;
                    return Ok(());
                }
                variable.width()?;
                let signed = self.type_is_signed(&self.field_type(base, field)?)?;
                self.emit_assignment_value(variable, op, value, signed)?;
                self.emit_store_width(variable);
            }
            Place::Access(path) => {
                self.emit_access_assignment(path, op, value)?;
            }
            Place::Deref(ptr) => {
                self.emit_deref_assignment(ptr, op, value)?;
            }
        }
        Ok(())
    }

    fn emit_array_initializer(
        &mut self,
        variable: Variable,
        ty: &Type,
        value: &Expr,
    ) -> Result<(), Diagnostic> {
        let Expr::Array(values) = value else {
            return Err(Diagnostic::new(
                "array initializer must be an array literal",
            ));
        };
        let element_size = variable
            .element_size
            .ok_or_else(|| Diagnostic::new("scalar variable cannot use array initializer"))?;
        let len = variable
            .len
            .ok_or_else(|| Diagnostic::new("array variable missing length"))?;
        let Type::Array {
            element: element_ty,
            ..
        } = self.symbols.resolved_type(ty)?
        else {
            return Err(Diagnostic::new("array initializer requires an array type"));
        };
        if values.len() as u32 > len {
            return Err(Diagnostic::new(format!(
                "array initializer has {} values but array length is {len}",
                values.len()
            )));
        }
        for index in 0..len {
            let element_addr = variable.addr + index * u32::from(element_size);
            let element = self.symbols.storage_at(element_addr, &element_ty)?;
            if let Some(value) = values.get(index as usize) {
                self.validate_expr_assignable_to_type(value, &element_ty)?;
                match self.symbols.resolved_type(&element_ty)? {
                    Type::Array { .. } => {
                        self.emit_array_initializer(element, &element_ty, value)?
                    }
                    Type::Named(name) if self.symbols.structs.contains_key(&name) => {
                        self.emit_struct_initializer(element, &element_ty, value)?
                    }
                    _ => {
                        self.emit_expr_to_width(value, element.width()?)?;
                        self.emit_store_width(element);
                    }
                }
            } else {
                self.emit_zero_storage(element);
            }
        }
        Ok(())
    }

    fn emit_struct_initializer(
        &mut self,
        variable: Variable,
        ty: &Type,
        value: &Expr,
    ) -> Result<(), Diagnostic> {
        let struct_name = self.struct_type_name(ty)?;
        let Expr::StructInit { ty, fields } = value else {
            return Err(Diagnostic::new(format!(
                "struct `{struct_name}` initializer must use `{struct_name} {{ ... }}`"
            )));
        };
        if ty != &struct_name {
            return Err(Diagnostic::new(format!(
                "initializer type `{ty}` does not match `{struct_name}`"
            )));
        }

        let layout = self
            .symbols
            .structs
            .get(&struct_name)
            .cloned()
            .ok_or_else(|| Diagnostic::new(format!("unknown struct `{struct_name}`")))?;
        let mut initialized = HashMap::new();
        for (field_name, field_value) in fields {
            let Some(field) = layout.fields.get(field_name) else {
                return Err(Diagnostic::new(format!(
                    "struct `{struct_name}` has no field `{field_name}`"
                )));
            };
            if initialized.insert(field_name.clone(), ()).is_some() {
                return Err(Diagnostic::new(format!(
                    "duplicate initializer for field `{field_name}`"
                )));
            }
            let field_var = self
                .symbols
                .storage_at(variable.addr + field.offset, &field.ty)?;
            self.validate_expr_assignable_to_type(field_value, &field.ty)?;
            self.emit_storage_initializer(field_var, &field.ty, field_value)?;
        }

        for (field_name, field) in &layout.fields {
            if initialized.contains_key(field_name) {
                continue;
            }
            let field_var = self
                .symbols
                .storage_at(variable.addr + field.offset, &field.ty)?;
            self.emit_zero_storage(field_var);
        }
        Ok(())
    }

    fn emit_storage_initializer(
        &mut self,
        variable: Variable,
        ty: &Type,
        value: &Expr,
    ) -> Result<(), Diagnostic> {
        self.validate_expr_arithmetic_compatibility(value)?;
        self.validate_expr_assignable_to_type(value, ty)?;
        if let Expr::Deref(ptr) = value {
            return self.emit_copy_pointed_storage_into(ptr, variable);
        }
        if let Some(source) = self.expr_storage_variable(value)? {
            return self.emit_copy_storage_into(source, variable);
        }
        match self.symbols.resolved_type(ty)? {
            Type::Array { .. } => self.emit_array_initializer(variable, ty, value),
            Type::Named(name) if self.symbols.structs.contains_key(&name) => {
                self.emit_struct_initializer(variable, ty, value)
            }
            _ => {
                self.emit_expr_to_width(value, variable.width()?)?;
                self.emit_store_width(variable);
                Ok(())
            }
        }
    }

    fn emit_wide_assignment_op(
        &mut self,
        variable: Variable,
        op: BinaryOp,
        value: &Expr,
    ) -> Result<(), Diagnostic> {
        self.emit_load_width(variable);
        self.line("    push hl");
        self.emit_expr_to_hl(value, variable.width()?)?;
        self.line("    pop bc");
        self.emit_wide_op_with_left_in_bc(op, variable.width()?)?;
        Ok(())
    }

    fn emit_wide_assignment_shift(
        &mut self,
        variable: Variable,
        op: BinaryOp,
        value: &Expr,
        signed: bool,
    ) -> Result<(), Diagnostic> {
        self.ensure_shift_count_compatible(value)?;
        let temp = self.alloc_var(variable.width()?.bytes());
        self.emit_load_width(variable);
        self.emit_store_width(temp);
        self.emit_shift_memory_by_expr(temp, op, value, signed)?;
        self.emit_load_width(temp);
        Ok(())
    }

    fn emit_call(&mut self, path: &[String], args: &[Expr]) -> Result<(), Diagnostic> {
        match path_text(path).as_str() {
            "test.pass" | "ezra.test.pass" => {
                self.line("    call __ezra_pass");
            }
            "test.fail" | "ezra.test.fail" => {
                let expr = args.first().cloned().unwrap_or(Expr::Int(1));
                self.validate_expr_assignable_to_type(&expr, &Type::Named("u8".to_owned()))?;
                self.emit_expr_to_a(&expr)?;
                self.emit_test_fail_call();
            }
            "test.assert_eq_u8" | "ezra.test.assert_eq_u8" => {
                if args.len() != 3 {
                    return Err(Diagnostic::new(
                        "test.assert_eq_u8 requires three arguments",
                    ));
                }
                self.validate_expr_has_test_width(&args[0], ValueWidth::U8, true)?;
                self.validate_expr_has_test_width(&args[1], ValueWidth::U8, true)?;
                self.validate_expr_assignable_to_type(&args[2], &Type::Named("u8".to_owned()))?;
                let ok = self.next_label("assert_ok");
                self.emit_expr_to_a(&args[0])?;
                self.line("    ld b, a");
                self.emit_expr_to_a(&args[1])?;
                self.line("    ld c, a");
                self.line("    ld a, b");
                self.line("    cp c");
                self.line(&format!("    jp z, {ok}"));
                self.emit_expr_to_a(&args[2])?;
                self.emit_test_fail_call();
                self.line(&format!("{ok}:"));
            }
            "test.assert_eq_u16" | "ezra.test.assert_eq_u16" => {
                if args.len() != 3 {
                    return Err(Diagnostic::new(
                        "test.assert_eq_u16 requires three arguments",
                    ));
                }
                self.validate_expr_has_test_width(&args[0], ValueWidth::U16, false)?;
                self.validate_expr_has_test_width(&args[1], ValueWidth::U16, false)?;
                self.validate_expr_assignable_to_type(&args[2], &Type::Named("u8".to_owned()))?;
                let ok = self.next_label("assert_ok");
                self.emit_expr_to_hl(&args[0], ValueWidth::U16)?;
                self.line("    push hl");
                self.emit_expr_to_hl(&args[1], ValueWidth::U16)?;
                self.line("    pop bc");
                self.line("    or a");
                self.line("    sbc hl, bc");
                self.line(&format!("    jp z, {ok}"));
                self.emit_expr_to_a(&args[2])?;
                self.emit_test_fail_call();
                self.line(&format!("{ok}:"));
            }
            "test.assert_eq_u24" | "ezra.test.assert_eq_u24" => {
                if args.len() != 3 {
                    return Err(Diagnostic::new(
                        "test.assert_eq_u24 requires three arguments",
                    ));
                }
                self.validate_expr_has_test_width(&args[0], ValueWidth::U24, false)?;
                self.validate_expr_has_test_width(&args[1], ValueWidth::U24, false)?;
                self.validate_expr_assignable_to_type(&args[2], &Type::Named("u8".to_owned()))?;
                let ok = self.next_label("assert_ok");
                self.emit_expr_to_hl(&args[0], ValueWidth::U24)?;
                self.line("    push hl");
                self.emit_expr_to_hl(&args[1], ValueWidth::U24)?;
                self.line("    pop bc");
                self.line("    or a");
                self.line("    sbc hl, bc");
                self.line(&format!("    jp z, {ok}"));
                self.emit_expr_to_a(&args[2])?;
                self.emit_test_fail_call();
                self.line(&format!("{ok}:"));
            }
            "debug.char" | "ezra.debug.char" => {
                let expr = args
                    .first()
                    .ok_or_else(|| Diagnostic::new("debug.char requires one argument"))?;
                self.validate_expr_assignable_to_type(expr, &Type::Named("u8".to_owned()))?;
                self.emit_expr_to_a(expr)?;
                self.emit_out_a(0x0C);
            }
            "debug.str" | "ezra.debug.str" => {
                self.emit_debug_str(args)?;
            }
            "debug.hex_u8" | "ezra.debug.hex_u8" => {
                self.emit_debug_hex(args, ValueWidth::U8)?;
            }
            "debug.hex_u16" | "ezra.debug.hex_u16" => {
                self.emit_debug_hex(args, ValueWidth::U16)?;
            }
            "debug.hex_u24" | "ezra.debug.hex_u24" => {
                self.emit_debug_hex(args, ValueWidth::U24)?;
            }
            "mem.poke8" | "ezra.mem.poke8" => {
                self.emit_mem_poke8(args)?;
            }
            "mem.memcpy" | "ezra.mem.memcpy" => {
                self.emit_memcpy(args)?;
            }
            "mem.memset" | "ezra.mem.memset" => {
                self.emit_memset(args)?;
            }
            path => self.emit_user_call(path, args)?,
        }
        Ok(())
    }

    fn emit_test_fail_call(&mut self) {
        self.line("    call __ezra_fail");
    }

    fn emit_debug_str(&mut self, args: &[Expr]) -> Result<(), Diagnostic> {
        if args.len() != 1 {
            return Err(Diagnostic::new("debug.str requires one argument"));
        }
        self.validate_expr_is_ptr_u8(&args[0])?;

        let cursor = self.alloc_var(ValueWidth::U24.bytes());
        let loop_label = self.next_label("debug_str");
        let done_label = self.next_label("debug_str_done");
        self.emit_expr_to_hl(&args[0], ValueWidth::U24)?;
        self.emit_store_hl(cursor);
        self.line(&format!("{loop_label}:"));
        self.emit_load_hl(cursor);
        self.line("    ld a, (hl)");
        self.line("    or a");
        self.line(&format!("    jp z, {done_label}"));
        self.emit_out_a(0x0C);
        self.emit_load_hl(cursor);
        self.line("    inc hl");
        self.emit_store_hl(cursor);
        self.line(&format!("    jp {loop_label}"));
        self.line(&format!("{done_label}:"));
        Ok(())
    }

    fn emit_debug_hex(&mut self, args: &[Expr], width: ValueWidth) -> Result<(), Diagnostic> {
        if args.len() != 1 {
            let suffix = match width {
                ValueWidth::U8 => "u8",
                ValueWidth::U16 => "u16",
                ValueWidth::U24 => "u24",
            };
            return Err(Diagnostic::new(format!(
                "debug.hex_{suffix} requires one argument"
            )));
        }
        self.validate_expr_assignable_to_type(&args[0], &width_unsigned_type(width))?;

        match width {
            ValueWidth::U8 => {
                self.emit_expr_to_a(&args[0])?;
                self.emit_debug_hex_byte_from_a();
            }
            ValueWidth::U16 | ValueWidth::U24 => {
                let value = self.alloc_var(width.bytes());
                self.emit_expr_to_hl(&args[0], width)?;
                self.emit_store_width(value);
                for offset in (0..width.bytes()).rev() {
                    self.line(&format!("    ld a, ({:06X}h)", value.addr + offset as u32));
                    self.emit_debug_hex_byte_from_a();
                }
            }
        }
        Ok(())
    }

    fn emit_debug_hex_byte_from_a(&mut self) {
        let byte = self.alloc_var(ValueWidth::U8.bytes());
        self.emit_store_a(byte);
        self.emit_load_a(byte);
        for _ in 0..4 {
            self.line("    srl a");
        }
        self.emit_debug_hex_nibble_from_a();
        self.emit_load_a(byte);
        self.line("    ld bc, 00000Fh");
        self.line("    and c");
        self.emit_debug_hex_nibble_from_a();
    }

    fn emit_debug_hex_nibble_from_a(&mut self) {
        let digit_label = self.next_label("debug_hex_digit");
        let end_label = self.next_label("debug_hex_end");
        self.line("    ld bc, 00000Ah");
        self.line("    cp c");
        self.line(&format!("    jp c, {digit_label}"));
        self.line("    ld bc, 000037h");
        self.line("    add a, c");
        self.line(&format!("    jp {end_label}"));
        self.line(&format!("{digit_label}:"));
        self.line("    ld bc, 000030h");
        self.line("    add a, c");
        self.line(&format!("{end_label}:"));
        self.emit_out_a(0x0C);
    }

    fn emit_user_call(&mut self, name: &str, args: &[Expr]) -> Result<(), Diagnostic> {
        let sig = self
            .symbols
            .functions
            .get(name)
            .cloned()
            .ok_or_else(|| Diagnostic::new(format!("unknown function `{name}`")))?;
        if sig.arity != args.len() {
            return Err(Diagnostic::new(format!(
                "function `{name}` expects {} arguments but got {}",
                sig.arity,
                args.len()
            )));
        }

        let mut temps = Vec::with_capacity(args.len());
        for (index, arg) in args.iter().enumerate() {
            let width = sig.params[index];
            let ty = &sig.param_types[index];
            let temp = self.alloc_var(width.bytes());
            self.emit_expr_to_type(arg, ty)?;
            self.emit_store_width(temp);
            temps.push(temp);
        }

        if let Some(function) = self.symbols.inline_functions.get(name).cloned() {
            if !self
                .inline_expansion_stack
                .iter()
                .any(|inline| inline == name)
            {
                self.inline_expansion_stack.push(name.to_owned());
                let inlined = (|| {
                    if self.emit_inline_return_call(&function, &temps)? {
                        return Ok(true);
                    }
                    self.emit_inline_void_call(&function, &temps)
                })();
                self.inline_expansion_stack.pop();
                if inlined? {
                    return Ok(());
                }
            }
        }
        let saved_variables = self.recursive_call_saved_variables(name);
        let return_temp = if saved_variables.is_empty() || sig.return_type.is_none() {
            None
        } else {
            Some(self.alloc_var(sig.return_width.bytes()))
        };

        if sig.uses_arg_slots {
            for (temp, slot) in temps.iter().copied().zip(sig.arg_slots.iter().copied()) {
                self.emit_load_width(temp);
                self.emit_store_width(slot);
            }
            self.emit_save_recursive_call_variables(&saved_variables);
            self.line(&format!("    call {}", function_label(name)));
            self.emit_store_recursive_call_return(return_temp);
            self.emit_restore_recursive_call_variables(&saved_variables);
            self.emit_load_recursive_call_return(return_temp);
            return Ok(());
        }

        self.emit_save_recursive_call_variables(&saved_variables);
        if sig.stack_arg_bytes > 0 {
            for temp in temps.iter().copied().skip(3).rev() {
                self.emit_push_stack_arg_variable(temp);
            }
        }
        if let Some(temp) = temps.get(2).copied() {
            if temp.size == 1 {
                self.emit_load_a(temp);
                self.line("    ld c, a");
            } else if sig.params.get(1).is_some_and(|width| width.bytes() != 1) {
                self.emit_load_width(temp);
                self.line("    push hl");
                self.line("    pop bc");
            } else {
                return Err(Diagnostic::new(
                    "current codegen supports a wide third argument only when the second argument is also wide",
                ));
            }
        }
        if let Some(temp) = temps.get(1).copied() {
            if temp.size == 1 {
                self.emit_load_a(temp);
                self.line("    ld b, a");
            } else {
                self.emit_load_width(temp);
                self.line("    ex de, hl");
            }
        }
        if let Some(temp) = temps.first().copied() {
            self.emit_load_width(temp);
        }
        self.line(&format!("    call {}", function_label(name)));
        if sig.stack_arg_bytes > 0 {
            self.emit_drop_stack_arg_bytes(sig.stack_arg_bytes);
        }
        self.emit_store_recursive_call_return(return_temp);
        self.emit_restore_recursive_call_variables(&saved_variables);
        self.emit_load_recursive_call_return(return_temp);
        Ok(())
    }

    fn recursive_call_saved_variables(&self, callee: &str) -> Vec<Variable> {
        let caller = self.current_function_name();
        if !self
            .recursive_call_edges
            .contains(&(caller.to_owned(), callee.to_owned()))
        {
            return Vec::new();
        }

        let Some(storage) = self.function_storage_stack.last() else {
            return Vec::new();
        };
        let mut variables = storage.clone();
        variables.sort_by_key(|variable| variable.addr);
        variables.dedup_by_key(|variable| variable.addr);
        variables
    }

    fn emit_save_recursive_call_variables(&mut self, variables: &[Variable]) {
        for variable in variables {
            for offset in 0..variable.size {
                self.line(&format!("    ld a, ({:06X}h)", variable.addr + offset));
                self.line("    dec sp");
                self.line("    ld hl, 000000h");
                self.line("    add hl, sp");
                self.line("    ld (hl), a");
            }
        }
    }

    fn emit_restore_recursive_call_variables(&mut self, variables: &[Variable]) {
        for variable in variables.iter().rev() {
            for offset in (0..variable.size).rev() {
                self.line("    ld hl, 000000h");
                self.line("    add hl, sp");
                self.line("    ld a, (hl)");
                self.line("    inc sp");
                self.line(&format!("    ld ({:06X}h), a", variable.addr + offset));
            }
        }
    }

    fn emit_store_recursive_call_return(&mut self, return_temp: Option<Variable>) {
        if let Some(return_temp) = return_temp {
            self.emit_store_width(return_temp);
        }
    }

    fn emit_load_recursive_call_return(&mut self, return_temp: Option<Variable>) {
        if let Some(return_temp) = return_temp {
            self.emit_load_width(return_temp);
        }
    }

    fn emit_inline_return_call(
        &mut self,
        function: &Function,
        temps: &[Variable],
    ) -> Result<bool, Diagnostic> {
        let Some((prefix, expr)) = inline_return_body(function) else {
            return Ok(false);
        };
        let Some(return_type) = &function.return_type else {
            return Ok(false);
        };

        self.scopes.push(HashMap::new());
        self.scope_types.push(HashMap::new());
        self.local_constants.push(HashMap::new());
        self.readonly_pointer_aliases.push(HashMap::new());
        self.assigned_names_stack
            .push(assigned_names_in_block(prefix));
        for (param, temp) in function.params.iter().zip(temps.iter().copied()) {
            self.current_scope_mut().insert(param.name.clone(), temp);
            self.current_scope_types_mut()
                .insert(param.name.clone(), param.ty.clone());
        }
        for stmt in prefix {
            self.emit_inline_prefix_stmt(stmt)?;
        }
        let result = self.emit_expr_to_type(&expr, return_type);
        self.assigned_names_stack.pop();
        self.readonly_pointer_aliases.pop();
        self.local_constants.pop();
        self.scope_types.pop();
        self.scopes.pop();
        result?;
        Ok(true)
    }

    fn emit_inline_void_call(
        &mut self,
        function: &Function,
        temps: &[Variable],
    ) -> Result<bool, Diagnostic> {
        let Some(body) = inline_void_body(function) else {
            return Ok(false);
        };

        self.scopes.push(HashMap::new());
        self.scope_types.push(HashMap::new());
        self.local_constants.push(HashMap::new());
        self.readonly_pointer_aliases.push(HashMap::new());
        self.assigned_names_stack
            .push(assigned_names_in_block(body));
        for (param, temp) in function.params.iter().zip(temps.iter().copied()) {
            self.current_scope_mut().insert(param.name.clone(), temp);
            self.current_scope_types_mut()
                .insert(param.name.clone(), param.ty.clone());
        }
        let result = self.emit_block(body);
        self.assigned_names_stack.pop();
        self.readonly_pointer_aliases.pop();
        self.local_constants.pop();
        self.scope_types.pop();
        self.scopes.pop();
        result?;
        Ok(true)
    }

    fn emit_inline_prefix_stmt(&mut self, stmt: &Stmt) -> Result<(), Diagnostic> {
        let Stmt::Let { name, ty, value } = stmt else {
            return self.emit_stmt(stmt);
        };
        if self.current_scope_types_mut().contains_key(name) {
            return Err(Diagnostic::new(format!(
                "local `{name}` shadows an existing name"
            )));
        }
        let variable = self.alloc_storage(ty)?;
        self.current_scope_mut().insert(name.clone(), variable);
        self.current_scope_types_mut()
            .insert(name.clone(), ty.clone());
        self.emit_storage_initializer(variable, ty, value)?;
        self.record_local_constant(name, ty, value);
        self.record_readonly_pointer_alias(name, value);
        Ok(())
    }

    fn emit_expr_to_width(&mut self, expr: &Expr, width: ValueWidth) -> Result<(), Diagnostic> {
        match width {
            ValueWidth::U8 => self.emit_expr_to_a(expr),
            ValueWidth::U16 | ValueWidth::U24 => self.emit_expr_to_hl(expr, width),
        }
    }

    fn emit_expr_to_type(&mut self, expr: &Expr, ty: &Type) -> Result<(), Diagnostic> {
        let width = self.symbols.type_width(ty)?;
        self.validate_expr_arithmetic_compatibility(expr)?;
        self.validate_expr_assignable_to_type(expr, ty)?;
        if let Expr::Cast { expr, ty } = expr {
            self.emit_cast_to_type(expr, ty)?;
            return Ok(());
        }
        if !self.is_pointer_arithmetic_expr(expr)? {
            if let Ok(value) = self.eval_i64_with_local_constants(expr) {
                let value = self.value_for_type(value, ty, width)?;
                match width {
                    ValueWidth::U8 => self.line(&format!("    ld a, {value:02X}h")),
                    ValueWidth::U16 | ValueWidth::U24 => {
                        self.line(&format!("    ld hl, {value:06X}h"))
                    }
                }
                return Ok(());
            }
        }
        self.emit_expr_to_width(expr, width)
    }

    fn is_pointer_arithmetic_expr(&self, expr: &Expr) -> Result<bool, Diagnostic> {
        if let Expr::Binary { left, op, right } = expr {
            return Ok(matches!(op, BinaryOp::Add | BinaryOp::Sub)
                && (self.pointer_pointee_size(left)?.is_some()
                    || self.pointer_pointee_size(right)?.is_some()));
        }
        Ok(false)
    }

    fn emit_cast_to_type(&mut self, expr: &Expr, ty: &Type) -> Result<(), Diagnostic> {
        self.validate_cast(expr, ty)?;
        let width = self.symbols.type_width(ty)?;
        let target_type = self.symbols.resolved_type(ty)?;
        let source_type = self.symbols.resolved_type(&self.expr_type(expr)?)?;
        if !self.is_pointer_arithmetic_expr(expr)? {
            if let Ok(value) = self.eval_i64_with_local_constants(expr) {
                let bits = u32::from(width.bytes()) * 8;
                let mask = (1_i128 << bits) - 1;
                let value = if type_is_bool(&target_type) {
                    u32::from(value != 0)
                } else {
                    ((value as i128) & mask) as u32
                };
                match width {
                    ValueWidth::U8 => self.line(&format!("    ld a, {value:02X}h")),
                    ValueWidth::U16 | ValueWidth::U24 => {
                        self.line(&format!("    ld hl, {value:06X}h"))
                    }
                }
                return Ok(());
            }
        }
        let source_width = self.expr_width(expr)?;
        match width {
            ValueWidth::U8 => {
                if type_is_bool(&target_type) {
                    if source_width == ValueWidth::U8 {
                        self.emit_expr_to_a(expr)?;
                        self.emit_normalize_a_to_bool();
                    } else {
                        self.emit_expr_to_hl(expr, source_width)?;
                        self.emit_normalize_hl_to_bool(source_width);
                    }
                } else if source_width == ValueWidth::U8 {
                    self.emit_expr_to_a(expr)?;
                } else {
                    self.emit_expr_to_hl(expr, source_width)?;
                    self.line("    ld a, l");
                }
            }
            ValueWidth::U16 => {
                if source_width == ValueWidth::U8 {
                    self.emit_expr_to_a(expr)?;
                    self.line("    ld hl, 000000h");
                    self.line("    ld l, a");
                    self.emit_sign_extend_widened_integer(&source_type, source_width, width)?;
                } else {
                    self.emit_expr_to_hl(expr, source_width)?;
                    self.zero_extend_hl16();
                }
            }
            ValueWidth::U24 => {
                if source_width == ValueWidth::U8 {
                    self.emit_expr_to_a(expr)?;
                    self.line("    ld hl, 000000h");
                    self.line("    ld l, a");
                    self.emit_sign_extend_widened_integer(&source_type, source_width, width)?;
                } else {
                    self.emit_expr_to_hl(expr, source_width)?;
                    self.emit_sign_extend_widened_integer(&source_type, source_width, width)?;
                }
            }
        }
        Ok(())
    }

    fn emit_normalize_a_to_bool(&mut self) {
        let true_label = self.next_label("cast_bool_true");
        let end_label = self.next_label("cast_bool_end");
        self.line("    or a");
        self.line(&format!("    jp nz, {true_label}"));
        self.line("    ld a, 00h");
        self.line(&format!("    jp {end_label}"));
        self.line(&format!("{true_label}:"));
        self.line("    ld a, 01h");
        self.line(&format!("{end_label}:"));
    }

    fn emit_normalize_hl_to_bool(&mut self, width: ValueWidth) {
        let value = self.alloc_var(width.bytes());
        self.emit_store_width(value);
        self.line("    xor a");
        for offset in 0..width.bytes() {
            self.line("    ld b, a");
            self.line(&format!("    ld a, ({:06X}h)", value.addr + offset as u32));
            self.line("    or b");
        }
        self.emit_normalize_a_to_bool();
    }

    fn emit_sign_extend_widened_integer(
        &mut self,
        source_type: &Type,
        source_width: ValueWidth,
        target_width: ValueWidth,
    ) -> Result<(), Diagnostic> {
        if source_width >= target_width || !type_is_signed(source_type) {
            return Ok(());
        }
        let (sign_register, extension) = match source_width {
            ValueWidth::U8 => ("l", 0xFFFF00),
            ValueWidth::U16 => ("h", 0xFF0000),
            ValueWidth::U24 => return Ok(()),
        };
        let done = self.next_label("cast_nonnegative");
        self.line(&format!("    ld a, {sign_register}"));
        self.line("    cp 80h");
        self.line(&format!("    jp c, {done}"));
        self.emit_add_hl_const(extension);
        self.line(&format!("{done}:"));
        Ok(())
    }

    fn validate_cast(&self, expr: &Expr, target: &Type) -> Result<(), Diagnostic> {
        let source_type = self.symbols.resolved_type(&self.expr_type(expr)?)?;
        let target_type = self.symbols.resolved_type(target)?;
        match (&source_type, &target_type) {
            (Type::Ptr(_), Type::Ptr(_)) => Ok(()),
            (Type::Ptr(_), Type::Named(name)) if name == "bool" => Ok(()),
            (Type::Ptr(_), Type::Named(name)) if is_raw_address_type(name) => Ok(()),
            (Type::Ptr(_), Type::Named(_)) => Err(Diagnostic::new(
                "pointer-to-integer casts produce u24 or ptr24",
            )),
            (Type::Named(name), Type::Ptr(_)) if is_raw_address_type(name) => Ok(()),
            (Type::Named(_), Type::Ptr(_)) => Err(Diagnostic::new(
                "integer-to-pointer casts require u24 or ptr24",
            )),
            _ => Ok(()),
        }
    }

    fn emit_expr_to_hl(&mut self, expr: &Expr, width: ValueWidth) -> Result<(), Diagnostic> {
        match expr {
            Expr::Ident(name) => {
                if let Some(value) = self.local_constant_value_for_width(name, width)? {
                    self.line(&format!("    ld hl, {value:06X}h"));
                } else if let Some(variable) = self.variable_opt(name) {
                    if variable.size == 1 {
                        self.emit_load_a(variable);
                        self.line("    ld hl, 000000h");
                        self.line("    ld l, a");
                    } else if variable.size == 2 {
                        self.emit_load_hl16(variable);
                    } else {
                        self.emit_load_hl(variable);
                    }
                } else {
                    let value = self.value_for_width(expr, width)?;
                    self.line(&format!("    ld hl, {:06X}h", value));
                }
            }
            Expr::AddressOfIndex { name, index } => {
                self.emit_array_element_address(name, index)?;
            }
            Expr::AddressOfField { base, field } => {
                self.emit_field_address(base, field)?;
            }
            Expr::AddressOfAccess(path) => {
                let path = self.canonical_access_path(path);
                self.emit_access_address(&path)?;
            }
            Expr::AddressOf(name) => {
                self.emit_variable_address(name)?;
            }
            Expr::String(value) => {
                self.emit_string_literal_address(value)?;
            }
            Expr::Deref(ptr) => {
                self.emit_deref_to_hl(ptr, width)?;
            }
            Expr::Field { base, field } => {
                if self.emit_dotted_constant_to_hl(base, field, width)? {
                    return Ok(());
                }
                if let Some(variable) = self.dotted_variable(base, field) {
                    self.emit_load_width(variable);
                    return Ok(());
                }
                let variable = self.field_variable(base, field)?;
                self.emit_load_width(variable);
            }
            Expr::Index { name, index } => {
                self.emit_load_indexed_element_to_hl(name, index)?;
            }
            Expr::Access(path) => {
                let path = self.canonical_access_path(path);
                if path.segments.is_empty()
                    && (self.named_value_type(&path.root).is_some()
                        || self.symbols.embed_property_value(&path.root).is_some())
                {
                    let value = self.value_for_width(&Expr::Ident(path.root.clone()), width)?;
                    self.line(&format!("    ld hl, {:06X}h", value));
                    return Ok(());
                }
                let ty = self.access_type(&path)?;
                let size = self.symbols.type_size(&ty)?;
                if size > 3 {
                    return Err(Diagnostic::new(format!(
                        "value `{}` is not scalar-sized",
                        access_path_summary(&path)
                    )));
                }
                if let Some(variable) = self.const_access_variable(&path)? {
                    self.emit_load_width(variable);
                    return Ok(());
                }
                self.emit_access_address(&path)?;
                let stored = self.alloc_var(size);
                self.emit_load_pointed_width_into(stored);
                self.emit_load_width(stored);
            }
            Expr::Int(_) | Expr::TypedInt(_, _) | Expr::Char(_) | Expr::Bool(_) => {
                let value = self.value_for_width(expr, width)?;
                self.line(&format!("    ld hl, {:06X}h", value));
            }
            Expr::Cast { expr, ty } => self.emit_cast_to_type(expr, ty)?,
            Expr::Unary { op, expr } => {
                if *op == UnaryOp::Not {
                    self.ensure_expr_is_bool(expr, "logical operand")?;
                }
                self.emit_unary_to_hl(*op, expr, width)?
            }
            Expr::Binary { left, op, right } => match op {
                BinaryOp::Add | BinaryOp::Sub
                    if self.emit_pointer_arithmetic(left, *op, right)? =>
                {
                    return Ok(());
                }
                BinaryOp::Add
                | BinaryOp::Sub
                | BinaryOp::Mul
                | BinaryOp::BitAnd
                | BinaryOp::BitOr
                | BinaryOp::BitXor => {
                    self.ensure_binary_arithmetic_operands_compatible(left, right)?;
                    if *op == BinaryOp::Mul {
                        self.emit_mul_to_width(
                            left,
                            right,
                            width,
                            self.binary_operands_are_signed(left, right)?,
                        )?;
                        return Ok(());
                    }
                    self.emit_expr_to_hl(left, width)?;
                    self.line("    push hl");
                    self.emit_expr_to_hl(right, width)?;
                    self.line("    pop bc");
                    self.emit_wide_op_with_left_in_bc(*op, width)?;
                }
                BinaryOp::Shl | BinaryOp::Shr => {
                    self.ensure_shift_operands_compatible(left, right)?;
                    let temp = self.alloc_var(width.bytes());
                    let signed = self.expr_is_signed(left)?;
                    self.emit_expr_to_hl(left, width)?;
                    self.emit_store_width(temp);
                    self.emit_shift_memory_by_expr(temp, *op, right, signed)?;
                    self.emit_load_width(temp);
                }
                BinaryOp::Div | BinaryOp::Mod => {
                    self.ensure_binary_arithmetic_operands_compatible(left, right)?;
                    if self.binary_operands_are_signed(left, right)? {
                        self.emit_signed_div_mod_to_width(left, right, *op, width)?;
                    } else {
                        self.emit_div_mod_to_width(left, right, *op, width)?;
                    }
                    return Ok(());
                }
                _ => {
                    return Err(Diagnostic::new(format!(
                        "binary operator `{op:?}` is not implemented in wide codegen yet"
                    )));
                }
            },
            Expr::Call { path, args } => {
                self.emit_user_call(&path_text(path), args)?;
            }
            Expr::Array(_) | Expr::StructInit { .. } | Expr::In(_) => {
                return Err(Diagnostic::new(format!(
                    "expression `{expr:?}` is not supported in u16 codegen"
                )));
            }
        }
        if width == ValueWidth::U16 {
            self.zero_extend_hl16();
        }
        Ok(())
    }

    fn zero_extend_hl16(&mut self) {
        let temp = self.alloc_var(ValueWidth::U16.bytes());
        self.emit_store_hl16(temp);
        self.emit_load_hl16(temp);
    }

    fn emit_pointer_arithmetic(
        &mut self,
        left: &Expr,
        op: BinaryOp,
        right: &Expr,
    ) -> Result<bool, Diagnostic> {
        let left_scale = self.pointer_pointee_size(left)?;
        let right_scale = self.pointer_pointee_size(right)?;
        match (op, left_scale, right_scale) {
            (BinaryOp::Add, Some(_), Some(_)) => Err(Diagnostic::new(
                "pointer arithmetic requires exactly one pointer operand",
            )),
            (BinaryOp::Add, Some(scale), None) => {
                self.ensure_pointer_offset_expr(right)?;
                self.emit_expr_to_hl(left, ValueWidth::U24)?;
                self.line("    push hl");
                self.emit_scaled_offset_to_hl(right, scale)?;
                self.line("    pop bc");
                self.line("    add hl, bc");
                Ok(true)
            }
            (BinaryOp::Add, None, Some(scale)) => {
                self.ensure_pointer_offset_expr(left)?;
                self.emit_expr_to_hl(right, ValueWidth::U24)?;
                self.line("    push hl");
                self.emit_scaled_offset_to_hl(left, scale)?;
                self.line("    pop bc");
                self.line("    add hl, bc");
                Ok(true)
            }
            (BinaryOp::Sub, Some(_), Some(_)) => Err(Diagnostic::new(
                "pointer subtraction between two pointers is not supported",
            )),
            (BinaryOp::Sub, Some(scale), None) => {
                self.ensure_pointer_offset_expr(right)?;
                self.emit_expr_to_hl(left, ValueWidth::U24)?;
                self.line("    push hl");
                self.emit_scaled_offset_to_hl(right, scale)?;
                self.line("    ex de, hl");
                self.line("    pop hl");
                self.line("    or a");
                self.line("    sbc hl, de");
                Ok(true)
            }
            (BinaryOp::Sub, None, Some(_)) => Err(Diagnostic::new(
                "cannot subtract a pointer from a non-pointer value",
            )),
            _ => Ok(false),
        }
    }

    fn ensure_pointer_offset_expr(&self, expr: &Expr) -> Result<(), Diagnostic> {
        let ty = self.symbols.resolved_type(&self.expr_type(expr)?)?;
        if type_is_bool(&ty) || matches!(ty, Type::Ptr(_)) {
            return Err(Diagnostic::new(
                "pointer arithmetic offset must be an integer",
            ));
        }
        self.symbols.type_width(&ty)?;
        Ok(())
    }

    fn emit_scaled_offset_to_hl(&mut self, expr: &Expr, scale: u32) -> Result<(), Diagnostic> {
        self.emit_expr_to_hl(expr, ValueWidth::U24)?;
        self.emit_sign_extend_pointer_offset(expr)?;
        match scale {
            1 => {}
            _ => {
                let base = self.alloc_var(ValueWidth::U24.bytes());
                self.emit_store_width(base);
                self.line("    ld hl, 000000h");
                for _ in 0..scale {
                    self.line("    push hl");
                    self.emit_load_width(base);
                    self.line("    pop bc");
                    self.line("    add hl, bc");
                }
            }
        }
        Ok(())
    }

    fn emit_sign_extend_pointer_offset(&mut self, expr: &Expr) -> Result<(), Diagnostic> {
        if !self.expr_is_signed(expr)? {
            return Ok(());
        }
        let width = self.symbols.type_width(&self.expr_type(expr)?)?;
        let (sign_register, extension) = match width {
            ValueWidth::U8 => ("l", 0xFFFF00),
            ValueWidth::U16 => ("h", 0xFF0000),
            ValueWidth::U24 => return Ok(()),
        };
        let done = self.next_label("offset_nonnegative");
        self.line(&format!("    ld a, {sign_register}"));
        self.line("    cp 80h");
        self.line(&format!("    jp c, {done}"));
        self.emit_add_hl_const(extension);
        self.line(&format!("{done}:"));
        Ok(())
    }

    fn emit_wide_op_with_left_in_bc(
        &mut self,
        op: BinaryOp,
        width: ValueWidth,
    ) -> Result<(), Diagnostic> {
        match op {
            BinaryOp::Add => {
                self.line("    add hl, bc");
            }
            BinaryOp::Sub => {
                self.line("    ex de, hl");
                self.line("    push bc");
                self.line("    pop hl");
                self.line("    or a");
                self.line("    sbc hl, de");
            }
            BinaryOp::BitAnd | BinaryOp::BitOr | BinaryOp::BitXor => {
                self.emit_wide_bitwise_from_bc_hl(op, width)?;
            }
            _ => unreachable!("unsupported wide op"),
        }
        Ok(())
    }

    fn emit_wide_bitwise_from_bc_hl(
        &mut self,
        op: BinaryOp,
        width: ValueWidth,
    ) -> Result<(), Diagnostic> {
        let right = self.alloc_var(width.bytes());
        self.emit_store_width(right);
        self.line("    push bc");
        self.line("    pop hl");
        let left = self.alloc_var(width.bytes());
        self.emit_store_width(left);
        let result = self.alloc_var(width.bytes());

        for offset in 0..width.bytes() {
            self.line(&format!("    ld a, ({:06X}h)", left.addr + offset as u32));
            self.line("    ld b, a");
            self.line(&format!("    ld a, ({:06X}h)", right.addr + offset as u32));
            match op {
                BinaryOp::BitAnd => self.line("    and b"),
                BinaryOp::BitOr => self.line("    or b"),
                BinaryOp::BitXor => self.line("    xor b"),
                _ => unreachable!("not a bitwise op"),
            }
            self.line(&format!("    ld ({:06X}h), a", result.addr + offset as u32));
        }

        self.emit_load_width(result);
        Ok(())
    }

    fn emit_expr_to_a(&mut self, expr: &Expr) -> Result<(), Diagnostic> {
        match expr {
            Expr::Ident(name) => {
                if let Some(value) = self.local_constant_value_for_width(name, ValueWidth::U8)? {
                    self.line(&format!("    ld a, {value:02X}h"));
                } else if let Some(variable) = self.variable_opt(name) {
                    self.emit_load_a(variable);
                } else {
                    let value = self.u8(expr)?;
                    self.line(&format!("    ld a, {:02X}h", value));
                }
            }
            Expr::In(port) => {
                let port = self.port(port)?;
                self.line(&format!("    in0 a, ({port:02X}h)"));
            }
            Expr::Index { name, index } => {
                self.emit_load_indexed_element_to_a(name, index)?;
            }
            Expr::Field { base, field } => {
                if self.emit_dotted_constant_to_a(base, field)? {
                    return Ok(());
                }
                if let Some(variable) = self.dotted_variable(base, field) {
                    if variable.size != 1 {
                        return Err(Diagnostic::new(format!(
                            "value `{base}.{field}` is not u8-sized"
                        )));
                    }
                    self.emit_load_a(variable);
                    return Ok(());
                }
                let variable = self.field_variable(base, field)?;
                if variable.size != 1 {
                    return Err(Diagnostic::new(format!(
                        "field `{base}.{field}` is not u8-sized"
                    )));
                }
                self.emit_load_a(variable);
            }
            Expr::Access(path) => {
                let path = self.canonical_access_path(path);
                if path.segments.is_empty()
                    && (self.named_value_type(&path.root).is_some()
                        || self.symbols.embed_property_value(&path.root).is_some())
                {
                    let value = self.u8(&Expr::Ident(path.root.clone()))?;
                    self.line(&format!("    ld a, {:02X}h", value));
                    return Ok(());
                }
                let ty = self.access_type(&path)?;
                let size = self.symbols.type_size(&ty)?;
                if size != 1 {
                    return Err(Diagnostic::new(format!(
                        "value `{}` is not u8-sized",
                        access_path_summary(&path)
                    )));
                }
                if let Some(variable) = self.const_access_variable(&path)? {
                    self.emit_load_a(variable);
                    return Ok(());
                }
                self.emit_access_address(&path)?;
                self.line("    ld a, (hl)");
            }
            Expr::Deref(ptr) => {
                self.emit_deref_to_a(ptr)?;
            }
            Expr::Int(_) | Expr::TypedInt(_, _) | Expr::Char(_) | Expr::Bool(_) => {
                let value = self.u8(expr)?;
                self.line(&format!("    ld a, {:02X}h", value));
            }
            Expr::Cast { expr, ty } => self.emit_cast_to_type(expr, ty)?,
            Expr::Unary { op, expr } => {
                if *op == UnaryOp::Not {
                    self.ensure_expr_is_bool(expr, "logical operand")?;
                }
                self.emit_unary_to_a(*op, expr)?
            }
            Expr::Binary { left, op, right } => self.emit_binary_expr(left, *op, right)?,
            Expr::Call { path, args }
                if matches!(path_text(path).as_str(), "mem.peek8" | "ezra.mem.peek8") =>
            {
                self.emit_mem_peek8(args)?;
            }
            Expr::Call { path, args } => {
                self.emit_user_call(&path_text(path), args)?;
            }
            Expr::AddressOfIndex { .. }
            | Expr::AddressOfField { .. }
            | Expr::AddressOfAccess(_)
            | Expr::AddressOf(_)
            | Expr::Array(_)
            | Expr::StructInit { .. }
            | Expr::String(_) => {
                return Err(Diagnostic::new(format!(
                    "expression `{expr:?}` is not supported in u8 codegen"
                )));
            }
        }
        Ok(())
    }

    fn emit_mem_peek8(&mut self, args: &[Expr]) -> Result<(), Diagnostic> {
        if args.len() != 1 {
            return Err(Diagnostic::new("mem.peek8 requires one argument"));
        }
        self.validate_expr_is_ptr_u8(&args[0])?;
        self.emit_expr_to_hl(&args[0], ValueWidth::U24)?;
        self.line("    ld a, (hl)");
        Ok(())
    }

    fn emit_mem_poke8(&mut self, args: &[Expr]) -> Result<(), Diagnostic> {
        if args.len() != 2 {
            return Err(Diagnostic::new("mem.poke8 requires two arguments"));
        }
        self.validate_expr_is_ptr_u8(&args[0])?;
        self.validate_expr_assignable_to_type(&args[1], &Type::Named("u8".to_owned()))?;
        let addr = self.alloc_var(ValueWidth::U24.bytes());
        let value = self.alloc_var(ValueWidth::U8.bytes());
        self.emit_expr_to_hl(&args[0], ValueWidth::U24)?;
        self.emit_store_hl(addr);
        self.emit_expr_to_a(&args[1])?;
        self.emit_store_a(value);
        self.emit_load_hl(addr);
        self.emit_load_a(value);
        self.line("    ld (hl), a");
        Ok(())
    }

    fn emit_memcpy(&mut self, args: &[Expr]) -> Result<(), Diagnostic> {
        if args.len() != 3 {
            return Err(Diagnostic::new("mem.memcpy requires three arguments"));
        }
        self.validate_expr_is_ptr_u8(&args[0])?;
        self.validate_expr_is_ptr_u8(&args[1])?;
        self.validate_expr_assignable_to_type(&args[2], &Type::Named("u24".to_owned()))?;
        let dst = self.alloc_var(ValueWidth::U24.bytes());
        let src = self.alloc_var(ValueWidth::U24.bytes());
        let len = self.alloc_var(ValueWidth::U24.bytes());

        self.emit_expr_to_hl(&args[0], ValueWidth::U24)?;
        self.emit_store_hl(dst);
        self.emit_expr_to_hl(&args[1], ValueWidth::U24)?;
        self.emit_store_hl(src);
        self.emit_expr_to_hl(&args[2], ValueWidth::U24)?;
        self.emit_store_hl(len);

        self.emit_load_hl(len);
        self.line("    push hl");
        self.line("    pop bc");
        self.emit_load_hl(src);
        self.line("    ex de, hl");
        self.emit_load_hl(dst);
        self.line("    call __ezra_memcpy");
        Ok(())
    }

    fn emit_memset(&mut self, args: &[Expr]) -> Result<(), Diagnostic> {
        if args.len() != 3 {
            return Err(Diagnostic::new("mem.memset requires three arguments"));
        }
        self.validate_expr_is_ptr_u8(&args[0])?;
        self.validate_expr_assignable_to_type(&args[1], &Type::Named("u8".to_owned()))?;
        self.validate_expr_assignable_to_type(&args[2], &Type::Named("u24".to_owned()))?;
        let dst = self.alloc_var(ValueWidth::U24.bytes());
        let value = self.alloc_var(ValueWidth::U8.bytes());
        let len = self.alloc_var(ValueWidth::U24.bytes());

        self.emit_expr_to_hl(&args[0], ValueWidth::U24)?;
        self.emit_store_hl(dst);
        self.emit_expr_to_a(&args[1])?;
        self.emit_store_a(value);
        self.emit_expr_to_hl(&args[2], ValueWidth::U24)?;
        self.emit_store_hl(len);

        self.emit_load_hl(len);
        self.line("    push hl");
        self.line("    pop bc");
        self.emit_load_a(value);
        self.emit_load_hl(dst);
        self.line("    call __ezra_memset");
        Ok(())
    }

    fn emit_dotted_constant_to_hl(
        &mut self,
        base: &str,
        field: &str,
        width: ValueWidth,
    ) -> Result<bool, Diagnostic> {
        let key = format!("{base}.{field}");
        if !self.symbols.constants.contains_key(&key) {
            return Ok(false);
        }
        let value = self.value_for_width(&Expr::Ident(key), width)?;
        self.line(&format!("    ld hl, {value:06X}h"));
        Ok(true)
    }

    fn emit_dotted_constant_to_a(&mut self, base: &str, field: &str) -> Result<bool, Diagnostic> {
        let key = format!("{base}.{field}");
        if !self.symbols.constants.contains_key(&key) {
            return Ok(false);
        }
        let value = self.u8(&Expr::Ident(key))?;
        self.line(&format!("    ld a, {value:02X}h"));
        Ok(true)
    }

    fn emit_string_literal_address(&mut self, value: &str) -> Result<(), Diagnostic> {
        if let Some(variable) = self.string_literals.get(value).copied() {
            self.line(&format!("    ld hl, {:06X}h", variable.addr));
            return Ok(());
        }

        let variable = self.symbols.intern_string_literal(value)?;
        self.emit_string_literal_initializer(value, variable);
        self.string_literals.insert(value.to_owned(), variable);
        self.line(&format!("    ld hl, {:06X}h", variable.addr));
        Ok(())
    }

    fn emit_deref_to_a(&mut self, ptr: &Expr) -> Result<(), Diagnostic> {
        self.emit_expr_to_hl(ptr, ValueWidth::U24)?;
        self.line("    ld a, (hl)");
        Ok(())
    }

    fn emit_deref_to_hl(&mut self, ptr: &Expr, width: ValueWidth) -> Result<(), Diagnostic> {
        self.emit_expr_to_hl(ptr, ValueWidth::U24)?;
        match width {
            ValueWidth::U8 => {
                self.line("    ld a, (hl)");
                self.line("    ld hl, 000000h");
                self.line("    ld l, a");
            }
            ValueWidth::U16 | ValueWidth::U24 => {
                let result = self.alloc_var(width.bytes());
                for offset in 0..width.bytes() {
                    if offset != 0 {
                        self.line("    inc hl");
                    }
                    self.line("    ld a, (hl)");
                    self.line(&format!("    ld ({:06X}h), a", result.addr + offset as u32));
                }
                self.emit_load_width(result);
            }
        }
        Ok(())
    }

    fn emit_deref_assignment(
        &mut self,
        ptr: &Expr,
        op: AssignOp,
        value: &Expr,
    ) -> Result<(), Diagnostic> {
        let pointee_type = match self.symbols.resolved_type(&self.expr_type(ptr)?)? {
            Type::Ptr(inner) => *inner,
            Type::Named(name) if name == "ptr24" => {
                return Err(Diagnostic::new(
                    "raw ptr24 dereference requires an explicit typed pointer cast",
                ));
            }
            other => {
                return Err(Diagnostic::new(format!(
                    "cannot assign through non-pointer expression of type `{other:?}`"
                )));
            }
        };
        self.ensure_pointer_write_target_is_mutable(ptr, &pointee_type)?;
        let addr = self.alloc_var(ValueWidth::U24.bytes());
        self.emit_expr_to_hl(ptr, ValueWidth::U24)?;
        self.emit_store_hl(addr);

        if op != AssignOp::Set {
            let width = self.symbols.type_width(&pointee_type)?;
            let current = self.alloc_var(width.bytes());
            self.emit_load_hl(addr);
            self.emit_load_pointed_width_into(current);
            let stored = self.alloc_var(width.bytes());
            let signed = self.type_is_signed(&pointee_type)?;
            self.emit_assignment_value(current, op, value, signed)?;
            self.emit_store_width(stored);
            self.emit_load_hl(addr);
            self.emit_store_var_to_pointed_width(stored);
            return Ok(());
        }

        self.validate_expr_assignable_to_type(value, &pointee_type)?;
        let stored = self.alloc_storage(&pointee_type)?;
        self.emit_storage_initializer(stored, &pointee_type, value)?;
        self.emit_load_hl(addr);
        self.emit_store_var_to_pointed_width(stored);
        Ok(())
    }

    fn ensure_pointer_write_target_is_mutable(
        &mut self,
        ptr: &Expr,
        pointee_type: &Type,
    ) -> Result<(), Diagnostic> {
        let Some(addr) = self.readonly_write_addr(ptr) else {
            return Ok(());
        };
        let size = u64::from(self.symbols.type_size(pointee_type)?);
        let write_start = u64::from(addr);
        let write_end = write_start.saturating_add(size);
        for (name, embed) in &self.symbols.embeds {
            let Some(len) = embed.variable.len else {
                continue;
            };
            if len == 0 {
                continue;
            }
            let embed_start = u64::from(embed.variable.addr);
            let embed_end = embed_start + u64::from(len);
            if write_start < embed_end && write_end > embed_start {
                return Err(Diagnostic::new(format!(
                    "embedded object `{name}` is read-only"
                )));
            }
        }
        if self
            .readonly_string_literal_for_range(write_start, write_end)
            .is_some()
        {
            return Err(Diagnostic::new("string literal is read-only"));
        }
        Ok(())
    }

    fn readonly_write_addr(&mut self, ptr: &Expr) -> Option<u32> {
        if let Some(addr) = self.readonly_expr_addr(ptr) {
            return Some(addr);
        }
        let Ok(addr) = self.symbols.eval_i64(ptr) else {
            return None;
        };
        Self::addr24(addr)
    }

    fn readonly_expr_addr(&mut self, expr: &Expr) -> Option<u32> {
        match expr {
            Expr::Ident(name) => self.readonly_pointer_alias(name).or_else(|| {
                self.symbols
                    .readonly_global_pointer_aliases
                    .get(name)
                    .copied()
            }),
            Expr::String(value) => {
                if let Some(variable) = self
                    .string_literals
                    .get(value)
                    .or_else(|| self.symbols.string_literals.get(value))
                {
                    return Some(variable.addr);
                }
                let variable = self.symbols.intern_string_literal(value).ok()?;
                self.string_literals.insert(value.clone(), variable);
                Some(variable.addr)
            }
            Expr::Cast { expr, .. } => self.readonly_expr_addr(expr),
            Expr::Binary {
                left,
                op: op @ (BinaryOp::Add | BinaryOp::Sub),
                right,
            } => {
                let base = self.readonly_expr_addr(left)?;
                let Type::Ptr(inner) = self
                    .expr_type(left)
                    .ok()
                    .and_then(|ty| self.symbols.resolved_type(&ty).ok())?
                else {
                    return None;
                };
                let offset = self.eval_i64_with_local_constants(right).ok()?;
                let offset = if *op == BinaryOp::Sub {
                    offset.wrapping_neg()
                } else {
                    offset
                };
                let scale = i64::from(self.symbols.type_size(&inner).ok()?);
                Self::addr24(i64::from(base).wrapping_add(offset.wrapping_mul(scale)))
            }
            _ => None,
        }
    }

    fn addr24(addr: i64) -> Option<u32> {
        if (0..=0xFF_FFFF).contains(&addr) {
            Some(addr as u32)
        } else {
            None
        }
    }

    fn emit_binary_expr(
        &mut self,
        left: &Expr,
        op: BinaryOp,
        right: &Expr,
    ) -> Result<(), Diagnostic> {
        if matches!(op, BinaryOp::And | BinaryOp::Or) {
            self.emit_short_circuit_logical(left, op, right)?;
            return Ok(());
        }
        if is_comparison(op) {
            self.ensure_comparison_operands_compatible(left, op, right)?;
            let width = self.expr_width(left)?.max(self.expr_width(right)?);
            if self.binary_operands_are_signed(left, right)? {
                self.emit_signed_comparison(left, op, right, width)?;
                return Ok(());
            }
            if width != ValueWidth::U8 {
                self.emit_wide_comparison(left, op, right, width)?;
                return Ok(());
            }
        }
        if matches!(op, BinaryOp::Shl | BinaryOp::Shr) {
            self.ensure_shift_operands_compatible(left, right)?;
            let signed = self.expr_is_signed(left)?;
            self.emit_expr_to_a(left)?;
            self.emit_shift_a_by_expr(op, right, signed)?;
            return Ok(());
        }
        if matches!(op, BinaryOp::Div | BinaryOp::Mod) {
            self.ensure_binary_arithmetic_operands_compatible(left, right)?;
            if self.binary_operands_are_signed(left, right)? {
                self.emit_signed_div_mod_to_width(left, right, op, ValueWidth::U8)?;
            } else {
                self.emit_u8_div_mod(left, right, op)?;
            }
            return Ok(());
        }
        if op == BinaryOp::Mul {
            self.ensure_binary_arithmetic_operands_compatible(left, right)?;
            self.emit_mul_to_width(
                left,
                right,
                ValueWidth::U8,
                self.binary_operands_are_signed(left, right)?,
            )?;
            return Ok(());
        }

        if matches!(
            op,
            BinaryOp::Add | BinaryOp::Sub | BinaryOp::BitAnd | BinaryOp::BitOr | BinaryOp::BitXor
        ) {
            self.ensure_binary_arithmetic_operands_compatible(left, right)?;
        }
        let left_var = self.alloc_var(ValueWidth::U8.bytes());
        self.emit_expr_to_a(left)?;
        self.emit_store_a(left_var);
        self.emit_expr_to_a(right)?;
        self.line("    ld c, a");
        self.emit_load_a(left_var);
        match op {
            BinaryOp::Add => self.line("    add a, c"),
            BinaryOp::Sub => self.line("    sub c"),
            BinaryOp::BitAnd => self.line("    and c"),
            BinaryOp::BitOr => self.line("    or c"),
            BinaryOp::BitXor => self.line("    xor c"),
            BinaryOp::Eq
            | BinaryOp::Ne
            | BinaryOp::Lt
            | BinaryOp::Le
            | BinaryOp::Gt
            | BinaryOp::Ge => self.emit_comparison(op),
            BinaryOp::And | BinaryOp::Or => unreachable!("logical ops handled before binary load"),
            BinaryOp::Div | BinaryOp::Mod | BinaryOp::Shl | BinaryOp::Shr => {
                return Err(Diagnostic::new(format!(
                    "binary operator `{op:?}` is not implemented in u8 codegen yet"
                )));
            }
            BinaryOp::Mul => unreachable!("multiplication handled before u8 binary dispatch"),
        }
        Ok(())
    }

    fn emit_short_circuit_logical(
        &mut self,
        left: &Expr,
        op: BinaryOp,
        right: &Expr,
    ) -> Result<(), Diagnostic> {
        self.ensure_expr_is_bool(left, "logical operand")?;
        self.ensure_expr_is_bool(right, "logical operand")?;
        let short_label = self.next_label("logical_short");
        let end_label = self.next_label("logical_end");

        self.emit_expr_to_a(left)?;
        self.line("    or a");
        match op {
            BinaryOp::And => {
                self.line(&format!("    jp z, {short_label}"));
                self.emit_expr_to_a(right)?;
                self.line("    or a");
                self.line(&format!("    jp z, {short_label}"));
                self.line("    ld a, 01h");
                self.line(&format!("    jp {end_label}"));
                self.line(&format!("{short_label}:"));
                self.line("    ld a, 00h");
            }
            BinaryOp::Or => {
                self.line(&format!("    jp nz, {short_label}"));
                self.emit_expr_to_a(right)?;
                self.line("    or a");
                self.line(&format!("    jp nz, {short_label}"));
                self.line("    ld a, 00h");
                self.line(&format!("    jp {end_label}"));
                self.line(&format!("{short_label}:"));
                self.line("    ld a, 01h");
            }
            _ => unreachable!("not a logical op"),
        }
        self.line(&format!("{end_label}:"));
        Ok(())
    }

    fn emit_wide_comparison(
        &mut self,
        left: &Expr,
        op: BinaryOp,
        right: &Expr,
        width: ValueWidth,
    ) -> Result<(), Diagnostic> {
        self.emit_expr_to_hl(left, width)?;
        self.line("    push hl");
        self.emit_expr_to_hl(right, width)?;
        self.line("    ex de, hl");
        self.line("    pop hl");
        self.line("    or a");
        self.line("    sbc hl, de");
        self.emit_comparison_from_flags(op);
        Ok(())
    }

    fn emit_signed_comparison(
        &mut self,
        left: &Expr,
        op: BinaryOp,
        right: &Expr,
        width: ValueWidth,
    ) -> Result<(), Diagnostic> {
        if matches!(op, BinaryOp::Eq | BinaryOp::Ne) {
            if width == ValueWidth::U8 {
                let left_var = self.alloc_var(width.bytes());
                self.emit_expr_to_width(left, width)?;
                self.emit_store_width(left_var);
                self.emit_expr_to_width(right, width)?;
                self.line("    ld c, a");
                self.emit_load_width(left_var);
                self.emit_comparison(op);
                return Ok(());
            }
            return self.emit_wide_comparison(left, op, right, width);
        }

        let left_var = self.alloc_var(width.bytes());
        let right_var = self.alloc_var(width.bytes());
        let same_sign_label = self.next_label("scmp_same_sign");
        let true_label = self.next_label("scmp_true");
        let false_label = self.next_label("scmp_false");
        let end_label = self.next_label("scmp_end");
        let sign_offset = u32::from(width.bytes() - 1);

        self.emit_expr_to_width(left, width)?;
        self.emit_store_width(left_var);
        self.emit_expr_to_width(right, width)?;
        self.emit_store_width(right_var);

        self.line("    ld a, 80h");
        self.line("    ld c, a");
        self.line(&format!("    ld a, ({:06X}h)", left_var.addr + sign_offset));
        self.line("    and c");
        self.line("    ld b, a");
        self.line(&format!(
            "    ld a, ({:06X}h)",
            right_var.addr + sign_offset
        ));
        self.line("    and c");
        self.line("    cp b");
        self.line(&format!("    jp z, {same_sign_label}"));
        self.line("    ld a, b");
        self.line("    or a");
        match op {
            BinaryOp::Lt | BinaryOp::Le => {
                self.line(&format!("    jp nz, {true_label}"));
                self.line(&format!("    jp {false_label}"));
            }
            BinaryOp::Gt | BinaryOp::Ge => {
                self.line(&format!("    jp nz, {false_label}"));
                self.line(&format!("    jp {true_label}"));
            }
            _ => unreachable!("not a signed ordering comparison"),
        }

        self.line(&format!("{same_sign_label}:"));
        if width == ValueWidth::U8 {
            self.emit_load_width(right_var);
            self.line("    ld c, a");
            self.emit_load_width(left_var);
            self.line("    cp c");
        } else {
            self.emit_load_width(left_var);
            self.line("    push hl");
            self.emit_load_width(right_var);
            self.line("    ex de, hl");
            self.line("    pop hl");
            self.line("    or a");
            self.line("    sbc hl, de");
        }
        match op {
            BinaryOp::Lt => self.line(&format!("    jp c, {true_label}")),
            BinaryOp::Ge => self.line(&format!("    jp nc, {true_label}")),
            BinaryOp::Le => {
                self.line(&format!("    jp c, {true_label}"));
                self.line(&format!("    jp z, {true_label}"));
            }
            BinaryOp::Gt => {
                self.line(&format!("    jp c, {false_label}"));
                self.line(&format!("    jp z, {false_label}"));
                self.line(&format!("    jp {true_label}"));
            }
            _ => unreachable!("not a signed ordering comparison"),
        }

        self.line(&format!("{false_label}:"));
        self.line("    ld a, 00h");
        self.line(&format!("    jp {end_label}"));
        self.line(&format!("{true_label}:"));
        self.line("    ld a, 01h");
        self.line(&format!("{end_label}:"));
        Ok(())
    }

    fn emit_u8_div_mod(
        &mut self,
        left: &Expr,
        right: &Expr,
        op: BinaryOp,
    ) -> Result<(), Diagnostic> {
        let left_var = self.alloc_var(1u32);
        self.emit_expr_to_a(left)?;
        self.emit_store_a(left_var);
        self.emit_expr_to_a(right)?;
        self.line("    ld c, a");
        self.emit_load_a(left_var);
        match op {
            BinaryOp::Div => self.line("    call __ezra_div_u8"),
            BinaryOp::Mod => self.line("    call __ezra_mod_u8"),
            _ => unreachable!("not a division op"),
        }
        Ok(())
    }

    fn emit_mul_to_width(
        &mut self,
        left: &Expr,
        right: &Expr,
        width: ValueWidth,
        signed: bool,
    ) -> Result<(), Diagnostic> {
        if width == ValueWidth::U8 {
            let left_var = self.alloc_var(1u32);
            self.emit_expr_to_a(left)?;
            self.emit_store_a(left_var);
            self.emit_expr_to_a(right)?;
            self.line("    ld c, a");
            self.emit_load_a(left_var);
            self.line("    call __ezra_mul_u8");
            return Ok(());
        }
        if width == ValueWidth::U16 {
            self.emit_expr_to_hl(left, width)?;
            self.line("    push hl");
            self.emit_expr_to_hl(right, width)?;
            self.line("    push hl");
            self.line("    pop bc");
            self.line("    pop hl");
            self.line("    call __ezra_mul_u16");
            return Ok(());
        }
        if width == ValueWidth::U24 {
            self.emit_expr_to_hl(left, width)?;
            self.line("    push hl");
            self.emit_expr_to_hl(right, width)?;
            self.line("    push hl");
            self.line("    pop bc");
            self.line("    pop hl");
            if signed {
                self.line("    call __ezra_mul_i24");
            } else {
                self.line("    call __ezra_mul_u24");
            }
            return Ok(());
        }

        let left_var = self.alloc_var(width.bytes());
        let counter = self.alloc_var(width.bytes());
        let result = self.alloc_var(width.bytes());
        let loop_label = self.next_label("mul_loop");
        let done_label = self.next_label("mul_done");

        self.emit_expr_to_width(left, width)?;
        self.emit_store_width(left_var);
        self.emit_expr_to_width(right, width)?;
        self.emit_store_width(counter);
        match width {
            ValueWidth::U8 => self.line("    xor a"),
            ValueWidth::U16 | ValueWidth::U24 => self.line("    ld hl, 000000h"),
        }
        self.emit_store_width(result);

        self.line(&format!("{loop_label}:"));
        self.emit_jump_if_memory_zero(counter, &done_label);
        if width == ValueWidth::U8 {
            self.emit_load_a(result);
            self.line("    ld b, a");
            self.emit_load_a(left_var);
            self.line("    add a, b");
            self.emit_store_a(result);
        } else {
            self.emit_load_width(result);
            self.line("    push hl");
            self.emit_load_width(left_var);
            self.line("    pop bc");
            self.emit_wide_op_with_left_in_bc(BinaryOp::Add, width)?;
            self.emit_store_width(result);
        }
        self.emit_decrement_memory(counter);
        self.line(&format!("    jp {loop_label}"));
        self.line(&format!("{done_label}:"));
        self.emit_load_width(result);
        Ok(())
    }

    fn emit_div_mod_to_width(
        &mut self,
        left: &Expr,
        right: &Expr,
        op: BinaryOp,
        width: ValueWidth,
    ) -> Result<(), Diagnostic> {
        if width == ValueWidth::U16 {
            self.emit_expr_to_hl(left, width)?;
            self.line("    push hl");
            self.emit_expr_to_hl(right, width)?;
            self.line("    push hl");
            self.line("    pop bc");
            self.line("    pop hl");
            match op {
                BinaryOp::Div => self.line("    call __ezra_div_u16"),
                BinaryOp::Mod => self.line("    call __ezra_mod_u16"),
                _ => unreachable!("not a division op"),
            }
            return Ok(());
        }
        if width == ValueWidth::U24 {
            self.emit_expr_to_hl(left, width)?;
            self.line("    push hl");
            self.emit_expr_to_hl(right, width)?;
            self.line("    push hl");
            self.line("    pop bc");
            self.line("    pop hl");
            match op {
                BinaryOp::Div => self.line("    call __ezra_div_u24"),
                BinaryOp::Mod => self.line("    call __ezra_mod_u24"),
                _ => unreachable!("not a division op"),
            }
            return Ok(());
        }

        let dividend = self.alloc_var(width.bytes());
        let divisor = self.alloc_var(width.bytes());
        let quotient = self.alloc_var(width.bytes());
        let loop_label = self.next_label("div_loop");
        let zero_label = self.next_label("div_zero");
        let done_label = self.next_label("div_done");

        self.emit_expr_to_hl(left, width)?;
        self.emit_store_width(dividend);
        self.emit_expr_to_hl(right, width)?;
        self.emit_store_width(divisor);
        self.emit_jump_if_memory_zero(divisor, &zero_label);
        self.emit_zero_variable(quotient);

        self.line(&format!("{loop_label}:"));
        self.emit_load_width(dividend);
        self.line("    push hl");
        self.emit_load_width(divisor);
        self.line("    ex de, hl");
        self.line("    pop hl");
        self.line("    or a");
        self.line("    sbc hl, de");
        self.line(&format!("    jp c, {done_label}"));
        self.emit_store_width(dividend);
        self.emit_increment_memory(quotient);
        self.line(&format!("    jp {loop_label}"));

        self.line(&format!("{zero_label}:"));
        self.emit_zero_variable(dividend);
        self.emit_zero_variable(quotient);
        self.line(&format!("{done_label}:"));
        match op {
            BinaryOp::Div => self.emit_load_width(quotient),
            BinaryOp::Mod => self.emit_load_width(dividend),
            _ => unreachable!("not a division op"),
        }
        Ok(())
    }

    fn emit_signed_div_mod_to_width(
        &mut self,
        left: &Expr,
        right: &Expr,
        op: BinaryOp,
        width: ValueWidth,
    ) -> Result<(), Diagnostic> {
        if width == ValueWidth::U24 {
            self.emit_expr_to_hl(left, width)?;
            self.line("    push hl");
            self.emit_expr_to_hl(right, width)?;
            self.line("    push hl");
            self.line("    pop bc");
            self.line("    pop hl");
            match op {
                BinaryOp::Div => self.line("    call __ezra_div_i24"),
                BinaryOp::Mod => self.line("    call __ezra_mod_i24"),
                _ => unreachable!("not a division op"),
            }
            return Ok(());
        }

        let dividend = self.alloc_var(width.bytes());
        let divisor = self.alloc_var(width.bytes());
        let quotient = self.alloc_var(width.bytes());
        let quotient_negative = self.alloc_var(ValueWidth::U8.bytes());
        let remainder_negative = self.alloc_var(ValueWidth::U8.bytes());
        let loop_label = self.next_label("sdiv_loop");
        let zero_label = self.next_label("sdiv_zero");
        let done_label = self.next_label("sdiv_done");
        let quotient_positive_label = self.next_label("sdiv_q_positive");
        let remainder_positive_label = self.next_label("sdiv_r_positive");
        let not_overflow_label = self.next_label("sdiv_not_overflow");
        let finished_label = self.next_label("sdiv_finished");

        self.emit_expr_to_width(left, width)?;
        self.emit_store_width(dividend);
        self.emit_expr_to_width(right, width)?;
        self.emit_store_width(divisor);
        self.emit_zero_variable(quotient);
        self.emit_zero_variable(quotient_negative);
        self.emit_zero_variable(remainder_negative);
        self.emit_jump_if_memory_zero(divisor, &zero_label);
        self.emit_jump_if_memory_not_equals(dividend, signed_min_bytes(width), &not_overflow_label);
        self.emit_jump_if_memory_not_equals(
            divisor,
            signed_negative_one_bytes(width),
            &not_overflow_label,
        );
        match op {
            BinaryOp::Div => self.emit_load_width(dividend),
            BinaryOp::Mod => {
                self.emit_zero_variable(dividend);
                self.emit_load_width(dividend);
            }
            _ => unreachable!("not a division op"),
        }
        self.line(&format!("    jp {finished_label}"));
        self.line(&format!("{not_overflow_label}:"));

        self.emit_abs_signed_variable(dividend, Some(quotient_negative), Some(remainder_negative));
        self.emit_abs_signed_variable(divisor, Some(quotient_negative), None);

        self.line(&format!("{loop_label}:"));
        if width == ValueWidth::U8 {
            self.emit_load_a(dividend);
            self.line("    ld b, a");
            self.emit_load_a(divisor);
            self.line("    ld c, a");
            self.line("    ld a, b");
            self.line("    cp c");
            self.line(&format!("    jp c, {done_label}"));
            self.line("    sub c");
            self.emit_store_a(dividend);
        } else {
            self.emit_load_width(dividend);
            self.line("    push hl");
            self.emit_load_width(divisor);
            self.line("    ex de, hl");
            self.line("    pop hl");
            self.line("    or a");
            self.line("    sbc hl, de");
            self.line(&format!("    jp c, {done_label}"));
            self.emit_store_width(dividend);
        }
        self.emit_increment_memory(quotient);
        self.line(&format!("    jp {loop_label}"));

        self.line(&format!("{zero_label}:"));
        self.emit_zero_variable(dividend);
        self.emit_zero_variable(quotient);
        self.line(&format!("{done_label}:"));
        self.emit_load_a(quotient_negative);
        self.line("    or a");
        self.line(&format!("    jp z, {quotient_positive_label}"));
        self.emit_negate_memory(quotient);
        self.line(&format!("{quotient_positive_label}:"));
        self.emit_load_a(remainder_negative);
        self.line("    or a");
        self.line(&format!("    jp z, {remainder_positive_label}"));
        self.emit_negate_memory(dividend);
        self.line(&format!("{remainder_positive_label}:"));

        match op {
            BinaryOp::Div => self.emit_load_width(quotient),
            BinaryOp::Mod => self.emit_load_width(dividend),
            _ => unreachable!("not a division op"),
        }
        self.line(&format!("{finished_label}:"));
        Ok(())
    }

    fn emit_abs_signed_variable(
        &mut self,
        variable: Variable,
        quotient_negative: Option<Variable>,
        remainder_negative: Option<Variable>,
    ) {
        let nonnegative_label = self.next_label("signed_nonnegative");
        let sign_addr = variable.addr + variable.size - 1;
        self.line(&format!("    ld a, ({sign_addr:06X}h)"));
        self.line("    ld b, a");
        self.line("    ld a, 7Fh");
        self.line("    cp b");
        self.line(&format!("    jp nc, {nonnegative_label}"));
        self.emit_negate_memory(variable);
        if let Some(flag) = quotient_negative {
            self.emit_toggle_u8(flag);
        }
        if let Some(flag) = remainder_negative {
            self.emit_toggle_u8(flag);
        }
        self.line(&format!("{nonnegative_label}:"));
    }

    fn emit_negate_memory(&mut self, variable: Variable) {
        for offset in 0..variable.size {
            let addr = variable.addr + offset;
            self.line(&format!("    ld a, ({addr:06X}h)"));
            self.line("    xor FFh");
            self.line(&format!("    ld ({addr:06X}h), a"));
        }
        self.emit_increment_memory(variable);
    }

    fn emit_toggle_u8(&mut self, variable: Variable) {
        self.emit_load_a(variable);
        self.line("    xor 01h");
        self.emit_store_a(variable);
    }

    fn emit_jump_if_memory_zero(&mut self, variable: Variable, zero_label: &str) {
        let nonzero_label = self.next_label("nonzero");
        for offset in 0..variable.size {
            self.line(&format!("    ld a, ({:06X}h)", variable.addr + offset));
            self.line("    or a");
            self.line(&format!("    jp nz, {nonzero_label}"));
        }
        self.line(&format!("    jp {zero_label}"));
        self.line(&format!("{nonzero_label}:"));
    }

    fn emit_jump_if_memory_not_equals(&mut self, variable: Variable, bytes: &[u8], label: &str) {
        for (offset, byte) in bytes.iter().copied().enumerate() {
            self.line(&format!(
                "    ld a, ({:06X}h)",
                variable.addr + offset as u32
            ));
            self.line("    ld b, a");
            self.line(&format!("    ld a, {byte:02X}h"));
            self.line("    cp b");
            self.line(&format!("    jp nz, {label}"));
        }
    }

    fn emit_zero_variable(&mut self, variable: Variable) {
        match variable.size {
            1 => self.line("    xor a"),
            2 | 3 => self.line("    ld hl, 000000h"),
            _ => unreachable!("unsupported variable size {}", variable.size),
        }
        self.emit_store_width(variable);
    }

    fn emit_zero_storage(&mut self, variable: Variable) {
        self.line("    xor a");
        for offset in 0..variable.size {
            self.line(&format!("    ld ({:06X}h), a", variable.addr + offset));
        }
    }

    fn emit_increment_memory(&mut self, variable: Variable) {
        let done_label = self.next_label("inc_done");
        for offset in 0..variable.size {
            let addr = variable.addr + offset;
            self.line(&format!("    ld a, ({addr:06X}h)"));
            self.line("    ld b, a");
            self.line("    ld a, 01h");
            self.line("    add a, b");
            self.line(&format!("    ld ({addr:06X}h), a"));
            self.line("    or a");
            self.line(&format!("    jp nz, {done_label}"));
        }
        self.line(&format!("{done_label}:"));
    }

    fn emit_decrement_memory(&mut self, variable: Variable) {
        let done_label = self.next_label("dec_done");
        for offset in 0..variable.size {
            let addr = variable.addr + offset;
            self.line(&format!("    ld a, ({addr:06X}h)"));
            self.line("    ld b, a");
            self.line("    ld a, 01h");
            self.line("    ld c, a");
            self.line("    ld a, b");
            self.line("    sub c");
            self.line(&format!("    ld ({addr:06X}h), a"));
            self.line("    ld a, b");
            self.line("    or a");
            self.line(&format!("    jp nz, {done_label}"));
        }
        self.line(&format!("{done_label}:"));
    }

    fn emit_shift_a(&mut self, op: BinaryOp, count: u8, signed: bool) -> Result<(), Diagnostic> {
        for _ in 0..count {
            match op {
                BinaryOp::Shl => self.line("    add a, a"),
                BinaryOp::Shr if signed => self.line("    sra a"),
                BinaryOp::Shr => self.line("    srl a"),
                _ => unreachable!("not a shift op"),
            }
        }
        Ok(())
    }

    fn emit_shift_a_by_expr(
        &mut self,
        op: BinaryOp,
        count: &Expr,
        signed: bool,
    ) -> Result<(), Diagnostic> {
        if let Some(count) = self.maybe_const_shift_count(count)? {
            return self.emit_shift_a(op, count, signed);
        }
        let temp = self.alloc_var(ValueWidth::U8.bytes());
        self.emit_store_a(temp);
        self.emit_expr_to_a(count)?;
        self.line("    ld b, a");
        self.emit_shift_memory_dynamic(temp, op, signed)?;
        self.emit_load_a(temp);
        Ok(())
    }

    fn emit_shift_memory(
        &mut self,
        variable: Variable,
        op: BinaryOp,
        count: u8,
        signed: bool,
    ) -> Result<(), Diagnostic> {
        for _ in 0..count {
            match op {
                BinaryOp::Shl => self.emit_shift_memory_left_once(variable),
                BinaryOp::Shr => self.emit_shift_memory_right_once(variable, signed),
                _ => unreachable!("not a shift op"),
            }
        }
        Ok(())
    }

    fn emit_shift_memory_by_expr(
        &mut self,
        variable: Variable,
        op: BinaryOp,
        count: &Expr,
        signed: bool,
    ) -> Result<(), Diagnostic> {
        if let Some(count) = self.maybe_const_shift_count(count)? {
            return self.emit_shift_memory(variable, op, count, signed);
        }
        self.emit_expr_to_a(count)?;
        self.line("    ld b, a");
        self.emit_shift_memory_dynamic(variable, op, signed)
    }

    fn emit_shift_memory_dynamic(
        &mut self,
        variable: Variable,
        op: BinaryOp,
        signed: bool,
    ) -> Result<(), Diagnostic> {
        let loop_label = self.next_label("shift_loop");
        let done_label = self.next_label("shift_done");
        self.line(&format!("{loop_label}:"));
        self.line("    ld a, b");
        self.line("    or a");
        self.line(&format!("    jp z, {done_label}"));
        match op {
            BinaryOp::Shl => self.emit_shift_memory_left_once(variable),
            BinaryOp::Shr => self.emit_shift_memory_right_once(variable, signed),
            _ => unreachable!("not a shift op"),
        }
        self.line("    dec b");
        self.line(&format!("    jp {loop_label}"));
        self.line(&format!("{done_label}:"));
        Ok(())
    }

    fn emit_shift_memory_left_once(&mut self, variable: Variable) {
        self.line(&format!("    ld a, ({:06X}h)", variable.addr));
        self.line("    add a, a");
        self.line(&format!("    ld ({:06X}h), a", variable.addr));
        for offset in 1..variable.size {
            let addr = variable.addr + offset as u32;
            self.line(&format!("    ld a, ({addr:06X}h)"));
            self.line("    rl a");
            self.line(&format!("    ld ({addr:06X}h), a"));
        }
    }

    fn emit_shift_memory_right_once(&mut self, variable: Variable, signed: bool) {
        for offset in (0..variable.size).rev() {
            let addr = variable.addr + offset as u32;
            self.line(&format!("    ld a, ({addr:06X}h)"));
            if offset == variable.size - 1 {
                if signed {
                    self.line("    sra a");
                } else {
                    self.line("    srl a");
                }
            } else {
                self.line("    rr a");
            }
            self.line(&format!("    ld ({addr:06X}h), a"));
        }
    }

    fn emit_unary_to_a(&mut self, op: UnaryOp, expr: &Expr) -> Result<(), Diagnostic> {
        match op {
            UnaryOp::Neg => {
                self.emit_expr_to_a(expr)?;
                self.line("    ld b, a");
                self.line("    xor a");
                self.line("    sub b");
            }
            UnaryOp::BitNot => {
                self.emit_expr_to_a(expr)?;
                self.line("    xor FFh");
            }
            UnaryOp::Not => {
                let true_label = self.next_label("not_true");
                let end_label = self.next_label("not_end");
                self.emit_expr_to_a(expr)?;
                self.line("    or a");
                self.line(&format!("    jp z, {true_label}"));
                self.line("    ld a, 00h");
                self.line(&format!("    jp {end_label}"));
                self.line(&format!("{true_label}:"));
                self.line("    ld a, 01h");
                self.line(&format!("{end_label}:"));
            }
        }
        Ok(())
    }

    fn emit_unary_to_hl(
        &mut self,
        op: UnaryOp,
        expr: &Expr,
        width: ValueWidth,
    ) -> Result<(), Diagnostic> {
        match op {
            UnaryOp::Neg => {
                self.emit_expr_to_hl(expr, width)?;
                self.line("    push hl");
                self.line("    ld hl, 000000h");
                self.line("    pop bc");
                self.line("    or a");
                self.line("    sbc hl, bc");
            }
            UnaryOp::BitNot => {
                self.emit_expr_to_hl(expr, width)?;
                let value = self.alloc_var(width.bytes());
                self.emit_store_width(value);
                let result = self.alloc_var(width.bytes());
                for offset in 0..width.bytes() {
                    self.line(&format!("    ld a, ({:06X}h)", value.addr + offset as u32));
                    self.line("    xor FFh");
                    self.line(&format!("    ld ({:06X}h), a", result.addr + offset as u32));
                }
                self.emit_load_width(result);
            }
            UnaryOp::Not => {
                let true_label = self.next_label("not_true");
                let end_label = self.next_label("not_end");
                self.emit_expr_to_hl(expr, width)?;
                self.line("    push hl");
                self.line("    ld hl, 000000h");
                self.line("    pop bc");
                self.line("    or a");
                self.line("    sbc hl, bc");
                self.line(&format!("    jp z, {true_label}"));
                self.line("    ld hl, 000000h");
                self.line(&format!("    jp {end_label}"));
                self.line(&format!("{true_label}:"));
                self.line("    ld hl, 000001h");
                self.line(&format!("{end_label}:"));
            }
        }
        Ok(())
    }

    fn emit_comparison(&mut self, op: BinaryOp) {
        self.line("    cp c");
        self.emit_comparison_from_flags(op);
    }

    fn emit_comparison_from_flags(&mut self, op: BinaryOp) {
        let true_label = self.next_label("cmp_true");
        let end_label = self.next_label("cmp_end");
        let false_label = self.next_label("cmp_false");
        match op {
            BinaryOp::Eq => self.line(&format!("    jp z, {true_label}")),
            BinaryOp::Ne => self.line(&format!("    jp nz, {true_label}")),
            BinaryOp::Lt => self.line(&format!("    jp c, {true_label}")),
            BinaryOp::Ge => self.line(&format!("    jp nc, {true_label}")),
            BinaryOp::Le => {
                self.line(&format!("    jp c, {true_label}"));
                self.line(&format!("    jp z, {true_label}"));
            }
            BinaryOp::Gt => {
                self.line(&format!("    jp c, {false_label}"));
                self.line(&format!("    jp z, {false_label}"));
                self.line(&format!("    jp {true_label}"));
            }
            _ => unreachable!("not a comparison"),
        }
        self.line(&format!("{false_label}:"));
        self.line("    ld a, 00h");
        self.line(&format!("    jp {end_label}"));
        self.line(&format!("{true_label}:"));
        self.line("    ld a, 01h");
        self.line(&format!("{end_label}:"));
    }

    fn emit_out(&mut self, port: u8, value: u8) {
        self.line(&format!("    ld a, {:02X}h", value));
        self.emit_out_a(port);
    }

    fn emit_out_a(&mut self, port: u8) {
        self.line(&format!("    out0 ({:02X}h), a", port));
    }

    fn emit_load_a(&mut self, variable: Variable) {
        debug_assert_eq!(variable.size, 1);
        self.line(&format!("    ld a, ({:06X}h)", variable.addr));
    }

    fn emit_store_a(&mut self, variable: Variable) {
        debug_assert_eq!(variable.size, 1);
        self.line(&format!("    ld ({:06X}h), a", variable.addr));
    }

    fn emit_load_hl(&mut self, variable: Variable) {
        debug_assert_eq!(variable.size, 3);
        self.line(&format!("    ld hl, ({:06X}h)", variable.addr));
    }

    fn emit_store_hl(&mut self, variable: Variable) {
        debug_assert_eq!(variable.size, 3);
        self.line(&format!("    ld ({:06X}h), hl", variable.addr));
    }

    fn emit_load_hl16(&mut self, variable: Variable) {
        debug_assert_eq!(variable.size, 2);
        self.line("    ld hl, 000000h");
        self.line(&format!("    ld a, ({:06X}h)", variable.addr));
        self.line("    ld l, a");
        self.line(&format!("    ld a, ({:06X}h)", variable.addr + 1));
        self.line("    ld h, a");
    }

    fn emit_store_hl16(&mut self, variable: Variable) {
        debug_assert_eq!(variable.size, 2);
        self.line("    ld a, l");
        self.line(&format!("    ld ({:06X}h), a", variable.addr));
        self.line("    ld a, h");
        self.line(&format!("    ld ({:06X}h), a", variable.addr + 1));
    }

    fn emit_load_width(&mut self, variable: Variable) {
        match variable.size {
            1 => self.emit_load_a(variable),
            2 => self.emit_load_hl16(variable),
            3 => self.emit_load_hl(variable),
            _ => unreachable!("unsupported variable size {}", variable.size),
        }
    }

    fn emit_store_width(&mut self, variable: Variable) {
        match variable.size {
            1 => self.emit_store_a(variable),
            2 => self.emit_store_hl16(variable),
            3 => self.emit_store_hl(variable),
            _ => unreachable!("unsupported variable size {}", variable.size),
        }
    }

    fn emit_load_ix_offset_width_into(
        &mut self,
        offset: u8,
        variable: Variable,
    ) -> Result<(), Diagnostic> {
        for byte_offset in 0..variable.size {
            let displacement = offset as u32 + byte_offset;
            if displacement > 0x7F {
                return Err(Diagnostic::new(format!(
                    "stack argument offset {displacement} exceeds IX displacement range"
                )));
            }
            self.line(&format!("    ld a, (ix+{displacement})"));
            self.line(&format!("    ld ({:06X}h), a", variable.addr + byte_offset));
        }
        Ok(())
    }

    fn emit_push_stack_arg_variable(&mut self, variable: Variable) {
        for byte_offset in (0..variable.size).rev() {
            self.line(&format!("    ld a, ({:06X}h)", variable.addr + byte_offset));
            self.line("    dec sp");
            self.line("    ld hl, 000000h");
            self.line("    add hl, sp");
            self.line("    ld (hl), a");
        }
    }

    fn emit_drop_stack_arg_bytes(&mut self, bytes: u8) {
        for _ in 0..bytes {
            self.line("    inc sp");
        }
    }

    fn emit_load_pointed_width_into(&mut self, variable: Variable) {
        for offset in 0..variable.size {
            if offset != 0 {
                self.line("    inc hl");
            }
            self.line("    ld a, (hl)");
            self.line(&format!(
                "    ld ({:06X}h), a",
                variable.addr + offset as u32
            ));
        }
    }

    fn emit_store_var_to_pointed_width(&mut self, variable: Variable) {
        for offset in 0..variable.size {
            if offset != 0 {
                self.line("    inc hl");
            }
            self.line(&format!(
                "    ld a, ({:06X}h)",
                variable.addr + offset as u32
            ));
            self.line("    ld (hl), a");
        }
    }

    fn emit_copy_storage_into(
        &mut self,
        source: Variable,
        target: Variable,
    ) -> Result<(), Diagnostic> {
        if source.size != target.size {
            return Err(Diagnostic::new("type mismatch"));
        }
        if storage_ranges_overlap(source, target) {
            let temp = self.alloc_var(source.size);
            self.emit_copy_storage_bytes(source, temp);
            self.emit_copy_storage_bytes(temp, target);
        } else {
            self.emit_copy_storage_bytes(source, target);
        }
        Ok(())
    }

    fn emit_copy_storage_bytes(&mut self, source: Variable, target: Variable) {
        for offset in 0..source.size {
            self.line(&format!("    ld a, ({:06X}h)", source.addr + offset as u32));
            self.line(&format!("    ld ({:06X}h), a", target.addr + offset as u32));
        }
    }

    fn emit_copy_pointed_storage_into(
        &mut self,
        ptr: &Expr,
        variable: Variable,
    ) -> Result<(), Diagnostic> {
        let temp = self.alloc_var(variable.size);
        self.emit_expr_to_hl(ptr, ValueWidth::U24)?;
        self.emit_load_pointed_width_into(temp);
        self.emit_copy_storage_bytes(temp, variable);
        Ok(())
    }

    fn expr_storage_variable(&self, expr: &Expr) -> Result<Option<Variable>, Diagnostic> {
        match expr {
            Expr::Ident(name) => Ok(self.variable_opt(name)),
            Expr::Field { base, field } => {
                if let Some(variable) = self.dotted_variable(base, field) {
                    return Ok(Some(variable));
                }
                if self.variable_opt(base).is_some() {
                    self.field_variable(base, field).map(Some)
                } else {
                    Ok(None)
                }
            }
            Expr::Index { name, index } => self.const_array_element_variable(name, index),
            Expr::Access(path) => {
                let path = self.canonical_access_path(path);
                if path.segments.is_empty() {
                    return Ok(self.variable_opt(&path.root));
                }
                if self.variable_opt(&path.root).is_some() {
                    self.const_access_variable(&path)
                } else {
                    Ok(None)
                }
            }
            _ => Ok(None),
        }
    }

    fn const_array_element_variable(
        &self,
        name: &str,
        index: &Expr,
    ) -> Result<Option<Variable>, Diagnostic> {
        self.validate_array_index_type(index)?;
        let (array, element_size, len) = self.array_info(name)?;
        let index_value = match self.symbols.eval_i64(index) {
            Ok(value) => value,
            Err(_) => return Ok(None),
        };
        if index_value < 0 || index_value as u32 >= len {
            return Err(Diagnostic::new(format!(
                "array index {index_value} is out of bounds for `{name}` length {len}"
            )));
        }
        let element_type = self.array_element_type(name)?;
        self.symbols
            .storage_at(
                array.addr + index_value as u32 * element_size as u32,
                &element_type,
            )
            .map(Some)
    }

    fn array_info(&self, name: &str) -> Result<(Variable, u32, u32), Diagnostic> {
        let array = self.variable(name)?;
        let element_size = array
            .element_size
            .ok_or_else(|| Diagnostic::new(format!("`{name}` is not an array")))?;
        let len = array
            .len
            .ok_or_else(|| Diagnostic::new(format!("array `{name}` is missing length")))?;
        Ok((array, element_size, len))
    }

    fn array_element_width(&self, name: &str) -> Result<ValueWidth, Diagnostic> {
        let (_, element_size, _) = self.array_info(name)?;
        scalar_var(0, element_size).width()
    }

    fn emit_variable_address(&mut self, name: &str) -> Result<(), Diagnostic> {
        let variable = self.variable(name)?;
        self.line(&format!("    ld hl, {:06X}h", variable.addr));
        Ok(())
    }

    fn emit_field_address(&mut self, base: &str, field: &str) -> Result<(), Diagnostic> {
        let variable = self.field_variable(base, field)?;
        self.line(&format!("    ld hl, {:06X}h", variable.addr));
        Ok(())
    }

    fn struct_type_name(&self, ty: &Type) -> Result<String, Diagnostic> {
        match self.symbols.resolved_type(ty)? {
            Type::Named(name) if self.symbols.structs.contains_key(&name) => Ok(name),
            other => Err(Diagnostic::new(format!(
                "type `{other:?}` is not a struct type"
            ))),
        }
    }

    fn field_variable(&self, base: &str, field: &str) -> Result<Variable, Diagnostic> {
        if let Some(variable) = self.dotted_variable(base, field) {
            return Ok(variable);
        }
        let base_variable = self.variable(base)?;
        let base_type = self
            .variable_type(base)
            .ok_or_else(|| Diagnostic::new(format!("unknown variable `{base}`")))?;
        let struct_name = self.struct_type_name(base_type)?;
        let layout = self
            .symbols
            .structs
            .get(&struct_name)
            .ok_or_else(|| Diagnostic::new(format!("unknown struct `{struct_name}`")))?;
        let field = layout.fields.get(field).ok_or_else(|| {
            Diagnostic::new(format!("struct `{struct_name}` has no field `{field}`"))
        })?;
        self.symbols
            .storage_at(base_variable.addr + field.offset, &field.ty)
    }

    fn field_type(&self, base: &str, field: &str) -> Result<Type, Diagnostic> {
        let key = format!("{base}.{field}");
        if let Some(ty) = self.named_value_type(&key) {
            return Ok(ty.clone());
        }
        let base_type = self
            .variable_type(base)
            .ok_or_else(|| Diagnostic::new(format!("unknown variable `{base}`")))?;
        let struct_name = self.struct_type_name(base_type)?;
        let layout = self
            .symbols
            .structs
            .get(&struct_name)
            .ok_or_else(|| Diagnostic::new(format!("unknown struct `{struct_name}`")))?;
        layout
            .fields
            .get(field)
            .map(|field| field.ty.clone())
            .ok_or_else(|| {
                Diagnostic::new(format!("struct `{struct_name}` has no field `{field}`"))
            })
    }

    fn access_type(&self, path: &AccessPath) -> Result<Type, Diagnostic> {
        let mut ty = self
            .variable_type(&path.root)
            .ok_or_else(|| Diagnostic::new(format!("unknown variable `{}`", path.root)))?
            .clone();
        for segment in &path.segments {
            ty = match segment {
                AccessSegment::Field(field) => {
                    let struct_name = self.struct_type_name(&ty)?;
                    let layout = self.symbols.structs.get(&struct_name).ok_or_else(|| {
                        Diagnostic::new(format!("unknown struct `{struct_name}`"))
                    })?;
                    layout
                        .fields
                        .get(field)
                        .map(|field| field.ty.clone())
                        .ok_or_else(|| {
                            Diagnostic::new(format!(
                                "struct `{struct_name}` has no field `{field}`"
                            ))
                        })?
                }
                AccessSegment::Index(index) => {
                    self.validate_array_index_type(index)?;
                    match self.symbols.resolved_type(&ty)? {
                        Type::Array { element, len } => {
                            self.validate_const_access_index_bounds(
                                index,
                                &len,
                                &access_path_summary(path),
                            )?;
                            *element
                        }
                        _ => {
                            return Err(Diagnostic::new(format!(
                                "value `{}` is not an array",
                                access_path_summary(path)
                            )));
                        }
                    }
                }
            };
        }
        Ok(ty)
    }

    fn const_access_variable(&self, path: &AccessPath) -> Result<Option<Variable>, Diagnostic> {
        let mut variable = self.variable(&path.root)?;
        let mut ty = self
            .variable_type(&path.root)
            .ok_or_else(|| Diagnostic::new(format!("unknown variable `{}`", path.root)))?
            .clone();

        for segment in &path.segments {
            match segment {
                AccessSegment::Field(field) => {
                    let struct_name = self.struct_type_name(&ty)?;
                    let layout = self.symbols.structs.get(&struct_name).ok_or_else(|| {
                        Diagnostic::new(format!("unknown struct `{struct_name}`"))
                    })?;
                    let field_info = layout.fields.get(field).ok_or_else(|| {
                        Diagnostic::new(format!("struct `{struct_name}` has no field `{field}`"))
                    })?;
                    variable = self
                        .symbols
                        .storage_at(variable.addr + field_info.offset, &field_info.ty)?;
                    ty = field_info.ty.clone();
                }
                AccessSegment::Index(index) => {
                    self.validate_array_index_type(index)?;
                    let index_value = match self.symbols.eval_i64(index) {
                        Ok(value) => value,
                        Err(_) => return Ok(None),
                    };
                    let Type::Array { element, len } = self.symbols.resolved_type(&ty)? else {
                        return Err(Diagnostic::new(format!(
                            "value `{}` is not an array",
                            access_path_summary(path)
                        )));
                    };
                    let len = self.symbols.array_len(&len)?;
                    if index_value < 0 || index_value as u32 >= len {
                        return Err(Diagnostic::new(format!(
                            "array index {index_value} is out of bounds for `{}` length {len}",
                            access_path_summary(path)
                        )));
                    }
                    let element_size = self.symbols.type_size(&element)?;
                    variable = self.symbols.storage_at(
                        variable.addr + index_value as u32 * element_size as u32,
                        &element,
                    )?;
                    ty = *element;
                }
            }
        }

        Ok(Some(variable))
    }

    fn emit_access_address(&mut self, path: &AccessPath) -> Result<(), Diagnostic> {
        let path = self.canonical_access_path(path);
        let path = &path;
        if path.segments.is_empty()
            && (self.named_value_type(&path.root).is_some()
                || self.symbols.embed_property_value(&path.root).is_some())
        {
            let value = self.value_for_width(&Expr::Ident(path.root.clone()), ValueWidth::U24)?;
            self.line(&format!("    ld hl, {value:06X}h"));
            return Ok(());
        }
        if let Some(variable) = self.const_access_variable(path)? {
            self.line(&format!("    ld hl, {:06X}h", variable.addr));
            return Ok(());
        }

        let root = self.variable(&path.root)?;
        let mut ty = self
            .variable_type(&path.root)
            .ok_or_else(|| Diagnostic::new(format!("unknown variable `{}`", path.root)))?
            .clone();
        self.line(&format!("    ld hl, {:06X}h", root.addr));

        for segment in &path.segments {
            match segment {
                AccessSegment::Field(field) => {
                    let struct_name = self.struct_type_name(&ty)?;
                    let layout = self.symbols.structs.get(&struct_name).ok_or_else(|| {
                        Diagnostic::new(format!("unknown struct `{struct_name}`"))
                    })?;
                    let field_info = layout.fields.get(field).ok_or_else(|| {
                        Diagnostic::new(format!("struct `{struct_name}` has no field `{field}`"))
                    })?;
                    let offset = field_info.offset;
                    let field_ty = field_info.ty.clone();
                    self.emit_add_hl_const(offset);
                    ty = field_ty;
                }
                AccessSegment::Index(index) => {
                    self.validate_array_index_type(index)?;
                    let Type::Array { element, len } = self.symbols.resolved_type(&ty)? else {
                        return Err(Diagnostic::new(format!(
                            "value `{}` is not an array",
                            access_path_summary(path)
                        )));
                    };
                    self.validate_const_access_index_bounds(
                        index,
                        &len,
                        &access_path_summary(path),
                    )?;
                    let element_size = self.symbols.type_size(&element)?;
                    let base_addr = self.alloc_var(ValueWidth::U24.bytes());
                    self.emit_store_hl(base_addr);
                    self.emit_expr_to_hl(index, ValueWidth::U24)?;
                    self.emit_scale_hl_by(element_size);
                    self.line("    push hl");
                    self.emit_load_hl(base_addr);
                    self.line("    pop bc");
                    self.line("    add hl, bc");
                    ty = *element;
                }
            }
        }
        Ok(())
    }

    fn emit_add_hl_const(&mut self, value: u32) {
        if value == 0 {
            return;
        }
        self.line("    push hl");
        self.line(&format!("    ld bc, {:06X}h", value & 0xFF_FFFF));
        self.line("    pop hl");
        self.line("    add hl, bc");
    }

    fn emit_scale_hl_by(&mut self, factor: u32) {
        match factor {
            0 | 1 => {}
            2 => self.line("    add hl, hl"),
            3 => {
                self.line("    push hl");
                self.line("    add hl, hl");
                self.line("    pop bc");
                self.line("    add hl, bc");
            }
            _ => {
                let index_value = self.alloc_var(ValueWidth::U24.bytes());
                self.emit_store_hl(index_value);
                for _ in 1..factor {
                    self.line("    push hl");
                    self.emit_load_hl(index_value);
                    self.line("    pop bc");
                    self.line("    add hl, bc");
                }
            }
        }
    }

    fn array_element_type(&self, name: &str) -> Result<Type, Diagnostic> {
        let Some(ty) = self.variable_type(name) else {
            return Err(Diagnostic::new(format!("unknown array `{name}`")));
        };
        match self.symbols.resolved_type(ty)? {
            Type::Array { element, .. } => Ok(*element),
            _ => Err(Diagnostic::new(format!("`{name}` is not an array"))),
        }
    }

    fn validate_array_index_type(&self, index: &Expr) -> Result<(), Diagnostic> {
        if expr_is_untyped_literal(index) {
            let value = self.symbols.eval_i64(index)?;
            if !(0..=0xFF_FFFF).contains(&value) {
                return Err(Diagnostic::new(format!(
                    "array index value {value} is outside u24 range"
                )));
            }
            return Ok(());
        }

        let ty = self.symbols.resolved_type(&self.expr_type(index)?)?;
        if matches!(&ty, Type::Named(name) if matches!(name.as_str(), "u8" | "u16" | "u24")) {
            Ok(())
        } else {
            Err(Diagnostic::new(format!(
                "array index type `{}` is not supported; use u8, u16, or u24",
                type_display(&ty)
            )))
        }
    }

    fn validate_const_access_index_bounds(
        &self,
        index: &Expr,
        len: &Expr,
        path: &str,
    ) -> Result<(), Diagnostic> {
        let len = self.symbols.array_len(len)?;
        if let Ok(index_value) = self.symbols.eval_i64(index) {
            if index_value < 0 || index_value as u32 >= len {
                return Err(Diagnostic::new(format!(
                    "array index {index_value} is out of bounds for `{path}` length {len}",
                )));
            }
        }
        Ok(())
    }

    fn pointer_pointee_size(&self, expr: &Expr) -> Result<Option<u32>, Diagnostic> {
        match self.expr_type(expr) {
            Ok(ty) => match self.symbols.resolved_type(&ty)? {
                Type::Ptr(inner) => Ok(Some(self.symbols.type_size(&inner)?)),
                _ => Ok(None),
            },
            Err(_) => Ok(None),
        }
    }

    fn emit_array_element_address(&mut self, name: &str, index: &Expr) -> Result<(), Diagnostic> {
        self.validate_array_index_type(index)?;
        if let Some(element) = self.const_array_element_variable(name, index)? {
            self.line(&format!("    ld hl, {:06X}h", element.addr));
            return Ok(());
        }

        let (array, element_size, _) = self.array_info(name)?;
        self.emit_expr_to_hl(index, ValueWidth::U24)?;
        match element_size {
            1 => {}
            2 => self.line("    add hl, hl"),
            3 => {
                self.line("    push hl");
                self.line("    add hl, hl");
                self.line("    pop bc");
                self.line("    add hl, bc");
            }
            _ => {
                let index_value = self.alloc_var(ValueWidth::U24.bytes());
                self.emit_store_hl(index_value);
                for _ in 1..element_size {
                    self.line("    push hl");
                    self.emit_load_hl(index_value);
                    self.line("    pop bc");
                    self.line("    add hl, bc");
                }
            }
        }
        self.line("    push hl");
        self.line(&format!("    ld hl, {:06X}h", array.addr));
        self.line("    pop bc");
        self.line("    add hl, bc");
        Ok(())
    }

    fn emit_load_indexed_element_to_a(
        &mut self,
        name: &str,
        index: &Expr,
    ) -> Result<(), Diagnostic> {
        let width = self.array_element_width(name)?;
        if width != ValueWidth::U8 {
            return Err(Diagnostic::new(format!(
                "array `{name}` element is not u8-sized"
            )));
        }
        self.emit_array_element_address(name, index)?;
        self.line("    ld a, (hl)");
        Ok(())
    }

    fn emit_load_indexed_element_to_hl(
        &mut self,
        name: &str,
        index: &Expr,
    ) -> Result<(), Diagnostic> {
        let (_, element_size, _) = self.array_info(name)?;
        if let Some(element) = self.const_array_element_variable(name, index)? {
            self.emit_load_width(element);
            return Ok(());
        }

        self.emit_array_element_address(name, index)?;
        match element_size {
            1 => {
                self.line("    ld a, (hl)");
                self.line("    ld hl, 000000h");
                self.line("    ld l, a");
            }
            2 | 3 => {
                let result = self.alloc_var(element_size);
                for offset in 0..element_size {
                    if offset != 0 {
                        self.line("    inc hl");
                    }
                    self.line("    ld a, (hl)");
                    self.line(&format!("    ld ({:06X}h), a", result.addr + offset as u32));
                }
                self.emit_load_width(result);
            }
            _ => unreachable!("unsupported array element size"),
        }
        Ok(())
    }

    fn emit_index_assignment(
        &mut self,
        name: &str,
        index: &Expr,
        op: AssignOp,
        value: &Expr,
    ) -> Result<(), Diagnostic> {
        let ty = self.array_element_type(name)?;
        if let Some(element) = self.const_array_element_variable(name, index)? {
            if op == AssignOp::Set {
                self.validate_expr_assignable_to_type(value, &ty)?;
                self.emit_storage_initializer(element, &ty, value)?;
                return Ok(());
            }
            element.width()?;
            let signed = self.type_is_signed(&ty)?;
            self.emit_assignment_value(element, op, value, signed)?;
            self.emit_store_width(element);
            return Ok(());
        }

        let (_, element_size, _) = self.array_info(name)?;
        let addr = self.alloc_var(ValueWidth::U24.bytes());
        self.emit_array_element_address(name, index)?;
        self.emit_store_hl(addr);

        let element = self.symbols.storage_at(0, &ty)?;
        if op != AssignOp::Set {
            element.width()?;
            let current = self.alloc_var(element_size);
            self.emit_load_hl(addr);
            self.emit_load_pointed_width_into(current);
            let stored = self.alloc_var(element_size);
            let signed = self.type_is_signed(&ty)?;
            self.emit_assignment_value(current, op, value, signed)?;
            self.emit_store_width(stored);
            self.emit_load_hl(addr);
            self.emit_store_var_to_pointed_width(stored);
            return Ok(());
        }

        if op == AssignOp::Set {
            self.validate_expr_assignable_to_type(value, &ty)?;
        }
        let stored = self.alloc_storage(&ty)?;
        self.emit_storage_initializer(stored, &ty, value)?;
        self.emit_load_hl(addr);
        self.emit_store_var_to_pointed_width(stored);
        Ok(())
    }

    fn emit_access_assignment(
        &mut self,
        path: &AccessPath,
        op: AssignOp,
        value: &Expr,
    ) -> Result<(), Diagnostic> {
        let path = self.canonical_access_path(path);
        let ty = self.access_type(&path)?;
        if let Some(variable) = self.const_access_variable(&path)? {
            if op == AssignOp::Set {
                self.validate_expr_assignable_to_type(value, &ty)?;
                self.emit_storage_initializer(variable, &ty, value)?;
                return Ok(());
            }
            variable.width()?;
            let signed = self.type_is_signed(&ty)?;
            self.emit_assignment_value(variable, op, value, signed)?;
            self.emit_store_width(variable);
            return Ok(());
        }

        let size = self.symbols.type_size(&ty)?;
        let addr = self.alloc_var(ValueWidth::U24.bytes());
        self.emit_access_address(&path)?;
        self.emit_store_hl(addr);

        if op != AssignOp::Set {
            let current = self.alloc_var(size);
            current.width()?;
            self.emit_load_hl(addr);
            self.emit_load_pointed_width_into(current);
            let stored = self.alloc_var(size);
            let signed = self.type_is_signed(&ty)?;
            self.emit_assignment_value(current, op, value, signed)?;
            self.emit_store_width(stored);
            self.emit_load_hl(addr);
            self.emit_store_var_to_pointed_width(stored);
            return Ok(());
        }

        self.validate_expr_assignable_to_type(value, &ty)?;
        let stored = self.alloc_storage(&ty)?;
        self.emit_storage_initializer(stored, &ty, value)?;
        self.emit_load_hl(addr);
        self.emit_store_var_to_pointed_width(stored);
        Ok(())
    }

    fn u8(&self, expr: &Expr) -> Result<u8, Diagnostic> {
        let value = self.symbols.eval_i64(expr)?;
        if !(0..=0xFF).contains(&value) {
            return Err(Diagnostic::new(format!(
                "value {value} is outside u8 range"
            )));
        }
        Ok(value as u8)
    }

    fn u16(&self, expr: &Expr) -> Result<u16, Diagnostic> {
        let value = self.symbols.eval_i64(expr)?;
        if !(0..=0xFFFF).contains(&value) {
            return Err(Diagnostic::new(format!(
                "value {value} is outside u16 range"
            )));
        }
        Ok(value as u16)
    }

    fn u24(&self, expr: &Expr) -> Result<u32, Diagnostic> {
        let value = self.symbols.eval_i64(expr)?;
        if !(0..=0xFF_FFFF).contains(&value) {
            return Err(Diagnostic::new(format!(
                "value {value} is outside u24 range"
            )));
        }
        Ok(value as u32)
    }

    fn value_for_width(&self, expr: &Expr, width: ValueWidth) -> Result<u32, Diagnostic> {
        match width {
            ValueWidth::U8 => self.u8(expr).map(u32::from),
            ValueWidth::U16 => self.u16(expr).map(u32::from),
            ValueWidth::U24 => self.u24(expr),
        }
    }

    fn eval_i64_with_local_constants(&self, expr: &Expr) -> Result<i64, Diagnostic> {
        match expr {
            Expr::Ident(name) => self
                .local_constant(name)
                .map(|constant| constant.value)
                .map(Ok)
                .unwrap_or_else(|| self.symbols.eval_i64(expr)),
            Expr::Unary { op, expr } => {
                let value = self.eval_i64_with_local_constants(expr)?;
                Ok(match op {
                    UnaryOp::Neg => value.wrapping_neg(),
                    UnaryOp::BitNot => !value,
                    UnaryOp::Not => i64::from(value == 0),
                })
            }
            Expr::Binary { left, op, right } => {
                let left_signed = self.expr_is_signed(left)?;
                let left_scale = self.pointer_pointee_size(left)?;
                let right_scale = self.pointer_pointee_size(right)?;
                let left = self.eval_i64_with_local_constants(left)?;
                let right = self.eval_i64_with_local_constants(right)?;
                Ok(match op {
                    BinaryOp::Mul => left.wrapping_mul(right),
                    BinaryOp::Div => trunc_div_or_zero(left, right),
                    BinaryOp::Mod => trunc_mod_or_zero(left, right),
                    BinaryOp::Add => match (left_scale, right_scale) {
                        (Some(scale), None) => left.wrapping_add(right.wrapping_mul(scale.into())),
                        (None, Some(scale)) => left.wrapping_mul(scale.into()).wrapping_add(right),
                        _ => left.wrapping_add(right),
                    },
                    BinaryOp::Sub => match (left_scale, right_scale) {
                        (Some(scale), None) => left.wrapping_sub(right.wrapping_mul(scale.into())),
                        _ => left.wrapping_sub(right),
                    },
                    BinaryOp::Shl => const_shl_or_zero(left, right),
                    BinaryOp::Shr => const_shr_or_zero(left, right, left_signed),
                    BinaryOp::Lt => i64::from(left < right),
                    BinaryOp::Le => i64::from(left <= right),
                    BinaryOp::Gt => i64::from(left > right),
                    BinaryOp::Ge => i64::from(left >= right),
                    BinaryOp::Eq => i64::from(left == right),
                    BinaryOp::Ne => i64::from(left != right),
                    BinaryOp::BitAnd => left & right,
                    BinaryOp::BitXor => left ^ right,
                    BinaryOp::BitOr => left | right,
                    BinaryOp::And => i64::from(left != 0 && right != 0),
                    BinaryOp::Or => i64::from(left != 0 || right != 0),
                })
            }
            Expr::Cast { expr, ty } => {
                let value = self.eval_i64_with_local_constants(expr)?;
                self.symbols.const_cast_value(value, ty)
            }
            _ => self.symbols.eval_i64(expr),
        }
    }

    fn local_constant_value_for_width(
        &self,
        name: &str,
        width: ValueWidth,
    ) -> Result<Option<u32>, Diagnostic> {
        let Some(constant) = self.local_constant(name) else {
            return Ok(None);
        };
        if self.symbols.type_width(&constant.ty)? != width {
            return Ok(None);
        }
        let value = self.value_for_type(constant.value, &constant.ty, width)?;
        Ok(Some(value))
    }

    fn value_for_type(&self, value: i64, ty: &Type, width: ValueWidth) -> Result<u32, Diagnostic> {
        let resolved = self.symbols.resolved_type(ty)?;
        self.symbols.validate_value_for_type(value, &resolved)?;
        let bits = u32::from(width.bytes()) * 8;
        let mask = (1_i128 << bits) - 1;
        Ok(((value as i128) & mask) as u32)
    }

    fn type_is_signed(&self, ty: &Type) -> Result<bool, Diagnostic> {
        Ok(type_is_signed(&self.symbols.resolved_type(ty)?))
    }

    fn expr_is_signed(&self, expr: &Expr) -> Result<bool, Diagnostic> {
        self.type_is_signed(&self.expr_type(expr)?)
    }

    fn expr_type(&self, expr: &Expr) -> Result<Type, Diagnostic> {
        match expr {
            Expr::Ident(name) => self
                .named_value_type(name)
                .cloned()
                .ok_or_else(|| Diagnostic::new(format!("unknown value `{name}`"))),
            Expr::Int(value) => {
                if (0..=0xFF).contains(value) {
                    Ok(Type::Named("u8".to_owned()))
                } else if (0..=0xFFFF).contains(value) {
                    Ok(Type::Named("u16".to_owned()))
                } else {
                    Ok(Type::Named("u24".to_owned()))
                }
            }
            Expr::TypedInt(_, ty) => Ok(ty.clone()),
            Expr::Char(_) | Expr::In(_) => Ok(Type::Named("u8".to_owned())),
            Expr::Bool(_) => Ok(Type::Named("bool".to_owned())),
            Expr::String(_) => Ok(Type::Ptr(Box::new(Type::Named("u8".to_owned())))),
            Expr::Array(_) => Err(Diagnostic::new("array literal does not have scalar type")),
            Expr::Index { name, .. } => self.array_element_type(name),
            Expr::Field { base, field } => self
                .named_value_type(&format!("{base}.{field}"))
                .cloned()
                .map(Ok)
                .unwrap_or_else(|| self.field_type(base, field)),
            Expr::Access(path) => {
                let path = self.canonical_access_path(path);
                if path.segments.is_empty() {
                    if let Some(ty) = self.named_value_type(&path.root) {
                        return Ok(ty.clone());
                    }
                    if let Some(ty) = self.embed_property_type(&path.root) {
                        return Ok(ty);
                    }
                }
                self.access_type(&path)
            }
            Expr::AddressOfIndex { name, .. } => {
                Ok(Type::Ptr(Box::new(self.array_element_type(name)?)))
            }
            Expr::AddressOfField { base, field } => {
                Ok(Type::Ptr(Box::new(self.field_type(base, field)?)))
            }
            Expr::AddressOfAccess(path) => {
                let path = self.canonical_access_path(path);
                Ok(Type::Ptr(Box::new(self.access_type(&path)?)))
            }
            Expr::AddressOf(name) => {
                let Some(ty) = self.variable_type(name) else {
                    return Err(Diagnostic::new(format!("unknown variable `{name}`")));
                };
                Ok(Type::Ptr(Box::new(self.symbols.resolved_type(ty)?)))
            }
            Expr::Deref(ptr) => match self.symbols.resolved_type(&self.expr_type(ptr)?)? {
                Type::Ptr(inner) => Ok(*inner),
                Type::Named(name) if name == "ptr24" => Err(Diagnostic::new(
                    "raw ptr24 dereference requires an explicit typed pointer cast",
                )),
                other => Err(Diagnostic::new(format!(
                    "cannot dereference non-pointer expression of type `{other:?}`"
                ))),
            },
            Expr::StructInit { ty, .. } => Ok(Type::Named(ty.clone())),
            Expr::Cast { ty, .. } => Ok(ty.clone()),
            Expr::Call { path, .. }
                if matches!(path_text(path).as_str(), "mem.peek8" | "ezra.mem.peek8") =>
            {
                Ok(Type::Named("u8".to_owned()))
            }
            Expr::Call { path, .. } => self.call_return_type(path),
            Expr::Unary { expr, op } => match op {
                UnaryOp::Not => {
                    self.ensure_expr_is_bool(expr, "logical operand")?;
                    Ok(Type::Named("bool".to_owned()))
                }
                UnaryOp::Neg | UnaryOp::BitNot => self.expr_type(expr),
            },
            Expr::Binary { left, op, right } => {
                if is_comparison(*op) || matches!(op, BinaryOp::And | BinaryOp::Or) {
                    Ok(Type::Named("bool".to_owned()))
                } else if matches!(op, BinaryOp::Add | BinaryOp::Sub)
                    && self.pointer_pointee_size(left)?.is_some()
                {
                    self.expr_type(left)
                } else if *op == BinaryOp::Add && self.pointer_pointee_size(right)?.is_some() {
                    self.expr_type(right)
                } else if self.expr_width(left)? >= self.expr_width(right)? {
                    self.expr_type(left)
                } else {
                    self.expr_type(right)
                }
            }
        }
    }

    fn expr_width(&self, expr: &Expr) -> Result<ValueWidth, Diagnostic> {
        match expr {
            Expr::Ident(name) => {
                if let Some(variable) = self.variable_opt(name) {
                    variable.width()
                } else if let Some(ty) = self.named_value_type(name) {
                    self.symbols.type_width(ty)
                } else {
                    let value = self.symbols.eval_i64(expr)?;
                    if (0..=0xFF).contains(&value) {
                        Ok(ValueWidth::U8)
                    } else if (0..=0xFFFF).contains(&value) {
                        Ok(ValueWidth::U16)
                    } else {
                        Ok(ValueWidth::U24)
                    }
                }
            }
            Expr::Int(value) => {
                if (0..=0xFF).contains(value) {
                    Ok(ValueWidth::U8)
                } else if (0..=0xFFFF).contains(value) {
                    Ok(ValueWidth::U16)
                } else {
                    Ok(ValueWidth::U24)
                }
            }
            Expr::TypedInt(_, ty) => self.symbols.type_width(ty),
            Expr::Char(_) | Expr::Bool(_) | Expr::In(_) => Ok(ValueWidth::U8),
            Expr::String(_) => Ok(ValueWidth::U24),
            Expr::Array(_) => Err(Diagnostic::new("array literal does not have scalar width")),
            Expr::StructInit { ty, .. } => Err(Diagnostic::new(format!(
                "struct `{ty}` literal does not have scalar width"
            ))),
            Expr::Index { name, .. } => self.array_element_width(name),
            Expr::Field { base, field } => {
                let key = format!("{base}.{field}");
                if let Some(ty) = self.named_value_type(&key) {
                    self.symbols.type_width(ty)
                } else {
                    self.field_variable(base, field)?.width()
                }
            }
            Expr::Access(path) => {
                let path = self.canonical_access_path(path);
                if path.segments.is_empty() {
                    if let Some(ty) = self.named_value_type(&path.root) {
                        return self.symbols.type_width(ty);
                    }
                    if self.symbols.embed_property_value(&path.root).is_some() {
                        return Ok(ValueWidth::U24);
                    }
                }
                if let Some(variable) = self.const_access_variable(&path)? {
                    variable.width()
                } else {
                    self.symbols.type_width(&self.access_type(&path)?)
                }
            }
            Expr::AddressOfIndex { .. } => Ok(ValueWidth::U24),
            Expr::AddressOfField { .. } => Ok(ValueWidth::U24),
            Expr::AddressOfAccess(_) => Ok(ValueWidth::U24),
            Expr::AddressOf(_) => Ok(ValueWidth::U24),
            Expr::Deref(ptr) => match self.symbols.resolved_type(&self.expr_type(ptr)?)? {
                Type::Ptr(inner) => self.symbols.type_width(&inner),
                Type::Named(name) if name == "ptr24" => Err(Diagnostic::new(
                    "raw ptr24 dereference requires an explicit typed pointer cast",
                )),
                other => Err(Diagnostic::new(format!(
                    "cannot dereference non-pointer expression of type `{other:?}`"
                ))),
            },
            Expr::Cast { ty, .. } => self.symbols.type_width(ty),
            Expr::Call { path, .. }
                if matches!(path_text(path).as_str(), "mem.peek8" | "ezra.mem.peek8") =>
            {
                Ok(ValueWidth::U8)
            }
            Expr::Call { path, .. } => self.call_return_width(path),
            Expr::Unary { expr, op } => match op {
                UnaryOp::Not => Ok(ValueWidth::U8),
                UnaryOp::Neg | UnaryOp::BitNot => self.expr_width(expr),
            },
            Expr::Binary { left, op, right } => {
                if is_comparison(*op) || matches!(op, BinaryOp::And | BinaryOp::Or) {
                    Ok(ValueWidth::U8)
                } else {
                    Ok(self.expr_width(left)?.max(self.expr_width(right)?))
                }
            }
        }
    }

    fn call_return_type(&self, path: &[String]) -> Result<Type, Diagnostic> {
        let name = path_text(path);
        let sig = self
            .symbols
            .functions
            .get(&name)
            .ok_or_else(|| Diagnostic::new(format!("unknown function `{name}`")))?;
        sig.return_type
            .clone()
            .ok_or_else(|| Diagnostic::new(format!("function `{name}` does not return a value")))
    }

    fn call_return_width(&self, path: &[String]) -> Result<ValueWidth, Diagnostic> {
        let name = path_text(path);
        let sig = self
            .symbols
            .functions
            .get(&name)
            .ok_or_else(|| Diagnostic::new(format!("unknown function `{name}`")))?;
        if sig.return_type.is_none() {
            return Err(Diagnostic::new(format!(
                "function `{name}` does not return a value"
            )));
        }
        Ok(sig.return_width)
    }

    fn maybe_const_shift_count(&self, expr: &Expr) -> Result<Option<u8>, Diagnostic> {
        match self.symbols.eval_i64(expr) {
            Ok(value) => self.validate_shift_count(value).map(Some),
            Err(_) => Ok(None),
        }
    }

    fn validate_shift_count(&self, value: i64) -> Result<u8, Diagnostic> {
        if !(0..=u8::MAX as i64).contains(&value) {
            return Err(Diagnostic::new(format!(
                "shift count {value} is outside supported range 0..=255"
            )));
        }
        Ok(value as u8)
    }

    fn binary_operands_are_signed(&self, left: &Expr, right: &Expr) -> Result<bool, Diagnostic> {
        Ok(
            type_is_signed(&self.symbols.resolved_type(&self.expr_type(left)?)?)
                || type_is_signed(&self.symbols.resolved_type(&self.expr_type(right)?)?),
        )
    }

    fn ensure_binary_arithmetic_operands_compatible(
        &self,
        left: &Expr,
        right: &Expr,
    ) -> Result<(), Diagnostic> {
        let left_type = self.symbols.resolved_type(&self.expr_type(left)?)?;
        let right_type = self.symbols.resolved_type(&self.expr_type(right)?)?;
        if type_is_bool(&left_type) || type_is_bool(&right_type) {
            return Err(Diagnostic::new("type mismatch"));
        }
        let left_is_literal = expr_is_untyped_literal(left);
        let right_is_literal = expr_is_untyped_literal(right);
        if left_is_literal && right_is_literal {
            return Ok(());
        }

        if matches!(left_type, Type::Ptr(_)) || matches!(right_type, Type::Ptr(_)) {
            return Err(Diagnostic::new("type mismatch"));
        }

        if left_is_literal {
            let value = self.symbols.eval_i64(left)?;
            return self.symbols.validate_value_for_type(value, &right_type);
        }
        if right_is_literal {
            let value = self.symbols.eval_i64(right)?;
            return self.symbols.validate_value_for_type(value, &left_type);
        }

        if type_is_signed(&left_type) != type_is_signed(&right_type) {
            return Err(Diagnostic::new("signed/unsigned mix without cast"));
        }
        if self.symbols.type_width(&left_type)? != self.symbols.type_width(&right_type)? {
            return Err(Diagnostic::new(
                "arithmetic operands must have same width without cast",
            ));
        }
        Ok(())
    }

    fn validate_expr_assignable_to_type(
        &self,
        expr: &Expr,
        target: &Type,
    ) -> Result<(), Diagnostic> {
        self.validate_typed_literal_ranges(expr)?;
        if let Expr::Array(values) = expr {
            let Type::Array { element, len } = self.symbols.resolved_type(target)? else {
                return Err(Diagnostic::new("type mismatch"));
            };
            let len = self.symbols.array_len(&len)?;
            if values.len() as u32 > len {
                return Err(Diagnostic::new(format!(
                    "array initializer has {} values but array length is {len}",
                    values.len()
                )));
            }
            for value in values {
                self.validate_expr_assignable_to_type(value, &element)?;
            }
            return Ok(());
        }
        if let Expr::Cast { ty, .. } = expr {
            self.symbols.type_width(ty)?;
            return self.validate_type_assignable_to_type(ty, target);
        }
        if expr_is_untyped_literal(expr) {
            if let Ok(value) = self.symbols.eval_i64(expr) {
                self.symbols.validate_value_for_type(value, target)?;
            }
            return Ok(());
        }

        let source_type = self.expr_type(expr)?;
        self.validate_type_assignable_to_type(&source_type, target)
    }

    fn validate_expr_is_ptr_u8(&self, expr: &Expr) -> Result<(), Diagnostic> {
        let ty = self.symbols.resolved_type(&self.expr_type(expr)?)?;
        if ty == ptr_u8_type() {
            Ok(())
        } else {
            Err(Diagnostic::new("type mismatch"))
        }
    }

    fn validate_expr_has_test_width(
        &self,
        expr: &Expr,
        width: ValueWidth,
        allow_bool: bool,
    ) -> Result<(), Diagnostic> {
        let ty = self.symbols.resolved_type(&self.expr_type(expr)?)?;
        if allow_bool && type_is_bool(&ty) {
            return Ok(());
        }
        if type_is_bool(&ty) || matches!(ty, Type::Ptr(_)) {
            return Err(Diagnostic::new("type mismatch"));
        }
        let actual = self.symbols.type_width(&ty)?;
        if actual == width {
            return Ok(());
        }
        if let Ok(value) = self.symbols.eval_i64(expr) {
            self.symbols
                .wrap_value_for_type(value, &width_unsigned_type(width))?;
            return Ok(());
        }
        if actual < width {
            return Err(Diagnostic::new("widening without cast"));
        }
        if actual > width {
            return Err(Diagnostic::new("narrowing without cast"));
        }
        Ok(())
    }

    fn validate_type_assignable_to_type(
        &self,
        source: &Type,
        target: &Type,
    ) -> Result<(), Diagnostic> {
        let source_type = self.symbols.resolved_type(source)?;
        let target_type = self.symbols.resolved_type(target)?;
        if source_type == target_type {
            return Ok(());
        }
        if let (
            Type::Array {
                element: source_element,
                len: source_len,
            },
            Type::Array {
                element: target_element,
                len: target_len,
            },
        ) = (&source_type, &target_type)
        {
            if self.symbols.array_len(source_len)? != self.symbols.array_len(target_len)? {
                return Err(Diagnostic::new("type mismatch"));
            }
            return self.validate_type_assignable_to_type(source_element, target_element);
        }
        if matches!(source_type, Type::Array { .. }) || matches!(target_type, Type::Array { .. }) {
            return Err(Diagnostic::new("type mismatch"));
        }
        if type_is_bool(&source_type) || type_is_bool(&target_type) {
            return Err(Diagnostic::new("type mismatch"));
        }
        if matches!(source_type, Type::Ptr(_)) || matches!(target_type, Type::Ptr(_)) {
            return Err(Diagnostic::new("type mismatch"));
        }

        let source_width = self.symbols.type_width(&source_type)?;
        let target_width = self.symbols.type_width(&target_type)?;
        if source_width < target_width {
            return Err(Diagnostic::new("widening without cast"));
        }
        if source_width > target_width {
            return Err(Diagnostic::new("narrowing without cast"));
        }
        if type_is_signed(&source_type) != type_is_signed(&target_type) {
            return Err(Diagnostic::new("signed/unsigned mix without cast"));
        }
        Err(Diagnostic::new("type mismatch"))
    }

    fn validate_typed_literal_ranges(&self, expr: &Expr) -> Result<(), Diagnostic> {
        match expr {
            Expr::TypedInt(value, ty) => self.symbols.validate_value_for_type(*value, ty),
            Expr::Unary {
                op: UnaryOp::Neg,
                expr,
            } => {
                if let Expr::TypedInt(value, ty) = expr.as_ref() {
                    let value = value.checked_neg().ok_or_else(|| {
                        Diagnostic::new(format!("value -{value} is outside i24 range"))
                    })?;
                    self.symbols.validate_value_for_type(value, ty)
                } else {
                    self.validate_typed_literal_ranges(expr)
                }
            }
            Expr::Unary { expr, .. } | Expr::Cast { expr, .. } | Expr::Deref(expr) => {
                self.validate_typed_literal_ranges(expr)
            }
            Expr::Binary { left, right, .. } => {
                self.validate_typed_literal_ranges(left)?;
                self.validate_typed_literal_ranges(right)
            }
            Expr::Index { index, .. } | Expr::AddressOfIndex { index, .. } => {
                self.validate_typed_literal_ranges(index)
            }
            Expr::Access(path) | Expr::AddressOfAccess(path) => {
                for segment in &path.segments {
                    if let AccessSegment::Index(index) = segment {
                        self.validate_typed_literal_ranges(index)?;
                    }
                }
                Ok(())
            }
            Expr::Array(values) => {
                for value in values {
                    self.validate_typed_literal_ranges(value)?;
                }
                Ok(())
            }
            Expr::StructInit { fields, .. } => {
                for (_, value) in fields {
                    self.validate_typed_literal_ranges(value)?;
                }
                Ok(())
            }
            Expr::Call { args, .. } => {
                for arg in args {
                    self.validate_typed_literal_ranges(arg)?;
                }
                Ok(())
            }
            Expr::Int(_)
            | Expr::Bool(_)
            | Expr::Char(_)
            | Expr::String(_)
            | Expr::Ident(_)
            | Expr::In(_)
            | Expr::Field { .. }
            | Expr::AddressOf(_)
            | Expr::AddressOfField { .. } => Ok(()),
        }
    }

    fn validate_expr_arithmetic_compatibility(&self, expr: &Expr) -> Result<(), Diagnostic> {
        self.validate_typed_literal_ranges(expr)?;
        match expr {
            Expr::Binary { left, op, right } => {
                self.validate_expr_arithmetic_compatibility(left)?;
                self.validate_expr_arithmetic_compatibility(right)?;
                if matches!(op, BinaryOp::And | BinaryOp::Or) {
                    self.ensure_expr_is_bool(left, "logical operand")?;
                    self.ensure_expr_is_bool(right, "logical operand")?;
                } else if is_comparison(*op) {
                    self.ensure_comparison_operands_compatible(left, *op, right)?;
                } else if matches!(op, BinaryOp::Add | BinaryOp::Sub)
                    && (self.pointer_pointee_size(left)?.is_some()
                        || self.pointer_pointee_size(right)?.is_some())
                {
                    self.ensure_pointer_arithmetic_expr_compatible(left, *op, right)?;
                } else if matches!(op, BinaryOp::Shl | BinaryOp::Shr) {
                    self.ensure_shift_operands_compatible(left, right)?;
                } else if matches!(
                    op,
                    BinaryOp::Add
                        | BinaryOp::Sub
                        | BinaryOp::Mul
                        | BinaryOp::Div
                        | BinaryOp::Mod
                        | BinaryOp::BitAnd
                        | BinaryOp::BitOr
                        | BinaryOp::BitXor
                ) {
                    self.ensure_binary_arithmetic_operands_compatible(left, right)?;
                }
            }
            Expr::Unary { expr, op } => {
                self.validate_expr_arithmetic_compatibility(expr)?;
                match op {
                    UnaryOp::Not => self.ensure_expr_is_bool(expr, "logical operand")?,
                    UnaryOp::Neg | UnaryOp::BitNot => {
                        let ty = self.symbols.resolved_type(&self.expr_type(expr)?)?;
                        validate_integer_unary_operand_type(&ty)?;
                    }
                }
            }
            Expr::Cast { expr, .. } | Expr::Deref(expr) => {
                self.validate_expr_arithmetic_compatibility(expr)?;
            }
            Expr::Index { index, .. } | Expr::AddressOfIndex { index, .. } => {
                self.validate_expr_arithmetic_compatibility(index)?;
            }
            Expr::Access(path) | Expr::AddressOfAccess(path) => {
                for segment in &path.segments {
                    if let AccessSegment::Index(index) = segment {
                        self.validate_expr_arithmetic_compatibility(index)?;
                    }
                }
            }
            Expr::Array(values) => {
                for value in values {
                    self.validate_expr_arithmetic_compatibility(value)?;
                }
            }
            Expr::StructInit { fields, .. } => {
                for (_, value) in fields {
                    self.validate_expr_arithmetic_compatibility(value)?;
                }
            }
            Expr::Call { args, .. } => {
                for arg in args {
                    self.validate_expr_arithmetic_compatibility(arg)?;
                }
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

    fn ensure_shift_operands_compatible(
        &self,
        left: &Expr,
        right: &Expr,
    ) -> Result<(), Diagnostic> {
        let left_type = self.symbols.resolved_type(&self.expr_type(left)?)?;
        validate_shift_operand_type(&left_type)?;
        self.ensure_shift_count_compatible(right)
    }

    fn ensure_shift_count_compatible(&self, count: &Expr) -> Result<(), Diagnostic> {
        let ty = self.symbols.resolved_type(&self.expr_type(count)?)?;
        validate_shift_count_integer_type(&ty)?;

        if let Ok(value) = self.eval_i64_with_local_constants(count) {
            self.validate_shift_count(value)?;
            return Ok(());
        }

        validate_runtime_shift_count_type(&ty)
    }

    fn ensure_pointer_arithmetic_expr_compatible(
        &self,
        left: &Expr,
        op: BinaryOp,
        right: &Expr,
    ) -> Result<(), Diagnostic> {
        let left_scale = self.pointer_pointee_size(left)?;
        let right_scale = self.pointer_pointee_size(right)?;
        match (op, left_scale, right_scale) {
            (BinaryOp::Add, Some(_), Some(_)) => Err(Diagnostic::new(
                "pointer arithmetic requires exactly one pointer operand",
            )),
            (BinaryOp::Add, Some(_), None) => self.ensure_pointer_offset_expr(right),
            (BinaryOp::Add, None, Some(_)) => self.ensure_pointer_offset_expr(left),
            (BinaryOp::Sub, Some(_), Some(_)) => Err(Diagnostic::new(
                "pointer subtraction between two pointers is not supported",
            )),
            (BinaryOp::Sub, Some(_), None) => self.ensure_pointer_offset_expr(right),
            (BinaryOp::Sub, None, Some(_)) => Err(Diagnostic::new(
                "cannot subtract a pointer from a non-pointer value",
            )),
            _ => Ok(()),
        }
    }

    fn ensure_comparison_operands_compatible(
        &self,
        left: &Expr,
        op: BinaryOp,
        right: &Expr,
    ) -> Result<(), Diagnostic> {
        let left_type = self.symbols.resolved_type(&self.expr_type(left)?)?;
        let right_type = self.symbols.resolved_type(&self.expr_type(right)?)?;
        if !type_is_bool(&left_type)
            && !type_is_bool(&right_type)
            && !matches!(left_type, Type::Ptr(_))
            && !matches!(right_type, Type::Ptr(_))
        {
            if expr_is_untyped_literal(left) && expr_is_untyped_literal(right) {
                return Ok(());
            }
            if expr_is_untyped_literal(left) {
                let value = self.symbols.eval_i64(left)?;
                return self.symbols.validate_value_for_type(value, &right_type);
            }
            if expr_is_untyped_literal(right) {
                let value = self.symbols.eval_i64(right)?;
                return self.symbols.validate_value_for_type(value, &left_type);
            }
        }
        validate_comparison_types(&left_type, op, &right_type, || {
            if expr_is_untyped_literal(left) || expr_is_untyped_literal(right) {
                None
            } else {
                Some((
                    self.symbols.type_width(&left_type).ok()?,
                    self.symbols.type_width(&right_type).ok()?,
                ))
            }
        })
    }

    fn ensure_expr_is_bool(&self, expr: &Expr, context: &str) -> Result<(), Diagnostic> {
        let ty = self.symbols.resolved_type(&self.expr_type(expr)?)?;
        if type_is_bool(&ty) {
            Ok(())
        } else {
            Err(Diagnostic::new(format!("{context} must be bool")))
        }
    }

    fn current_return_type(&self) -> &Type {
        self.return_type_stack
            .last()
            .and_then(|ty| ty.as_ref())
            .expect("function return type exists during value return emission")
    }

    fn current_function_requires_return_value(&self) -> bool {
        *self
            .return_value_stack
            .last()
            .expect("function return kind exists during emission")
    }

    fn current_function_name(&self) -> &str {
        self.function_name_stack
            .last()
            .expect("function name exists during emission")
    }

    fn current_function_uses_frame(&self) -> bool {
        self.function_frame_stack
            .last()
            .copied()
            .expect("function frame state exists during emission")
    }

    fn current_function_is_interrupt(&self) -> bool {
        self.function_interrupt_stack
            .last()
            .copied()
            .expect("function interrupt state exists during emission")
    }

    fn current_function_is_naked(&self) -> bool {
        self.function_naked_stack
            .last()
            .copied()
            .expect("function naked state exists during emission")
    }

    fn port(&self, name: &str) -> Result<u8, Diagnostic> {
        self.symbols
            .ports
            .get(name)
            .copied()
            .ok_or_else(|| Diagnostic::new(format!("unknown port `{name}`")))
    }

    fn variable(&self, name: &str) -> Result<Variable, Diagnostic> {
        self.variable_opt(name)
            .ok_or_else(|| Diagnostic::new(format!("unknown variable `{name}`")))
    }

    fn dotted_variable(&self, base: &str, field: &str) -> Option<Variable> {
        self.variable_opt(&format!("{base}.{field}"))
    }

    fn variable_opt(&self, name: &str) -> Option<Variable> {
        self.scopes
            .iter()
            .rev()
            .find_map(|scope| scope.get(name).copied())
            .or_else(|| self.symbols.globals.get(name).copied())
    }

    fn variable_type(&self, name: &str) -> Option<&Type> {
        self.scope_types
            .iter()
            .rev()
            .find_map(|scope| scope.get(name))
            .or_else(|| self.symbols.global_types.get(name))
    }

    fn named_value_type(&self, name: &str) -> Option<&Type> {
        self.variable_type(name)
            .or_else(|| self.symbols.constant_types.get(name))
    }

    fn embed_property_type(&self, name: &str) -> Option<Type> {
        self.symbols.embed_property_value(name)?;
        let (_, property) = name.rsplit_once('.')?;
        match property {
            "ptr" | "end" => Some(Type::Ptr(Box::new(Type::Named("u8".to_owned())))),
            "len" => Some(Type::Named("u24".to_owned())),
            _ => None,
        }
    }

    fn canonical_access_path(&self, path: &AccessPath) -> AccessPath {
        if self.named_value_type(&path.root).is_some() {
            return path.clone();
        }

        let mut candidate = path.root.clone();
        let mut best = None;
        for (index, segment) in path.segments.iter().enumerate() {
            let AccessSegment::Field(field) = segment else {
                break;
            };
            candidate.push('.');
            candidate.push_str(field);
            if self.named_value_type(&candidate).is_some()
                || self.symbols.embed_property_value(&candidate).is_some()
            {
                best = Some((candidate.clone(), index + 1));
            }
            if let Some((_, original)) = candidate.split_once('.') {
                if self.named_value_type(original).is_some()
                    || self.symbols.embed_property_value(original).is_some()
                {
                    best = Some((original.to_owned(), index + 1));
                }
            }
        }

        if let Some((root, consumed)) = best {
            AccessPath {
                root,
                segments: path.segments[consumed..].to_vec(),
            }
        } else {
            path.clone()
        }
    }

    fn name_in_current_function(&self, name: &str) -> bool {
        self.scope_types
            .iter()
            .any(|scope| scope.contains_key(name))
            || self.symbols.global_types.contains_key(name)
            || self.symbols.constant_types.contains_key(name)
            || self.symbols.functions.contains_key(name)
    }

    fn current_scope_mut(&mut self) -> &mut HashMap<String, Variable> {
        self.scopes
            .last_mut()
            .expect("function scope exists during statement emission")
    }

    fn current_scope_types_mut(&mut self) -> &mut HashMap<String, Type> {
        self.scope_types
            .last_mut()
            .expect("function type scope exists during statement emission")
    }

    fn current_local_constants_mut(&mut self) -> &mut HashMap<String, LocalConstant> {
        self.local_constants
            .last_mut()
            .expect("function local constant scope exists during statement emission")
    }

    fn current_readonly_pointer_aliases_mut(&mut self) -> &mut HashMap<String, u32> {
        self.readonly_pointer_aliases
            .last_mut()
            .expect("function read-only pointer alias scope exists during statement emission")
    }

    fn local_constant(&self, name: &str) -> Option<&LocalConstant> {
        for index in (0..self.local_constants.len()).rev() {
            if let Some(constant) = self.local_constants[index].get(name) {
                return Some(constant);
            }
            if self
                .scope_types
                .get(index)
                .is_some_and(|scope| scope.contains_key(name))
            {
                return None;
            }
        }
        None
    }

    fn readonly_pointer_alias(&self, name: &str) -> Option<u32> {
        for index in (0..self.readonly_pointer_aliases.len()).rev() {
            if let Some(addr) = self.readonly_pointer_aliases[index].get(name) {
                return Some(*addr);
            }
            if self
                .scope_types
                .get(index)
                .is_some_and(|scope| scope.contains_key(name))
            {
                return None;
            }
        }
        None
    }

    fn current_function_assigns(&self, name: &str) -> bool {
        self.assigned_names_stack
            .last()
            .is_some_and(|names| names.contains(name))
    }

    fn record_local_constant(&mut self, name: &str, ty: &Type, value: &Expr) {
        if self.current_function_assigns(name) {
            self.current_local_constants_mut().remove(name);
            return;
        }
        if !self.local_constant_supported_type(ty) {
            self.current_local_constants_mut().remove(name);
            return;
        }
        let Ok(width) = self.symbols.type_width(ty) else {
            return;
        };
        let Ok(value) = self.eval_i64_with_local_constants(value) else {
            self.current_local_constants_mut().remove(name);
            return;
        };
        if self.value_for_type(value, ty, width).is_ok() {
            self.current_local_constants_mut().insert(
                name.to_owned(),
                LocalConstant {
                    value,
                    ty: ty.clone(),
                },
            );
        }
    }

    fn local_constant_supported_type(&self, ty: &Type) -> bool {
        match self.symbols.resolved_type(ty) {
            Ok(Type::Ptr(_)) => true,
            Ok(Type::Named(name)) => matches!(
                name.as_str(),
                "u8" | "i8" | "bool" | "u16" | "i16" | "u24" | "i24" | "ptr24"
            ),
            _ => false,
        }
    }

    fn invalidate_local_constant(&mut self, name: &str) {
        for scope in self.local_constants.iter_mut().rev() {
            if scope.remove(name).is_some() {
                return;
            }
        }
    }

    fn record_readonly_pointer_alias(&mut self, name: &str, value: &Expr) {
        let Some(addr) = self.readonly_write_addr(value) else {
            self.current_readonly_pointer_aliases_mut().remove(name);
            return;
        };
        if self.readonly_embed_name_for_addr(addr).is_some() {
            self.current_readonly_pointer_aliases_mut()
                .insert(name.to_owned(), addr);
        } else if self.readonly_string_literal_for_addr(addr).is_some() {
            self.current_readonly_pointer_aliases_mut()
                .insert(name.to_owned(), addr);
        } else {
            self.current_readonly_pointer_aliases_mut().remove(name);
        }
    }

    fn readonly_embed_name_for_addr(&self, addr: u32) -> Option<&str> {
        let addr = u64::from(addr);
        for (name, embed) in &self.symbols.embeds {
            let Some(len) = embed.variable.len else {
                continue;
            };
            let start = u64::from(embed.variable.addr);
            let end = start + u64::from(len);
            if addr >= start && addr < end {
                return Some(name.as_str());
            }
        }
        None
    }

    fn readonly_string_literal_for_addr(&self, addr: u32) -> Option<&str> {
        self.readonly_string_literal_for_range(u64::from(addr), u64::from(addr) + 1)
    }

    fn readonly_string_literal_for_range(&self, start: u64, end: u64) -> Option<&str> {
        for (value, variable) in self
            .string_literals
            .iter()
            .chain(self.symbols.string_literals.iter())
        {
            let Some(len) = variable.len else {
                continue;
            };
            if len == 0 {
                continue;
            }
            let literal_start = u64::from(variable.addr);
            let literal_end = literal_start + u64::from(len);
            if start < literal_end && end > literal_start {
                return Some(value.as_str());
            }
        }
        None
    }

    fn invalidate_readonly_pointer_alias(&mut self, name: &str) {
        for scope in self.readonly_pointer_aliases.iter_mut().rev() {
            if scope.remove(name).is_some() {
                return;
            }
        }
    }

    fn next_label(&mut self, prefix: &str) -> String {
        let label = format!(".L_{prefix}_{}", self.label_counter);
        self.label_counter += 1;
        label
    }

    fn line(&mut self, line: &str) {
        self.out.push_str(line);
        self.out.push('\n');
    }
}

fn trunc_div_or_zero(left: i64, right: i64) -> i64 {
    if right == 0 {
        0
    } else {
        ((left as i128) / (right as i128)) as i64
    }
}

fn trunc_mod_or_zero(left: i64, right: i64) -> i64 {
    if right == 0 {
        0
    } else {
        ((left as i128) % (right as i128)) as i64
    }
}

fn alloc_from_cursor(cursor: &mut u32, align: u32, size: u32) -> Result<Variable, Diagnostic> {
    if align > 1 {
        let mask = align - 1;
        *cursor = cursor
            .checked_add(mask)
            .map(|addr| addr & !mask)
            .ok_or_else(|| Diagnostic::new("section alignment exceeds 24-bit address space"))?;
    }
    let variable = Variable {
        addr: *cursor,
        size,
        element_size: Some(u32::from(ValueWidth::U8.bytes())),
        len: Some(size),
    };
    *cursor = cursor
        .checked_add(size)
        .ok_or_else(|| Diagnostic::new("section allocation exceeds 24-bit address space"))?;
    if *cursor > Address24::MAX + 1 {
        return Err(Diagnostic::new(
            "section allocation exceeds 24-bit address space",
        ));
    }
    Ok(variable)
}

fn section_cursor<'a>(cursors: &'a mut Vec<(String, u32)>, section: &str) -> &'a mut u32 {
    let index = cursors
        .iter()
        .position(|(name, _)| name == section)
        .expect("section cursor exists");
    &mut cursors[index].1
}

fn recursive_call_edges(
    program: &Program,
    functions: &HashMap<String, FunctionSig>,
) -> HashSet<(String, String)> {
    let graph = function_call_graph(program, functions);
    let mut edges = HashSet::new();
    for (caller, callees) in &graph {
        for callee in callees {
            if function_reaches(callee, caller, &graph) {
                edges.insert((caller.clone(), callee.clone()));
            }
        }
    }
    edges
}

fn function_call_graph(
    program: &Program,
    functions: &HashMap<String, FunctionSig>,
) -> HashMap<String, Vec<String>> {
    let mut graph = HashMap::new();
    for declaration in &program.declarations {
        let Declaration::Function(function) = declaration else {
            continue;
        };
        let mut calls = Vec::new();
        collect_stmt_calls(&function.body, &mut calls);
        calls.retain(|name| functions.contains_key(name));
        graph.insert(function.name.clone(), calls);
    }
    graph
}

fn function_reaches(start: &str, target: &str, graph: &HashMap<String, Vec<String>>) -> bool {
    let mut stack = vec![start.to_owned()];
    let mut visited = HashSet::new();
    while let Some(function) = stack.pop() {
        if !visited.insert(function.clone()) {
            continue;
        }
        if function == target {
            return true;
        }
        if let Some(calls) = graph.get(&function) {
            stack.extend(calls.iter().cloned());
        }
    }
    false
}

fn validate_all_function_calls(
    program: &Program,
    functions: &HashMap<String, FunctionSig>,
) -> Result<(), Diagnostic> {
    for declaration in &program.declarations {
        let Declaration::Function(function) = declaration else {
            continue;
        };
        validate_stmt_calls(&function.body, functions)?;
    }
    Ok(())
}

fn validate_all_function_bodies(
    program: &Program,
    symbols: Symbols,
    recursive_call_edges: HashSet<(String, String)>,
) -> Result<(), Diagnostic> {
    let mut emitter = Emitter::new(symbols, AssemblyOptions::default(), recursive_call_edges);
    emitter.disable_dead_code_elimination();
    if let Some(main) = program.main_function() {
        emitter.emit_function(main)?;
    }
    for declaration in &program.declarations {
        let Declaration::Function(function) = declaration else {
            continue;
        };
        if function.name == "main" {
            continue;
        }
        emitter.emit_function(function)?;
    }
    Ok(())
}

fn validate_stmt_calls(
    stmts: &[Stmt],
    functions: &HashMap<String, FunctionSig>,
) -> Result<(), Diagnostic> {
    for stmt in stmts {
        match stmt {
            Stmt::Let { value, .. } => validate_expr_calls(value, functions)?,
            Stmt::Assign { target, value, .. } => {
                validate_place_calls(target, functions)?;
                validate_expr_calls(value, functions)?;
            }
            Stmt::If {
                condition,
                then_body,
                else_body,
            } => {
                validate_expr_calls(condition, functions)?;
                validate_stmt_calls(then_body, functions)?;
                validate_stmt_calls(else_body, functions)?;
            }
            Stmt::While { condition, body } => {
                validate_expr_calls(condition, functions)?;
                validate_stmt_calls(body, functions)?;
            }
            Stmt::Loop { body } => validate_stmt_calls(body, functions)?,
            Stmt::Return(Some(expr)) | Stmt::Expr(expr) => validate_expr_calls(expr, functions)?,
            Stmt::Out { value, .. } => validate_expr_calls(value, functions)?,
            Stmt::Break | Stmt::Continue | Stmt::Return(None) | Stmt::Asm { .. } => {}
        }
    }
    Ok(())
}

fn validate_place_calls(
    place: &Place,
    functions: &HashMap<String, FunctionSig>,
) -> Result<(), Diagnostic> {
    match place {
        Place::Index { index, .. } | Place::Deref(index) => validate_expr_calls(index, functions),
        Place::Access(path) => validate_access_calls(path, functions),
        Place::Ident(_) | Place::Field { .. } => Ok(()),
    }
}

fn validate_expr_calls(
    expr: &Expr,
    functions: &HashMap<String, FunctionSig>,
) -> Result<(), Diagnostic> {
    match expr {
        Expr::Array(values) => {
            for value in values {
                validate_expr_calls(value, functions)?;
            }
        }
        Expr::Index { index, .. } | Expr::AddressOfIndex { index, .. } | Expr::Deref(index) => {
            validate_expr_calls(index, functions)?;
        }
        Expr::Access(path) | Expr::AddressOfAccess(path) => {
            validate_access_calls(path, functions)?;
        }
        Expr::StructInit { fields, .. } => {
            for (_, value) in fields {
                validate_expr_calls(value, functions)?;
            }
        }
        Expr::Call { path, args } => {
            let name = path_text(path);
            validate_call_signature(&name, args.len(), functions)?;
            for arg in args {
                validate_expr_calls(arg, functions)?;
            }
        }
        Expr::Unary { expr, .. } | Expr::Cast { expr, .. } => validate_expr_calls(expr, functions)?,
        Expr::Binary { left, right, .. } => {
            validate_expr_calls(left, functions)?;
            validate_expr_calls(right, functions)?;
        }
        Expr::Int(_)
        | Expr::TypedInt(_, _)
        | Expr::Bool(_)
        | Expr::Char(_)
        | Expr::String(_)
        | Expr::Ident(_)
        | Expr::In(_)
        | Expr::Field { .. }
        | Expr::AddressOfField { .. }
        | Expr::AddressOf(_) => {}
    }
    Ok(())
}

fn validate_access_calls(
    path: &AccessPath,
    functions: &HashMap<String, FunctionSig>,
) -> Result<(), Diagnostic> {
    for segment in &path.segments {
        if let AccessSegment::Index(index) = segment {
            validate_expr_calls(index, functions)?;
        }
    }
    Ok(())
}

fn validate_call_signature(
    name: &str,
    arity: usize,
    functions: &HashMap<String, FunctionSig>,
) -> Result<(), Diagnostic> {
    if let Some(expected) = builtin_function_arity(name) {
        if expected != arity {
            return Err(Diagnostic::new(builtin_arity_error(name)));
        }
        return Ok(());
    }
    let sig = functions
        .get(name)
        .ok_or_else(|| Diagnostic::new(format!("unknown function `{name}`")))?;
    if sig.arity != arity {
        return Err(Diagnostic::new(format!(
            "function `{name}` expects {} arguments but got {arity}",
            sig.arity
        )));
    }
    Ok(())
}

fn builtin_function_arity(name: &str) -> Option<usize> {
    match name {
        "test.pass" | "ezra.test.pass" => Some(0),
        "test.fail" | "ezra.test.fail" => Some(1),
        "test.assert_eq_u8" | "ezra.test.assert_eq_u8" => Some(3),
        "test.assert_eq_u16" | "ezra.test.assert_eq_u16" => Some(3),
        "test.assert_eq_u24" | "ezra.test.assert_eq_u24" => Some(3),
        "debug.char" | "ezra.debug.char" => Some(1),
        "debug.str" | "ezra.debug.str" => Some(1),
        "debug.hex_u8" | "ezra.debug.hex_u8" => Some(1),
        "debug.hex_u16" | "ezra.debug.hex_u16" => Some(1),
        "debug.hex_u24" | "ezra.debug.hex_u24" => Some(1),
        "mem.poke8" | "ezra.mem.poke8" => Some(2),
        "mem.peek8" | "ezra.mem.peek8" => Some(1),
        "mem.memcpy" | "ezra.mem.memcpy" => Some(3),
        "mem.memset" | "ezra.mem.memset" => Some(3),
        _ => None,
    }
}

fn builtin_arity_error(name: &str) -> String {
    match name.strip_prefix("ezra.").unwrap_or(name) {
        "test.pass" => "test.pass requires no arguments".to_owned(),
        "test.fail" => "test.fail requires one argument".to_owned(),
        "test.assert_eq_u8" => "test.assert_eq_u8 requires three arguments".to_owned(),
        "test.assert_eq_u16" => "test.assert_eq_u16 requires three arguments".to_owned(),
        "test.assert_eq_u24" => "test.assert_eq_u24 requires three arguments".to_owned(),
        "debug.char" => "debug.char requires one argument".to_owned(),
        "debug.str" => "debug.str requires one argument".to_owned(),
        "debug.hex_u8" => "debug.hex_u8 requires one argument".to_owned(),
        "debug.hex_u16" => "debug.hex_u16 requires one argument".to_owned(),
        "debug.hex_u24" => "debug.hex_u24 requires one argument".to_owned(),
        "mem.poke8" => "mem.poke8 requires two arguments".to_owned(),
        "mem.peek8" => "mem.peek8 requires one argument".to_owned(),
        "mem.memcpy" => "mem.memcpy requires three arguments".to_owned(),
        "mem.memset" => "mem.memset requires three arguments".to_owned(),
        builtin => format!("{builtin} has invalid argument count"),
    }
}

fn inline_return_body(function: &Function) -> Option<(&[Stmt], Expr)> {
    let (last, prefix) = function.body.split_last()?;
    if prefix.iter().any(stmt_contains_return) {
        return None;
    }
    match last {
        Stmt::Return(Some(expr)) => Some((prefix, expr.clone())),
        _ => None,
    }
}

fn inline_void_body(function: &Function) -> Option<&[Stmt]> {
    if function.return_type.is_some() {
        return None;
    }
    match function.body.split_last() {
        Some((Stmt::Return(None), prefix)) if !prefix.iter().any(stmt_contains_return) => {
            Some(prefix)
        }
        _ if !function.body.iter().any(stmt_contains_return) => Some(&function.body),
        _ => None,
    }
}

fn is_inlinable_function(function: &Function) -> bool {
    has_attr(function, "inline")
        && (inline_return_body(function).is_some() || inline_void_body(function).is_some())
}

fn stmt_contains_return(stmt: &Stmt) -> bool {
    match stmt {
        Stmt::Return(_) => true,
        Stmt::If {
            then_body,
            else_body,
            ..
        } => {
            then_body.iter().any(stmt_contains_return) || else_body.iter().any(stmt_contains_return)
        }
        Stmt::While { body, .. } | Stmt::Loop { body } => body.iter().any(stmt_contains_return),
        Stmt::Let { .. }
        | Stmt::Assign { .. }
        | Stmt::Break
        | Stmt::Continue
        | Stmt::Asm { .. }
        | Stmt::Out { .. }
        | Stmt::Expr(_) => false,
    }
}

fn reachable_function_names(program: &Program, symbols: &Symbols) -> HashSet<String> {
    let mut graph = HashMap::new();
    let mut seeds = Vec::new();
    for declaration in &program.declarations {
        let Declaration::Function(function) = declaration else {
            continue;
        };
        let calls = reachable_calls_for_body(&function.body, symbols);
        graph.insert(function.name.clone(), calls);
        if function.name == "main"
            || function.public
            || has_attr(function, "naked")
            || has_attr(function, "interrupt")
        {
            seeds.push(function.name.clone());
        }
    }

    let mut reachable = HashSet::new();
    let mut stack = seeds;
    while let Some(name) = stack.pop() {
        if !reachable.insert(name.clone()) {
            continue;
        }
        if let Some(calls) = graph.get(&name) {
            stack.extend(calls.iter().cloned());
        }
    }
    reachable
}

fn reachable_calls_for_body(stmts: &[Stmt], symbols: &Symbols) -> Vec<String> {
    reachable_calls_for_body_with_inline_stack(stmts, symbols, &mut Vec::new())
}

fn reachable_calls_for_body_with_inline_stack(
    stmts: &[Stmt],
    symbols: &Symbols,
    inline_stack: &mut Vec<String>,
) -> Vec<String> {
    let mut raw_calls = Vec::new();
    collect_reachable_stmt_calls(stmts, &mut raw_calls, symbols);
    let mut calls = Vec::new();
    for name in raw_calls {
        if !symbols.functions.contains_key(&name) {
            continue;
        }
        if let Some(inline) = symbols.inline_functions.get(&name) {
            if inline_stack.iter().any(|inline_name| inline_name == &name) {
                calls.push(name);
            } else {
                inline_stack.push(name);
                calls.extend(reachable_calls_for_body_with_inline_stack(
                    &inline.body,
                    symbols,
                    inline_stack,
                ));
                inline_stack.pop();
            }
        } else {
            calls.push(name);
        }
    }
    calls
}

fn collect_stmt_calls(stmts: &[Stmt], calls: &mut Vec<String>) {
    collect_stmt_calls_with_symbols(stmts, calls, None)
}

fn collect_reachable_stmt_calls(stmts: &[Stmt], calls: &mut Vec<String>, symbols: &Symbols) {
    collect_stmt_calls_with_symbols(stmts, calls, Some(symbols))
}

fn collect_stmt_calls_with_symbols(
    stmts: &[Stmt],
    calls: &mut Vec<String>,
    symbols: Option<&Symbols>,
) {
    for stmt in stmts {
        match stmt {
            Stmt::Let { value, .. } => collect_expr_calls(value, calls),
            Stmt::Assign { target, value, .. } => {
                collect_place_calls(target, calls);
                collect_expr_calls(value, calls);
            }
            Stmt::If {
                condition,
                then_body,
                else_body,
            } => {
                collect_expr_calls(condition, calls);
                if let Some(symbols) = symbols {
                    if let Ok(value) = symbols.eval_i64(condition) {
                        if value == 0 {
                            collect_stmt_calls_with_symbols(else_body, calls, Some(symbols));
                        } else {
                            collect_stmt_calls_with_symbols(then_body, calls, Some(symbols));
                        }
                        if stmt_terminates_current_block(stmt) {
                            break;
                        }
                        continue;
                    }
                }
                collect_stmt_calls_with_symbols(then_body, calls, symbols);
                collect_stmt_calls_with_symbols(else_body, calls, symbols);
            }
            Stmt::While { condition, body } => {
                collect_expr_calls(condition, calls);
                if let Some(symbols) = symbols {
                    if symbols.eval_i64(condition).is_ok_and(|value| value == 0) {
                        if stmt_terminates_current_block(stmt) {
                            break;
                        }
                        continue;
                    }
                }
                collect_stmt_calls_with_symbols(body, calls, symbols);
            }
            Stmt::Loop { body } => collect_stmt_calls_with_symbols(body, calls, symbols),
            Stmt::Return(Some(expr)) | Stmt::Expr(expr) => collect_expr_calls(expr, calls),
            Stmt::Out { value, .. } => collect_expr_calls(value, calls),
            Stmt::Break | Stmt::Continue | Stmt::Return(None) | Stmt::Asm { .. } => {}
        }
        if stmt_terminates_current_block(stmt) {
            break;
        }
    }
}

fn collect_place_calls(place: &Place, calls: &mut Vec<String>) {
    match place {
        Place::Index { index, .. } | Place::Deref(index) => collect_expr_calls(index, calls),
        Place::Access(path) => collect_access_calls(path, calls),
        Place::Ident(_) | Place::Field { .. } => {}
    }
}

fn collect_expr_calls(expr: &Expr, calls: &mut Vec<String>) {
    match expr {
        Expr::Array(values) => {
            for value in values {
                collect_expr_calls(value, calls);
            }
        }
        Expr::Index { index, .. } | Expr::AddressOfIndex { index, .. } | Expr::Deref(index) => {
            collect_expr_calls(index, calls);
        }
        Expr::Access(path) | Expr::AddressOfAccess(path) => collect_access_calls(path, calls),
        Expr::StructInit { fields, .. } => {
            for (_, value) in fields {
                collect_expr_calls(value, calls);
            }
        }
        Expr::Call { path, args } => {
            calls.push(path_text(path));
            for arg in args {
                collect_expr_calls(arg, calls);
            }
        }
        Expr::Unary { expr, .. } | Expr::Cast { expr, .. } => collect_expr_calls(expr, calls),
        Expr::Binary { left, right, .. } => {
            collect_expr_calls(left, calls);
            collect_expr_calls(right, calls);
        }
        Expr::Int(_)
        | Expr::TypedInt(_, _)
        | Expr::Bool(_)
        | Expr::Char(_)
        | Expr::String(_)
        | Expr::Ident(_)
        | Expr::In(_)
        | Expr::Field { .. }
        | Expr::AddressOfField { .. }
        | Expr::AddressOf(_) => {}
    }
}

fn addr24(addr: i64) -> Option<u32> {
    if (0..=0xFF_FFFF).contains(&addr) {
        Some(addr as u32)
    } else {
        None
    }
}

fn collect_access_calls(path: &AccessPath, calls: &mut Vec<String>) {
    for segment in &path.segments {
        if let AccessSegment::Index(index) = segment {
            collect_expr_calls(index, calls);
        }
    }
}

fn const_shl_or_zero(left: i64, right: i64) -> i64 {
    if !(0..64).contains(&right) {
        0
    } else {
        left.wrapping_shl(right as u32)
    }
}

fn const_shr_or_zero(left: i64, right: i64, signed: bool) -> i64 {
    if right < 0 {
        return 0;
    }
    if signed {
        if right >= 64 {
            if left < 0 { -1 } else { 0 }
        } else {
            left >> right as u32
        }
    } else if right >= 64 {
        0
    } else {
        left.wrapping_shr(right as u32)
    }
}

fn path_text(path: &[String]) -> String {
    path.join(".")
}

fn module_alias_original_name(name: &str) -> Option<&str> {
    name.rsplit_once('.').map(|(_, original)| original)
}

fn function_label(name: &str) -> String {
    let mut label = String::from("_");
    for ch in name.chars() {
        if ch.is_ascii_alphanumeric() || ch == '_' {
            label.push(ch);
        } else {
            label.push('_');
        }
    }
    label
}

fn scalar_var(addr: u32, size: u32) -> Variable {
    Variable {
        addr,
        size,
        element_size: None,
        len: None,
    }
}

fn storage_ranges_overlap(left: Variable, right: Variable) -> bool {
    let left_start = u64::from(left.addr);
    let left_end = left_start + u64::from(left.size);
    let right_start = u64::from(right.addr);
    let right_end = right_start + u64::from(right.size);
    left_start < right_end && right_start < left_end
}

fn declaration_name(declaration: &Declaration) -> Option<&str> {
    match declaration {
        Declaration::Import(_) => None,
        Declaration::Const(decl) => Some(&decl.name),
        Declaration::Alias(decl) => Some(&decl.name),
        Declaration::Port(decl) => Some(&decl.name),
        Declaration::Mmio(decl) => Some(&decl.name),
        Declaration::Embed(decl) => Some(&decl.name),
        Declaration::Global(decl) => Some(&decl.name),
        Declaration::Struct(decl) => Some(&decl.name),
        Declaration::ExternAsmFunction(decl) => Some(&decl.name),
        Declaration::Function(decl) => Some(&decl.name),
    }
}

fn find_const_declaration<'a>(
    program: &'a Program,
    name: &str,
) -> Option<&'a crate::ast::ConstDecl> {
    program
        .declarations
        .iter()
        .find_map(|declaration| match declaration {
            Declaration::Const(decl) if decl.name == name => Some(decl),
            _ => None,
        })
}

fn collect_const_dependency_names(expr: &Expr, names: &mut Vec<String>) {
    match expr {
        Expr::Ident(name) => names.push(name.clone()),
        Expr::Field { base, field } => names.push(format!("{base}.{field}")),
        Expr::Access(path) => {
            if let Ok(name) = const_access_name(path) {
                names.push(name);
            }
            for segment in &path.segments {
                if let AccessSegment::Index(index) = segment {
                    collect_const_dependency_names(index, names);
                }
            }
        }
        Expr::Unary { expr, .. } | Expr::Cast { expr, .. } | Expr::Deref(expr) => {
            collect_const_dependency_names(expr, names)
        }
        Expr::Binary { left, right, .. } => {
            collect_const_dependency_names(left, names);
            collect_const_dependency_names(right, names);
        }
        Expr::Array(values) => {
            for value in values {
                collect_const_dependency_names(value, names);
            }
        }
        Expr::Index { index, .. } => collect_const_dependency_names(index, names),
        Expr::StructInit { fields, .. } => {
            for (_, value) in fields {
                collect_const_dependency_names(value, names);
            }
        }
        Expr::Call { args, .. } => {
            for arg in args {
                collect_const_dependency_names(arg, names);
            }
        }
        Expr::AddressOfIndex { index, .. } => collect_const_dependency_names(index, names),
        Expr::AddressOfAccess(path) => {
            for segment in &path.segments {
                if let AccessSegment::Index(index) = segment {
                    collect_const_dependency_names(index, names);
                }
            }
        }
        Expr::Int(_)
        | Expr::TypedInt(_, _)
        | Expr::Bool(_)
        | Expr::Char(_)
        | Expr::String(_)
        | Expr::In(_)
        | Expr::AddressOf(_)
        | Expr::AddressOfField { .. } => {}
    }
}

fn collect_const_address_roots(expr: &Expr, roots: &mut Vec<String>) {
    match expr {
        Expr::AddressOf(name) => roots.push(name.clone()),
        Expr::AddressOfIndex { name, index } => {
            roots.push(name.clone());
            collect_const_address_roots(index, roots);
        }
        Expr::AddressOfField { base, .. } => roots.push(base.clone()),
        Expr::AddressOfAccess(path) => {
            roots.push(path.root.clone());
            for segment in &path.segments {
                if let AccessSegment::Index(index) = segment {
                    collect_const_address_roots(index, roots);
                }
            }
        }
        Expr::Unary { expr, .. } | Expr::Cast { expr, .. } | Expr::Deref(expr) => {
            collect_const_address_roots(expr, roots)
        }
        Expr::Binary { left, right, .. } => {
            collect_const_address_roots(left, roots);
            collect_const_address_roots(right, roots);
        }
        Expr::Array(values) => {
            for value in values {
                collect_const_address_roots(value, roots);
            }
        }
        Expr::Index { index, .. } => {
            collect_const_address_roots(index, roots);
        }
        Expr::Access(path) => {
            for segment in &path.segments {
                if let AccessSegment::Index(index) = segment {
                    collect_const_address_roots(index, roots);
                }
            }
        }
        Expr::StructInit { fields, .. } => {
            for (_, value) in fields {
                collect_const_address_roots(value, roots);
            }
        }
        Expr::Call { args, .. } => {
            for arg in args {
                collect_const_address_roots(arg, roots);
            }
        }
        Expr::Int(_)
        | Expr::TypedInt(_, _)
        | Expr::Bool(_)
        | Expr::Char(_)
        | Expr::String(_)
        | Expr::Ident(_)
        | Expr::In(_)
        | Expr::Field { .. } => {}
    }
}

fn has_attr(function: &Function, attr: &str) -> bool {
    function.attrs.iter().any(|candidate| candidate == attr)
}

fn validate_function_attrs(function: &Function) -> Result<(), Diagnostic> {
    let mut seen = HashSet::new();
    for attr in &function.attrs {
        if !seen.insert(attr.as_str()) {
            return Err(Diagnostic::new(format!(
                "duplicate attribute `{attr}` on function `{}`",
                function.name
            )));
        }
    }
    Ok(())
}

fn block_guarantees_value_return(stmts: &[Stmt], symbols: &Symbols) -> bool {
    stmts
        .iter()
        .any(|stmt| stmt_guarantees_value_return(stmt, symbols))
}

fn stmt_guarantees_value_return(stmt: &Stmt, symbols: &Symbols) -> bool {
    match stmt {
        Stmt::Return(Some(_)) => true,
        Stmt::If {
            then_body,
            else_body,
            ..
        } if !else_body.is_empty() => {
            block_guarantees_value_return(then_body, symbols)
                && block_guarantees_value_return(else_body, symbols)
        }
        Stmt::Loop { body } => {
            !block_can_break_current_loop(body) && block_guarantees_value_return(body, symbols)
        }
        Stmt::While { condition, body } if condition_is_const_true(condition, symbols) => {
            !block_can_break_current_loop(body) && block_guarantees_value_return(body, symbols)
        }
        _ => false,
    }
}

fn condition_is_const_true(condition: &Expr, symbols: &Symbols) -> bool {
    matches!(condition, Expr::Bool(true))
        || symbols.eval_i64(condition).is_ok_and(|value| value != 0)
}

fn block_terminates_current_block(stmts: &[Stmt]) -> bool {
    stmts.iter().any(stmt_terminates_current_block)
}

fn stmt_terminates_current_block(stmt: &Stmt) -> bool {
    match stmt {
        Stmt::Return(_) | Stmt::Break | Stmt::Continue => true,
        Stmt::If {
            then_body,
            else_body,
            ..
        } if !else_body.is_empty() => {
            block_terminates_current_block(then_body) && block_terminates_current_block(else_body)
        }
        Stmt::Loop { body } => {
            !block_can_break_current_loop(body) && block_terminates_current_block(body)
        }
        Stmt::While {
            condition: Expr::Bool(true),
            body,
        } => !block_can_break_current_loop(body) && block_terminates_current_block(body),
        _ => false,
    }
}

fn assigned_names_in_block(stmts: &[Stmt]) -> HashSet<String> {
    let mut names = HashSet::new();
    collect_assigned_names(stmts, &mut names);
    names
}

fn assigned_names_in_program(program: &Program) -> HashSet<String> {
    let mut names = HashSet::new();
    for declaration in &program.declarations {
        if let Declaration::Function(function) = declaration {
            collect_assigned_names(&function.body, &mut names);
        }
    }
    names
}

fn collect_assigned_names(stmts: &[Stmt], names: &mut HashSet<String>) {
    for stmt in stmts {
        match stmt {
            Stmt::Assign { target, .. } => collect_assigned_place(target, names),
            Stmt::If {
                then_body,
                else_body,
                ..
            } => {
                collect_assigned_names(then_body, names);
                collect_assigned_names(else_body, names);
            }
            Stmt::While { body, .. } | Stmt::Loop { body } => collect_assigned_names(body, names),
            _ => {}
        }
    }
}

fn collect_assigned_place(place: &Place, names: &mut HashSet<String>) {
    if let Place::Ident(name) = place {
        names.insert(name.clone());
    }
}

fn block_can_break_current_loop(stmts: &[Stmt]) -> bool {
    stmts.iter().any(stmt_can_break_current_loop)
}

fn stmt_can_break_current_loop(stmt: &Stmt) -> bool {
    match stmt {
        Stmt::Break => true,
        Stmt::If {
            then_body,
            else_body,
            ..
        } => block_can_break_current_loop(then_body) || block_can_break_current_loop(else_body),
        Stmt::While { .. } | Stmt::Loop { .. } => false,
        _ => false,
    }
}

fn stmt_summary(stmt: &Stmt) -> String {
    match stmt {
        Stmt::Let { name, ty, value } => {
            format!("let {name}: {} = {}", type_display(ty), expr_summary(value))
        }
        Stmt::Assign { target, op, value } => {
            format!(
                "{} {} {}",
                place_summary(target),
                assign_op_summary(*op),
                expr_summary(value)
            )
        }
        Stmt::If { condition, .. } => format!("if {}", expr_summary(condition)),
        Stmt::While { condition, .. } => format!("while {}", expr_summary(condition)),
        Stmt::Loop { .. } => "loop".to_owned(),
        Stmt::Break => "break".to_owned(),
        Stmt::Continue => "continue".to_owned(),
        Stmt::Return(Some(expr)) => format!("return {}", expr_summary(expr)),
        Stmt::Return(None) => "return".to_owned(),
        Stmt::Asm { volatile, .. } => {
            if *volatile {
                "asm volatile".to_owned()
            } else {
                "asm".to_owned()
            }
        }
        Stmt::Out { port, value } => format!("out {port}, {}", expr_summary(value)),
        Stmt::Expr(expr) => expr_summary(expr),
    }
}

fn place_summary(place: &Place) -> String {
    match place {
        Place::Ident(name) => name.clone(),
        Place::Index { name, index } => format!("{name}[{}]", expr_summary(index)),
        Place::Field { base, field } => format!("{base}.{field}"),
        Place::Access(path) => access_path_summary(path),
        Place::Deref(expr) => format!("*{}", expr_summary(expr)),
    }
}

fn expr_summary(expr: &Expr) -> String {
    match expr {
        Expr::Int(value) => value.to_string(),
        Expr::TypedInt(value, ty) => format!("{value}{}", type_display(ty)),
        Expr::Bool(value) => value.to_string(),
        Expr::Char(value) => format!("'{}'", char::from(*value).escape_default()),
        Expr::String(value) => format!("{value:?}"),
        Expr::Array(values) => format!(
            "[{}]",
            values
                .iter()
                .map(expr_summary)
                .collect::<Vec<_>>()
                .join(", ")
        ),
        Expr::Ident(name) => name.clone(),
        Expr::In(port) => format!("in {port}"),
        Expr::Index { name, index } => format!("{name}[{}]", expr_summary(index)),
        Expr::Field { base, field } => format!("{base}.{field}"),
        Expr::AddressOfIndex { name, index } => format!("&{name}[{}]", expr_summary(index)),
        Expr::AddressOfField { base, field } => format!("&{base}.{field}"),
        Expr::Access(path) => access_path_summary(path),
        Expr::AddressOfAccess(path) => format!("&{}", access_path_summary(path)),
        Expr::AddressOf(name) => format!("&{name}"),
        Expr::StructInit { ty, fields } => format!(
            "{ty} {{ {} }}",
            fields
                .iter()
                .map(|(name, value)| format!("{name}: {}", expr_summary(value)))
                .collect::<Vec<_>>()
                .join(", ")
        ),
        Expr::Deref(expr) => format!("*{}", expr_summary(expr)),
        Expr::Call { path, args } => format!(
            "{}({})",
            path_text(path),
            args.iter().map(expr_summary).collect::<Vec<_>>().join(", ")
        ),
        Expr::Unary { op, expr } => format!("{}{}", unary_op_summary(*op), expr_summary(expr)),
        Expr::Binary { left, op, right } => format!(
            "{} {} {}",
            expr_summary(left),
            binary_op_summary(*op),
            expr_summary(right)
        ),
        Expr::Cast { ty, expr } => format!("cast<{}>({})", type_display(ty), expr_summary(expr)),
    }
}

fn access_path_summary(path: &AccessPath) -> String {
    let mut out = path.root.clone();
    for segment in &path.segments {
        match segment {
            AccessSegment::Field(field) => {
                out.push('.');
                out.push_str(field);
            }
            AccessSegment::Index(index) => {
                out.push('[');
                out.push_str(&expr_summary(index));
                out.push(']');
            }
        }
    }
    out
}

fn const_access_name(path: &AccessPath) -> Result<String, Diagnostic> {
    if path
        .segments
        .iter()
        .all(|segment| matches!(segment, AccessSegment::Field(_)))
    {
        Ok(access_path_summary(path))
    } else {
        Err(Diagnostic::new(
            "expression is not supported in a constant declaration",
        ))
    }
}

fn assign_op_summary(op: AssignOp) -> &'static str {
    match op {
        AssignOp::Set => "=",
        AssignOp::Add => "+=",
        AssignOp::Sub => "-=",
        AssignOp::BitAnd => "&=",
        AssignOp::BitOr => "|=",
        AssignOp::BitXor => "^=",
        AssignOp::Shl => "<<=",
        AssignOp::Shr => ">>=",
    }
}

fn unary_op_summary(op: UnaryOp) -> &'static str {
    match op {
        UnaryOp::Neg => "-",
        UnaryOp::BitNot => "~",
        UnaryOp::Not => "!",
    }
}

fn binary_op_summary(op: BinaryOp) -> &'static str {
    match op {
        BinaryOp::Mul => "*",
        BinaryOp::Div => "/",
        BinaryOp::Mod => "%",
        BinaryOp::Add => "+",
        BinaryOp::Sub => "-",
        BinaryOp::Shl => "<<",
        BinaryOp::Shr => ">>",
        BinaryOp::Lt => "<",
        BinaryOp::Le => "<=",
        BinaryOp::Gt => ">",
        BinaryOp::Ge => ">=",
        BinaryOp::Eq => "==",
        BinaryOp::Ne => "!=",
        BinaryOp::BitAnd => "&",
        BinaryOp::BitXor => "^",
        BinaryOp::BitOr => "|",
        BinaryOp::And => "&&",
        BinaryOp::Or => "||",
    }
}

fn type_display(ty: &Type) -> String {
    match ty {
        Type::Named(name) => name.clone(),
        Type::Ptr(inner) => format!("ptr<{}>", type_display(inner)),
        Type::Array { element, len } => {
            format!("[{}; {}]", type_display(element), expr_summary(len))
        }
    }
}

fn type_is_signed(ty: &Type) -> bool {
    matches!(ty, Type::Named(name) if matches!(name.as_str(), "i8" | "i16" | "i24"))
}

fn signed_min_bytes(width: ValueWidth) -> &'static [u8] {
    match width {
        ValueWidth::U8 => &[0x80],
        ValueWidth::U16 => &[0x00, 0x80],
        ValueWidth::U24 => &[0x00, 0x00, 0x80],
    }
}

fn signed_negative_one_bytes(width: ValueWidth) -> &'static [u8] {
    match width {
        ValueWidth::U8 => &[0xFF],
        ValueWidth::U16 => &[0xFF, 0xFF],
        ValueWidth::U24 => &[0xFF, 0xFF, 0xFF],
    }
}

fn type_is_bool(ty: &Type) -> bool {
    matches!(ty, Type::Named(name) if name == "bool")
}

fn validate_integer_unary_operand_type(ty: &Type) -> Result<(), Diagnostic> {
    if type_is_bool(ty) || matches!(ty, Type::Ptr(_)) {
        return Err(Diagnostic::new("unary operand must be an integer"));
    }
    match ty {
        Type::Named(name)
            if matches!(name.as_str(), "u8" | "i8" | "u16" | "i16" | "u24" | "i24") =>
        {
            Ok(())
        }
        Type::Named(name) if name == "ptr24" => {
            Err(Diagnostic::new("unary operand must be an integer"))
        }
        Type::Named(name) if matches!(name.as_str(), "u32" | "i32" | "u64" | "i64") => {
            Err(Diagnostic::new(format!(
                "type `{name}` is not supported; use explicit u8/u16/u24 or i8/i16/i24"
            )))
        }
        Type::Named(name) => Err(Diagnostic::new(format!("unknown type `{name}`"))),
        Type::Array { .. } => Err(Diagnostic::new("unary operand must be an integer")),
        Type::Ptr(_) => Err(Diagnostic::new("unary operand must be an integer")),
    }
}

fn validate_shift_operand_type(ty: &Type) -> Result<(), Diagnostic> {
    if type_is_bool(ty) || matches!(ty, Type::Ptr(_)) {
        return Err(Diagnostic::new("shift operand must be an integer"));
    }
    match ty {
        Type::Named(name)
            if matches!(name.as_str(), "u8" | "i8" | "u16" | "i16" | "u24" | "i24") =>
        {
            Ok(())
        }
        Type::Named(name) if name == "ptr24" => {
            Err(Diagnostic::new("shift operand must be an integer"))
        }
        Type::Named(name) if matches!(name.as_str(), "u32" | "i32" | "u64" | "i64") => {
            Err(Diagnostic::new(format!(
                "type `{name}` is not supported; use explicit u8/u16/u24 or i8/i16/i24"
            )))
        }
        Type::Named(name) => Err(Diagnostic::new(format!("unknown type `{name}`"))),
        Type::Array { .. } => Err(Diagnostic::new("shift operand must be an integer")),
        Type::Ptr(_) => Err(Diagnostic::new("shift operand must be an integer")),
    }
}

fn validate_shift_count_integer_type(ty: &Type) -> Result<(), Diagnostic> {
    if type_is_bool(ty) || matches!(ty, Type::Ptr(_)) {
        return Err(Diagnostic::new("shift count must be an integer"));
    }
    match ty {
        Type::Named(name)
            if matches!(name.as_str(), "u8" | "i8" | "u16" | "i16" | "u24" | "i24") =>
        {
            Ok(())
        }
        Type::Named(name) if name == "ptr24" => {
            Err(Diagnostic::new("shift count must be an integer"))
        }
        Type::Named(name) if matches!(name.as_str(), "u32" | "i32" | "u64" | "i64") => {
            Err(Diagnostic::new(format!(
                "type `{name}` is not supported; use explicit u8/u16/u24 or i8/i16/i24"
            )))
        }
        Type::Named(name) => Err(Diagnostic::new(format!("unknown type `{name}`"))),
        Type::Array { .. } => Err(Diagnostic::new("shift count must be an integer")),
        Type::Ptr(_) => Err(Diagnostic::new("shift count must be an integer")),
    }
}

fn validate_runtime_shift_count_type(ty: &Type) -> Result<(), Diagnostic> {
    match ty {
        Type::Named(name) if name == "u8" => Ok(()),
        _ => Err(Diagnostic::new("runtime shift count must be u8")),
    }
}

fn ptr_u8_type() -> Type {
    Type::Ptr(Box::new(Type::Named("u8".to_owned())))
}

fn width_unsigned_type(width: ValueWidth) -> Type {
    let name = match width {
        ValueWidth::U8 => "u8",
        ValueWidth::U16 => "u16",
        ValueWidth::U24 => "u24",
    };
    Type::Named(name.to_owned())
}

fn is_raw_address_type(name: &str) -> bool {
    matches!(name, "u24" | "ptr24")
}

fn validate_comparison_types<F>(
    left_type: &Type,
    op: BinaryOp,
    right_type: &Type,
    widths: F,
) -> Result<(), Diagnostic>
where
    F: FnOnce() -> Option<(ValueWidth, ValueWidth)>,
{
    if type_is_bool(left_type) || type_is_bool(right_type) {
        if matches!(op, BinaryOp::Eq | BinaryOp::Ne) && left_type == right_type {
            return Ok(());
        }
        return Err(Diagnostic::new("type mismatch"));
    }

    let left_is_ptr = matches!(left_type, Type::Ptr(_));
    let right_is_ptr = matches!(right_type, Type::Ptr(_));
    if left_is_ptr || right_is_ptr {
        if matches!(op, BinaryOp::Eq | BinaryOp::Ne) && left_type == right_type {
            return Ok(());
        }
        if matches!(op, BinaryOp::Eq | BinaryOp::Ne) {
            return Err(Diagnostic::new("type mismatch"));
        }
        if left_is_ptr && right_is_ptr {
            return Err(Diagnostic::new(
                "pointer comparisons support only == and !=",
            ));
        }
        return Err(Diagnostic::new("type mismatch"));
    }

    if type_is_signed(left_type) != type_is_signed(right_type) {
        return Err(Diagnostic::new("signed/unsigned mix without cast"));
    }
    if let Some((left_width, right_width)) = widths() {
        if left_width != right_width {
            return Err(Diagnostic::new(
                "comparison operands must have same width without cast",
            ));
        }
    }
    Ok(())
}

fn int_value_type(value: i64) -> Type {
    if (0..=0xFF).contains(&value) {
        Type::Named("u8".to_owned())
    } else if (0..=0xFFFF).contains(&value) {
        Type::Named("u16".to_owned())
    } else {
        Type::Named("u24".to_owned())
    }
}

fn expr_is_untyped_literal(expr: &Expr) -> bool {
    match expr {
        Expr::Int(_) | Expr::Char(_) => true,
        Expr::Unary {
            op: UnaryOp::Neg,
            expr,
        } => matches!(expr.as_ref(), Expr::Int(_)),
        _ => false,
    }
}

fn format_immediate(value: i64, width: ValueWidth) -> String {
    match width {
        ValueWidth::U8 => format!("{:02X}h", (value as u64) & 0xFF),
        ValueWidth::U16 => format!("{:04X}h", (value as u64) & 0xFFFF),
        ValueWidth::U24 => format!("{:06X}h", (value as u64) & 0xFF_FFFF),
    }
}

fn validate_main_signature(main: &Function) -> Result<(), Diagnostic> {
    if !main.params.is_empty() {
        return Err(Diagnostic::new("main function cannot take parameters"));
    }
    if main.return_type.is_some() {
        return Err(Diagnostic::new("main function cannot return a value"));
    }
    Ok(())
}

fn validate_inline_asm_clobbers(
    clobbers: &[String],
    lines: &[String],
    allow_sp_clobber: bool,
) -> Result<(), Diagnostic> {
    let mut seen = HashSet::new();
    for clobber in clobbers {
        if !is_allowed_inline_asm_clobber(clobber) {
            return Err(Diagnostic::new(format!(
                "unknown inline asm clobber `{clobber}`"
            )));
        }
        if !seen.insert(clobber.as_str()) {
            return Err(Diagnostic::new(format!(
                "duplicate inline asm clobber `{clobber}`"
            )));
        }
    }
    if asm_clobbers_include(clobbers, "sp") && !allow_sp_clobber {
        return Err(Diagnostic::new(
            "inline asm clobber `sp` is only allowed in naked functions",
        ));
    }
    for line in lines {
        let lower = line.to_ascii_lowercase();
        for register in ["ix", "iy", "sp"] {
            if asm_line_mentions_word(&lower, register) && !asm_clobbers_include(clobbers, register)
            {
                return Err(Diagnostic::new(format!(
                    "inline asm uses `{register}` without declaring clobber `{register}`"
                )));
            }
        }
        if asm_line_uses_ports(&lower) && !asm_clobbers_include(clobbers, "ports") {
            return Err(Diagnostic::new(
                "inline asm uses ports without declaring clobber `ports`",
            ));
        }
        if asm_line_clobbers_flags(&lower) && !asm_clobbers_include_flags(clobbers) {
            return Err(Diagnostic::new(
                "inline asm changes flags without declaring clobber `flags`",
            ));
        }
        if asm_line_uses_memory(&lower) && !asm_clobbers_include(clobbers, "memory") {
            return Err(Diagnostic::new(
                "inline asm uses memory without declaring clobber `memory`",
            ));
        }
        for register in asm_line_modified_registers(&lower) {
            if !asm_clobbers_include_register(clobbers, register) {
                return Err(Diagnostic::new(format!(
                    "inline asm modifies `{register}` without declaring clobber `{register}`"
                )));
            }
        }
    }
    Ok(())
}

fn is_allowed_inline_asm_clobber(clobber: &str) -> bool {
    matches!(
        clobber,
        "a" | "f"
            | "af"
            | "b"
            | "c"
            | "bc"
            | "d"
            | "e"
            | "de"
            | "h"
            | "l"
            | "hl"
            | "ix"
            | "iy"
            | "sp"
            | "memory"
            | "ports"
            | "flags"
    )
}

fn asm_clobbers_include(clobbers: &[String], name: &str) -> bool {
    clobbers.iter().any(|clobber| clobber == name)
}

fn asm_clobbers_include_flags(clobbers: &[String]) -> bool {
    asm_clobbers_include(clobbers, "flags")
        || asm_clobbers_include(clobbers, "f")
        || asm_clobbers_include(clobbers, "af")
}

fn asm_clobbers_include_register(clobbers: &[String], register: &str) -> bool {
    if asm_clobbers_include(clobbers, register) {
        return true;
    }
    match register {
        "a" | "f" => asm_clobbers_include(clobbers, "af"),
        "b" | "c" => asm_clobbers_include(clobbers, "bc"),
        "d" | "e" => asm_clobbers_include(clobbers, "de"),
        "h" | "l" => asm_clobbers_include(clobbers, "hl"),
        "af" => {
            asm_clobbers_include(clobbers, "a")
                && (asm_clobbers_include(clobbers, "f") || asm_clobbers_include(clobbers, "flags"))
        }
        "bc" => asm_clobbers_include(clobbers, "b") && asm_clobbers_include(clobbers, "c"),
        "de" => asm_clobbers_include(clobbers, "d") && asm_clobbers_include(clobbers, "e"),
        "hl" => asm_clobbers_include(clobbers, "h") && asm_clobbers_include(clobbers, "l"),
        _ => false,
    }
}

fn asm_line_uses_ports(line: &str) -> bool {
    let mnemonic_uses_ports = asm_line_mnemonic_and_operands(line).is_some_and(|(mnemonic, _)| {
        matches!(
            mnemonic,
            "ini" | "inir" | "ind" | "indr" | "outi" | "otir" | "outd" | "otdr"
        )
    });
    mnemonic_uses_ports
        || asm_line_mentions_word(line, "out")
        || asm_line_mentions_word(line, "out0")
        || asm_line_mentions_word(line, "in")
        || asm_line_mentions_word(line, "in0")
}

fn asm_line_uses_memory(line: &str) -> bool {
    asm_line_mnemonic_and_operands(line).is_some_and(|(mnemonic, _)| {
        matches!(
            mnemonic,
            "ldi"
                | "ldir"
                | "ldd"
                | "lddr"
                | "cpi"
                | "cpir"
                | "cpd"
                | "cpdr"
                | "ini"
                | "inir"
                | "ind"
                | "indr"
                | "outi"
                | "otir"
                | "outd"
                | "otdr"
        )
    })
}

fn asm_line_modified_registers(line: &str) -> Vec<&'static str> {
    let Some((mnemonic, operands)) = asm_line_mnemonic_and_operands(line) else {
        return Vec::new();
    };
    let first = asm_first_operand(operands);
    match mnemonic {
        "ld" | "lea" | "in" | "in0" => asm_operand_register(first).into_iter().collect(),
        "push" => vec!["sp"],
        "pop" => {
            let mut registers: Vec<_> = asm_operand_register(first).into_iter().collect();
            registers.push("sp");
            registers
        }
        "inc" | "dec" | "rl" | "rlc" | "rr" | "rrc" | "sla" | "sra" | "srl" => {
            asm_operand_register(first).into_iter().collect()
        }
        "add" | "adc" | "sbc" => match asm_operand_register(first) {
            Some(register) => vec![register],
            None => vec!["a"],
        },
        "sub" | "and" | "or" | "xor" | "cpl" | "daa" | "neg" | "rla" | "rlca" | "rra" | "rrca" => {
            vec!["a"]
        }
        "ex" => asm_line_exchange_registers(operands),
        "exx" => vec!["bc", "de", "hl"],
        "call" => vec!["af", "bc", "de", "hl"],
        "ldi" | "ldir" | "ldd" | "lddr" => vec!["bc", "de", "hl"],
        "cpi" | "cpir" | "cpd" | "cpdr" => vec!["bc", "hl"],
        "ini" | "inir" | "ind" | "indr" | "outi" | "otir" | "outd" | "otdr" => {
            vec!["bc", "hl"]
        }
        "mlt" => asm_operand_register(first).into_iter().collect(),
        _ => Vec::new(),
    }
}

fn asm_line_clobbers_flags(line: &str) -> bool {
    let Some((mnemonic, _)) = asm_line_mnemonic_and_operands(line) else {
        return false;
    };
    matches!(
        mnemonic,
        "adc"
            | "add"
            | "and"
            | "bit"
            | "cp"
            | "cpl"
            | "daa"
            | "dec"
            | "inc"
            | "neg"
            | "or"
            | "rl"
            | "rla"
            | "rlc"
            | "rlca"
            | "rr"
            | "rra"
            | "rrc"
            | "rrca"
            | "sbc"
            | "sla"
            | "sra"
            | "srl"
            | "sub"
            | "xor"
            | "ldi"
            | "ldir"
            | "ldd"
            | "lddr"
            | "cpi"
            | "cpir"
            | "cpd"
            | "cpdr"
            | "ini"
            | "inir"
            | "ind"
            | "indr"
            | "outi"
            | "otir"
            | "outd"
            | "otdr"
    )
}

fn asm_line_mnemonic_and_operands(line: &str) -> Option<(&str, &str)> {
    let mut text = line.trim_start();
    if let Some((label, rest)) = text.split_once(':') {
        if !label.chars().any(char::is_whitespace) {
            text = rest.trim_start();
        }
    }
    let mnemonic_end = text
        .find(|ch: char| ch.is_ascii_whitespace())
        .unwrap_or(text.len());
    if mnemonic_end == 0 {
        return None;
    }
    let mnemonic = &text[..mnemonic_end];
    let operands = text[mnemonic_end..].trim_start();
    Some((mnemonic, operands))
}

fn asm_first_operand(operands: &str) -> &str {
    operands
        .split_once(',')
        .map(|(first, _)| first)
        .unwrap_or(operands)
        .trim()
}

fn asm_operand_register(operand: &str) -> Option<&'static str> {
    let register = operand
        .trim()
        .trim_end_matches(',')
        .trim_end_matches(':')
        .trim();
    match register {
        "a" => Some("a"),
        "f" => Some("f"),
        "af" => Some("af"),
        "b" => Some("b"),
        "c" => Some("c"),
        "bc" => Some("bc"),
        "d" => Some("d"),
        "e" => Some("e"),
        "de" => Some("de"),
        "h" => Some("h"),
        "l" => Some("l"),
        "hl" => Some("hl"),
        "ix" => Some("ix"),
        "iy" => Some("iy"),
        "sp" => Some("sp"),
        _ => None,
    }
}

fn asm_line_exchange_registers(operands: &str) -> Vec<&'static str> {
    let mut registers = Vec::new();
    for operand in operands.split(',') {
        if let Some(register) = asm_operand_register(operand) {
            registers.push(register);
        }
    }
    registers
}

fn asm_line_mentions_word(line: &str, word: &str) -> bool {
    let mut start = 0;
    while let Some(offset) = line[start..].find(word) {
        let index = start + offset;
        let before = line[..index].chars().next_back();
        let after = line[index + word.len()..].chars().next();
        if !is_asm_word_char(before) && !is_asm_word_char(after) {
            return true;
        }
        start = index + word.len();
    }
    false
}

fn is_asm_word_char(ch: Option<char>) -> bool {
    matches!(ch, Some(ch) if ch.is_ascii_alphanumeric() || ch == '_')
}

fn substitute_inline_asm_operands(
    line: &str,
    operands: &HashMap<String, String>,
) -> Result<String, Diagnostic> {
    let mut output = String::new();
    let mut rest = line;
    while let Some(start) = rest.find('{') {
        output.push_str(&rest[..start]);
        let after_start = &rest[start + 1..];
        let Some(end) = after_start.find('}') else {
            return Err(Diagnostic::new(format!(
                "unterminated inline asm operand placeholder in `{line}`"
            )));
        };
        let name = &after_start[..end];
        let Some(binding) = operands.get(name) else {
            return Err(Diagnostic::new(format!(
                "unknown inline asm operand placeholder `{name}`"
            )));
        };
        output.push_str(binding);
        rest = &after_start[end + 1..];
    }
    if rest.contains('}') {
        return Err(Diagnostic::new(format!(
            "unmatched inline asm operand placeholder in `{line}`"
        )));
    }
    output.push_str(rest);
    Ok(output)
}

fn is_comparison(op: BinaryOp) -> bool {
    matches!(
        op,
        BinaryOp::Eq | BinaryOp::Ne | BinaryOp::Lt | BinaryOp::Le | BinaryOp::Gt | BinaryOp::Ge
    )
}

fn sdk_constants(options: &AssemblyOptions) -> HashMap<String, i64> {
    let mut constants = HashMap::from([
        ("EZRA_LOAD_ADDR".to_owned(), options.load_addr.get() as i64),
        (
            "EZRA_ENTRY_ADDR".to_owned(),
            options.entry_addr.get() as i64,
        ),
        ("EZRA_CODE_BASE".to_owned(), options.code_base.get() as i64),
        ("EZRA_STACK_TOP".to_owned(), options.stack_top.get() as i64),
        ("EZRA_RAM_BASE".to_owned(), options.ram_base.get() as i64),
        ("EZRA_VRAM_BASE".to_owned(), options.vram_base.get() as i64),
        (
            "EZRA_AUDIO_BASE".to_owned(),
            options.audio_base.get() as i64,
        ),
        (
            "EZRA_ASSET_BASE".to_owned(),
            options.asset_base.get() as i64,
        ),
        (
            "EZRA_RODATA_BASE".to_owned(),
            options.rodata_base.get() as i64,
        ),
    ]);
    if options.default_sdk_symbols {
        constants.extend([
            ("VRAM_BASE".to_owned(), options.vram_base.get() as i64),
            ("AUDIO_BASE".to_owned(), options.audio_base.get() as i64),
            ("BTN_B".to_owned(), 0x0001),
            ("BTN_Y".to_owned(), 0x0002),
            ("BTN_SELECT".to_owned(), 0x0004),
            ("BTN_START".to_owned(), 0x0008),
            ("BTN_UP".to_owned(), 0x0010),
            ("BTN_DOWN".to_owned(), 0x0020),
            ("BTN_LEFT".to_owned(), 0x0040),
            ("BTN_RIGHT".to_owned(), 0x0080),
            ("BTN_A".to_owned(), 0x0100),
            ("BTN_X".to_owned(), 0x0200),
            ("BTN_L".to_owned(), 0x0400),
            ("BTN_R".to_owned(), 0x0800),
            ("VIDEO_PRESENT".to_owned(), 1),
            ("VIDEO_CLEAR".to_owned(), 2),
            ("VIDEO_SET_MODE".to_owned(), 3),
            ("AUDIO_SUBMIT_BUFFER".to_owned(), 1),
            ("AUDIO_STOP".to_owned(), 2),
        ]);
    }
    constants
}

fn sdk_constant_types(options: &AssemblyOptions) -> HashMap<String, Type> {
    let mut types = HashMap::new();
    for name in [
        "EZRA_LOAD_ADDR",
        "EZRA_ENTRY_ADDR",
        "EZRA_CODE_BASE",
        "EZRA_STACK_TOP",
        "EZRA_RAM_BASE",
        "EZRA_VRAM_BASE",
        "EZRA_AUDIO_BASE",
        "EZRA_ASSET_BASE",
        "EZRA_RODATA_BASE",
    ] {
        types.insert(name.to_owned(), Type::Named("u24".to_owned()));
    }
    if !options.default_sdk_symbols {
        return types;
    }
    for name in ["VRAM_BASE", "AUDIO_BASE"] {
        types.insert(
            name.to_owned(),
            Type::Ptr(Box::new(Type::Named("u8".to_owned()))),
        );
    }
    for name in [
        "BTN_B",
        "BTN_Y",
        "BTN_SELECT",
        "BTN_START",
        "BTN_UP",
        "BTN_DOWN",
        "BTN_LEFT",
        "BTN_RIGHT",
        "BTN_A",
        "BTN_X",
        "BTN_L",
        "BTN_R",
    ] {
        types.insert(name.to_owned(), Type::Named("u16".to_owned()));
    }
    for name in [
        "VIDEO_PRESENT",
        "VIDEO_CLEAR",
        "VIDEO_SET_MODE",
        "AUDIO_SUBMIT_BUFFER",
        "AUDIO_STOP",
    ] {
        types.insert(name.to_owned(), Type::Named("u8".to_owned()));
    }
    types
}

fn sdk_ports(options: &AssemblyOptions) -> HashMap<String, u8> {
    if !options.default_sdk_symbols {
        return HashMap::new();
    }
    HashMap::from([
        ("PAD1_LO".to_owned(), 0x01),
        ("PAD1_HI".to_owned(), 0x02),
        ("PAD2_LO".to_owned(), 0x03),
        ("PAD2_HI".to_owned(), 0x04),
        ("PAD3_LO".to_owned(), 0x05),
        ("PAD3_HI".to_owned(), 0x06),
        ("PAD4_LO".to_owned(), 0x07),
        ("PAD4_HI".to_owned(), 0x08),
        ("VIDEO_CMD".to_owned(), 0x09),
        ("AUDIO_CMD".to_owned(), 0x0A),
        ("SYS_STATUS".to_owned(), 0x0B),
        ("DEBUG_CHAR".to_owned(), 0x0C),
        ("TEST_RESULT".to_owned(), 0x0D),
        ("TEST_HALT".to_owned(), 0x0E),
        ("EXT_ADDR0".to_owned(), 0x10),
        ("EXT_ADDR1".to_owned(), 0x11),
        ("EXT_ADDR2".to_owned(), 0x12),
        ("EXT_LEN0".to_owned(), 0x13),
        ("EXT_LEN1".to_owned(), 0x14),
        ("EXT_MODE".to_owned(), 0x15),
        ("EXT_COMMAND".to_owned(), 0x16),
        ("EXT_STATUS".to_owned(), 0x17),
    ])
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use crate::{
        compile::load_program,
        parser::parse_program,
        vm::{TestRunOptions, run_assembly_test, run_assembly_test_with_options},
    };

    use super::*;

    #[test]
    fn emits_test_pass_ports() {
        let program = parse_program(Path::new("game.ezra"), "fn main() { test.pass() }").unwrap();
        let asm = emit_ez80_assembly(&program).unwrap();

        assert!(asm.contains("__ezra_pass:"));
        assert!(asm.contains("__ezra_fail:"));
        assert!(asm.contains("    call __ezra_pass"));
        assert!(asm.contains("out0 (0Dh), a"));
        assert!(asm.contains("out0 (0Eh), a"));
    }

    #[test]
    fn emits_test_fail_helper_calls() {
        let source = r#"
            fn main() {
                test.fail(7)
            }
        "#;
        let program = parse_program(Path::new("game.ezra"), source).unwrap();
        let asm = emit_ez80_assembly(&program).unwrap();
        let run = run_assembly_test(&asm, 1_000).unwrap();

        assert!(asm.contains("    call __ezra_fail"), "{asm}");
        assert!(run.halted, "{asm}");
        assert_eq!(run.result_code, 7, "{asm}");
    }

    #[test]
    fn emits_and_runs_memcpy_runtime_helper() {
        let source = r#"
            fn main() {
                asm volatile(clobber af, clobber bc, clobber de, clobber hl, clobber memory) {
                    "ld a, 41h"
                    "ld (040300h), a"
                    "ld a, 42h"
                    "ld (040301h), a"
                    "ld a, 43h"
                    "ld (040302h), a"
                    "ld hl, 040310h"
                    "ld de, 040300h"
                    "ld bc, 000003h"
                    "call __ezra_memcpy"
                }
                test.assert_eq_u8(mem.peek8(cast<ptr<u8>>(0x040310)), 0x41, 1)
                test.assert_eq_u8(mem.peek8(cast<ptr<u8>>(0x040311)), 0x42, 2)
                test.assert_eq_u8(mem.peek8(cast<ptr<u8>>(0x040312)), 0x43, 3)
                test.pass()
            }
        "#;
        let program = parse_program(Path::new("game.ezra"), source).unwrap();
        let asm = emit_ez80_assembly(&program).unwrap();
        let run = run_assembly_test(&asm, 3_000).unwrap();

        assert!(asm.contains("__ezra_memcpy:"), "{asm}");
        assert!(
            asm.contains(
                "__ezra_memcpy:\n    push de\n    push hl\n    push bc\n    pop hl\n    ld de, 000000h\n    or a\n    sbc hl, de\n    pop hl\n    pop de\n    ret z\n    ex de, hl\n    ldir\n    ret"
            ),
            "{asm}"
        );
        assert!(run.halted, "{asm}");
        assert_eq!(run.result_code, 0, "{asm}");
    }

    #[test]
    fn emits_and_runs_memset_runtime_helper() {
        let source = r#"
            fn main() {
                asm volatile(clobber af, clobber bc, clobber de, clobber hl, clobber memory) {
                    "ld hl, 040320h"
                    "ld a, 5Ah"
                    "ld bc, 000003h"
                    "call __ezra_memset"
                }
                test.assert_eq_u8(mem.peek8(cast<ptr<u8>>(0x040320)), 0x5A, 1)
                test.assert_eq_u8(mem.peek8(cast<ptr<u8>>(0x040321)), 0x5A, 2)
                test.assert_eq_u8(mem.peek8(cast<ptr<u8>>(0x040322)), 0x5A, 3)
                test.pass()
            }
        "#;
        let program = parse_program(Path::new("game.ezra"), source).unwrap();
        let asm = emit_ez80_assembly(&program).unwrap();
        let run = run_assembly_test(&asm, 3_000).unwrap();

        assert!(asm.contains("__ezra_memset:"), "{asm}");
        assert!(
            asm.contains(
                "__ezra_memset:\n    push hl\n    push bc\n    pop hl\n    ld de, 000000h\n    or a\n    sbc hl, de\n    pop hl\n    ret z\n    ld (hl), a\n    dec bc\n    push hl\n    push bc\n    pop hl\n    ld de, 000000h\n    or a\n    sbc hl, de\n    pop hl\n    ret z\n    push hl\n    inc hl\n    ex de, hl\n    pop hl\n    ldir\n    ret"
            ),
            "{asm}"
        );
        assert!(run.halted, "{asm}");
        assert_eq!(run.result_code, 0, "{asm}");
    }

    #[test]
    fn emits_and_runs_memcpy_and_memset_builtins() {
        let source = r#"
            global src: [u8; 5] = [0x11, 0x22, 0x33, 0x44, 0x55]
            global dst: [u8; 5] = [0, 0, 0, 0, 0]

            fn main() {
                mem.memcpy(&dst[1], &src[0], 3)
                test.assert_eq_u8(dst[0], 0, 1)
                test.assert_eq_u8(dst[1], 0x11, 2)
                test.assert_eq_u8(dst[2], 0x22, 3)
                test.assert_eq_u8(dst[3], 0x33, 4)
                test.assert_eq_u8(dst[4], 0, 5)

                ezra.mem.memset(&dst[2], 0x7A, 2)
                test.assert_eq_u8(dst[1], 0x11, 6)
                test.assert_eq_u8(dst[2], 0x7A, 7)
                test.assert_eq_u8(dst[3], 0x7A, 8)
                test.assert_eq_u8(dst[4], 0, 9)

                mem.memcpy(&dst[4], &src[4], 0)
                mem.memset(&dst[4], 0xEE, 0)
                test.assert_eq_u8(dst[4], 0, 10)
                mem.memset(&dst[4], 0xCC, 1)
                test.assert_eq_u8(dst[4], 0xCC, 11)
                test.pass()
            }
        "#;
        let program = parse_program(Path::new("game.ezra"), source).unwrap();
        let asm = emit_ez80_assembly(&program).unwrap();
        let run = run_assembly_test(&asm, 6_000).unwrap();

        assert!(asm.contains("    call __ezra_memcpy"), "{asm}");
        assert!(asm.contains("    call __ezra_memset"), "{asm}");
        assert!(run.halted, "{asm}");
        assert_eq!(run.result_code, 0, "{asm}");
    }

    #[test]
    fn emits_and_runs_mul_u8_runtime_helper() {
        let expected = 17u8.wrapping_mul(15);
        let source = format!(
            r#"
            fn main() {{
                asm volatile(clobber af, clobber bc, clobber de, clobber hl, clobber memory) {{
                    "ld a, 0Fh"
                    "ld c, a"
                    "ld a, 11h"
                    "call __ezra_mul_u8"
                    "ld (040330h), a"
                }}
                test.assert_eq_u8(mem.peek8(cast<ptr<u8>>(0x040330)), {expected}, 1)
                test.pass()
            }}
        "#
        );
        let program = parse_program(Path::new("game.ezra"), &source).unwrap();
        let asm = emit_ez80_assembly(&program).unwrap();
        let run = run_assembly_test(&asm, 3_000).unwrap();

        assert!(asm.contains("__ezra_mul_u8:"), "{asm}");
        assert!(run.halted, "{asm}");
        assert_eq!(run.result_code, 0, "{asm}");
    }

    #[test]
    fn emits_and_runs_div_mod_u8_runtime_helpers() {
        let expected_div = 23u8 / 5;
        let expected_mod = 23u8 % 5;
        let source = format!(
            r#"
            fn main() {{
                asm volatile(clobber af, clobber bc, clobber de, clobber hl, clobber memory) {{
                    "ld a, 05h"
                    "ld c, a"
                    "ld a, 17h"
                    "call __ezra_div_u8"
                    "ld (040340h), a"
                    "ld a, 05h"
                    "ld c, a"
                    "ld a, 17h"
                    "call __ezra_mod_u8"
                    "ld (040341h), a"
                    "ld a, 00h"
                    "ld c, a"
                    "ld a, 17h"
                    "call __ezra_div_u8"
                    "ld (040342h), a"
                    "ld a, 00h"
                    "ld c, a"
                    "ld a, 17h"
                    "call __ezra_mod_u8"
                    "ld (040343h), a"
                }}
                test.assert_eq_u8(mem.peek8(cast<ptr<u8>>(0x040340)), {expected_div}, 1)
                test.assert_eq_u8(mem.peek8(cast<ptr<u8>>(0x040341)), {expected_mod}, 2)
                test.assert_eq_u8(mem.peek8(cast<ptr<u8>>(0x040342)), 0, 3)
                test.assert_eq_u8(mem.peek8(cast<ptr<u8>>(0x040343)), 0, 4)
                test.pass()
            }}
        "#
        );
        let program = parse_program(Path::new("game.ezra"), &source).unwrap();
        let asm = emit_ez80_assembly(&program).unwrap();
        let run = run_assembly_test(&asm, 3_000).unwrap();

        assert!(asm.contains("__ezra_div_u8:"), "{asm}");
        assert!(asm.contains("__ezra_mod_u8:"), "{asm}");
        assert!(run.halted, "{asm}");
        assert_eq!(run.result_code, 0, "{asm}");
    }

    #[test]
    fn emits_required_assembly_sections() {
        let program = parse_program(Path::new("game.ezra"), "fn main() { test.pass() }").unwrap();
        let asm = emit_ez80_assembly(&program).unwrap();

        for section in [
            "section .header",
            "section .text",
            "section .rodata",
            "section .data",
            "section .bss",
            "section .assets",
            "section .scratch",
        ] {
            assert!(asm.contains(section), "{asm}");
        }
        assert!(asm.starts_with("; generated by ezrac\n"), "{asm}");
        let start = asm
            .split_once("__ezra_start:")
            .map(|(_, tail)| tail)
            .expect("assembly should contain startup label");
        let di = start
            .find("    di")
            .expect("startup should disable interrupts");
        let stack = start
            .find("    ld sp, F00000h")
            .expect("startup should initialize the stack");
        assert!(di < stack, "{asm}");
    }

    #[test]
    fn emits_source_comments_in_debug_mode() {
        let source = r#"
            fn main() {
                let x: u8 = 4
                x += 1
                test.assert_eq_u8(x, 5, 1)
                test.pass()
            }
        "#;
        let program = parse_program(Path::new("game.ezra"), source).unwrap();
        let plain = emit_ez80_assembly(&program).unwrap();
        let asm = emit_ez80_assembly_with_debug_comments(&program, true).unwrap();
        let run = run_assembly_test(&asm, 1_000).unwrap();

        assert!(!plain.contains("; source:"), "{plain}");
        assert!(asm.contains("; source: let x: u8 = 4"), "{asm}");
        assert!(asm.contains("; source: x += 1"), "{asm}");
        assert!(
            asm.contains("; source: test.assert_eq_u8(x, 5, 1)"),
            "{asm}"
        );
        assert!(run.halted, "{asm}");
        assert_eq!(run.result_code, 0, "{asm}");
    }

    #[test]
    fn peephole_removes_adjacent_duplicate_register_loads() {
        let asm = peephole_cleanup(
            r#"
section .text
    ld a, 01h
    ld a, 01h
    ld hl, 040000h
    ld hl, 040000h
    ld e, 02h
    ld e, 02h
    ld iy, 040000h
    ld iy, 040000h
    ld b, a
"#,
        );

        assert_eq!(asm.matches("    ld a, 01h").count(), 1, "{asm}");
        assert_eq!(asm.matches("    ld hl, 040000h").count(), 1, "{asm}");
        assert_eq!(asm.matches("    ld e, 02h").count(), 1, "{asm}");
        assert_eq!(asm.matches("    ld iy, 040000h").count(), 1, "{asm}");
        assert!(asm.contains("    ld b, a"), "{asm}");
    }

    #[test]
    fn peephole_preserves_volatile_sensitive_operations() {
        let asm = peephole_cleanup(
            r#"
section .text
    ld a, (040000h)
    ld a, (040000h)
    ld (040000h), a
    ld (040000h), a
    in0 a, (01h)
    in0 a, (01h)
    out0 (0Ch), a
    out0 (0Ch), a
"#,
        );

        assert_eq!(asm.matches("    ld a, (040000h)").count(), 2, "{asm}");
        assert_eq!(asm.matches("    ld (040000h), a").count(), 2, "{asm}");
        assert_eq!(asm.matches("    in0 a, (01h)").count(), 2, "{asm}");
        assert_eq!(asm.matches("    out0 (0Ch), a").count(), 2, "{asm}");
    }

    #[test]
    fn rejects_duplicate_top_level_declarations() {
        let source = r#"
            const VALUE: u8 = 1
            global VALUE: u8 = 2
            fn main() { test.pass() }
        "#;
        let program = parse_program(Path::new("game.ezra"), source).unwrap();
        let error = emit_ez80_assembly(&program).unwrap_err();

        assert_eq!(error.message, "duplicate declaration `VALUE`");
    }

    #[test]
    fn rejects_duplicate_function_parameters() {
        let source = r#"
            fn add(value: u8, value: u8) -> u8 {
                return value
            }

            fn main() { test.pass() }
        "#;
        let program = parse_program(Path::new("game.ezra"), source).unwrap();
        let error = emit_ez80_assembly(&program).unwrap_err();

        assert_eq!(
            error.message,
            "function `add` has duplicate parameter `value`"
        );
    }

    #[test]
    fn rejects_duplicate_struct_fields() {
        let source = r#"
            struct Pair {
                value: u8
                value: u16
            }
            fn main() { test.pass() }
        "#;
        let program = parse_program(Path::new("game.ezra"), source).unwrap();
        let error = emit_ez80_assembly(&program).unwrap_err();

        assert_eq!(error.message, "duplicate struct field `value`");
    }

    #[test]
    fn rejects_array_and_struct_function_values() {
        let cases = [
            (
                r#"
                fn bad(values: [u8; 2]) {}
                fn main() { test.pass() }
                "#,
                "function `bad` parameter `values` type `[u8; 2]` is an array; pass it by pointer",
            ),
            (
                r#"
                fn bad() -> [u8; 2] {
                    return [1, 2]
                }
                fn main() { test.pass() }
                "#,
                "function `bad` return type `[u8; 2]` is an array; pass it by pointer",
            ),
            (
                r#"
                struct Pair { x: u8 }
                fn bad(value: Pair) {}
                fn main() { test.pass() }
                "#,
                "function `bad` parameter `value` type `Pair` is a struct; pass it by pointer",
            ),
            (
                r#"
                struct Pair { x: u8 }
                fn bad() -> Pair {
                    return Pair { x: 1 }
                }
                fn main() { test.pass() }
                "#,
                "function `bad` return type `Pair` is a struct; pass it by pointer",
            ),
            (
                r#"
                alias Bytes = [u8; 2]
                fn bad(values: Bytes) {}
                fn main() { test.pass() }
                "#,
                "function `bad` parameter `values` type `Bytes` is an array; pass it by pointer",
            ),
            (
                r#"
                struct Pair { x: u8 }
                alias AliasPair = Pair
                fn bad(value: AliasPair) {}
                fn main() { test.pass() }
                "#,
                "function `bad` parameter `value` type `AliasPair` is a struct; pass it by pointer",
            ),
        ];

        for (source, expected) in cases {
            let program = parse_program(Path::new("game.ezra"), source).unwrap();
            let error = emit_ez80_assembly(&program).unwrap_err();

            assert_eq!(error.message, expected);
        }
    }

    #[test]
    fn emits_and_runs_recursive_function_calls() {
        let source = r#"
            fn sum_to(value: u8) -> u8 {
                if value == 0 {
                    return 0
                }
                let current: u8 = value
                return current + sum_to(value - 1)
            }

            fn even(value: u8) -> bool {
                if value == 0 {
                    return true
                }
                return odd(value - 1)
            }

            fn odd(value: u8) -> bool {
                if value == 0 {
                    return false
                }
                return even(value - 1)
            }

            fn main() {
                test.assert_eq_u8(sum_to(4), 10, 1)
                test.assert_eq_u8(even(6), true, 2)
                test.assert_eq_u8(odd(6), false, 3)
                test.pass()
            }
        "#;
        let program = parse_program(Path::new("game.ezra"), source).unwrap();
        let asm = emit_ez80_assembly(&program).unwrap();
        let run = run_assembly_test(&asm, 80_000).unwrap();

        assert!(asm.contains("call _sum_to"), "{asm}");
        assert!(asm.contains("call _odd"), "{asm}");
        assert!(run.halted, "{asm}");
        assert_eq!(run.result_code, 0, "{asm}");
    }

    #[test]
    fn emits_and_runs_recursive_function_with_stack_arguments() {
        let source = r#"
            fn stepped(value: u8, base: u8, filler: u8, step: u8) -> u8 {
                if value == 0 {
                    return base
                }
                let saved_step: u8 = step
                return saved_step + stepped(value - 1, base, filler, step)
            }

            fn main() {
                test.assert_eq_u8(stepped(3, 2, 7, 4), 14, 1)
                test.pass()
            }
        "#;
        let program = parse_program(Path::new("game.ezra"), source).unwrap();
        let asm = emit_ez80_assembly(&program).unwrap();
        let run = run_assembly_test(&asm, 12_000).unwrap();

        assert!(run.halted, "{asm}");
        assert_eq!(run.result_code, 0, "{asm}");
    }

    #[test]
    fn omits_unused_private_functions_but_preserves_public_functions() {
        let source = r#"
            fn used(value: u8) -> u8 {
                return value + 1
            }

            fn unused_private(value: u8) -> u8 {
                return value + 2
            }

            pub fn exported(value: u8) -> u8 {
                return value + 3
            }

            pub inline fn exported_inline(value: u8) -> u8 {
                return value + 4
            }

            fn main() {
                test.assert_eq_u8(used(4), 5, 1)
                test.pass()
            }
        "#;
        let program = parse_program(Path::new("game.ezra"), source).unwrap();
        let asm = emit_ez80_assembly(&program).unwrap();
        let run = run_assembly_test(&asm, 4_000).unwrap();

        assert!(run.halted, "{asm}");
        assert_eq!(run.result_code, 0, "{asm}");
        assert!(asm.contains("_used:"));
        assert!(asm.contains("_exported:"));
        assert!(asm.contains("_exported_inline:"));
        assert!(!asm.contains("_unused_private:"));
    }

    #[test]
    fn validates_calls_in_unused_private_functions_before_omitting_them() {
        let source = r#"
            fn unused_private() {
                missing()
            }

            fn main() {
                test.pass()
            }
        "#;
        let program = parse_program(Path::new("game.ezra"), source).unwrap();
        let error = emit_ez80_assembly(&program).unwrap_err();

        assert_eq!(error.message, "unknown function `missing`");
    }

    #[test]
    fn omits_unreachable_statements_after_terminators() {
        let source = r#"
            fn choose(flag: bool) -> u8 {
                if flag {
                    return 1
                } else {
                    return 2
                }
                test.fail(7)
                return 3
            }

            fn main() {
                test.assert_eq_u8(choose(true), 1, 1)
                test.assert_eq_u8(choose(false), 2, 2)
                test.pass()
            }
        "#;
        let program = parse_program(Path::new("game.ezra"), source).unwrap();
        let asm = emit_ez80_assembly_with_debug_comments(&program, true).unwrap();
        let run = run_assembly_test(&asm, 4_000).unwrap();

        assert!(!asm.contains("; source: test.fail(7)"), "{asm}");
        assert!(!asm.contains("; source: return 3"), "{asm}");
        assert!(run.halted, "{asm}");
        assert_eq!(run.result_code, 0, "{asm}");
    }

    #[test]
    fn omits_unreachable_statements_after_nonbreaking_loop() {
        let source = r#"
            fn exit_loop() {
                loop {
                    return
                }
                test.fail(7)
            }

            fn main() {
                exit_loop()
                test.fail(8)
            }
        "#;
        let program = parse_program(Path::new("game.ezra"), source).unwrap();
        let asm = emit_ez80_assembly_with_debug_comments(&program, true).unwrap();
        let run = run_assembly_test(&asm, 4_000).unwrap();

        assert!(!asm.contains("; source: test.fail(7)"), "{asm}");
        assert!(run.halted, "{asm}");
        assert_eq!(run.result_code, 8, "{asm}");
    }

    #[test]
    fn validates_unreachable_statements_before_omitting_them() {
        let source = r#"
            fn done() {
                return;
                let value: u8 = 0x100
            }

            fn main() {
                done()
                test.pass()
            }
        "#;
        let program = parse_program(Path::new("game.ezra"), source).unwrap();
        let error = emit_ez80_assembly(&program).unwrap_err();

        assert_eq!(error.message, "value 256 is outside u8 range");
    }

    #[test]
    fn omits_constant_dead_if_and_while_branches() {
        let source = r#"
            const RUN_COLD: bool = false

            fn cold() {
                test.fail(9)
            }

            fn choose() -> u8 {
                if RUN_COLD {
                    cold()
                    return 9
                } else {
                    return 4
                }
            }

            fn main() {
                while false {
                    test.fail(7)
                }
                test.assert_eq_u8(choose(), 4, 1)
                test.pass()
            }
        "#;
        let program = parse_program(Path::new("game.ezra"), source).unwrap();
        let asm = emit_ez80_assembly_with_debug_comments(&program, true).unwrap();
        let run = run_assembly_test(&asm, 4_000).unwrap();

        assert!(!asm.contains("_cold:"), "{asm}");
        assert!(!asm.contains("; source: cold()"), "{asm}");
        assert!(!asm.contains("; source: test.fail(7)"), "{asm}");
        assert!(!asm.contains("; source: return 9"), "{asm}");
        assert!(run.halted, "{asm}");
        assert_eq!(run.result_code, 0, "{asm}");
    }

    #[test]
    fn omits_constant_true_while_condition_checks() {
        let source = r#"
            const KEEP_RUNNING: bool = true

            fn main() {
                let count: u8 = 0
                while KEEP_RUNNING {
                    count += 1
                    if count == 3 {
                        break
                    }
                }
                test.assert_eq_u8(count, 3, 1)
                test.pass()
            }
        "#;
        let program = parse_program(Path::new("game.ezra"), source).unwrap();
        let asm = emit_ez80_assembly_with_debug_comments(&program, true).unwrap();
        let run = run_assembly_test(&asm, 4_000).unwrap();
        let while_body = asm
            .split("; source: while KEEP_RUNNING")
            .nth(1)
            .and_then(|tail| tail.split("; source: test.assert_eq_u8").next())
            .unwrap();

        assert!(!while_body.contains("    jp z, .L_endwhile"), "{asm}");
        assert!(while_body.contains("    jp .L_while"), "{asm}");
        assert!(run.halted, "{asm}");
        assert_eq!(run.result_code, 0, "{asm}");
    }

    #[test]
    fn omits_unreachable_statements_after_const_true_while_return() {
        let source = r#"
            const KEEP_RUNNING: bool = true

            fn done() {
                while KEEP_RUNNING {
                    return
                }
                test.fail(7)
            }

            fn choose() -> u8 {
                if KEEP_RUNNING {
                    return 5
                }
                test.fail(8)
                return 9
            }

            fn main() {
                done()
                test.assert_eq_u8(choose(), 5, 1)
                test.pass()
            }
        "#;
        let program = parse_program(Path::new("game.ezra"), source).unwrap();
        let asm = emit_ez80_assembly_with_debug_comments(&program, true).unwrap();
        let run = run_assembly_test(&asm, 4_000).unwrap();

        assert!(!asm.contains("; source: test.fail(7)"), "{asm}");
        assert!(!asm.contains("; source: test.fail(8)"), "{asm}");
        assert!(!asm.contains("; source: return 9"), "{asm}");
        assert!(run.halted, "{asm}");
        assert_eq!(run.result_code, 0, "{asm}");
    }

    #[test]
    fn validates_constant_dead_branches_before_omitting_them() {
        let source = r#"
            fn main() {
                if false {
                    let value: u8 = 0x100
                }
                test.pass()
            }
        "#;
        let program = parse_program(Path::new("game.ezra"), source).unwrap();
        let error = emit_ez80_assembly(&program).unwrap_err();

        assert_eq!(error.message, "value 256 is outside u8 range");
    }

    #[test]
    fn omits_private_functions_only_called_from_unreachable_statements() {
        let source = r#"
            fn unreachable_private() {
                test.fail(7)
            }

            fn done() {
                return;
                unreachable_private()
            }

            fn main() {
                done()
                test.pass()
            }
        "#;
        let program = parse_program(Path::new("game.ezra"), source).unwrap();
        let asm = emit_ez80_assembly_with_debug_comments(&program, true).unwrap();
        let run = run_assembly_test(&asm, 4_000).unwrap();

        assert!(!asm.contains("_unreachable_private:"), "{asm}");
        assert!(!asm.contains("; source: unreachable_private()"), "{asm}");
        assert!(run.halted, "{asm}");
        assert_eq!(run.result_code, 0, "{asm}");
    }

    #[test]
    fn propagates_local_scalar_constants_until_assignment() {
        let source = r#"
            fn copied() -> u8 {
                let base: u8 = 4
                let derived: u8 = base + 3
                return derived
            }

            fn assigned() -> u8 {
                let value: u8 = 4
                value = value + 1
                return value
            }

            fn main() {
                test.assert_eq_u8(copied(), 7, 1)
                test.assert_eq_u8(assigned(), 5, 2)
                test.pass()
            }
        "#;
        let program = parse_program(Path::new("game.ezra"), source).unwrap();
        let asm = emit_ez80_assembly(&program).unwrap();
        let run = run_assembly_test(&asm, 4_000).unwrap();
        let copied = asm
            .split("_copied:")
            .nth(1)
            .and_then(|tail| tail.split("_assigned:").next())
            .unwrap();
        let assigned = asm
            .split("_assigned:")
            .nth(1)
            .and_then(|tail| tail.split("section .header").next())
            .unwrap();

        assert!(copied.contains("    ld a, 07h\n    ret"), "{asm}");
        assert!(assigned.contains("    ld a, (040"), "{asm}");
        assert!(run.halted, "{asm}");
        assert_eq!(run.result_code, 0, "{asm}");
    }

    #[test]
    fn propagates_local_pointer_constants_until_assignment() {
        let source = r#"
            global byte: u8 = 0

            fn copied_ptr() -> u24 {
                let base: ptr<u8> = &byte
                let copied: ptr<u8> = base
                return cast<u24>(copied)
            }

            fn copied_raw() -> u24 {
                let raw: ptr24 = cast<ptr24>(&byte)
                return cast<u24>(raw)
            }

            fn assigned_ptr() -> u24 {
                let value: ptr<u8> = &byte
                value = value + 1
                return cast<u24>(value)
            }

            fn main() {
                test.assert_eq_u24(copied_ptr(), cast<u24>(&byte), 1)
                test.assert_eq_u24(copied_raw(), cast<u24>(&byte), 2)
                test.assert_eq_u24(assigned_ptr(), cast<u24>(&byte) + 1, 3)
                test.pass()
            }
        "#;
        let program = parse_program(Path::new("game.ezra"), source).unwrap();
        let asm = emit_ez80_assembly(&program).unwrap();
        let run = run_assembly_test(&asm, 6_000).unwrap();
        let copied_ptr = asm
            .split("_copied_ptr:")
            .nth(1)
            .and_then(|tail| tail.split("_copied_raw:").next())
            .unwrap();
        let copied_raw = asm
            .split("_copied_raw:")
            .nth(1)
            .and_then(|tail| tail.split("_assigned_ptr:").next())
            .unwrap();
        let assigned_ptr = asm
            .split("_assigned_ptr:")
            .nth(1)
            .and_then(|tail| tail.split("section .header").next())
            .unwrap();

        assert!(copied_ptr.contains("    ld hl, 040"), "{asm}");
        assert!(copied_ptr.contains("    ret"), "{asm}");
        assert!(copied_raw.contains("    ld hl, 040"), "{asm}");
        assert!(copied_raw.contains("    ret"), "{asm}");
        assert!(assigned_ptr.contains("    ld hl, (040"), "{asm}");
        assert!(run.halted, "{asm}");
        assert_eq!(run.result_code, 0, "{asm}");
    }

    #[test]
    fn rejects_local_shadowing() {
        let source = r#"
            global score: u8 = 0

            fn bump(value: u8) {
                let value: u8 = 1
            }

            fn main() {
                let score: u8 = 1
                test.pass()
            }
        "#;
        let program = parse_program(Path::new("game.ezra"), source).unwrap();
        let error = emit_ez80_assembly(&program).unwrap_err();

        assert_eq!(error.message, "local `score` shadows an existing name");
    }

    #[test]
    fn rejects_forbidden_integer_widths() {
        let source = r#"
            global score: u32 = 0
            fn main() { test.pass() }
        "#;
        let program = parse_program(Path::new("game.ezra"), source).unwrap();
        let error = emit_ez80_assembly(&program).unwrap_err();

        assert_eq!(
            error.message,
            "type `u32` is not supported; use explicit u8/u16/u24 or i8/i16/i24"
        );
    }

    #[test]
    fn rejects_unknown_types() {
        let cases = [
            r#"
            global value: Missing = 0
            fn main() { test.pass() }
            "#,
            r#"
            fn takes_missing(value: Missing) {}
            fn main() { test.pass() }
            "#,
            r#"
            fn returns_missing() -> Missing {
                return 0
            }
            fn main() { test.pass() }
            "#,
            r#"
            alias MissingAlias = Missing
            fn main() { test.pass() }
            "#,
            r#"
            alias MissingPtr = ptr<Missing>
            fn main() { test.pass() }
            "#,
        ];

        for source in cases {
            let program = parse_program(Path::new("game.ezra"), source).unwrap();
            let error = emit_ez80_assembly(&program).unwrap_err();

            assert_eq!(error.message, "unknown type `Missing`");
        }
    }

    #[test]
    fn rejects_constant_values_outside_declared_type_range() {
        let cases = [
            (
                r#"
                const NEG: u8 = -1
                fn main() { test.pass() }
                "#,
                "value -1 is outside u8 range",
            ),
            (
                r#"
                const WIDE: i8 = 128
                fn main() { test.pass() }
                "#,
                "value 128 is outside i8 range",
            ),
            (
                r#"
                const WIDE: i8 = 128i8
                fn main() { test.pass() }
                "#,
                "value 128 is outside i8 range",
            ),
            (
                r#"
                alias tiny = i8
                const WIDE: tiny = -129
                fn main() { test.pass() }
                "#,
                "value -129 is outside i8 range",
            ),
            (
                r#"
                const BAD: bool = 2
                fn main() { test.pass() }
                "#,
                "value 2 is outside bool range",
            ),
            (
                r#"
                global WIDE: u16 = cast<u16>(300u8)
                fn main() { test.pass() }
                "#,
                "value 300 is outside u8 range",
            ),
            (
                r#"
                fn takes_word(value: u16) {}
                fn main() {
                    takes_word(cast<u16>(300u8))
                    test.pass()
                }
                "#,
                "value 300 is outside u8 range",
            ),
            (
                r#"
                fn bad() -> u16 {
                    return cast<u16>(300u8)
                }
                fn main() { test.pass() }
                "#,
                "value 300 is outside u8 range",
            ),
        ];

        for (source, expected) in cases {
            let program = parse_program(Path::new("game.ezra"), source).unwrap();
            let error = emit_ez80_assembly(&program).unwrap_err();

            assert_eq!(error.message, expected);
        }
    }

    #[test]
    fn rejects_embed_alignment_outside_address_space() {
        let source = r#"
            embed sprite: bytes = bytes [0xAA] align 0x100000000
            fn main() { test.pass() }
        "#;
        let program = parse_program(Path::new("game.ezra"), source).unwrap();
        let error = emit_ez80_assembly(&program).unwrap_err();

        assert_eq!(
            error.message,
            "embed `sprite` alignment 4294967296 exceeds 24-bit address space"
        );
    }

    #[test]
    fn rejects_non_integer_embed_alignment() {
        let cases = [
            r#"
            embed sprite: bytes = bytes [0xAA] align true
            fn main() { test.pass() }
            "#,
            r#"
            const ALIGN: bool = true
            embed sprite: bytes = bytes [0xAA] align ALIGN
            fn main() { test.pass() }
            "#,
            r#"
            embed sprite: bytes = bytes [0xAA] align (1 == 1)
            fn main() { test.pass() }
            "#,
        ];

        for source in cases {
            let program = parse_program(Path::new("game.ezra"), source).unwrap();
            let error = emit_ez80_assembly(&program).unwrap_err();

            assert_eq!(
                error.message,
                "embed `sprite` alignment must be an integer constant"
            );
        }
    }

    #[test]
    fn rejects_non_pointer_mmio_types() {
        let cases = [
            (
                r#"
                volatile mmio STATUS: u8 = 0x080000
                fn main() { test.pass() }
                "#,
                "mmio `STATUS` type `u8` must be a pointer type",
            ),
            (
                r#"
                alias byte = u8
                volatile mmio STATUS: byte = 0x080000
                fn main() { test.pass() }
                "#,
                "mmio `STATUS` type `byte` must be a pointer type",
            ),
        ];

        for (source, expected) in cases {
            let program = parse_program(Path::new("game.ezra"), source).unwrap();
            let error = emit_ez80_assembly(&program).unwrap_err();

            assert_eq!(error.message, expected);
        }
    }

    #[test]
    fn validates_port_declaration_types() {
        let ok = r#"
            alias byte = u8
            port DEBUG: byte = 0x0C

            fn main() {
                out DEBUG, 65
                test.pass()
            }
        "#;
        let program = parse_program(Path::new("game.ezra"), ok).unwrap();
        let asm = emit_ez80_assembly(&program).unwrap();
        let run = run_assembly_test(&asm, 4_000).unwrap();

        assert!(run.halted, "{asm}");
        assert_eq!(run.result_code, 0, "{asm}");

        let cases = [
            (
                r#"
                port WIDE: u16 = 0x01
                fn main() { test.pass() }
                "#,
                "port `WIDE` type `u16` must be u8",
            ),
            (
                r#"
                alias word = u16
                port BAD: word = 0x01
                fn main() { test.pass() }
                "#,
                "port `BAD` type `word` must be u8",
            ),
        ];

        for (source, expected) in cases {
            let program = parse_program(Path::new("game.ezra"), source).unwrap();
            let error = emit_ez80_assembly(&program).unwrap_err();

            assert_eq!(error.message, expected);
        }
    }

    #[test]
    fn rejects_signed_unsigned_arithmetic_mix_without_cast() {
        let cases = [
            r#"
            fn main() {
                let signed: i8 = 1
                let unsigned: u8 = 2
                let mixed: i8 = signed + unsigned
                test.pass()
            }
            "#,
            r#"
            const SIGNED: i16 = -1
            const UNSIGNED: u16 = 2
            fn main() {
                let mixed: i16 = SIGNED + UNSIGNED
                test.pass()
            }
            "#,
            r#"
            fn main() {
                let mixed: i8 = 1i8 + 2u8
                test.pass()
            }
            "#,
        ];

        for source in cases {
            let program = parse_program(Path::new("game.ezra"), source).unwrap();
            let error = emit_ez80_assembly(&program).unwrap_err();

            assert_eq!(error.message, "signed/unsigned mix without cast");
        }
    }

    #[test]
    fn emits_and_runs_arithmetic_with_fitting_untyped_literals() {
        let source = r#"
            const BASE: u16 = 0x0100
            const SUM: u16 = BASE + 2

            fn main() {
                let word: u16 = 0x0100
                let plus: u16 = word + 2
                let minus: u16 = 0x0105 - word
                let signed: i16 = -3
                let bumped: i16 = signed + 2
                let zero: i16 = bumped + 1
                test.assert_eq_u24(cast<u24>(SUM), 0x000102, 1)
                test.assert_eq_u24(cast<u24>(plus), 0x000102, 2)
                test.assert_eq_u24(cast<u24>(minus), 0x000005, 3)
                test.assert_eq_u24(cast<u24>(zero), 0, 4)
                test.pass()
            }
        "#;
        let program = parse_program(Path::new("game.ezra"), source).unwrap();
        let asm = emit_ez80_assembly(&program).unwrap();
        let run = run_assembly_test(&asm, 4_000).unwrap();

        assert!(run.halted, "{asm}");
        assert_eq!(run.result_code, 0, "{asm}");
    }

    #[test]
    fn rejects_arithmetic_width_mismatch_without_cast() {
        let cases = [
            (
                r#"
                fn main() {
                    let byte: u8 = 1
                    let word: u16 = 2
                    let mixed: u16 = byte + word
                    test.pass()
                }
                "#,
                "arithmetic operands must have same width without cast",
            ),
            (
                r#"
                const BYTE: u8 = 1
                const WORD: u16 = 2
                const MIXED: u16 = BYTE + WORD
                fn main() { test.pass() }
                "#,
                "arithmetic operands must have same width without cast",
            ),
            (
                r#"
                fn main() {
                    let byte: u8 = 1
                    let mixed: u16 = byte + 300
                    test.pass()
                }
                "#,
                "value 300 is outside u8 range",
            ),
            (
                r#"
                const BYTE: u8 = 1
                const MIXED: u16 = BYTE + 300
                fn main() { test.pass() }
                "#,
                "value 300 is outside u8 range",
            ),
        ];

        for (source, expected) in cases {
            let program = parse_program(Path::new("game.ezra"), source).unwrap();
            let error = emit_ez80_assembly(&program).unwrap_err();

            assert_eq!(error.message, expected);
        }
    }

    #[test]
    fn rejects_invalid_comparison_operand_types() {
        let cases = [
            (
                r#"
                fn main() {
                    let signed: i8 = 1
                    let unsigned: u8 = 1
                    let same: bool = signed == unsigned
                    test.pass()
                }
                "#,
                "signed/unsigned mix without cast",
            ),
            (
                r#"
                fn main() {
                    let byte: u8 = 1
                    let word: u16 = 1
                    let same: bool = byte == word
                    test.pass()
                }
                "#,
                "comparison operands must have same width without cast",
            ),
            (
                r#"
                fn main() {
                    let left: bool = false
                    let right: bool = true
                    let ordered: bool = left < right
                    test.pass()
                }
                "#,
                "type mismatch",
            ),
            (
                r#"
                global byte: u8 = 0
                global word: u16 = 0
                fn main() {
                    let bp: ptr<u8> = &byte
                    let wp: ptr<u16> = &word
                    let same: bool = bp == wp
                    test.pass()
                }
                "#,
                "type mismatch",
            ),
            (
                r#"
                global left: u8 = 0
                global right: u8 = 0
                fn main() {
                    let lp: ptr<u8> = &left
                    let rp: ptr<u8> = &right
                    let ordered: bool = lp < rp
                    test.pass()
                }
                "#,
                "pointer comparisons support only == and !=",
            ),
            (
                r#"
                const BYTE: u8 = 1
                const WORD: u16 = 1
                const SAME: bool = BYTE == WORD
                fn main() { test.pass() }
                "#,
                "comparison operands must have same width without cast",
            ),
            (
                r#"
                fn main() {
                    let byte: u8 = 1
                    let same: bool = byte == 300
                    test.pass()
                }
                "#,
                "value 300 is outside u8 range",
            ),
            (
                r#"
                const BYTE: u8 = 1
                const SAME: bool = BYTE == 300
                fn main() { test.pass() }
                "#,
                "value 300 is outside u8 range",
            ),
        ];

        for (source, expected) in cases {
            let program = parse_program(Path::new("game.ezra"), source).unwrap();
            let error = emit_ez80_assembly(&program).unwrap_err();

            assert_eq!(error.message, expected);
        }
    }

    #[test]
    fn rejects_invalid_const_expression_operand_types() {
        let cases = [
            (
                r#"
                const SIGNED: i16 = -1
                const UNSIGNED: u16 = 2
                const MIXED: i16 = SIGNED + UNSIGNED
                fn main() { test.pass() }
                "#,
                "signed/unsigned mix without cast",
            ),
            (
                r#"
                const FLAG: bool = true
                const VALUE: u8 = 1
                const BAD: u8 = FLAG + VALUE
                fn main() { test.pass() }
                "#,
                "type mismatch",
            ),
        ];

        for (source, expected) in cases {
            let program = parse_program(Path::new("game.ezra"), source).unwrap();
            let error = emit_ez80_assembly(&program).unwrap_err();

            assert_eq!(error.message, expected);
        }
    }

    #[test]
    fn rejects_invalid_array_index_types() {
        let cases = [
            (
                r#"
                global bytes: [u8; 2] = [1, 2]
                fn main() {
                    let flag: bool = true
                    let value: u8 = bytes[flag]
                    test.pass()
                }
                "#,
                "array index type `bool` is not supported; use u8, u16, or u24",
            ),
            (
                r#"
                global bytes: [u8; 2] = [1, 2]
                fn main() {
                    let idx: i8 = 1
                    bytes[idx] = 7
                    test.pass()
                }
                "#,
                "array index type `i8` is not supported; use u8, u16, or u24",
            ),
            (
                r#"
                global bytes: [u8; 2] = [1, 2]
                fn main() {
                    let ptr: ptr<u8> = &bytes[0]
                    let p: ptr<u8> = &bytes[ptr]
                    test.pass()
                }
                "#,
                "array index type `ptr<u8>` is not supported; use u8, u16, or u24",
            ),
            (
                r#"
                global bytes: [u8; 2] = [1, 2]
                fn main() {
                    let value: u8 = bytes[-1]
                    test.pass()
                }
                "#,
                "array index value -1 is outside u24 range",
            ),
            (
                r#"
                global bytes: [u8; 2] = [1, 2]
                fn main() {
                    let value: u8 = bytes[2]
                    test.pass()
                }
                "#,
                "array index 2 is out of bounds for `bytes` length 2",
            ),
            (
                r#"
                global bytes: [u8; 2] = [1, 2]
                fn main() {
                    bytes[2] = 7
                    test.pass()
                }
                "#,
                "array index 2 is out of bounds for `bytes` length 2",
            ),
            (
                r#"
                global bytes: [u8; 2] = [1, 2]
                fn main() {
                    let p: ptr<u8> = &bytes[2]
                    test.pass()
                }
                "#,
                "array index 2 is out of bounds for `bytes` length 2",
            ),
            (
                r#"
                const IDX: u8 = 2
                global bytes: [u8; 2] = [1, 2]
                fn main() {
                    let value: u8 = bytes[IDX]
                    test.pass()
                }
                "#,
                "array index 2 is out of bounds for `bytes` length 2",
            ),
            (
                r#"
                global grid: [[u8; 2]; 2] = [[1, 2], [3, 4]]
                fn main() {
                    let row: u8 = 0
                    let value: u8 = grid[row][2]
                    test.pass()
                }
                "#,
                "array index 2 is out of bounds for `grid[row][2]` length 2",
            ),
        ];

        for (source, expected) in cases {
            let program = parse_program(Path::new("game.ezra"), source).unwrap();
            let error = emit_ez80_assembly(&program).unwrap_err();

            assert_eq!(error.message, expected);
        }
    }

    #[test]
    fn rejects_array_lengths_outside_address_space() {
        let cases = [
            (
                r#"
                global bytes: [u8; 0x1000000] = []
                fn main() { test.pass() }
                "#,
                "array length 16777216 exceeds 24-bit address space",
            ),
            (
                r#"
                const LEN: u24 = 0xFFFFFF
                global bytes: [u8; LEN + 1] = []
                fn main() { test.pass() }
                "#,
                "array length 16777216 exceeds 24-bit address space",
            ),
            (
                r#"
                fn main() {
                    let bytes: [u8; 0x1000000] = []
                    test.pass()
                }
                "#,
                "array length 16777216 exceeds 24-bit address space",
            ),
        ];

        for (source, expected) in cases {
            let program = parse_program(Path::new("game.ezra"), source).unwrap();
            let error = emit_ez80_assembly(&program).unwrap_err();

            assert_eq!(error.message, expected);
        }
    }

    #[test]
    fn rejects_assignment_width_changes_without_cast() {
        let cases = [
            (
                r#"
                fn main() {
                    let small: u8 = 1
                    let wide: u16 = small
                    test.pass()
                }
                "#,
                "widening without cast",
            ),
            (
                r#"
                fn main() {
                    let wide: u16 = 0x1234
                    let small: u8 = wide
                    test.pass()
                }
                "#,
                "narrowing without cast",
            ),
            (
                r#"
                fn value() -> u8 {
                    let wide: u16 = 1
                    return wide
                }
                fn main() { test.pass() }
                "#,
                "narrowing without cast",
            ),
            (
                r#"
                fn main() {
                    let wide: u16 = 1
                    let small: u8 = 0
                    small = wide
                    test.pass()
                }
                "#,
                "narrowing without cast",
            ),
        ];

        for (source, expected) in cases {
            let program = parse_program(Path::new("game.ezra"), source).unwrap();
            let error = emit_ez80_assembly(&program).unwrap_err();

            assert_eq!(error.message, expected);
        }
    }

    #[test]
    fn rejects_indirect_assignment_type_changes_without_cast() {
        let cases = [
            (
                r#"
                global bytes: [u8; 2] = [0, 0]
                fn main() {
                    let wide: u16 = 0x1234
                    bytes[0] = wide
                    test.pass()
                }
                "#,
                "narrowing without cast",
            ),
            (
                r#"
                global words: [u16; 2] = [0, 0]
                fn main() {
                    let small: u8 = 1
                    let index: u8 = 1
                    words[index] = small
                    test.pass()
                }
                "#,
                "widening without cast",
            ),
            (
                r#"
                global signed: [i8; 1] = [0]
                fn main() {
                    let unsigned: u8 = 1
                    signed[0] = unsigned
                    test.pass()
                }
                "#,
                "signed/unsigned mix without cast",
            ),
            (
                r#"
                global byte: u8 = 0
                fn main() {
                    let p: ptr<u8> = &byte;
                    let wide: u16 = 1;
                    *p = wide;
                    test.pass()
                }
                "#,
                "narrowing without cast",
            ),
        ];

        for (source, expected) in cases {
            let program = parse_program(Path::new("game.ezra"), source).unwrap();
            let error = emit_ez80_assembly(&program).unwrap_err();

            assert_eq!(error.message, expected);
        }
    }

    #[test]
    fn rejects_initializer_type_changes_without_cast() {
        let cases = [
            (
                r#"
                global words: [u16; 2] = [1, 2]
                fn main() {
                    let small: u8 = 1
                    let values: [u16; 2] = [small, 2]
                    test.pass()
                }
                "#,
                "widening without cast",
            ),
            (
                r#"
                global values: [i8; 1] = [0]
                fn main() {
                    let unsigned: u8 = 1
                    let local: [i8; 1] = [unsigned]
                    test.pass()
                }
                "#,
                "signed/unsigned mix without cast",
            ),
            (
                r#"
                struct Pair { value: u8 }
                fn main() {
                    let wide: u16 = 1
                    let pair: Pair = Pair { value: wide }
                    test.pass()
                }
                "#,
                "narrowing without cast",
            ),
            (
                r#"
                struct Pair { value: i8 }
                fn main() {
                    let unsigned: u8 = 1
                    let pair: Pair = Pair { value: unsigned }
                    test.pass()
                }
                "#,
                "signed/unsigned mix without cast",
            ),
        ];

        for (source, expected) in cases {
            let program = parse_program(Path::new("game.ezra"), source).unwrap();
            let error = emit_ez80_assembly(&program).unwrap_err();

            assert_eq!(error.message, expected);
        }
    }

    #[test]
    fn rejects_call_argument_type_changes_without_cast() {
        let cases = [
            (
                r#"
                fn takes_wide(value: u16) {}
                fn main() {
                    let small: u8 = 1
                    takes_wide(small)
                    test.pass()
                }
                "#,
                "widening without cast",
            ),
            (
                r#"
                fn takes_small(value: u8) {}
                fn main() {
                    let wide: u16 = 0x1234
                    takes_small(wide)
                    test.pass()
                }
                "#,
                "narrowing without cast",
            ),
            (
                r#"
                fn takes_unsigned(value: u8) {}
                fn main() {
                    let signed: i8 = 1
                    takes_unsigned(signed)
                    test.pass()
                }
                "#,
                "signed/unsigned mix without cast",
            ),
        ];

        for (source, expected) in cases {
            let program = parse_program(Path::new("game.ezra"), source).unwrap();
            let error = emit_ez80_assembly(&program).unwrap_err();

            assert_eq!(error.message, expected);
        }
    }

    #[test]
    fn rejects_bool_integer_mismatch() {
        let cases = [
            r#"
            fn main() {
                let value: u8 = true
                test.pass()
            }
            "#,
            r#"
            fn main() {
                let flag: bool = true
                let value: u8 = 1
                let mixed: u8 = flag + value
                test.pass()
            }
            "#,
            r#"
            fn takes_array(values: ptr<[u8; 2]>) {}
            fn main() {
                let values: [u8; 2] = [1, 2]
                takes_array(values)
                test.pass()
            }
            "#,
        ];

        for source in cases {
            let program = parse_program(Path::new("game.ezra"), source).unwrap();
            let error = emit_ez80_assembly(&program).unwrap_err();

            assert_eq!(error.message, "type mismatch");
        }
    }

    #[test]
    fn rejects_non_integer_unary_operands() {
        let cases = [
            (
                r#"
                fn main() {
                    let value: bool = ~true
                    test.pass()
                }
                "#,
                "unary operand must be an integer",
            ),
            (
                r#"
                fn main() {
                    let value: bool = -false
                    test.pass()
                }
                "#,
                "unary operand must be an integer",
            ),
            (
                r#"
                const BAD: bool = ~true
                fn main() { test.pass() }
                "#,
                "unary operand must be an integer",
            ),
            (
                r#"
                global byte: u8 = 1
                fn main() {
                    let ptr: ptr<u8> = &byte
                    let value: u24 = ~ptr
                    test.pass()
                }
                "#,
                "unary operand must be an integer",
            ),
            (
                r#"
                fn main() {
                    let raw: ptr24 = cast<ptr24>(0x040000)
                    let value: ptr24 = -raw
                    test.pass()
                }
                "#,
                "unary operand must be an integer",
            ),
        ];

        for (source, expected) in cases {
            let program = parse_program(Path::new("game.ezra"), source).unwrap();
            let error = emit_ez80_assembly(&program).unwrap_err();

            assert_eq!(error.message, expected);
        }
    }

    #[test]
    fn rejects_unknown_struct_fields() {
        let cases = [
            (
                r#"
                struct Entity { x: u8 }
                global player: Entity = Entity { x: 1 }
                fn main() {
                    let y: u8 = player.y
                    test.pass()
                }
                "#,
                "struct `Entity` has no field `y`",
            ),
            (
                r#"
                struct Entity { x: u8 }
                global player: Entity = Entity { x: 1 }
                fn main() {
                    player.y = 2
                    test.pass()
                }
                "#,
                "struct `Entity` has no field `y`",
            ),
            (
                r#"
                struct Entity { x: u8 }
                global player: Entity = Entity { x: 1 }
                fn main() {
                    let p: ptr<u8> = &player.y
                    test.pass()
                }
                "#,
                "struct `Entity` has no field `y`",
            ),
            (
                r#"
                struct Entity { x: u8 }
                global player: Entity = Entity { x: 1, y: 2 }
                fn main() { test.pass() }
                "#,
                "struct `Entity` has no field `y`",
            ),
            (
                r#"
                struct Inner { x: u8 }
                struct Outer { inner: Inner }
                global outer: Outer = Outer { inner: Inner { x: 1 } }
                fn main() {
                    let value: u8 = outer.inner.y
                    test.pass()
                }
                "#,
                "struct `Inner` has no field `y`",
            ),
        ];

        for (source, expected) in cases {
            let program = parse_program(Path::new("game.ezra"), source).unwrap();
            let error = emit_ez80_assembly(&program).unwrap_err();

            assert_eq!(error.message, expected);
        }
    }

    #[test]
    fn rejects_non_bool_logical_operands_and_conditions() {
        let cases = [
            (
                r#"
                fn main() {
                    let flag: bool = 1 && true
                    test.pass()
                }
                "#,
                "logical operand must be bool",
            ),
            (
                r#"
                const FLAG: bool = 1 || false
                fn main() { test.pass() }
                "#,
                "logical operand must be bool",
            ),
            (
                r#"
                fn main() {
                    let flag: bool = !1
                    test.pass()
                }
                "#,
                "logical operand must be bool",
            ),
            (
                r#"
                fn main() {
                    if 1 {
                        test.pass()
                    }
                }
                "#,
                "if condition must be bool",
            ),
            (
                r#"
                fn main() {
                    while 1 {
                        break
                    }
                    test.pass()
                }
                "#,
                "while condition must be bool",
            ),
        ];

        for (source, expected) in cases {
            let program = parse_program(Path::new("game.ezra"), source).unwrap();
            let error = emit_ez80_assembly(&program).unwrap_err();

            assert_eq!(error.message, expected);
        }
    }

    #[test]
    fn rejects_non_integer_shift_operands_and_counts() {
        let cases = [
            (
                r#"
                fn main() {
                    let value: u8 = true << 1
                    test.pass()
                }
                "#,
                "shift operand must be an integer",
            ),
            (
                r#"
                global byte: u8 = 1
                fn main() {
                    let ptr: ptr<u8> = &byte
                    let value: u24 = ptr >> 1
                    test.pass()
                }
                "#,
                "shift operand must be an integer",
            ),
            (
                r#"
                fn main() {
                    let value: u8 = 1 << false
                    test.pass()
                }
                "#,
                "shift count must be an integer",
            ),
            (
                r#"
                global byte: u8 = 1
                fn main() {
                    let ptr: ptr<u8> = &byte
                    let value: u8 = 1 << ptr
                    test.pass()
                }
                "#,
                "shift count must be an integer",
            ),
            (
                r#"
                fn shift(value: u8, count: i8) -> u8 {
                    return value << count
                }
                fn main() {
                    let value: u8 = shift(1, 1)
                    test.pass()
                }
                "#,
                "runtime shift count must be u8",
            ),
            (
                r#"
                fn shift(value: u16, count: u16) -> u16 {
                    return value >> count
                }
                fn main() {
                    let value: u16 = shift(0x1234, 1)
                    test.pass()
                }
                "#,
                "runtime shift count must be u8",
            ),
            (
                r#"
                const BAD: u8 = true << 1
                fn main() { test.pass() }
                "#,
                "shift operand must be an integer",
            ),
            (
                r#"
                const BAD: u8 = 1 << false
                fn main() { test.pass() }
                "#,
                "shift count must be an integer",
            ),
            (
                r#"
                const BAD: u8 = 1 << -1i8
                fn main() { test.pass() }
                "#,
                "shift count -1 is outside supported range 0..=255",
            ),
            (
                r#"
                fn main() {
                    let value: u8 = 1
                    value <<= false
                    test.pass()
                }
                "#,
                "shift count must be an integer",
            ),
            (
                r#"
                fn shift(value: u24, count: u16) -> u24 {
                    value <<= count
                    return value
                }
                fn main() {
                    let value: u24 = shift(1, 1)
                    test.pass()
                }
                "#,
                "runtime shift count must be u8",
            ),
            (
                r#"
                global byte: u8 = 1
                fn main() {
                    let value: u16 = 1
                    let ptr: ptr<u8> = &byte
                    value >>= ptr
                    test.pass()
                }
                "#,
                "shift count must be an integer",
            ),
        ];

        for (source, expected) in cases {
            let program = parse_program(Path::new("game.ezra"), source).unwrap();
            let error = emit_ez80_assembly(&program).unwrap_err();

            assert_eq!(error.message, expected);
        }
    }

    #[test]
    fn rejects_break_and_continue_outside_loops() {
        let cases = [
            (
                r#"
                fn main() {
                    break
                }
                "#,
                "`break` outside loop",
            ),
            (
                r#"
                fn main() {
                    continue
                }
                "#,
                "`continue` outside loop",
            ),
        ];

        for (source, expected) in cases {
            let program = parse_program(Path::new("game.ezra"), source).unwrap();
            let error = emit_ez80_assembly(&program).unwrap_err();

            assert_eq!(error.message, expected);
        }
    }

    #[test]
    fn rejects_invalid_out_value_types() {
        let cases = [
            (
                r#"
                port DEBUG: u8 = 0x0C
                fn main() {
                    let wide: u16 = 0x1234
                    out DEBUG, wide
                    test.pass()
                }
                "#,
                "narrowing without cast",
            ),
            (
                r#"
                port DEBUG: u8 = 0x0C
                fn main() {
                    let signed: i8 = 1
                    out DEBUG, signed
                    test.pass()
                }
                "#,
                "signed/unsigned mix without cast",
            ),
            (
                r#"
                port DEBUG: u8 = 0x0C
                fn main() {
                    out DEBUG, true
                    test.pass()
                }
                "#,
                "type mismatch",
            ),
        ];

        for (source, expected) in cases {
            let program = parse_program(Path::new("game.ezra"), source).unwrap();
            let error = emit_ez80_assembly(&program).unwrap_err();

            assert_eq!(error.message, expected);
        }
    }

    #[test]
    fn rejects_invalid_mem_builtin_argument_types() {
        let cases = [
            (
                r#"
                fn main() {
                    let value: u8 = mem.peek8(0x040000)
                    test.pass()
                }
                "#,
                "type mismatch",
            ),
            (
                r#"
                fn main() {
                    mem.poke8(cast<ptr<u8>>(0x040000), 0x0100)
                    test.pass()
                }
                "#,
                "value 256 is outside u8 range",
            ),
            (
                r#"
                global src: [u8; 1] = [1]
                global dst: [u8; 1] = [0]
                fn main() {
                    mem.memcpy(&dst[0], &src[0], true)
                    test.pass()
                }
                "#,
                "type mismatch",
            ),
            (
                r#"
                global dst: [u8; 1] = [0]
                fn main() {
                    mem.memset(0x040000, 0, 1)
                    test.pass()
                }
                "#,
                "type mismatch",
            ),
        ];

        for (source, expected) in cases {
            let program = parse_program(Path::new("game.ezra"), source).unwrap();
            let error = emit_ez80_assembly(&program).unwrap_err();

            assert_eq!(error.message, expected);
        }
    }

    #[test]
    fn rejects_invalid_debug_builtin_argument_types() {
        let cases = [
            (
                r#"
                fn main() {
                    let wide: u16 = 0x1234
                    debug.char(wide)
                    test.pass()
                }
                "#,
                "narrowing without cast",
            ),
            (
                r#"
                fn main() {
                    debug.str(0x040000)
                    test.pass()
                }
                "#,
                "type mismatch",
            ),
            (
                r#"
                fn main() {
                    let byte: u8 = 0x12
                    debug.hex_u16(byte)
                    test.pass()
                }
                "#,
                "widening without cast",
            ),
            (
                r#"
                fn main() {
                    let signed: i8 = -1
                    debug.hex_u8(signed)
                    test.pass()
                }
                "#,
                "signed/unsigned mix without cast",
            ),
        ];

        for (source, expected) in cases {
            let program = parse_program(Path::new("game.ezra"), source).unwrap();
            let error = emit_ez80_assembly(&program).unwrap_err();

            assert_eq!(error.message, expected);
        }
    }

    #[test]
    fn rejects_invalid_test_builtin_argument_types() {
        let cases = [
            (
                r#"
                fn main() {
                    test.fail(0x0100)
                }
                "#,
                "value 256 is outside u8 range",
            ),
            (
                r#"
                fn main() {
                    let wide: u16 = 0x0012
                    test.assert_eq_u8(wide, 0x12, 1)
                    test.pass()
                }
                "#,
                "narrowing without cast",
            ),
            (
                r#"
                fn main() {
                    let byte: u8 = 0x12
                    test.assert_eq_u16(byte, 0x0012, 1)
                    test.pass()
                }
                "#,
                "widening without cast",
            ),
            (
                r#"
                fn main() {
                    let pointer: ptr<u8> = cast<ptr<u8>>(0x040000)
                    test.assert_eq_u24(pointer, 0x040000, 1)
                    test.pass()
                }
                "#,
                "type mismatch",
            ),
            (
                r#"
                fn main() {
                    test.assert_eq_u8(true, true, 0x0100)
                    test.pass()
                }
                "#,
                "value 256 is outside u8 range",
            ),
        ];

        for (source, expected) in cases {
            let program = parse_program(Path::new("game.ezra"), source).unwrap();
            let error = emit_ez80_assembly(&program).unwrap_err();

            assert_eq!(error.message, expected);
        }
    }

    #[test]
    fn rejects_invalid_pointer_casts() {
        let cases = [
            (
                r#"
                fn main() {
                    let raw: u16 = 0x1234
                    let p: ptr<u8> = cast<ptr<u8>>(raw)
                    test.pass()
                }
                "#,
                "integer-to-pointer casts require u24 or ptr24",
            ),
            (
                r#"
                global byte: u8 = 0
                fn main() {
                    let p: ptr<u8> = &byte
                    let raw: u16 = cast<u16>(p)
                    test.pass()
                }
                "#,
                "pointer-to-integer casts produce u24 or ptr24",
            ),
        ];

        for (source, expected) in cases {
            let program = parse_program(Path::new("game.ezra"), source).unwrap();
            let error = emit_ez80_assembly(&program).unwrap_err();

            assert_eq!(error.message, expected);
        }
    }

    #[test]
    fn emits_and_runs_ptr24_pointer_casts() {
        let source = r#"
            volatile mmio SCRATCH: ptr24 = 0x040180

            fn read_raw(raw: ptr24) -> u8 {
                let p: ptr<u8> = cast<ptr<u8>>(raw)
                return *p
            }

            fn main() {
                let p: ptr<u8> = cast<ptr<u8>>(SCRATCH);
                *(p) = 0x5A;
                let raw: ptr24 = cast<ptr24>(p);
                test.assert_eq_u8(read_raw(raw), 0x5A, 1);
                test.pass()
            }
        "#;
        let program = parse_program(Path::new("game.ezra"), source).unwrap();
        let asm = emit_ez80_assembly(&program).unwrap();
        let run = run_assembly_test(&asm, 6_000).unwrap();

        assert!(run.halted, "{asm}");
        assert_eq!(run.result_code, 0, "{asm}");
    }

    #[test]
    fn rejects_invalid_constant_pointer_casts() {
        let cases = [
            (
                r#"
                const VRAM_BASE: ptr<u8> = 0x040180
                const RAW: u16 = cast<u16>(VRAM_BASE)

                fn main() {
                    test.pass()
                }
                "#,
                "pointer-to-integer casts produce u24 or ptr24",
            ),
            (
                r#"
                const VRAM_BASE: ptr<u8> = cast<ptr<u8>>(0x1234)

                fn main() {
                    test.pass()
                }
                "#,
                "integer-to-pointer casts require u24 or ptr24",
            ),
        ];

        for (source, expected) in cases {
            let program = parse_program(Path::new("game.ezra"), source).unwrap();
            let error = emit_ez80_assembly(&program).unwrap_err();

            assert_eq!(error.message, expected);
        }
    }

    #[test]
    fn emits_and_runs_constant_ptr24_pointer_casts() {
        let source = r#"
            const RAW: ptr24 = cast<ptr24>(cast<ptr<u8>>(0x040190))
            const BYTE_PTR: ptr<u8> = cast<ptr<u8>>(RAW)

            fn main() {
                *BYTE_PTR = 0x6B;
                test.assert_eq_u8(*BYTE_PTR, 0x6B, 1);
                test.pass()
            }
        "#;
        let program = parse_program(Path::new("game.ezra"), source).unwrap();
        let asm = emit_ez80_assembly(&program).unwrap();
        let run = run_assembly_test(&asm, 4_000).unwrap();

        assert!(run.halted, "{asm}");
        assert_eq!(run.result_code, 0, "{asm}");
    }

    #[test]
    fn emits_and_runs_constant_storage_addresses() {
        let source = r#"
            struct Cell {
                value: u8
                next: u16
            }

            struct Packet {
                cells: [Cell; 2]
            }

            global byte: u8 = 0
            global bytes: [u8; 3] = [0, 0, 0]
            global cell: Cell = Cell { value: 0, next: 0 }
            global packet: Packet = Packet {
                cells: [
                    Cell { value: 0, next: 0 },
                    Cell { value: 0, next: 0 }
                ]
            }

            const BYTE: ptr<u8> = &byte
            const SECOND: ptr<u8> = &bytes[1]
            const CELL_NEXT: ptr<u16> = &cell.next
            const PACKET_NEXT: ptr<u16> = &packet.cells[1].next
            const RAW_THIRD: ptr24 = cast<ptr24>(&bytes[2])

            fn main() {
                let byte_ptr: ptr<u8> = BYTE;
                let second_ptr: ptr<u8> = SECOND;
                let cell_next_ptr: ptr<u16> = CELL_NEXT;
                let packet_next_ptr: ptr<u16> = PACKET_NEXT;
                *(byte_ptr) = 0x11;
                *(second_ptr) = 0x22;
                *(cell_next_ptr) = 0x3344;
                *(packet_next_ptr) = 0x5566;
                let third: ptr<u8> = cast<ptr<u8>>(RAW_THIRD);
                *(third) = 0x77;

                test.assert_eq_u8(byte, 0x11, 1)
                test.assert_eq_u8(bytes[1], 0x22, 2)
                test.assert_eq_u16(cell.next, 0x3344, 3)
                test.assert_eq_u16(packet.cells[1].next, 0x5566, 4)
                test.assert_eq_u8(bytes[2], 0x77, 5)
                test.pass()
            }
        "#;
        let program = parse_program(Path::new("game.ezra"), source).unwrap();
        let asm = emit_ez80_assembly(&program).unwrap();
        let run = run_assembly_test(&asm, 20_000).unwrap();

        assert!(run.halted, "{asm}");
        assert_eq!(run.result_code, 0, "{asm}");
    }

    #[test]
    fn emits_and_runs_forward_constant_storage_addresses() {
        let source = r#"
            struct Pair {
                left: u8
                right: u16
            }

            const SECOND: ptr<u8> = &bytes[1]
            const PAIR_RIGHT: ptr<u16> = &pair.right
            const RAW_THIRD: ptr24 = cast<ptr24>(&bytes[2])

            const MARKER_ALIGN: u8 = 4
            embed marker: bytes = bytes [0xAA, 0xBB] align MARKER_ALIGN
            global prefix: u8 = 0
            global bytes: [u8; 3] = [0, 0, 0]
            global pair: Pair = Pair { left: 0, right: 0 }

            fn main() {
                let second: ptr<u8> = SECOND;
                let pair_right: ptr<u16> = PAIR_RIGHT;
                *(second) = 0x44;
                *(pair_right) = 0x5678;
                let third: ptr<u8> = cast<ptr<u8>>(RAW_THIRD);
                *(third) = 0x99;

                test.assert_eq_u24(cast<u24>(marker.ptr), EZRA_ASSET_BASE, 1)
                test.assert_eq_u24(cast<u24>(&bytes[0]), cast<u24>(&prefix) + 1, 2)
                test.assert_eq_u8(bytes[1], 0x44, 3)
                test.assert_eq_u8(bytes[2], 0x99, 4)
                test.assert_eq_u16(pair.right, 0x5678, 5)
                test.assert_eq_u24(cast<u24>(marker.end), EZRA_ASSET_BASE + 2, 6)
                test.pass()
            }
        "#;
        let program = parse_program(Path::new("game.ezra"), source).unwrap();
        let asm = emit_ez80_assembly(&program).unwrap();
        let run = run_assembly_test(&asm, 20_000).unwrap();

        assert!(run.halted, "{asm}");
        assert_eq!(run.result_code, 0, "{asm}");
    }

    #[test]
    fn emits_and_runs_forward_constants_in_storage_layout() {
        let source = r#"
            global prefix: u8 = 0
            global bytes: [u8; LEN + EXTRA] = [0x11, 0x22, 0x33, 0x44]
            embed marker: bytes = bytes [0xAA] align ALIGN

            const LEN: u8 = BASE + 1
            const EXTRA: u8 = 1
            const BASE: u8 = 2
            const ALIGN: u8 = 8

            fn main() {
                test.assert_eq_u8(bytes[0], 0x11, 1)
                test.assert_eq_u8(bytes[3], 0x44, 2)
                test.assert_eq_u24(cast<u24>(marker.ptr) & 7, 0, 3)
                test.assert_eq_u24(cast<u24>(&bytes[0]), cast<u24>(&prefix) + 1, 4)
                test.pass()
            }
        "#;
        let program = parse_program(Path::new("game.ezra"), source).unwrap();
        let asm = emit_ez80_assembly(&program).unwrap();
        let run = run_assembly_test(&asm, 20_000).unwrap();

        assert!(run.halted, "{asm}");
        assert_eq!(run.result_code, 0, "{asm}");
    }

    #[test]
    fn emits_and_runs_forward_constants_in_hardware_and_alias_declarations() {
        let source = r#"
            alias Row = [u8; ROW_LEN]

            port INPUT: u8 = PORT_BASE + 1
            port OUTPUT: u8 = PORT_BASE + 2
            volatile mmio SCRATCH: ptr<u8> = MMIO_BASE + 0x20
            embed header: bytes = bytes [FILL, FILL + 1] align ALIGN
            embed blank: bytes = repeat(FILL, REPEAT_COUNT)
            global row: Row = [0x11, 0x22, 0x33]

            const ROW_LEN: u8 = 3
            const PORT_BASE: u8 = 0x20
            const MMIO_BASE: u24 = 0x040100
            const FILL: u8 = 0x44
            const ALIGN: u8 = 8
            const REPEAT_COUNT: u8 = 2

            fn main() {
                let value: u8 = in INPUT
                out OUTPUT, value + 1
                mem.poke8(SCRATCH, cast<u8>(header.len + blank.len))
                row[2] = value

                test.assert_eq_u8(value, 0x5A, 1)
                test.assert_eq_u8(mem.peek8(SCRATCH), 4, 2)
                test.assert_eq_u8(row[2], 0x5A, 3)
                test.assert_eq_u8(*(header.ptr + 0), 0x44, 4)
                test.assert_eq_u8(*(header.ptr + 1), 0x45, 5)
                test.assert_eq_u8(*(blank.ptr + 1), 0x44, 6)
                test.assert_eq_u24(cast<u24>(header.ptr) & 7, 0, 7)
                test.pass()
            }
        "#;
        let program = parse_program(Path::new("game.ezra"), source).unwrap();
        let asm = emit_ez80_assembly(&program).unwrap();
        let run = run_assembly_test_with_options(
            &asm,
            &TestRunOptions {
                instruction_budget: 20_000,
                initial_ports: vec![(0x21, 0x5A)],
                initial_memory: Vec::new(),
                stack_top: EZRA_STACK_TOP.get(),
            },
        )
        .unwrap();

        assert!(asm.contains("in0 a, (21h)"), "{asm}");
        assert!(asm.contains("out0 (22h), a"), "{asm}");
        assert!(run.halted, "{asm}");
        assert_eq!(run.result_code, 0, "{asm}");
        assert_eq!(run.ports[0x22], 0x5B, "{asm}");
    }

    #[test]
    fn rejects_invalid_pointer_arithmetic() {
        let cases = [
            (
                r#"
                global left: u8 = 0
                global right: u8 = 0
                fn main() {
                    let lp: ptr<u8> = &left
                    let rp: ptr<u8> = &right
                    let bad: ptr<u8> = lp + rp
                    test.pass()
                }
                "#,
                "pointer arithmetic requires exactly one pointer operand",
            ),
            (
                r#"
                global byte: u8 = 0
                fn main() {
                    let p: ptr<u8> = &byte
                    let bad: ptr<u8> = p - p
                    test.pass()
                }
                "#,
                "pointer subtraction between two pointers is not supported",
            ),
            (
                r#"
                global byte: u8 = 0
                fn main() {
                    let p: ptr<u8> = &byte
                    let bad: ptr<u8> = 1 - p
                    test.pass()
                }
                "#,
                "cannot subtract a pointer from a non-pointer value",
            ),
            (
                r#"
                global byte: u8 = 0
                fn main() {
                    let p: ptr<u8> = &byte
                    let flag: bool = true
                    let bad: ptr<u8> = p + flag
                    test.pass()
                }
                "#,
                "pointer arithmetic offset must be an integer",
            ),
            (
                r#"
                global byte: u8 = 0
                fn main() {
                    let p: ptr<u8> = &byte
                    let bad: u24 = p & 0x00FFFF
                    test.pass()
                }
                "#,
                "type mismatch",
            ),
        ];

        for (source, expected) in cases {
            let program = parse_program(Path::new("game.ezra"), source).unwrap();
            let error = emit_ez80_assembly(&program).unwrap_err();

            assert_eq!(error.message, expected);
        }
    }

    #[test]
    fn rejects_missing_return_value_in_non_void_function() {
        let cases = [
            r#"
                fn answer() -> u8 {
                    let value: u8 = 1
                }

                fn main() { test.pass() }
            "#,
            r#"
                fn answer() -> u8 {
                    loop {
                        break
                        return 1
                    }
                }

                fn main() { test.pass() }
            "#,
            r#"
                fn answer(flag: bool) -> u8 {
                    loop {
                        if flag {
                            break
                        } else {
                            return 1
                        }
                    }
                }

                fn main() { test.pass() }
            "#,
        ];

        for source in cases {
            let program = parse_program(Path::new("game.ezra"), source).unwrap();
            let error = emit_ez80_assembly(&program).unwrap_err();

            assert_eq!(error.message, "missing return value in function `answer`");
        }
    }

    #[test]
    fn rejects_empty_return_in_non_void_function() {
        let source = r#"
            fn answer() -> u8 {
                return
            }

            fn main() { test.pass() }
        "#;
        let program = parse_program(Path::new("game.ezra"), source).unwrap();
        let error = emit_ez80_assembly(&program).unwrap_err();

        assert_eq!(error.message, "missing return value in function `answer`");
    }

    #[test]
    fn rejects_value_return_in_void_function() {
        let source = r#"
            fn main() {
                return 1
            }
        "#;
        let program = parse_program(Path::new("game.ezra"), source).unwrap();
        let error = emit_ez80_assembly(&program).unwrap_err();

        assert_eq!(error.message, "void function `main` cannot return a value");
    }

    #[test]
    fn rejects_void_function_calls_used_as_values() {
        let cases = [
            r#"
                fn effect() {}

                fn main() {
                    let value: u8 = effect()
                    test.pass()
                }
            "#,
            r#"
                fn effect() {}

                fn main() {
                    if effect() {
                        test.pass()
                    }
                    test.pass()
                }
            "#,
        ];

        for source in cases {
            let program = parse_program(Path::new("game.ezra"), source).unwrap();
            let error = emit_ez80_assembly(&program).unwrap_err();

            assert_eq!(error.message, "function `effect` does not return a value");
        }
    }

    #[test]
    fn rejects_invalid_main_signatures() {
        for (source, expected) in [
            (
                "fn main(code: u8) {}\n",
                "main function cannot take parameters",
            ),
            (
                "fn main() -> u8 { return 0 }\n",
                "main function cannot return a value",
            ),
        ] {
            let program = parse_program(Path::new("game.ezra"), source).unwrap();
            let error = emit_ez80_assembly(&program).unwrap_err();

            assert_eq!(error.message, expected);
        }
    }

    #[test]
    fn emits_and_runs_u8_loop_with_assertion() {
        let source = r#"
            global total: u8 = 0
            fn main() {
                let i: u8 = 0
                while i < 4 {
                    total += 2
                    i += 1
                }
                test.assert_eq_u8(total, 8, 7)
                test.pass()
            }
        "#;
        let program = parse_program(Path::new("game.ezra"), source).unwrap();
        let asm = emit_ez80_assembly(&program).unwrap();
        let run = run_assembly_test(&asm, 1_000).unwrap();

        assert!(run.halted, "{asm}");
        assert_eq!(run.result_code, 0, "{asm}");
    }

    #[test]
    fn emits_and_runs_loop_break_and_continue() {
        let source = r#"
            fn main() {
                let i: u8 = 0
                let total: u8 = 0
                loop {
                    i += 1
                    if i == 2 {
                        continue
                    }
                    if i == 5 {
                        break
                    }
                    total += i
                }
                test.assert_eq_u8(total, 1 + 3 + 4, 1)
                test.pass()
            }
        "#;
        let program = parse_program(Path::new("game.ezra"), source).unwrap();
        let asm = emit_ez80_assembly(&program).unwrap();
        let run = run_assembly_test(&asm, 2_000).unwrap();

        assert!(run.halted, "{asm}");
        assert_eq!(run.result_code, 0, "{asm}");
    }

    #[test]
    fn emits_and_runs_u8_function_with_returning_if_else() {
        let source = r#"
            fn choose(flag: bool) -> u8 {
                if flag {
                    return 1
                } else {
                    return 2
                }
            }

            fn main() {
                let yes: u8 = choose(true)
                let no: u8 = choose(false)
                test.assert_eq_u8(yes, 1, 9)
                test.assert_eq_u8(no, 2, 10)
                test.pass()
            }
        "#;
        let program = parse_program(Path::new("game.ezra"), source).unwrap();
        let asm = emit_ez80_assembly(&program).unwrap();
        let run = run_assembly_test(&asm, 1_000).unwrap();

        assert!(run.halted, "{asm}");
        assert_eq!(run.result_code, 0, "{asm}");
    }

    #[test]
    fn emits_and_runs_else_if_chains() {
        let source = r#"
            fn choose(value: u8) -> u8 {
                if value == 1 {
                    return 10
                } else if value == 2 {
                    return 20
                } else if value == 3 {
                    return 30
                } else {
                    return 40
                }
            }

            fn main() {
                test.assert_eq_u8(choose(1), 10, 1)
                test.assert_eq_u8(choose(2), 20, 2)
                test.assert_eq_u8(choose(3), 30, 3)
                test.assert_eq_u8(choose(4), 40, 4)
                test.pass()
            }
        "#;
        let program = parse_program(Path::new("game.ezra"), source).unwrap();
        let asm = emit_ez80_assembly(&program).unwrap();
        let run = run_assembly_test(&asm, 3_000).unwrap();

        assert!(run.halted, "{asm}");
        assert_eq!(run.result_code, 0, "{asm}");
    }

    #[test]
    fn emits_and_runs_function_returning_from_loop() {
        let source = r#"
            fn answer() -> u8 {
                loop {
                    return 42
                }
            }

            fn choose(flag: bool) -> u8 {
                loop {
                    if flag {
                        return 7
                    } else {
                        return 9
                    }
                }
            }

            fn main() {
                test.assert_eq_u8(answer(), 42, 1)
                test.assert_eq_u8(choose(true), 7, 2)
                test.assert_eq_u8(choose(false), 9, 3)
                test.pass()
            }
        "#;
        let program = parse_program(Path::new("game.ezra"), source).unwrap();
        let asm = emit_ez80_assembly(&program).unwrap();
        let run = run_assembly_test(&asm, 4_000).unwrap();

        assert!(run.halted, "{asm}");
        assert_eq!(run.result_code, 0, "{asm}");
    }

    #[test]
    fn emits_and_runs_function_returning_from_true_while() {
        let source = r#"
            fn answer() -> u8 {
                while true {
                    return 42
                }
            }

            fn choose(flag: bool) -> u8 {
                while true {
                    if flag {
                        return 7
                    } else {
                        return 9
                    }
                }
            }

            fn main() {
                test.assert_eq_u8(answer(), 42, 1)
                test.assert_eq_u8(choose(true), 7, 2)
                test.assert_eq_u8(choose(false), 9, 3)
                test.pass()
            }
        "#;
        let program = parse_program(Path::new("game.ezra"), source).unwrap();
        let asm = emit_ez80_assembly(&program).unwrap();
        let run = run_assembly_test(&asm, 4_000).unwrap();

        assert!(run.halted, "{asm}");
        assert_eq!(run.result_code, 0, "{asm}");
    }

    #[test]
    fn emits_and_runs_function_returning_from_const_true_while() {
        let source = r#"
            const RUN: bool = true
            const SHOULD_SKIP: bool = false

            fn answer() -> u8 {
                while RUN {
                    if SHOULD_SKIP {
                        return 1
                    } else {
                        return 42
                    }
                }
            }

            fn main() {
                test.assert_eq_u8(answer(), 42, 1)
                test.pass()
            }
        "#;
        let program = parse_program(Path::new("game.ezra"), source).unwrap();
        let asm = emit_ez80_assembly(&program).unwrap();
        let run = run_assembly_test(&asm, 2_000).unwrap();

        assert!(run.halted, "{asm}");
        assert_eq!(run.result_code, 0, "{asm}");
    }

    #[test]
    fn rejects_true_while_with_break_as_missing_return() {
        let cases = [
            r#"
                fn answer() -> u8 {
                    while true {
                        break
                        return 1
                    }
                }

                fn main() { test.pass() }
            "#,
            r#"
                const RUN: bool = false

                fn answer() -> u8 {
                    while RUN {
                        return 1
                    }
                }

                fn main() { test.pass() }
            "#,
        ];

        for source in cases {
            let program = parse_program(Path::new("game.ezra"), source).unwrap();
            let error = emit_ez80_assembly(&program).unwrap_err();

            assert_eq!(error.message, "missing return value in function `answer`");
        }
    }

    #[test]
    fn emits_and_runs_user_function_returning_u8() {
        let source = r#"
            fn answer() -> u8 {
                return 42
            }

            fn main() {
                let x: u8 = answer()
                test.assert_eq_u8(x, 42, 9)
                test.pass()
            }
        "#;
        let program = parse_program(Path::new("game.ezra"), source).unwrap();
        let asm = emit_ez80_assembly(&program).unwrap();
        let run = run_assembly_test(&asm, 1_000).unwrap();

        assert!(run.halted, "{asm}");
        assert_eq!(run.result_code, 0, "{asm}");
    }

    #[test]
    fn emits_and_runs_user_function_with_u8_parameters() {
        let source = r#"
            fn inc(v: u8) -> u8 {
                return v + 1
            }

            fn add(a: u8, b: u8) -> u8 {
                return a + b
            }

            fn mix(a: u8, b: u8, c: u8) -> u8 {
                return a + b + c
            }

            fn main() {
                let x: u8 = inc(4)
                let y: u8 = add(x, 6)
                let z: u8 = mix(y, 2, 3)
                test.assert_eq_u8(z, 16, 8)
                test.pass()
            }
        "#;
        let program = parse_program(Path::new("game.ezra"), source).unwrap();
        let asm = emit_ez80_assembly(&program).unwrap();
        let run = run_assembly_test(&asm, 2_000).unwrap();

        assert!(run.halted, "{asm}");
        assert_eq!(run.result_code, 0, "{asm}");
    }

    #[test]
    fn emits_and_runs_simple_inline_return_functions() {
        let source = r#"
            inline fn pressed(pad: u16, button: u16) -> bool {
                return (pad & button) != 0
            }

            fn main() {
                let pad: u16 = 0x0011
                test.assert_eq_u8(pressed(pad, 0x0010), true, 1)
                test.assert_eq_u8(pressed(pad, 0x0002), false, 2)
                test.pass()
            }
        "#;
        let program = parse_program(Path::new("game.ezra"), source).unwrap();
        let asm = emit_ez80_assembly(&program).unwrap();
        let run = run_assembly_test(&asm, 3_000).unwrap();

        assert!(run.halted, "{asm}");
        assert_eq!(run.result_code, 0, "{asm}");
        assert!(!asm.contains("call _pressed"), "{asm}");
        assert!(!asm.contains("_pressed:"), "{asm}");
    }

    #[test]
    fn emits_and_runs_inline_functions_with_local_prefix() {
        let source = r#"
            inline fn score(value: u8) -> u8 {
                let caller: u8 = value + 1
                let doubled: u8 = caller * 2
                return doubled + 1
            }

            fn main() {
                let caller: u8 = 3
                test.assert_eq_u8(score(caller), 9, 1)
                test.pass()
            }
        "#;
        let program = parse_program(Path::new("game.ezra"), source).unwrap();
        let asm = emit_ez80_assembly(&program).unwrap();
        let run = run_assembly_test(&asm, 4_000).unwrap();

        assert!(run.halted, "{asm}");
        assert_eq!(run.result_code, 0, "{asm}");
        assert!(!asm.contains("call _score"), "{asm}");
        assert!(!asm.contains("_score:"), "{asm}");
    }

    #[test]
    fn emits_and_runs_void_inline_functions() {
        let source = r#"
            port DEBUG: u8 = 0x0C

            inline fn send(value: u8) {
                out DEBUG, value
            }

            fn main() {
                send('A')
                test.pass()
            }
        "#;
        let program = parse_program(Path::new("game.ezra"), source).unwrap();
        let asm = emit_ez80_assembly(&program).unwrap();
        let run = run_assembly_test(&asm, 4_000).unwrap();

        assert!(run.halted, "{asm}");
        assert_eq!(run.result_code, 0, "{asm}");
        assert!(!asm.contains("call _send"), "{asm}");
        assert!(!asm.contains("_send:"), "{asm}");
    }

    #[test]
    fn emits_and_runs_void_inline_functions_with_final_return() {
        let source = r#"
            global value: u8 = 0

            inline fn store(value_arg: u8) {
                value = value_arg
                return
            }

            fn main() {
                store(7)
                test.assert_eq_u8(value, 7, 1)
                test.pass()
            }
        "#;
        let program = parse_program(Path::new("game.ezra"), source).unwrap();
        let asm = emit_ez80_assembly(&program).unwrap();
        let run = run_assembly_test(&asm, 4_000).unwrap();

        assert!(run.halted, "{asm}");
        assert_eq!(run.result_code, 0, "{asm}");
        assert!(!asm.contains("call _store"), "{asm}");
        assert!(!asm.contains("_store:"), "{asm}");
    }

    #[test]
    fn void_inline_functions_keep_helper_calls_reachable() {
        let source = r#"
            port DEBUG: u8 = 0x0C

            fn add_one(value: u8) -> u8 {
                return value + 1
            }

            inline fn send_next(value: u8) {
                let next: u8 = add_one(value)
                out DEBUG, next
            }

            fn main() {
                send_next(4)
                test.pass()
            }
        "#;
        let program = parse_program(Path::new("game.ezra"), source).unwrap();
        let asm = emit_ez80_assembly(&program).unwrap();
        let run = run_assembly_test(&asm, 4_000).unwrap();

        assert!(run.halted, "{asm}");
        assert_eq!(run.result_code, 0, "{asm}");
        assert!(asm.contains("_add_one:"), "{asm}");
        assert!(asm.contains("call _add_one"), "{asm}");
        assert!(!asm.contains("_send_next:"), "{asm}");
        assert!(!asm.contains("call _send_next"), "{asm}");
    }

    #[test]
    fn recursive_inline_functions_fall_back_to_calls() {
        let source = r#"
            pub inline fn self_call(value: u8) -> u8 {
                return self_call(value)
            }

            fn main() {
                test.pass()
            }
        "#;
        let program = parse_program(Path::new("game.ezra"), source).unwrap();
        let asm = emit_ez80_assembly(&program).unwrap();
        let run = run_assembly_test(&asm, 4_000).unwrap();

        assert!(run.halted, "{asm}");
        assert_eq!(run.result_code, 0, "{asm}");
        assert!(asm.contains("_self_call:"), "{asm}");
        assert!(asm.contains("call _self_call"), "{asm}");
    }

    #[test]
    fn recursive_inline_wrappers_run_with_normal_call_fallback() {
        let source = r#"
            inline fn count_down(value: u8) -> u8 {
                return count_down_impl(value)
            }

            fn count_down_impl(value: u8) -> u8 {
                if value == 0 {
                    return 0
                }
                return count_down(value - 1) + 1
            }

            fn main() {
                test.assert_eq_u8(count_down(4), 4, 1)
                test.pass()
            }
        "#;
        let program = parse_program(Path::new("game.ezra"), source).unwrap();
        let asm = emit_ez80_assembly(&program).unwrap();
        let run = run_assembly_test(&asm, 20_000).unwrap();

        assert!(run.halted, "{asm}");
        assert_eq!(run.result_code, 0, "{asm}");
        assert!(asm.contains("_count_down_impl:"), "{asm}");
        assert!(asm.contains("call _count_down_impl"), "{asm}");
        assert!(!asm.contains("_count_down:"), "{asm}");
        assert!(
            !asm.lines()
                .any(|line| line.trim_start() == "call _count_down"),
            "{asm}"
        );
    }

    #[test]
    fn inline_return_functions_keep_helper_calls_reachable() {
        let source = r#"
            fn add_one(value: u8) -> u8 {
                return value + 1
            }

            inline fn add_two(value: u8) -> u8 {
                return add_one(value) + 1
            }

            fn main() {
                test.assert_eq_u8(add_two(5), 7, 1)
                test.pass()
            }
        "#;
        let program = parse_program(Path::new("game.ezra"), source).unwrap();
        let asm = emit_ez80_assembly(&program).unwrap();
        let run = run_assembly_test(&asm, 4_000).unwrap();

        assert!(run.halted, "{asm}");
        assert_eq!(run.result_code, 0, "{asm}");
        assert!(asm.contains("_add_one:"), "{asm}");
        assert!(asm.contains("call _add_one"), "{asm}");
        assert!(!asm.contains("_add_two:"), "{asm}");
        assert!(!asm.contains("call _add_two"), "{asm}");
    }

    #[test]
    fn emits_and_runs_wide_third_argument_after_byte_second_argument() {
        let expected = 0x10u32 + 0x12 + 0x000345;
        let source = format!(
            r#"
            fn mixed(first: u8, second: u8, third: u24) -> u24 {{
                return cast<u24>(first) + cast<u24>(second) + third
            }}

            fn main() {{
                test.assert_eq_u24(mixed(0x10, 0x12, 0x000345), 0x{expected:06X}, 1)
                test.pass()
            }}
        "#
        );
        let program = parse_program(Path::new("game.ezra"), &source).unwrap();
        let asm = emit_ez80_assembly(&program).unwrap();
        let run = run_assembly_test(&asm, 4_000).unwrap();

        assert!(run.halted, "{asm}");
        assert_eq!(run.result_code, 0, "{asm}");
        assert!(asm.contains("call _mixed"), "{asm}");
    }

    #[test]
    fn emits_and_runs_user_function_calls_with_explicit_casts() {
        let source = r#"
            fn low(value: u8) -> u8 {
                return value
            }

            fn wide(value: u16) -> u16 {
                return value
            }

            fn main() {
                let small: u8 = 0x12
                let big: u16 = 0x1234
                test.assert_eq_u16(wide(cast<u16>(small)), 0x0012, 1)
                test.assert_eq_u8(low(cast<u8>(big)), 0x34, 2)
                test.pass()
            }
        "#;
        let program = parse_program(Path::new("game.ezra"), source).unwrap();
        let asm = emit_ez80_assembly(&program).unwrap();
        let run = run_assembly_test(&asm, 2_000).unwrap();

        assert!(run.halted, "{asm}");
        assert_eq!(run.result_code, 0, "{asm}");
    }

    #[test]
    fn emits_and_runs_user_function_with_wide_register_parameters() {
        let expected_pair = (0x010000u32 + 0x000123) & 0x00FF_FFFF;
        let expected_three = (0x000100u32 + 0x000020 + 0x000003) & 0x00FF_FFFF;
        let source = format!(
            r#"
            fn add_pair(a: u24, b: u24) -> u24 {{
                return a + b
            }}

            fn add_three(a: u24, b: u24, c: u24) -> u24 {{
                return a + b + c
            }}

            fn add_count(base: u24, count: u8) -> u24 {{
                return base + cast<u24>(count)
            }}

            fn main() {{
                let pair: u24 = add_pair(0x010000, 0x000123)
                let three: u24 = add_three(0x000100, 0x000020, 0x000003)
                let mixed: u24 = add_count(0x000200, 5)
                test.assert_eq_u24(pair, 0x{expected_pair:06X}, 1)
                test.assert_eq_u24(three, 0x{expected_three:06X}, 2)
                test.assert_eq_u24(mixed, 0x000205, 3)
                test.pass()
            }}
            "#
        );
        let program = parse_program(Path::new("game.ezra"), &source).unwrap();
        let asm = emit_ez80_assembly(&program).unwrap();
        let run = run_assembly_test(&asm, 4_000).unwrap();

        assert!(run.halted, "{asm}");
        assert_eq!(run.result_code, 0, "{asm}");
    }

    #[test]
    fn emits_and_runs_user_function_with_spilled_parameters() {
        let expected_mixed = 0x000100u32 + 5 + 0x000020 + 7;
        let source = format!(
            r#"
            fn add_four(a: u8, b: u8, c: u8, d: u8) -> u8 {{
                return a + b + c + d
            }}

            fn wide_third(a: u24, b: u8, c: u24) -> u24 {{
                return a + cast<u24>(b) + c
            }}

            fn wide_third_with_extra(a: u24, b: u8, c: u24, d: u8) -> u24 {{
                return a + cast<u24>(b) + c + cast<u24>(d)
            }}

            fn main() {{
                test.assert_eq_u8(add_four(1, 2, 3, 4), 10, 1)
                test.assert_eq_u24(wide_third(0x000100, 5, 0x000020), 0x000125, 2)
                test.assert_eq_u24(wide_third_with_extra(0x000100, 5, 0x000020, 7), 0x{expected_mixed:06X}, 3)
                test.pass()
            }}
        "#
        );
        let program = parse_program(Path::new("game.ezra"), &source).unwrap();
        let asm = emit_ez80_assembly(&program).unwrap();
        let run = run_assembly_test(&asm, 6_000).unwrap();

        assert!(run.halted, "{asm}");
        assert_eq!(run.result_code, 0, "{asm}");
    }

    #[test]
    fn emits_and_runs_direct_port_read() {
        let source = r#"
            port PAD1_LO: u8 = 0x01
            fn main() {
                let pad: u8 = in PAD1_LO
                test.assert_eq_u8(pad, 0, 4)
                test.pass()
            }
        "#;
        let program = parse_program(Path::new("game.ezra"), source).unwrap();
        let asm = emit_ez80_assembly(&program).unwrap();
        let run = run_assembly_test(&asm, 1_000).unwrap();

        assert!(run.halted, "{asm}");
        assert_eq!(run.result_code, 0, "{asm}");
    }

    #[test]
    fn emits_and_runs_inline_asm_statements() {
        let source = r#"
            fn main() {
                let ch: u8 = 0x41
                let result: u8 = 0
                asm volatile(in ch: u8 as reg8, out result: u8 as reg8, clobber a, clobber ports) {
                    "ld a, 0x41"
                    "out0 (0Ch), a"
                }
                test.pass()
            }
        "#;
        let program = parse_program(Path::new("game.ezra"), source).unwrap();
        let asm = emit_ez80_assembly(&program).unwrap();
        let run = run_assembly_test(&asm, 4_000).unwrap();

        assert!(asm.contains("    ; asm volatile"));
        assert!(asm.contains("    ; in ch: u8 as reg8"));
        assert!(asm.contains("    ; out result: u8 as reg8"));
        assert!(asm.contains("    ; clobber a, ports"));
        assert!(asm.contains("    ld a, 0x41"));
        assert!(run.halted, "{asm}");
        assert_eq!(run.result_code, 0, "{asm}");
        assert_eq!(run.debug_output, b"A", "{asm}");
    }

    #[test]
    fn emits_and_runs_inline_asm_with_inferred_operand_classes() {
        let source = r#"
            fn main() {
                let ch: u8 = 0x53
                let result: u8 = 0
                asm volatile(in ch: u8, out result: u8, clobber a, clobber ports) {
                    "out0 (0Ch), a"
                }
                test.assert_eq_u8(result, 0x53, 1)
                test.pass()
            }
        "#;
        let program = parse_program(Path::new("game.ezra"), source).unwrap();
        let asm = emit_ez80_assembly(&program).unwrap();
        let run = run_assembly_test(&asm, 4_000).unwrap();

        assert!(asm.contains("    ; in ch: u8 as reg8"), "{asm}");
        assert!(asm.contains("    ; out result: u8 as reg8"), "{asm}");
        assert!(run.halted, "{asm}");
        assert_eq!(run.result_code, 0, "{asm}");
        assert_eq!(run.debug_output, b"S", "{asm}");
    }

    #[test]
    fn emits_and_runs_inline_asm_reg8_and_imm_placeholders() {
        let source = r#"
            const DEBUG_PORT: u8 = 0x0C

            fn main() {
                let port: u8 = DEBUG_PORT
                let ch: u8 = 0x43
                asm volatile(in port: u8 as imm, in ch: u8 as reg8, clobber ports) {
                    "out0 ({port}), {ch}"
                }
                test.pass()
            }
        "#;
        let program = parse_program(Path::new("game.ezra"), source).unwrap();
        let asm = emit_ez80_assembly(&program).unwrap();
        let run = run_assembly_test(&asm, 4_000).unwrap();

        assert!(asm.contains("    ; in port: u8 as imm"), "{asm}");
        assert!(asm.contains("    out0 (0Ch), a"), "{asm}");
        assert!(run.halted, "{asm}");
        assert_eq!(run.result_code, 0, "{asm}");
        assert_eq!(run.debug_output, b"C", "{asm}");
    }

    #[test]
    fn emits_and_runs_inline_asm_adc_and_sbc() {
        let source = r#"
            fn main() {
                let base: u8 = 0x40
                let result: u8 = 0
                asm volatile(in base: u8 as reg8, out result: u8 as reg8, clobber a, clobber flags) {
                    "cp 41h"
                    "adc a, 01h"
                    "cp 43h"
                    "sbc a, 00h"
                }
                test.assert_eq_u8(result, 0x41, 1)
                test.pass()
            }
        "#;
        let program = parse_program(Path::new("game.ezra"), source).unwrap();
        let asm = emit_ez80_assembly(&program).unwrap();
        let run = run_assembly_test(&asm, 4_000).unwrap();

        assert!(asm.contains("    adc a, 01h"), "{asm}");
        assert!(asm.contains("    sbc a, 00h"), "{asm}");
        assert!(run.halted, "{asm}");
        assert_eq!(run.result_code, 0, "{asm}");
    }

    #[test]
    fn rejects_runtime_values_as_inline_asm_immediates() {
        let source = r#"
            fn main() {
                let port: u8 = 0x0C
                port = port + 1
                let ch: u8 = 0x43
                asm volatile(in port: u8 as imm, in ch: u8 as reg8, clobber ports) {
                    "out0 ({port}), {ch}"
                }
                test.pass()
            }
        "#;
        let program = parse_program(Path::new("game.ezra"), source).unwrap();
        let error = emit_ez80_assembly(&program).unwrap_err();

        assert_eq!(error.message, "unknown constant `port`");
    }

    #[test]
    fn emits_and_runs_inline_asm_output_writeback() {
        let source = r#"
            fn main() {
                let result: u8 = 0
                asm volatile(out result: u8 as reg8, clobber a) {
                    "ld a, 07h"
                }
                test.assert_eq_u8(result, 7, 11)
                test.pass()
            }
        "#;
        let program = parse_program(Path::new("game.ezra"), source).unwrap();
        let asm = emit_ez80_assembly(&program).unwrap();
        let run = run_assembly_test(&asm, 4_000).unwrap();

        assert!(run.halted, "{asm}");
        assert_eq!(run.result_code, 0, "{asm}");
    }

    #[test]
    fn emits_and_runs_inline_asm_mem_operands() {
        let source = r#"
            fn main() {
                let source: u8 = 0x2A
                let result: u8 = 0
                asm volatile(in source: u8 as mem, out result: u8 as mem, clobber a, clobber flags, clobber memory) {
                    "ld a, {source}"
                    "add a, a"
                    "ld {result}, a"
                }
                test.assert_eq_u8(result, 0x54, 1)
                test.pass()
            }
        "#;
        let program = parse_program(Path::new("game.ezra"), source).unwrap();
        let asm = emit_ez80_assembly(&program).unwrap();
        let run = run_assembly_test(&asm, 4_000).unwrap();

        assert!(asm.contains("    ; in source: u8 as mem"), "{asm}");
        assert!(asm.contains("    ; out result: u8 as mem"), "{asm}");
        assert!(asm.contains("    ; clobber a, flags, memory"), "{asm}");
        assert!(run.halted, "{asm}");
        assert_eq!(run.result_code, 0, "{asm}");
    }

    #[test]
    fn emits_and_runs_inline_asm_reg16_and_reg24_operands() {
        let source = r#"
            fn main() {
                let word: u16 = 0x1234
                let word_result: u16 = 0
                asm volatile(in word: u16 as reg16, out word_result: u16 as reg16, clobber hl, clobber flags) {
                    "inc hl"
                }

                let long: u24 = 0x040123
                let long_result: u24 = 0
                asm volatile(in long: u24 as reg24, out long_result: u24 as reg24, clobber hl, clobber flags) {
                    "inc hl"
                }

                test.assert_eq_u16(word_result, 0x1235, 1)
                test.assert_eq_u24(long_result, 0x040124, 2)
                test.pass()
            }
        "#;
        let program = parse_program(Path::new("game.ezra"), source).unwrap();
        let asm = emit_ez80_assembly(&program).unwrap();
        let run = run_assembly_test(&asm, 4_000).unwrap();

        assert!(asm.contains("    ; in word: u16 as reg16"), "{asm}");
        assert!(asm.contains("    ; out word_result: u16 as reg16"), "{asm}");
        assert!(asm.contains("    ; in long: u24 as reg24"), "{asm}");
        assert!(asm.contains("    ; out long_result: u24 as reg24"), "{asm}");
        assert!(run.halted, "{asm}");
        assert_eq!(run.result_code, 0, "{asm}");
    }

    #[test]
    fn accepts_inline_asm_operand_alias_types() {
        let source = r#"
            alias byte = u8

            fn main() {
                let ch: byte = 0x41
                let result: byte = 0
                asm volatile(in ch: byte, out result: byte, clobber a) {
                    "ld a, {ch}"
                    "ld {result}, a"
                }
                test.assert_eq_u8(result, 0x41, 1)
                test.pass()
            }
        "#;
        let program = parse_program(Path::new("game.ezra"), source).unwrap();
        let asm = emit_ez80_assembly(&program).unwrap();
        let run = run_assembly_test(&asm, 4_000).unwrap();

        assert!(run.halted, "{asm}");
        assert_eq!(run.result_code, 0, "{asm}");
    }

    #[test]
    fn rejects_inline_asm_operand_type_mismatch() {
        let input = r#"
            fn main() {
                let value: u16 = 0
                asm volatile(in value: u8 as reg8) {
                    "ld a, {value}"
                }
                test.pass()
            }
        "#;
        let output = r#"
            fn main() {
                let result: u8 = 0
                asm volatile(out result: u16 as reg16, clobber hl) {
                    "ld hl, 000007h"
                }
                test.pass()
            }
        "#;
        let program = parse_program(Path::new("game.ezra"), input).unwrap();
        let error = emit_ez80_assembly(&program).unwrap_err();
        assert_eq!(
            error.message,
            "inline asm input `value` declared type `u8` does not match bound type `u16`"
        );

        let program = parse_program(Path::new("game.ezra"), output).unwrap();
        let error = emit_ez80_assembly(&program).unwrap_err();
        assert_eq!(
            error.message,
            "inline asm output `result` declared type `u16` does not match bound type `u8`"
        );
    }

    #[test]
    fn rejects_unknown_inline_asm_operand_placeholder() {
        let source = r#"
            fn main() {
                asm volatile(clobber a) {
                    "ld a, {missing}"
                }
                test.pass()
            }
        "#;
        let program = parse_program(Path::new("game.ezra"), source).unwrap();
        let error = emit_ez80_assembly(&program).unwrap_err();

        assert_eq!(
            error.message,
            "unknown inline asm operand placeholder `missing`"
        );
    }

    #[test]
    fn rejects_duplicate_inline_asm_operands() {
        let source = r#"
            fn main() {
                let value: u8 = 0
                asm volatile(in value: u8 as reg8, out value: u8 as reg8) {
                    "ld a, 1"
                }
                test.pass()
            }
        "#;
        let program = parse_program(Path::new("game.ezra"), source).unwrap();
        let error = emit_ez80_assembly(&program).unwrap_err();

        assert_eq!(error.message, "duplicate inline asm operand `value`");
    }

    #[test]
    fn emits_and_runs_imported_module_qualified_calls() {
        let root = std::env::temp_dir().join(format!(
            "ezra_module_calls_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(root.join("lib")).unwrap();
        let main_path = root.join("game.ezra");
        std::fs::write(
            root.join("lib/math.ezra"),
            "pub fn add(a: u8, b: u8) -> u8 { return a + b }\n",
        )
        .unwrap();
        std::fs::write(
            &main_path,
            r#"
            import lib.math
            fn main() {
                let value: u8 = math.add(2, 3)
                test.assert_eq_u8(value, 5, 1)
                math.add(1, 2)
                test.assert_eq_u8(lib.math.add(4, 5), 9, 2)
                test.pass()
            }
            "#,
        )
        .unwrap();

        let program = load_program(&main_path).unwrap();
        let asm = emit_ez80_assembly(&program).unwrap();
        let run = run_assembly_test(&asm, 4_000).unwrap();

        let _ = std::fs::remove_dir_all(&root);
        assert!(asm.contains("_math_add:"), "{asm}");
        assert!(asm.contains("    call _math_add"), "{asm}");
        assert!(run.halted, "{asm}");
        assert_eq!(run.result_code, 0, "{asm}");
    }

    #[test]
    fn emits_and_runs_imported_module_qualified_constants() {
        let root = std::env::temp_dir().join(format!(
            "ezra_module_constants_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(root.join("lib")).unwrap();
        let main_path = root.join("game.ezra");
        std::fs::write(
            root.join("lib/hw.ezra"),
            r#"
            pub const VALUE: u8 = 0x37
            pub volatile mmio SCRATCH: ptr<u8> = 0x040120
            "#,
        )
        .unwrap();
        std::fs::write(
            &main_path,
            r#"
            import lib.hw
            fn main() {
                mem.poke8(hw.SCRATCH, hw.VALUE)
                test.assert_eq_u8(mem.peek8(hw.SCRATCH), 0x37, 1)
                mem.poke8(lib.hw.SCRATCH, lib.hw.VALUE + 1)
                test.assert_eq_u8(mem.peek8(hw.SCRATCH), 0x38, 2)
                mem.poke8(lib.hw.SCRATCH + 1, lib.hw.VALUE + 2)
                test.assert_eq_u8(mem.peek8(hw.SCRATCH + 1), 0x39, 3)
                test.pass()
            }
            "#,
        )
        .unwrap();

        let program = load_program(&main_path).unwrap();
        let asm = emit_ez80_assembly(&program).unwrap();
        let run = run_assembly_test(&asm, 6_000).unwrap();

        let _ = std::fs::remove_dir_all(&root);
        assert!(run.halted, "{asm}");
        assert_eq!(run.result_code, 0, "{asm}");
    }

    #[test]
    fn emits_and_runs_imported_module_qualified_types() {
        let root = std::env::temp_dir().join(format!(
            "ezra_module_types_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(root.join("lib")).unwrap();
        let main_path = root.join("game.ezra");
        std::fs::write(
            root.join("lib/types.ezra"),
            r#"
            pub alias Byte = u8
            pub struct Pair {
                lo: Byte
                hi: Byte
            }
            "#,
        )
        .unwrap();
        std::fs::write(
            &main_path,
            r#"
            import lib.types
            fn main() {
                let lo: types.Byte = 3
                let pair: types.Pair = types.Pair { lo: lo, hi: 4 }
                let full_pair: lib.types.Pair = lib.types.Pair {
                    lo: cast<lib.types.Byte>(5),
                    hi: 6,
                }
                test.assert_eq_u8(pair.lo, 3, 1)
                test.assert_eq_u8(pair.hi, 4, 2)
                test.assert_eq_u8(full_pair.lo, 5, 3)
                test.assert_eq_u8(full_pair.hi, 6, 4)
                test.pass()
            }
            "#,
        )
        .unwrap();

        let program = load_program(&main_path).unwrap();
        let asm = emit_ez80_assembly(&program).unwrap();
        let run = run_assembly_test(&asm, 6_000).unwrap();

        let _ = std::fs::remove_dir_all(&root);
        assert!(run.halted, "{asm}");
        assert_eq!(run.result_code, 0, "{asm}");
    }

    #[test]
    fn emits_and_runs_imported_module_qualified_globals() {
        let root = std::env::temp_dir().join(format!(
            "ezra_module_globals_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(root.join("lib")).unwrap();
        let main_path = root.join("game.ezra");
        std::fs::write(root.join("lib/state.ezra"), "pub global score: u8 = 5\n").unwrap();
        std::fs::write(
            &main_path,
            r#"
            import lib.state
            fn main() {
                state.score += 2
                test.assert_eq_u8(state.score, 7, 1)
                test.assert_eq_u8(score, 7, 2)
                lib.state.score += 1
                test.assert_eq_u8(state.score, 8, 3)
                test.pass()
            }
            "#,
        )
        .unwrap();

        let program = load_program(&main_path).unwrap();
        let asm = emit_ez80_assembly(&program).unwrap();
        let run = run_assembly_test(&asm, 4_000).unwrap();

        let _ = std::fs::remove_dir_all(&root);
        assert!(run.halted, "{asm}");
        assert_eq!(run.result_code, 0, "{asm}");
    }

    #[test]
    fn emits_and_runs_imported_module_qualified_array_globals() {
        let root = std::env::temp_dir().join(format!(
            "ezra_module_array_globals_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(root.join("lib")).unwrap();
        let main_path = root.join("game.ezra");
        std::fs::write(
            root.join("lib/state.ezra"),
            r#"
            pub const LEN: u8 = 3
            pub global bytes: [u8; LEN] = [1, 2, 3]
            "#,
        )
        .unwrap();
        std::fs::write(
            &main_path,
            r#"
            import lib.state
            global short_sized: [u8; state.LEN] = [4, 5, 6]
            global full_sized: [u8; lib.state.LEN] = [7, 8, 9]
            global copied_short: [u8; state.LEN] = state.bytes
            global copied_full: [u8; lib.state.LEN] = lib.state.bytes

            fn main() {
                test.assert_eq_u8(state.bytes[1], 2, 1)
                state.bytes[2] = state.bytes[1] + 5
                test.assert_eq_u8(bytes[2], 7, 2)
                let ptr: ptr<u8> = &state.bytes[0]
                test.assert_eq_u8(*(ptr + 2), 7, 3)
                lib.state.bytes[0] = lib.state.bytes[2] + 1
                test.assert_eq_u8(state.bytes[0], 8, 4)
                test.assert_eq_u8(short_sized[2], 6, 5)
                test.assert_eq_u8(full_sized[2], 9, 6)
                test.assert_eq_u8(copied_short[0], 1, 7)
                test.assert_eq_u8(copied_short[2], 3, 8)
                test.assert_eq_u8(copied_full[0], 1, 9)
                test.assert_eq_u8(copied_full[2], 3, 10)
                test.pass()
            }
            "#,
        )
        .unwrap();

        let program = load_program(&main_path).unwrap();
        let asm = emit_ez80_assembly(&program).unwrap();
        let run = run_assembly_test(&asm, 8_000).unwrap();

        let _ = std::fs::remove_dir_all(&root);
        assert!(run.halted, "{asm}");
        assert_eq!(run.result_code, 0, "{asm}");
    }

    #[test]
    fn emits_and_runs_imported_module_qualified_embeds() {
        let root = std::env::temp_dir().join(format!(
            "ezra_module_embeds_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(root.join("lib")).unwrap();
        let main_path = root.join("game.ezra");
        std::fs::write(
            root.join("lib/assets.ezra"),
            "pub embed sprite: bytes = bytes [0x41, 0x42]\n",
        )
        .unwrap();
        std::fs::write(
            &main_path,
            r#"
            import lib.assets
            fn main() {
                test.assert_eq_u24(assets.sprite.len, 2, 1)
                test.assert_eq_u8(*(assets.sprite.ptr + 0), 0x41, 2)
                test.assert_eq_u8(*(assets.sprite.ptr + 1), 0x42, 3)
                test.assert_eq_u8(*(sprite.ptr + 1), 0x42, 4)
                test.assert_eq_u8(*(lib.assets.sprite.ptr + 0), 0x41, 5)
                test.pass()
            }
            "#,
        )
        .unwrap();

        let program = load_program(&main_path).unwrap();
        let asm = emit_ez80_assembly(&program).unwrap();
        let run = run_assembly_test(&asm, 8_000).unwrap();

        let _ = std::fs::remove_dir_all(&root);
        assert!(run.halted, "{asm}");
        assert_eq!(run.result_code, 0, "{asm}");
    }

    #[test]
    fn emits_and_runs_imported_module_qualified_ports() {
        let root = std::env::temp_dir().join(format!(
            "ezra_module_ports_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(root.join("lib")).unwrap();
        let main_path = root.join("game.ezra");
        std::fs::write(
            root.join("lib/hw.ezra"),
            r#"
            pub port PAD_LO: u8 = 0x01
            pub port DEBUG: u8 = 0x0C
            "#,
        )
        .unwrap();
        std::fs::write(
            &main_path,
            r#"
            import lib.hw
            fn main() {
                let pad: u8 = in hw.PAD_LO
                out hw.DEBUG, 'P'
                test.assert_eq_u8(pad, 0, 1)
                let full_pad: u8 = in lib.hw.PAD_LO
                out lib.hw.DEBUG, 'Q'
                test.assert_eq_u8(full_pad, 0, 2)
                test.pass()
            }
            "#,
        )
        .unwrap();

        let program = load_program(&main_path).unwrap();
        let asm = emit_ez80_assembly(&program).unwrap();
        let run = run_assembly_test(&asm, 4_000).unwrap();

        let _ = std::fs::remove_dir_all(&root);
        assert!(asm.contains("in0 a, (01h)"), "{asm}");
        assert!(asm.contains("out0 (0Ch), a"), "{asm}");
        assert!(run.halted, "{asm}");
        assert_eq!(run.result_code, 0, "{asm}");
        assert_eq!(run.debug_output, b"PQ", "{asm}");
    }

    #[test]
    fn emits_and_runs_imported_sdk_style_game_frame() {
        let root = std::env::temp_dir().join(format!(
            "ezra_sdk_frame_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(root.join("sdk")).unwrap();
        std::fs::create_dir_all(root.join("assets")).unwrap();
        let main_path = root.join("game.ezra");
        std::fs::write(root.join("assets/player.bin"), [0x2A, 0x7E]).unwrap();
        std::fs::write(
            root.join("sdk/input.ezra"),
            r#"
            pub const BTN_RIGHT: u16 = 0x0080
            pub port PAD_LO: u8 = 0x01
            pub port PAD_HI: u8 = 0x02

            pub fn read_pad(index: u8) -> u16 {
                let lo: u8 = in PAD_LO
                let hi: u8 = in PAD_HI
                let wide_hi: u16 = cast<u16>(hi) << 8
                if index == 0 {
                    return BTN_RIGHT | cast<u16>(lo) | wide_hi
                }
                return 0
            }

            pub fn pressed(pad: u16, button: u16) -> bool {
                return (pad & button) != 0
            }
            "#,
        )
        .unwrap();
        std::fs::write(
            root.join("sdk/video.ezra"),
            r#"
            pub const VIDEO_PRESENT: u8 = 1
            pub volatile mmio VRAM_BASE: ptr<u8> = 0x040180
            pub port VIDEO_CMD: u8 = 0x09

            pub fn present() {
                out VIDEO_CMD, VIDEO_PRESENT
            }

            pub fn clear(value: u8) {
                let i: u8 = 0
                while i < 4 {
                    *(VRAM_BASE + cast<u24>(i)) = value
                    i += 1
                }
            }

            pub fn poke(offset: u24, value: u8) {
                *(VRAM_BASE + offset) = value
            }

            pub fn peek(offset: u24) -> u8 {
                return *(VRAM_BASE + offset)
            }
            "#,
        )
        .unwrap();
        std::fs::write(
            root.join("sdk/math.ezra"),
            r#"
            pub const SUBPX_SHIFT: u8 = 8
            pub const SUBPX_ONE: i24 = 256

            pub fn subpx_from_int(v: i16) -> i24 {
                return cast<i24>(v) * SUBPX_ONE
            }

            pub fn subpx_to_int(v: i24) -> i16 {
                return cast<i16>(v / SUBPX_ONE)
            }
            "#,
        )
        .unwrap();
        std::fs::write(
            &main_path,
            r#"
            import sdk.input
            import sdk.video
            import sdk.math

            alias pos = i24

            embed player_sprite: bytes = file("assets/player.bin") section .assets align 16

            global player_x: pos = 20 * SUBPX_ONE
            global player_y: pos = 20 * SUBPX_ONE

            fn update() {
                let pad: u16 = input.read_pad(0)
                if input.pressed(pad, BTN_RIGHT) {
                    player_x += SUBPX_ONE
                }
            }

            fn draw() {
                let sx: u16 = cast<u16>(math.subpx_to_int(player_x))
                let sy: u16 = cast<u16>(math.subpx_to_int(player_y))
                let offset: u24 = cast<u24>(sy) * 32 + cast<u24>(sx)
                let color: u8 = *player_sprite.ptr
                video.poke(offset, color)
            }

            fn main() {
                video.clear(0)
                let frames: u8 = 0
                loop {
                    update()
                    draw()
                    video.present()
                    frames += 1
                    if frames == 2 {
                        break
                    }
                }

                test.assert_eq_u24(cast<u24>(player_x), 0x001600, 1)
                test.assert_eq_u8(video.peek(661), 0x2A, 2)
                test.assert_eq_u8(video.peek(0), 0, 3)
                test.assert_eq_u8(video.peek(662), 0x2A, 4)
                test.assert_eq_u24(player_sprite.len, 2, 5)
                test.pass()
            }
            "#,
        )
        .unwrap();

        let program = load_program(&main_path).unwrap();
        let asm = emit_ez80_assembly(&program).unwrap();
        let run = run_assembly_test(&asm, 100_000).unwrap();

        let _ = std::fs::remove_dir_all(&root);
        assert!(asm.contains("_input_read_pad:"), "{asm}");
        assert!(asm.contains("_video_poke:"), "{asm}");
        assert!(asm.contains("_math_subpx_to_int:"), "{asm}");
        assert!(asm.contains("in0 a, (01h)"), "{asm}");
        assert!(asm.contains("out0 (09h), a"), "{asm}");
        assert!(run.halted, "{asm}");
        assert_eq!(run.result_code, 0, "{asm}");
    }

    #[test]
    fn emits_and_runs_audio_sdk_style_port_sequence() {
        let root = std::env::temp_dir().join(format!(
            "ezra_audio_sdk_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(root.join("sdk")).unwrap();
        let main_path = root.join("game.ezra");
        std::fs::write(
            root.join("sdk/audio.ezra"),
            r#"
            pub const AUDIO_SUBMIT_BUFFER: u8 = 1
            pub const AUDIO_STOP: u8 = 2
            pub volatile mmio AUDIO_BASE: ptr<u8> = 0x0C1234
            pub port AUDIO_CMD: u8 = 0x0A
            pub port EXT_ADDR0: u8 = 0x10
            pub port EXT_ADDR1: u8 = 0x11
            pub port EXT_ADDR2: u8 = 0x12
            pub port EXT_LEN0: u8 = 0x13
            pub port EXT_LEN1: u8 = 0x14

            pub fn submit(addr: ptr<u8>, len: u16) {
                let raw: u24 = cast<u24>(addr)
                out EXT_ADDR0, cast<u8>(raw)
                out EXT_ADDR1, cast<u8>(raw >> 8)
                out EXT_ADDR2, cast<u8>(raw >> 16)
                out EXT_LEN0, cast<u8>(len)
                out EXT_LEN1, cast<u8>(len >> 8)
                out AUDIO_CMD, AUDIO_SUBMIT_BUFFER
            }

            pub fn stop() {
                out AUDIO_CMD, AUDIO_STOP
            }
            "#,
        )
        .unwrap();
        std::fs::write(
            &main_path,
            r#"
            import sdk.audio

            fn main() {
                audio.submit(audio.AUDIO_BASE + 0x56, 0x2345)
                test.pass()
            }
            "#,
        )
        .unwrap();

        let program = load_program(&main_path).unwrap();
        let asm = emit_ez80_assembly(&program).unwrap();
        let run = run_assembly_test(&asm, 8_000).unwrap();

        let _ = std::fs::remove_dir_all(&root);
        assert!(asm.contains("_audio_submit:"), "{asm}");
        assert!(asm.contains("out0 (0Ah), a"), "{asm}");
        assert!(asm.contains("out0 (10h), a"), "{asm}");
        assert!(asm.contains("out0 (14h), a"), "{asm}");
        assert!(run.halted, "{asm}");
        assert_eq!(run.result_code, 0, "{asm}");
        assert_eq!(run.ports[0x10], 0x8A, "{asm}");
        assert_eq!(run.ports[0x11], 0x12, "{asm}");
        assert_eq!(run.ports[0x12], 0x0C, "{asm}");
        assert_eq!(run.ports[0x13], 0x45, "{asm}");
        assert_eq!(run.ports[0x14], 0x23, "{asm}");
        assert_eq!(run.ports[0x0A], 1, "{asm}");
    }

    #[test]
    fn rejects_inline_asm_missing_required_clobbers() {
        let cases = [
            (
                r#"
                fn main() {
                    asm volatile {
                        "ld ix, 0"
                    }
                    test.pass()
                }
                "#,
                "inline asm uses `ix` without declaring clobber `ix`",
            ),
            (
                r#"
                fn main() {
                    asm volatile {
                        "out0 (0Ch), a"
                    }
                    test.pass()
                }
                "#,
                "inline asm uses ports without declaring clobber `ports`",
            ),
            (
                r#"
                fn main() {
                    asm volatile(clobber sp) {
                        "ld sp, 0F00000h"
                    }
                    test.pass()
                }
                "#,
                "inline asm clobber `sp` is only allowed in naked functions",
            ),
            (
                r#"
                fn main() {
                    asm volatile(clobber a) {
                        "xor a"
                    }
                    test.pass()
                }
                "#,
                "inline asm changes flags without declaring clobber `flags`",
            ),
            (
                r#"
                fn main() {
                    asm volatile {
                        "ld a, 1"
                    }
                    test.pass()
                }
                "#,
                "inline asm modifies `a` without declaring clobber `a`",
            ),
            (
                r#"
                fn main() {
                    asm volatile {
                        "ld hl, 040000h"
                    }
                    test.pass()
                }
                "#,
                "inline asm modifies `hl` without declaring clobber `hl`",
            ),
            (
                r#"
                fn main() {
                    asm volatile(clobber hl) {
                        "push hl"
                    }
                    test.pass()
                }
                "#,
                "inline asm modifies `sp` without declaring clobber `sp`",
            ),
            (
                r#"
                fn main() {
                    asm volatile(clobber hl) {
                        "pop hl"
                    }
                    test.pass()
                }
                "#,
                "inline asm modifies `sp` without declaring clobber `sp`",
            ),
            (
                r#"
                fn main() {
                    asm volatile {
                        "call .L_inline_sub"
                        "jr .L_inline_after"
                        ".L_inline_sub:"
                        "ret"
                        ".L_inline_after:"
                    }
                    test.pass()
                }
                "#,
                "inline asm modifies `af` without declaring clobber `af`",
            ),
            (
                r#"
                fn main() {
                    asm volatile {
                        "mlt bc"
                    }
                    test.pass()
                }
                "#,
                "inline asm modifies `bc` without declaring clobber `bc`",
            ),
            (
                r#"
                fn main() {
                    asm volatile(clobber bc, clobber de, clobber hl) {
                        "ldir"
                    }
                    test.pass()
                }
                "#,
                "inline asm changes flags without declaring clobber `flags`",
            ),
            (
                r#"
                fn main() {
                    asm volatile(clobber bc, clobber hl, clobber flags, clobber ports) {
                        "otir"
                    }
                    test.pass()
                }
                "#,
                "inline asm uses memory without declaring clobber `memory`",
            ),
        ];

        for (source, expected) in cases {
            let program = parse_program(Path::new("game.ezra"), source).unwrap();
            let error = emit_ez80_assembly(&program).unwrap_err();

            assert_eq!(error.message, expected);
        }
    }

    #[test]
    fn rejects_duplicate_inline_asm_clobbers() {
        let source = r#"
            fn main() {
                asm volatile(clobber a, clobber a) {
                    "ld a, 1"
                }
                test.pass()
            }
        "#;
        let program = parse_program(Path::new("game.ezra"), source).unwrap();
        let error = emit_ez80_assembly(&program).unwrap_err();

        assert_eq!(error.message, "duplicate inline asm clobber `a`");
    }

    #[test]
    fn rejects_unknown_inline_asm_clobbers() {
        let error =
            validate_inline_asm_clobbers(&["scratch".to_owned()], &["nop".to_owned()], false)
                .unwrap_err();

        assert_eq!(error.message, "unknown inline asm clobber `scratch`");
    }

    #[test]
    fn accepts_inline_asm_declared_flags_clobbers() {
        for clobber in ["flags", "f", "af"] {
            let source = format!(
                r#"
                fn main() {{
                    asm volatile(clobber a, clobber {clobber}) {{
                        "xor a"
                    }}
                    test.pass()
                }}
                "#
            );
            let program = parse_program(Path::new("game.ezra"), &source).unwrap();
            let asm = emit_ez80_assembly(&program).unwrap();
            let run = run_assembly_test(&asm, 1_000).unwrap();

            assert!(run.halted, "{asm}");
            assert_eq!(run.result_code, 0, "{asm}");
        }
    }

    #[test]
    fn accepts_inline_asm_declared_register_clobbers() {
        let cases = [
            "asm volatile(clobber af, clobber flags) { \"xor a\" }",
            "asm volatile(clobber b, clobber c) { \"ld bc, 1234h\" }",
            "asm volatile(clobber h, clobber l) { \"ld hl, 040000h\" }",
            "asm volatile(clobber de, clobber hl) { \"ex de, hl\" }",
            "asm volatile(clobber bc) { \"ld b, 11h\" \"ld c, 0Fh\" \"mlt bc\" }",
            "asm volatile(clobber d, clobber e) { \"ld d, 02h\" \"ld e, 03h\" \"mlt de\" }",
            "asm volatile(clobber h, clobber l) { \"ld h, 04h\" \"ld l, 05h\" \"mlt hl\" }",
        ];

        for asm_stmt in cases {
            let source = format!(
                r#"
                fn main() {{
                    {asm_stmt}
                    test.pass()
                }}
                "#
            );
            let program = parse_program(Path::new("game.ezra"), &source).unwrap();
            let asm = emit_ez80_assembly(&program).unwrap();
            let run = run_assembly_test(&asm, 1_000).unwrap();

            assert!(run.halted, "{asm}");
            assert_eq!(run.result_code, 0, "{asm}");
        }
    }

    #[test]
    fn accepts_inline_asm_declared_call_clobbers() {
        let source = r#"
            fn main() {
                asm volatile(clobber af, clobber bc, clobber de, clobber hl) {
                    "call .L_inline_sub"
                    "jr .L_inline_after"
                    ".L_inline_sub:"
                    "ret"
                    ".L_inline_after:"
                }
                test.pass()
            }
        "#;
        let program = parse_program(Path::new("game.ezra"), source).unwrap();
        let asm = emit_ez80_assembly(&program).unwrap();
        let run = run_assembly_test(&asm, 1_000).unwrap();

        assert!(run.halted, "{asm}");
        assert_eq!(run.result_code, 0, "{asm}");
    }

    #[test]
    fn emits_and_runs_inline_asm_ldir_with_declared_clobbers() {
        let source = r#"
            global src: [u8; 3] = [0x41, 0x42, 0x43]
            global dst: [u8; 3] = [0, 0, 0]

            fn main() {
                asm volatile(clobber bc, clobber de, clobber hl, clobber flags, clobber memory) {
                    "ld hl, 040000h"
                    "ld de, 040003h"
                    "ld bc, 000003h"
                    "ldir"
                }
                test.assert_eq_u8(dst[0], 0x41, 1)
                test.assert_eq_u8(dst[1], 0x42, 2)
                test.assert_eq_u8(dst[2], 0x43, 3)
                test.pass()
            }
        "#;
        let program = parse_program(Path::new("game.ezra"), source).unwrap();
        let asm = emit_ez80_assembly(&program).unwrap();
        let run = run_assembly_test(&asm, 4_000).unwrap();

        assert!(asm.contains("    ldir"), "{asm}");
        assert!(run.halted, "{asm}");
        assert_eq!(run.result_code, 0, "{asm}");
    }

    #[test]
    fn emits_and_runs_inline_asm_cpir_with_declared_clobbers() {
        let source = r#"
            global bytes: [u8; 3] = [0x11, 0x42, 0x33]
            global remaining: u8 = 0

            fn main() {
                asm volatile(clobber a, clobber bc, clobber hl, clobber flags, clobber memory) {
                    "ld a, 42h"
                    "ld hl, 040000h"
                    "ld bc, 000003h"
                    "cpir"
                    "ld a, c"
                    "ld (040003h), a"
                }
                test.assert_eq_u8(remaining, 1, 1)
                test.pass()
            }
        "#;
        let program = parse_program(Path::new("game.ezra"), source).unwrap();
        let asm = emit_ez80_assembly(&program).unwrap();
        let run = run_assembly_test(&asm, 4_000).unwrap();

        assert!(asm.contains("    cpir"), "{asm}");
        assert!(run.halted, "{asm}");
        assert_eq!(run.result_code, 0, "{asm}");
    }

    #[test]
    fn emits_and_runs_inline_asm_otir_with_declared_clobbers() {
        let source = r#"
            global bytes: [u8; 2] = [0x11, 0x42]

            fn main() {
                asm volatile(clobber bc, clobber hl, clobber flags, clobber memory, clobber ports) {
                    "ld hl, 040000h"
                    "ld bc, 000220h"
                    "otir"
                }
                test.pass()
            }
        "#;
        let program = parse_program(Path::new("game.ezra"), source).unwrap();
        let asm = emit_ez80_assembly(&program).unwrap();
        let run = run_assembly_test(&asm, 4_000).unwrap();

        assert!(asm.contains("    otir"), "{asm}");
        assert!(run.halted, "{asm}");
        assert_eq!(run.result_code, 0, "{asm}");
        assert_eq!(run.ports[0x20], 0x42, "{asm}");
    }

    #[test]
    fn emits_and_runs_naked_asm_functions_without_epilogue() {
        let source = r#"
            naked fn raw_debug() {
                asm volatile(clobber a, clobber ports) {
                    "ld a, 0x42"
                    "out0 (0Ch), a"
                    "ret"
                }
            }

            fn main() {
                raw_debug()
                test.pass()
            }
        "#;
        let program = parse_program(Path::new("game.ezra"), source).unwrap();
        let asm = emit_ez80_assembly(&program).unwrap();
        let run = run_assembly_test(&asm, 4_000).unwrap();

        let raw_debug = asm.split("_raw_debug:").nth(1).unwrap();
        let raw_debug = raw_debug.split("_main:").next().unwrap();
        assert_eq!(raw_debug.matches("    ret").count(), 1, "{asm}");
        assert!(run.halted, "{asm}");
        assert_eq!(run.result_code, 0, "{asm}");
        assert_eq!(run.debug_output, b"B", "{asm}");
    }

    #[test]
    fn emits_naked_asm_functions_with_sp_clobber() {
        let source = r#"
            naked fn raw_entry() {
                asm volatile(clobber af, clobber bc, clobber de, clobber hl, clobber sp) {
                    "ld sp, 0F00000h"
                    "call _main"
                    "jp $"
                }
            }

            fn main() {
                test.pass()
            }
        "#;
        let program = parse_program(Path::new("game.ezra"), source).unwrap();
        let asm = emit_ez80_assembly(&program).unwrap();
        let raw_entry = asm.split("_raw_entry:").nth(1).unwrap();
        let raw_entry = raw_entry.split("_main:").next().unwrap();

        assert!(raw_entry.contains("    ld sp, 0F00000h"), "{asm}");
        assert!(raw_entry.contains("    call _main"), "{asm}");
        assert!(raw_entry.contains("    jp $"), "{asm}");
        let run = run_assembly_test(&asm, 4_000).unwrap();
        assert!(run.halted, "{asm}");
        assert_eq!(run.result_code, 0, "{asm}");
    }

    #[test]
    fn emits_and_runs_interrupt_functions_with_reti() {
        let source = r#"
            interrupt fn vblank_irq() {
                debug.char('I')
            }

            fn main() {
                vblank_irq()
                test.pass()
            }
        "#;
        let program = parse_program(Path::new("game.ezra"), source).unwrap();
        let asm = emit_ez80_assembly(&program).unwrap();
        let run = run_assembly_test(&asm, 4_000).unwrap();

        let irq = asm.split("_vblank_irq:").nth(1).unwrap();
        let irq = irq.split("_main:").next().unwrap();
        assert!(irq.contains("    push af"), "{asm}");
        assert!(irq.contains("    push ix"), "{asm}");
        assert!(irq.contains("    push iy"), "{asm}");
        assert!(irq.contains("    pop iy"), "{asm}");
        assert!(irq.contains("    pop ix"), "{asm}");
        assert!(irq.contains("    pop af"), "{asm}");
        assert!(irq.contains("    reti"), "{asm}");
        assert!(run.halted, "{asm}");
        assert_eq!(run.result_code, 0, "{asm}");
        assert_eq!(run.debug_output, b"I", "{asm}");
    }

    #[test]
    fn emits_interrupt_epilogue_for_explicit_return() {
        let source = r#"
            interrupt fn vblank_irq() {
                debug.char('R')
                if true {
                    return
                }
                debug.char('X')
            }

            fn main() {
                vblank_irq()
                test.pass()
            }
        "#;
        let program = parse_program(Path::new("game.ezra"), source).unwrap();
        let asm = emit_ez80_assembly(&program).unwrap();
        let run = run_assembly_test(&asm, 4_000).unwrap();

        let irq = asm.split("_vblank_irq:").nth(1).unwrap();
        let irq = irq.split("_main:").next().unwrap();
        let return_site = irq
            .split("out0 (0Ch), a")
            .nth(1)
            .expect("debug output in interrupt handler");
        assert!(return_site.contains("    pop iy"), "{asm}");
        assert!(return_site.contains("    pop ix"), "{asm}");
        assert!(return_site.contains("    pop hl"), "{asm}");
        assert!(return_site.contains("    pop af"), "{asm}");
        assert!(return_site.contains("    reti"), "{asm}");
        assert!(run.halted, "{asm}");
        assert_eq!(run.result_code, 0, "{asm}");
        assert_eq!(run.debug_output, b"R", "{asm}");
    }

    #[test]
    fn emits_and_runs_naked_interrupt_functions() {
        let source = r#"
            naked interrupt fn raw_irq() {
                asm volatile(clobber a, clobber ports) {
                    "ld a, 0x4E"
                    "out0 (0Ch), a"
                    "reti"
                }
            }

            fn main() {
                raw_irq()
                test.pass()
            }
        "#;
        let program = parse_program(Path::new("game.ezra"), source).unwrap();
        let asm = emit_ez80_assembly(&program).unwrap();
        let run = run_assembly_test(&asm, 4_000).unwrap();

        let raw_irq = asm.split("_raw_irq:").nth(1).unwrap();
        let raw_irq = raw_irq.split("_main:").next().unwrap();
        assert!(!raw_irq.contains("    push af"), "{asm}");
        assert!(raw_irq.contains("    reti"), "{asm}");
        assert!(run.halted, "{asm}");
        assert_eq!(run.result_code, 0, "{asm}");
        assert_eq!(run.debug_output, b"N", "{asm}");
    }

    #[test]
    fn rejects_duplicate_function_attributes() {
        let cases = [
            (
                r#"
                inline inline fn invalid() {}
                fn main() { test.pass() }
                "#,
                "duplicate attribute `inline` on function `invalid`",
            ),
            (
                r#"
                naked naked fn invalid() {
                    asm { "ret" }
                }
                fn main() { test.pass() }
                "#,
                "duplicate attribute `naked` on function `invalid`",
            ),
            (
                r#"
                interrupt interrupt fn invalid() {}
                fn main() { test.pass() }
                "#,
                "duplicate attribute `interrupt` on function `invalid`",
            ),
        ];

        for (source, expected) in cases {
            let program = parse_program(Path::new("game.ezra"), source).unwrap();
            let error = emit_ez80_assembly(&program).unwrap_err();

            assert_eq!(error.message, expected);
        }
    }

    #[test]
    fn rejects_interrupt_function_parameters() {
        let source = r#"
            interrupt fn invalid(code: u8) {
                debug.char(code)
            }

            fn main() {
                test.pass()
            }
        "#;
        let program = parse_program(Path::new("game.ezra"), source).unwrap();
        let error = emit_ez80_assembly(&program).unwrap_err();

        assert_eq!(
            error.message,
            "interrupt function `invalid` cannot take parameters"
        );
    }

    #[test]
    fn rejects_interrupt_function_return_values() {
        let source = r#"
            interrupt fn invalid() -> u8 {
                return 1
            }

            fn main() {
                test.pass()
            }
        "#;
        let program = parse_program(Path::new("game.ezra"), source).unwrap();
        let error = emit_ez80_assembly(&program).unwrap_err();

        assert_eq!(
            error.message,
            "interrupt function `invalid` cannot return a value"
        );
    }

    #[test]
    fn rejects_non_asm_statements_in_naked_functions() {
        let source = r#"
            naked fn invalid() {
                let value: u8 = 1
            }

            fn main() {
                test.pass()
            }
        "#;
        let program = parse_program(Path::new("game.ezra"), source).unwrap();
        let error = emit_ez80_assembly(&program).unwrap_err();

        assert_eq!(
            error.message,
            "naked function `invalid` may contain only asm blocks"
        );
    }

    #[test]
    fn rejects_operand_asm_in_naked_functions() {
        let source = r#"
            naked fn invalid() {
                asm volatile(in value: u8 as reg8) {
                    "ret"
                }
            }

            fn main() {
                test.pass()
            }
        "#;
        let program = parse_program(Path::new("game.ezra"), source).unwrap();
        let error = emit_ez80_assembly(&program).unwrap_err();

        assert_eq!(
            error.message,
            "naked function `invalid` asm blocks cannot use operands"
        );
    }

    #[test]
    fn emits_calls_to_extern_asm_functions_without_bodies() {
        let source = r#"
            extern asm fn raw_add(a: u8, b: u8) -> u8

            fn main() {
                let value: u8 = raw_add(0x17, 0x2B)
                test.assert_eq_u8(value, 0x42, 1)
                debug.char(value)
                test.pass()
            }
        "#;
        let program = parse_program(Path::new("game.ezra"), source).unwrap();
        let asm = emit_ez80_assembly(&program).unwrap();
        let linked = format!("{asm}\n_raw_add:\n    add a, b\n    ret\n");
        let run = run_assembly_test(&linked, 4_000).unwrap();

        assert!(asm.contains("    call _raw_add"), "{asm}");
        assert!(!asm.contains("_raw_add:"), "{asm}");
        assert!(run.halted, "{linked}");
        assert_eq!(run.result_code, 0, "{linked}");
        assert_eq!(run.debug_output, b"B", "{linked}");
    }

    #[test]
    fn rejects_extern_asm_signatures_that_need_internal_arg_slots() {
        let source = r#"
            extern asm fn raw_mixed(first: u8, second: u8, third: u24) -> u24

            fn main() {
                test.pass()
            }
        "#;
        let program = parse_program(Path::new("game.ezra"), source).unwrap();
        let error = emit_ez80_assembly(&program).unwrap_err();

        assert_eq!(
            error.message,
            "extern asm function `raw_mixed` cannot use a byte second argument followed by a wide third argument"
        );
    }

    #[test]
    fn rejects_duplicate_extern_asm_parameters() {
        let source = r#"
            extern asm fn raw_dup(value: u8, value: u8) -> u8

            fn main() {
                test.pass()
            }
        "#;
        let program = parse_program(Path::new("game.ezra"), source).unwrap();
        let error = emit_ez80_assembly(&program).unwrap_err();

        assert_eq!(
            error.message,
            "function `raw_dup` has duplicate parameter `value`"
        );
    }

    #[test]
    fn emits_and_runs_extern_asm_stack_arguments() {
        let source = r#"
            extern asm fn raw_add4(a: u8, b: u8, c: u8, d: u8) -> u8

            fn main() {
                test.assert_eq_u8(raw_add4(1, 2, 3, 4), 10, 1)
                test.pass()
            }
        "#;
        let program = parse_program(Path::new("game.ezra"), source).unwrap();
        let asm = emit_ez80_assembly(&program).unwrap();
        let linked = format!(
            "{asm}\n_raw_add4:\n    add a, b\n    add a, c\n    ld b, a\n    ld hl, 000003h\n    add hl, sp\n    ld a, (hl)\n    add a, b\n    ret\n"
        );
        let run = run_assembly_test(&linked, 4_000).unwrap();

        assert!(asm.contains("    call _raw_add4"), "{asm}");
        assert!(!asm.contains("_raw_add4:"), "{asm}");
        assert!(run.halted, "{linked}");
        assert_eq!(run.result_code, 0, "{linked}");
    }

    #[test]
    fn emits_and_runs_u16_storage_and_return() {
        let source = r#"
            global total: u16 = 0x0100

            fn add_base(v: u16) -> u16 {
                return v + 0x0023
            }

            fn main() {
                let x: u16 = add_base(total)
                x += 0x0010
                test.assert_eq_u16(x, 0x0133, 5)
                test.pass()
            }
        "#;
        let program = parse_program(Path::new("game.ezra"), source).unwrap();
        let asm = emit_ez80_assembly(&program).unwrap();
        let run = run_assembly_test(&asm, 2_000).unwrap();

        assert!(run.halted, "{asm}");
        assert_eq!(run.result_code, 0, "{asm}");
    }

    #[test]
    fn emits_byte_accurate_u16_store_without_clobbering_next_variable() {
        let source = r#"
            fn main() {
                let wide: u16 = 0x1234
                let guard: u8 = 0x7A
                wide += 1
                test.assert_eq_u16(wide, 0x1235, 6)
                test.assert_eq_u8(guard, 0x7A, 7)
                test.pass()
            }
        "#;
        let program = parse_program(Path::new("game.ezra"), source).unwrap();
        let asm = emit_ez80_assembly(&program).unwrap();
        let run = run_assembly_test(&asm, 2_000).unwrap();

        assert!(run.halted, "{asm}");
        assert_eq!(run.result_code, 0, "{asm}");
    }

    #[test]
    fn emits_and_runs_u24_storage_and_return() {
        let source = r#"
            global base: u24 = 0x010000

            fn bump(v: u24) -> u24 {
                return v + 0x000123
            }

            fn main() {
                let x: u24 = bump(base)
                x += 0x000010
                test.assert_eq_u24(x, 0x010133, 8)
                test.pass()
            }
        "#;
        let program = parse_program(Path::new("game.ezra"), source).unwrap();
        let asm = emit_ez80_assembly(&program).unwrap();
        let run = run_assembly_test(&asm, 2_000).unwrap();

        assert!(run.halted, "{asm}");
        assert_eq!(run.result_code, 0, "{asm}");
    }

    #[test]
    fn emits_and_runs_wide_sub_and_bitwise_ops() {
        let expected_u16 = (((0x12F0u16 - 0x0010) & 0x0FF0) | 0x1000) ^ 0x00F0;
        let expected_u24 =
            ((((0x010123u32 - 0x000020) & 0x01FFFF) | 0x020000) ^ 0x000003) & 0x00FF_FFFF;
        let source = format!(
            r#"
            fn main() {{
                let a: u16 = 0x12F0 - 0x0010
                a &= 0x0FF0
                a |= 0x1000
                a ^= 0x00F0
                test.assert_eq_u16(a, 0x{expected_u16:04X}, 10)

                let b: u24 = 0x010123 - 0x000020
                b &= 0x01FFFF
                b |= 0x020000
                b ^= 0x000003
                test.assert_eq_u24(b, 0x{expected_u24:06X}, 11)
                test.pass()
            }}
        "#
        );
        let program = parse_program(Path::new("game.ezra"), &source).unwrap();
        let asm = emit_ez80_assembly(&program).unwrap();
        let run = run_assembly_test(&asm, 4_000).unwrap();

        assert!(run.halted, "{asm}");
        assert_eq!(run.result_code, 0, "{asm}");
    }

    #[test]
    fn emits_and_runs_dynamic_unary_ops() {
        let expected_u8_neg = 0u8.wrapping_sub(5);
        let expected_u8_not = !0x5Au8;
        let expected_u16_neg = 0u16.wrapping_sub(0x0023);
        let expected_u16_not = !0x120Fu16;
        let expected_u24_neg = (0u32.wrapping_sub(0x000123)) & 0x00FF_FFFF;
        let expected_u24_not = (!0x010203u32) & 0x00FF_FFFF;
        let source = format!(
            r#"
            fn main() {{
                let a: u8 = 5
                let b: u8 = 0x5A
                test.assert_eq_u8(-a, 0x{expected_u8_neg:02X}, 1)
                test.assert_eq_u8(~b, 0x{expected_u8_not:02X}, 2)
                test.assert_eq_u8(!(a == 0), 1, 3)
                test.assert_eq_u8(!(a != 0), 0, 4)

                let c: u16 = 0x0023
                let d: u16 = 0x120F
                test.assert_eq_u16(-c, 0x{expected_u16_neg:04X}, 5)
                test.assert_eq_u16(~d, 0x{expected_u16_not:04X}, 6)

                let e: u24 = 0x000123
                let f: u24 = 0x010203
                test.assert_eq_u24(-e, 0x{expected_u24_neg:06X}, 7)
                test.assert_eq_u24(~f, 0x{expected_u24_not:06X}, 8)
                test.pass()
            }}
            "#
        );
        let program = parse_program(Path::new("game.ezra"), &source).unwrap();
        let asm = emit_ez80_assembly(&program).unwrap();
        let run = run_assembly_test(&asm, 6_000).unwrap();

        assert!(run.halted, "{asm}");
        assert_eq!(run.result_code, 0, "{asm}");
    }

    #[test]
    fn emits_and_runs_short_circuit_logical_ops() {
        let source = r#"
            alias flag = bool
            global calls: u8 = 0

            fn bump(value: bool) -> bool {
                calls += 1
                return value
            }

            fn main() {
                calls = 0
                let and_skip: bool = false && bump(true)
                test.assert_eq_u8(and_skip, false, 1)
                test.assert_eq_u8(calls, 0, 2)

                let or_skip: flag = true || bump(false)
                test.assert_eq_u8(or_skip, true, 3)
                test.assert_eq_u8(calls, 0, 4)

                let and_run: bool = true && bump(true)
                test.assert_eq_u8(and_run, true, 5)
                test.assert_eq_u8(calls, 1, 6)

                let or_run: bool = false || bump(true)
                test.assert_eq_u8(or_run, true, 7)
                test.assert_eq_u8(calls, 2, 8)

                test.pass()
            }
        "#;
        let program = parse_program(Path::new("game.ezra"), source).unwrap();
        let asm = emit_ez80_assembly(&program).unwrap();
        let run = run_assembly_test(&asm, 8_000).unwrap();

        assert!(run.halted, "{asm}");
        assert_eq!(run.result_code, 0, "{asm}");
    }

    #[test]
    fn emits_and_runs_constant_shift_ops() {
        let expected_u8_assign = 0x12u8.wrapping_shl(2) >> 1;
        let expected_u8_expr = 0x81u8 >> 3;
        let expected_u16_expr = 0x1234u16.wrapping_shl(3) >> 2;
        let expected_u16_assign = 0x00F0u16.wrapping_shl(4) >> 3;
        let expected_u24_expr = ((0x010203u32 << 4) & 0x00FF_FFFF) >> 3;
        let expected_u24_assign = ((0x000F00u32 << 5) & 0x00FF_FFFF) >> 2;
        let expected_i16_const = ((-0x1234i16) >> 3) as u16;
        let expected_i24_const = ((-0x012345i32) >> 5) & 0x00FF_FFFF;
        let source = format!(
            r#"
            const SIGNED_WORD_SHIFT: i16 = (-0x1234i16) >> 3
            const SIGNED_WIDE_SHIFT: i24 = (-0x012345i24) >> 5
            const SIGNED_BYTE_BIG_SHIFT: i8 = (-1i8) >> 64
            const SIGNED_WORD_BIG_SHIFT: i16 = (-1i16) >> 64
            const SIGNED_WIDE_BIG_SHIFT: i24 = (-1i24) >> 64

            fn main() {{
                let a: u8 = 0x12
                a <<= 2
                a >>= 1
                test.assert_eq_u8(a, 0x{expected_u8_assign:02X}, 1)
                test.assert_eq_u8(0x81 >> 3, 0x{expected_u8_expr:02X}, 2)

                let b: u16 = 0x1234
                let c: u16 = (b << 3) >> 2
                test.assert_eq_u16(c, 0x{expected_u16_expr:04X}, 3)
                let d: u16 = 0x00F0
                d <<= 4
                d >>= 3
                test.assert_eq_u16(d, 0x{expected_u16_assign:04X}, 4)

                let e: u24 = 0x010203
                let f: u24 = (e << 4) >> 3
                test.assert_eq_u24(f, 0x{expected_u24_expr:06X}, 5)
                let g: u24 = 0x000F00
                g <<= 5
                g >>= 2
                test.assert_eq_u24(g, 0x{expected_u24_assign:06X}, 6)
                test.assert_eq_u16(cast<u16>(SIGNED_WORD_SHIFT), 0x{expected_i16_const:04X}, 7)
                test.assert_eq_u24(cast<u24>(SIGNED_WIDE_SHIFT), 0x{expected_i24_const:06X}, 8)
                test.assert_eq_u8(cast<u8>(SIGNED_BYTE_BIG_SHIFT), 0xFF, 9)
                test.assert_eq_u16(cast<u16>(SIGNED_WORD_BIG_SHIFT), 0xFFFF, 10)
                test.assert_eq_u24(cast<u24>(SIGNED_WIDE_BIG_SHIFT), 0xFFFFFF, 11)
                test.pass()
            }}
            "#
        );
        let program = parse_program(Path::new("game.ezra"), &source).unwrap();
        let asm = emit_ez80_assembly(&program).unwrap();
        let run = run_assembly_test(&asm, 10_000).unwrap();

        assert!(run.halted, "{asm}");
        assert_eq!(run.result_code, 0, "{asm}");
    }

    #[test]
    fn emits_and_runs_runtime_shift_counts() {
        let source = r#"
            const BYTE_COUNT: u8 = 3
            const WORD_COUNT: u16 = 4
            const SIGNED_CONST_COUNT: i8 = 1
            const WIDE_LEFT: u24 = 0x010203u24 << BYTE_COUNT
            const WIDE_RIGHT: u24 = WIDE_LEFT >> WORD_COUNT
            const SIGNED_RIGHT: i16 = (-0x1234i16) >> BYTE_COUNT

            fn shl8(value: u8, count: u8) -> u8 {
                return value << count
            }

            fn shr8(value: u8, count: u8) -> u8 {
                return value >> count
            }

            fn main() {
                let count: u8 = 3
                test.assert_eq_u8(shl8(0x12, count), 0x90, 1)
                test.assert_eq_u8(shr8(0x81, count), 0x10, 2)

                let word_count: u8 = 4
                let word: u16 = 0x1234 << word_count
                test.assert_eq_u16(word, 0x2340, 3)

                let word_shift: u8 = 3
                let word_assign: u16 = word
                word_assign >>= word_shift
                test.assert_eq_u16(word_assign, 0x0468, 4)

                let wide_count: u8 = 4
                let wide: u24 = 0x010203 << wide_count
                test.assert_eq_u24(wide, 0x102030, 5)

                let wide_assign: u24 = wide
                let wide_shift: u8 = 2
                wide_assign >>= wide_shift
                test.assert_eq_u24(wide_assign, 0x04080C, 6)

                let byte: u8 = 0x80
                let byte_shift: u8 = 8
                let zero: u8 = byte >> byte_shift
                test.assert_eq_u8(zero, 0, 7)

                test.assert_eq_u24(WIDE_LEFT, 0x081018, 8)
                test.assert_eq_u24(WIDE_RIGHT, 0x008101, 9)
                test.assert_eq_u16(cast<u16>(SIGNED_RIGHT), 0xFDB9, 10)

                test.assert_eq_u16(0x1234u16 >> SIGNED_CONST_COUNT, 0x091A, 11)
                test.pass()
            }
        "#;
        let program = parse_program(Path::new("game.ezra"), source).unwrap();
        let asm = emit_ez80_assembly(&program).unwrap();
        let run = run_assembly_test(&asm, 40_000).unwrap();

        assert!(asm.contains("    dec b"), "{asm}");
        assert!(run.halted, "{asm}");
        assert_eq!(run.result_code, 0, "{asm}");
    }

    #[test]
    fn emits_and_runs_large_literal_shift_counts() {
        let source = r#"
            fn main() {
                test.assert_eq_u8(0x80 >> 25, 0, 1)
                test.assert_eq_u8(0x01 << 25, 0, 2)
                test.assert_eq_u16(0x8000 >> 25, 0, 3)
                test.assert_eq_u16(0x0001 << 25, 0, 4)
                test.assert_eq_u24(0x800000 >> 25, 0, 5)
                test.assert_eq_u24(0x000001 << 25, 0, 6)
                test.assert_eq_u8((-1i8) >> 25, 0xFF, 7)
                test.assert_eq_u16((-1i16) >> 25, 0xFFFF, 8)
                test.assert_eq_u24((-1i24) >> 25, 0xFFFFFF, 9)
                test.assert_eq_u8((-1i8) >> 64, 0xFF, 10)
                test.assert_eq_u16((-1i16) >> 64, 0xFFFF, 11)
                test.assert_eq_u24((-1i24) >> 64, 0xFFFFFF, 12)
                test.pass()
            }
        "#;
        let program = parse_program(Path::new("game.ezra"), source).unwrap();
        let asm = emit_ez80_assembly(&program).unwrap();
        let run = run_assembly_test(&asm, 20_000).unwrap();

        assert!(run.halted, "{asm}");
        assert_eq!(run.result_code, 0, "{asm}");
    }

    #[test]
    fn emits_and_runs_signed_right_shifts() {
        let expected_i8_expr = ((-4i8) >> 1) as u8;
        let expected_i8_assign = ((-5i8) >> 2) as u8;
        let expected_i16_expr = ((-0x1234i16) >> 3) as u16;
        let expected_i16_assign = ((-0x2345i16) >> 4) as u16;
        let expected_i24_expr = ((-0x012345i32) >> 5) & 0x00FF_FFFF;
        let expected_i24_assign = ((-0x023456i32) >> 6) & 0x00FF_FFFF;
        let source = format!(
            r#"
            fn shr8(value: i8, count: u8) -> i8 {{
                return value >> count
            }}

            fn shr16(value: i16, count: u8) -> i16 {{
                return value >> count
            }}

            fn shr24(value: i24, count: u8) -> i24 {{
                return value >> count
            }}

            fn main() {{
                let one: u8 = 1
                test.assert_eq_u8(shr8(-4, one), 0x{expected_i8_expr:02X}, 1)

                let byte: i8 = -5
                let two: u8 = 2
                byte >>= two
                test.assert_eq_u8(byte, 0x{expected_i8_assign:02X}, 2)

                let three: u8 = 3
                test.assert_eq_u16(shr16(-0x1234, three), 0x{expected_i16_expr:04X}, 3)

                let word: i16 = -0x2345
                let four: u8 = 4
                word >>= four
                test.assert_eq_u16(word, 0x{expected_i16_assign:04X}, 4)

                let five: u8 = 5
                test.assert_eq_u24(shr24(-0x012345, five), 0x{expected_i24_expr:06X}, 5)

                let wide: i24 = -0x023456
                let six: u8 = 6
                wide >>= six
                test.assert_eq_u24(wide, 0x{expected_i24_assign:06X}, 6)
                test.pass()
            }}
            "#
        );
        let program = parse_program(Path::new("game.ezra"), &source).unwrap();
        let asm = emit_ez80_assembly(&program).unwrap();
        let run = run_assembly_test(&asm, 40_000).unwrap();

        assert!(asm.contains("    sra a"), "{asm}");
        assert!(run.halted, "{asm}");
        assert_eq!(run.result_code, 0, "{asm}");
    }

    #[test]
    fn emits_and_runs_defined_u8_division_and_modulo() {
        let expected_div = 23u8 / 5;
        let expected_mod = 23u8 % 5;
        let expected_div_zero = 0u8;
        let expected_mod_zero = 0u8;
        let expected_const_div_zero = 0u8;
        let expected_const_mod_zero = 0u8;
        let source = format!(
            r#"
            const CONST_DIV_ZERO: u8 = 10 / 0
            const CONST_MOD_ZERO: u8 = 10 % 0

            fn div(v: u8, by: u8) -> u8 {{
                return v / by
            }}

            fn rem(v: u8, by: u8) -> u8 {{
                return v % by
            }}

            fn main() {{
                let a: u8 = div(23, 5)
                let b: u8 = rem(23, 5)
                let c: u8 = div(23, 0)
                let d: u8 = rem(23, 0)
                test.assert_eq_u8(a, {expected_div}, 1)
                test.assert_eq_u8(b, {expected_mod}, 2)
                test.assert_eq_u8(c, {expected_div_zero}, 3)
                test.assert_eq_u8(d, {expected_mod_zero}, 4)
                test.assert_eq_u8(CONST_DIV_ZERO, {expected_const_div_zero}, 5)
                test.assert_eq_u8(CONST_MOD_ZERO, {expected_const_mod_zero}, 6)
                test.pass()
            }}
            "#
        );
        let program = parse_program(Path::new("game.ezra"), &source).unwrap();
        let asm = emit_ez80_assembly(&program).unwrap();
        let run = run_assembly_test(&asm, 4_000).unwrap();

        assert!(asm.contains("    call __ezra_div_u8"), "{asm}");
        assert!(asm.contains("    call __ezra_mod_u8"), "{asm}");
        assert!(run.halted, "{asm}");
        assert_eq!(run.result_code, 0, "{asm}");
    }

    #[test]
    fn emits_and_runs_signed_runtime_division_and_modulo() {
        let expected_i8_div = ((-3i8) / 2) as u8;
        let expected_i8_mod = ((-3i8) % 2) as u8;
        let expected_i16_div = ((-300i16) / 7) as u16;
        let expected_i16_mod = ((-300i16) % 7) as u16;
        let expected_i24_div = ((-0x012345i32) / 17) & 0x00FF_FFFF;
        let expected_i24_mod = ((-0x012345i32) % 17) & 0x00FF_FFFF;
        let expected_i8_neg_divisor_div = (7i8 / -3) as u8;
        let expected_i8_neg_divisor_mod = (7i8 % -3) as u8;
        let expected_i8_both_negative_div = (-7i8 / -3) as u8;
        let expected_i8_both_negative_mod = (-7i8 % -3) as u8;
        let expected_i16_neg_divisor_div = (300i16 / -7) as u16;
        let expected_i16_neg_divisor_mod = (300i16 % -7) as u16;
        let expected_i24_both_negative_div = ((-0x012345i32) / -17) & 0x00FF_FFFF;
        let expected_i24_both_negative_mod = ((-0x012345i32) % -17) & 0x00FF_FFFF;
        let expected_i8_overflow_div = i8::MIN as u8;
        let expected_i16_overflow_div = i16::MIN as u16;
        let expected_i24_overflow_div = 0x800000u32;
        let expected_signed_div_zero = 0;
        let expected_signed_mod_zero = 0;
        let source = format!(
            r#"
            alias subpx = i24
            const CONST_I16_DIV_ZERO: i16 = -300 / 0
            const CONST_I16_MOD_ZERO: i16 = -300 % 0
            const CONST_I24_DIV_ZERO: subpx = -0x012345 / 0
            const CONST_I24_MOD_ZERO: subpx = -0x012345 % 0

            fn div8(a: i8, b: i8) -> i8 {{
                return a / b
            }}

            fn mod8(a: i8, b: i8) -> i8 {{
                return a % b
            }}

            fn div16(a: i16, b: i16) -> i16 {{
                return a / b
            }}

            fn mod16(a: i16, b: i16) -> i16 {{
                return a % b
            }}

            fn div24(a: subpx, b: subpx) -> subpx {{
                return a / b
            }}

            fn mod24(a: subpx, b: subpx) -> subpx {{
                return a % b
            }}

            fn main() {{
                let a: i8 = -3
                let b: i8 = 2
                test.assert_eq_u8(div8(a, b), 0x{expected_i8_div:02X}, 1)
                test.assert_eq_u8(mod8(a, b), 0x{expected_i8_mod:02X}, 2)
                test.assert_eq_u8(div8(a, 0), 0, 3)
                test.assert_eq_u8(mod8(a, 0), 0, 4)
                test.assert_eq_u8(div8(-128, -1), 0x{expected_i8_overflow_div:02X}, 5)
                test.assert_eq_u8(mod8(-128, -1), 0, 6)
                test.assert_eq_u8(div8(7, -3), 0x{expected_i8_neg_divisor_div:02X}, 15)
                test.assert_eq_u8(mod8(7, -3), 0x{expected_i8_neg_divisor_mod:02X}, 16)
                test.assert_eq_u8(div8(-7, -3), 0x{expected_i8_both_negative_div:02X}, 17)
                test.assert_eq_u8(mod8(-7, -3), 0x{expected_i8_both_negative_mod:02X}, 18)

                let c: i16 = -300
                let d: i16 = 7
                test.assert_eq_u16(div16(c, d), 0x{expected_i16_div:04X}, 7)
                test.assert_eq_u16(mod16(c, d), 0x{expected_i16_mod:04X}, 8)
                test.assert_eq_u16(div16(-32768, -1), 0x{expected_i16_overflow_div:04X}, 9)
                test.assert_eq_u16(mod16(-32768, -1), 0, 10)
                test.assert_eq_u16(div16(300, -7), 0x{expected_i16_neg_divisor_div:04X}, 19)
                test.assert_eq_u16(mod16(300, -7), 0x{expected_i16_neg_divisor_mod:04X}, 20)
                test.assert_eq_u16(div16(c, 0), {expected_signed_div_zero}, 23)
                test.assert_eq_u16(mod16(c, 0), {expected_signed_mod_zero}, 24)

                let e: subpx = -0x012345
                let f: subpx = 17
                test.assert_eq_u24(div24(e, f), 0x{expected_i24_div:06X}, 11)
                test.assert_eq_u24(mod24(e, f), 0x{expected_i24_mod:06X}, 12)
                test.assert_eq_u24(div24(-0x800000, -1), 0x{expected_i24_overflow_div:06X}, 13)
                test.assert_eq_u24(mod24(-0x800000, -1), 0, 14)
                test.assert_eq_u24(div24(-0x012345, -17), 0x{expected_i24_both_negative_div:06X}, 21)
                test.assert_eq_u24(mod24(-0x012345, -17), 0x{expected_i24_both_negative_mod:06X}, 22)
                test.assert_eq_u24(div24(e, 0), {expected_signed_div_zero}, 25)
                test.assert_eq_u24(mod24(e, 0), {expected_signed_mod_zero}, 26)
                test.assert_eq_u16(CONST_I16_DIV_ZERO, {expected_signed_div_zero}, 27)
                test.assert_eq_u16(CONST_I16_MOD_ZERO, {expected_signed_mod_zero}, 28)
                test.assert_eq_u24(CONST_I24_DIV_ZERO, {expected_signed_div_zero}, 29)
                test.assert_eq_u24(CONST_I24_MOD_ZERO, {expected_signed_mod_zero}, 30)
                test.pass()
            }}
            "#
        );
        let program = parse_program(Path::new("game.ezra"), &source).unwrap();
        let asm = emit_ez80_assembly(&program).unwrap();
        let run = run_assembly_test(&asm, 1_000_000).unwrap();

        assert!(asm.contains("    call __ezra_div_i24"), "{asm}");
        assert!(asm.contains("    call __ezra_mod_i24"), "{asm}");
        assert!(asm.contains("__ezra_div_i24:"), "{asm}");
        assert!(asm.contains("__ezra_mod_i24:"), "{asm}");
        assert!(run.halted, "{asm}");
        assert_eq!(run.result_code, 0, "{asm}");
    }

    #[test]
    fn emits_and_runs_signed_constant_returns() {
        let expected_i16 = (-300i16) as u16;
        let expected_i24 = (-0x012345i32) & 0x00FF_FFFF;
        let source = format!(
            r#"
            alias subpx = i24
            const NEG16: i16 = -300
            const NEG24: subpx = -0x012345

            fn neg16() -> i16 {{
                return NEG16
            }}

            fn neg24() -> subpx {{
                return NEG24
            }}

            fn main() {{
                test.assert_eq_u16(neg16(), 0x{expected_i16:04X}, 1)
                test.assert_eq_u24(neg24(), 0x{expected_i24:06X}, 2)
                test.pass()
            }}
            "#
        );
        let program = parse_program(Path::new("game.ezra"), &source).unwrap();
        let asm = emit_ez80_assembly(&program).unwrap();
        let run = run_assembly_test(&asm, 4_000).unwrap();

        assert!(run.halted, "{asm}");
        assert_eq!(run.result_code, 0, "{asm}");
    }

    #[test]
    fn emits_and_runs_forward_scalar_constants() {
        let source = r#"
            const DOUBLE_WIDTH: u16 = SCREEN_W * 2
            const BYTE_COUNT: u8 = BASE + EXTRA
            const MASKED: u8 = (FLAGS & ENABLED) | READY
            const CASTED: u24 = cast<u24>(DOUBLE_WIDTH) + 1

            const SCREEN_W: u16 = 160
            const BASE: u8 = 3
            const EXTRA: u8 = 4
            const FLAGS: u8 = 0b1010
            const ENABLED: u8 = 0b0110
            const READY: u8 = 0b0001

            fn main() {
                test.assert_eq_u16(DOUBLE_WIDTH, 320, 1)
                test.assert_eq_u8(BYTE_COUNT, 7, 2)
                test.assert_eq_u8(MASKED, 3, 3)
                test.assert_eq_u24(CASTED, 321, 4)
                test.pass()
            }
        "#;
        let program = parse_program(Path::new("game.ezra"), source).unwrap();
        let asm = emit_ez80_assembly(&program).unwrap();
        let run = run_assembly_test(&asm, 4_000).unwrap();

        assert!(run.halted, "{asm}");
        assert_eq!(run.result_code, 0, "{asm}");
    }

    #[test]
    fn rejects_circular_constant_references() {
        let source = r#"
            const A: u8 = B + 1
            const B: u8 = A + 1
            fn main() {
                test.pass()
            }
        "#;
        let program = parse_program(Path::new("game.ezra"), source).unwrap();
        let error = emit_ez80_assembly(&program).unwrap_err();

        assert_eq!(error.message, "circular constant reference involving `A`");
    }

    #[test]
    fn emits_and_runs_signed_arithmetic_with_untyped_literals() {
        let expected_i8 = (-3i8).wrapping_add(1) as u8;
        let expected_i16 = (-300i16).wrapping_add(1) as u16;
        let source = format!(
            r#"
            fn main() {{
                let a: i8 = -3
                test.assert_eq_u8(a + 1, 0x{expected_i8:02X}, 1)

                let b: i16 = -300
                test.assert_eq_u16(b + 1, 0x{expected_i16:04X}, 2)
                test.pass()
            }}
            "#
        );
        let program = parse_program(Path::new("game.ezra"), &source).unwrap();
        let asm = emit_ez80_assembly(&program).unwrap();
        let run = run_assembly_test(&asm, 4_000).unwrap();

        assert!(run.halted, "{asm}");
        assert_eq!(run.result_code, 0, "{asm}");
    }

    #[test]
    fn emits_and_runs_typed_integer_literal_suffixes() {
        let expected_i8 = (-3i8) as u8;
        let expected_i16 = (-300i16) as u16;
        let expected_i24 = (-0x012345i32) & 0x00FF_FFFF;
        let source = format!(
            r#"
            const BYTE: u8 = 123u8
            const NEG8: i8 = -3i8
            const WORD: u16 = 12345u16
            const NEG16: i16 = -300i16
            const LONG: u24 = 0x123456u24
            const NEG24: i24 = -0x012345i24

            fn main() {{
                let byte: u8 = 7u8
                let signed: i8 = -3i8
                let word: u16 = 0x2345u16
                let wide: u24 = 0x345678u24
                test.assert_eq_u8(BYTE, 123, 1)
                test.assert_eq_u8(cast<u8>(NEG8), 0x{expected_i8:02X}, 2)
                test.assert_eq_u16(WORD, 12345, 3)
                test.assert_eq_u16(cast<u16>(NEG16), 0x{expected_i16:04X}, 4)
                test.assert_eq_u24(LONG, 0x123456, 5)
                test.assert_eq_u24(cast<u24>(NEG24), 0x{expected_i24:06X}, 6)
                test.assert_eq_u8(byte, 7, 7)
                test.assert_eq_u8(cast<u8>(signed), 0x{expected_i8:02X}, 8)
                test.assert_eq_u16(word, 0x2345, 9)
                test.assert_eq_u24(wide, 0x345678, 10)
                test.pass()
            }}
            "#
        );
        let program = parse_program(Path::new("game.ezra"), &source).unwrap();
        let asm = emit_ez80_assembly(&program).unwrap();
        let run = run_assembly_test(&asm, 8_000).unwrap();

        assert!(run.halted, "{asm}");
        assert_eq!(run.result_code, 0, "{asm}");
    }

    #[test]
    fn emits_and_runs_signed_i24_multiply_runtime_helper() {
        let expected = ((-0x123i32) * 0x45) & 0x00FF_FFFF;
        let source = format!(
            r#"
            alias subpx = i24

            fn mul24(a: subpx, b: subpx) -> subpx {{
                return a * b
            }}

            fn main() {{
                let a: subpx = -0x123
                let b: subpx = 0x45
                test.assert_eq_u24(mul24(a, b), 0x{expected:06X}, 1)
                test.pass()
            }}
            "#
        );
        let program = parse_program(Path::new("game.ezra"), &source).unwrap();
        let asm = emit_ez80_assembly(&program).unwrap();
        let run = run_assembly_test(&asm, 80_000).unwrap();

        assert!(asm.contains("    call __ezra_mul_i24"), "{asm}");
        assert!(asm.contains("__ezra_mul_i24:"), "{asm}");
        assert!(run.halted, "{asm}");
        assert_eq!(run.result_code, 0, "{asm}");
    }

    #[test]
    fn emits_and_runs_explicit_integer_casts() {
        let source = r#"
            const SMALL: u8 = 0x12
            const WIDE: u16 = cast<u16>(SMALL) + 0x0100

            fn low_byte(v: u16) -> u8 {
                return cast<u8>(v)
            }

            fn widen(v: u8) -> u16 {
                return cast<u16>(v)
            }

            fn widen_signed_byte(v: i8) -> i16 {
                return cast<i16>(v)
            }

            fn widen_signed_word(v: i16) -> i24 {
                return cast<i24>(v)
            }

            fn widen_signed_byte_to_u24(v: i8) -> u24 {
                return cast<u24>(v)
            }

            fn bool_from_u8(v: u8) -> bool {
                return cast<bool>(v)
            }

            fn bool_from_i8(v: i8) -> bool {
                return cast<bool>(v)
            }

            fn bool_from_u16(v: u16) -> bool {
                return cast<bool>(v)
            }

            fn bool_from_u24(v: u24) -> bool {
                return cast<bool>(v)
            }

            fn bool_from_ptr(v: ptr<u8>) -> bool {
                return cast<bool>(v)
            }

            fn main() {
                let wide: u16 = cast<u16>(0x12)
                let narrow: u8 = cast<u8>(0x1234)
                let local_true: bool = cast<bool>(2)
                let local_false: bool = cast<bool>(0)
                let local_ptr_true: bool = cast<bool>(cast<ptr<u8>>(0x040123))
                let local_ptr_false: bool = cast<bool>(cast<ptr<u8>>(0u24))
                let assigned: u8 = 0
                assigned = cast<u8>(0x01FE)
                test.assert_eq_u16(wide, 0x0012, 1)
                test.assert_eq_u8(narrow, 0x34, 2)
                test.assert_eq_u8(assigned, 0xFE, 3)
                test.assert_eq_u8(low_byte(0xABCD), 0xCD, 4)
                test.assert_eq_u16(widen(0x7A), 0x007A, 5)
                test.assert_eq_u16(WIDE, 0x0112, 6)
                test.assert_eq_u16(widen_signed_byte(-3), 0xFFFD, 7)
                test.assert_eq_u24(widen_signed_word(-300), 0xFFFED4, 8)
                test.assert_eq_u24(widen_signed_byte_to_u24(-3), 0xFFFFFD, 9)
                test.assert_eq_u8(local_true, true, 10)
                test.assert_eq_u8(local_false, false, 11)
                test.assert_eq_u8(bool_from_u8(2), true, 12)
                test.assert_eq_u8(bool_from_u8(0), false, 13)
                test.assert_eq_u8(bool_from_i8(-3), true, 14)
                test.assert_eq_u8(bool_from_u16(0x0100), true, 15)
                test.assert_eq_u8(bool_from_u16(0), false, 16)
                test.assert_eq_u8(bool_from_u24(0x010000), true, 17)
                test.assert_eq_u8(bool_from_u24(0), false, 18)
                test.assert_eq_u8(local_ptr_true, true, 19)
                test.assert_eq_u8(local_ptr_false, false, 20)
                test.assert_eq_u8(bool_from_ptr(cast<ptr<u8>>(0x040123)), true, 21)
                test.assert_eq_u8(bool_from_ptr(cast<ptr<u8>>(0u24)), false, 22)
                test.pass()
            }
        "#;
        let program = parse_program(Path::new("game.ezra"), source).unwrap();
        let asm = emit_ez80_assembly(&program).unwrap();
        let run = run_assembly_test(&asm, 8_000).unwrap();

        assert!(run.halted, "{asm}");
        assert_eq!(run.result_code, 0, "{asm}");
    }

    #[test]
    fn emits_and_runs_constant_cast_semantics() {
        let source = r#"
            alias byte = u8

            const NARROW: u8 = cast<u8>(0x1234)
            const WIDE: u16 = cast<u16>(0x12)
            const BIT_PATTERN: u8 = cast<u8>(-1)
            const ALIAS_NARROW: byte = cast<byte>(0x01AB)
            const TRUE_VALUE: bool = cast<bool>(2)
            const FALSE_VALUE: bool = cast<bool>(0)
            const RAW: u24 = cast<u24>(cast<ptr<u8>>(0x040123))
            const PTR_TRUE: bool = cast<bool>(cast<ptr<u8>>(0x040123))
            const PTR_FALSE: bool = cast<bool>(cast<ptr<u8>>(0u24))

            fn main() {
                test.assert_eq_u8(NARROW, 0x34, 1)
                test.assert_eq_u16(WIDE, 0x0012, 2)
                test.assert_eq_u8(BIT_PATTERN, 0xFF, 3)
                test.assert_eq_u8(ALIAS_NARROW, 0xAB, 4)
                test.assert_eq_u8(TRUE_VALUE, true, 5)
                test.assert_eq_u8(FALSE_VALUE, false, 6)
                test.assert_eq_u24(RAW, 0x040123, 7)
                test.assert_eq_u8(PTR_TRUE, true, 8)
                test.assert_eq_u8(PTR_FALSE, false, 9)
                test.pass()
            }
        "#;
        let program = parse_program(Path::new("game.ezra"), source).unwrap();
        let asm = emit_ez80_assembly(&program).unwrap();
        let run = run_assembly_test(&asm, 6_000).unwrap();

        assert!(run.halted, "{asm}");
        assert_eq!(run.result_code, 0, "{asm}");
    }

    #[test]
    fn emits_and_runs_wrapping_constant_arithmetic() {
        let source = format!(
            r#"
            const U8_WRAP: u8 = 255 + 1
            const I8_WRAP: i8 = 127 + 1
            const U16_WRAP: u16 = 0xFFFF + 2
            const I16_WRAP: i16 = 32767 + 1
            const U8_NOT: u8 = ~0
            const U8_SHIFT: u8 = 1 << 8
            const HOST_ADD_WRAP: u24 = 9223372036854775807 + 1
            const HOST_SUB_WRAP: u24 = (-9223372036854775807 - 1) - 1
            const HOST_MUL_WRAP: u24 = 9223372036854775807 * 3
            const HOST_NEG_WRAP: u24 = -(-9223372036854775807 - 1)

            fn main() {{
                test.assert_eq_u8(U8_WRAP, 0x{:02X}, 1)
                test.assert_eq_u8(cast<u8>(I8_WRAP), 0x{:02X}, 2)
                test.assert_eq_u16(U16_WRAP, 0x{:04X}, 3)
                test.assert_eq_u16(cast<u16>(I16_WRAP), 0x{:04X}, 4)
                test.assert_eq_u8(U8_NOT, 0x{:02X}, 5)
                test.assert_eq_u8(U8_SHIFT, 0x{:02X}, 6)
                test.assert_eq_u24(HOST_ADD_WRAP, 0x{:06X}, 7)
                test.assert_eq_u24(HOST_SUB_WRAP, 0x{:06X}, 8)
                test.assert_eq_u24(HOST_MUL_WRAP, 0x{:06X}, 9)
                test.assert_eq_u24(HOST_NEG_WRAP, 0x{:06X}, 10)
                test.pass()
            }}
            "#,
            255u8.wrapping_add(1),
            127i8.wrapping_add(1) as u8,
            0xFFFFu16.wrapping_add(2),
            32767i16.wrapping_add(1) as u16,
            !0u8,
            1u16.wrapping_shl(8) as u8,
            (i64::MAX.wrapping_add(1) as u64 & 0x00FF_FFFF) as u32,
            (i64::MIN.wrapping_sub(1) as u64 & 0x00FF_FFFF) as u32,
            (i64::MAX.wrapping_mul(3) as u64 & 0x00FF_FFFF) as u32,
            (i64::MIN.wrapping_neg() as u64 & 0x00FF_FFFF) as u32,
        );
        let program = parse_program(Path::new("game.ezra"), &source).unwrap();
        let asm = emit_ez80_assembly(&program).unwrap();
        let run = run_assembly_test(&asm, 6_000).unwrap();

        assert!(run.halted, "{asm}");
        assert_eq!(run.result_code, 0, "{asm}");
    }

    #[test]
    fn emits_and_runs_pointer_constants() {
        let source = r#"
            const TI_VRAM: ptr<u8> = 0x040180
            const TI_VRAM_RAW: u24 = cast<u24>(TI_VRAM)
            const AGON_VDP_BUFFER: ptr<u16> = 0x040190

            fn main() {
                *(TI_VRAM) = 0x42;
                test.assert_eq_u8(*TI_VRAM, 0x42, 1);

                let ti_next: ptr<u8> = TI_VRAM + 1;
                *(ti_next) = 0x43;
                test.assert_eq_u8(*(TI_VRAM + 1), 0x43, 2);
                test.assert_eq_u24(TI_VRAM_RAW, 0x040180, 3);

                let agon_next: ptr<u16> = AGON_VDP_BUFFER + 1;
                *(agon_next) = 0x1234;
                test.assert_eq_u16(*(AGON_VDP_BUFFER + 1), 0x1234, 4);
                test.assert_eq_u24(cast<u24>(agon_next), cast<u24>(AGON_VDP_BUFFER) + 2, 5);
                test.pass();
            }
        "#;
        let program = parse_program(Path::new("game.ezra"), source).unwrap();
        let asm = emit_ez80_assembly(&program).unwrap();
        let run = run_assembly_test(&asm, 8_000).unwrap();

        assert!(run.halted, "{asm}");
        assert_eq!(run.result_code, 0, "{asm}");
    }

    #[test]
    fn emits_and_runs_null_pointer_constants() {
        let source = r#"
            const NULL_BYTE: ptr<u8> = 0
            const NULL_WORD: ptr<u16> = cast<ptr<u16>>(0u24)
            const NULL_RAW: ptr24 = cast<ptr24>(NULL_BYTE)

            fn is_null(p: ptr<u8>) -> bool {
                return p == NULL_BYTE
            }

            fn main() {
                let local_null: ptr<u8> = cast<ptr<u8>>(0u24)
                test.assert_eq_u24(cast<u24>(NULL_BYTE), 0, 1)
                test.assert_eq_u24(cast<u24>(NULL_WORD), 0, 2)
                test.assert_eq_u24(cast<u24>(local_null), 0, 3)
                test.assert_eq_u24(cast<u24>(cast<ptr<u8>>(NULL_RAW)), 0, 4)
                test.assert_eq_u8(is_null(local_null), true, 5)
                test.assert_eq_u8(local_null != cast<ptr<u8>>(0u24), false, 6)
                test.pass()
            }
        "#;
        let program = parse_program(Path::new("game.ezra"), source).unwrap();
        let asm = emit_ez80_assembly(&program).unwrap();
        let run = run_assembly_test(&asm, 4_000).unwrap();

        assert!(run.halted, "{asm}");
        assert_eq!(run.result_code, 0, "{asm}");
    }

    #[test]
    fn emits_and_runs_same_type_pointer_comparisons() {
        let source = r#"
            global left: u8 = 1
            global right: u8 = 2
            global words: [u16; 2] = [0x0102, 0x0304]

            fn same_byte(a: ptr<u8>, b: ptr<u8>) -> bool {
                return a == b
            }

            fn different_word(a: ptr<u16>, b: ptr<u16>) -> bool {
                return a != b
            }

            fn main() {
                let left_ptr: ptr<u8> = &left
                let also_left: ptr<u8> = &left
                let right_ptr: ptr<u8> = &right
                let first_word: ptr<u16> = &words[0]
                let second_word: ptr<u16> = &words[1]

                test.assert_eq_u8(same_byte(left_ptr, also_left), true, 1)
                test.assert_eq_u8(same_byte(left_ptr, right_ptr), false, 2)
                test.assert_eq_u8(different_word(first_word, second_word), true, 3)
                test.assert_eq_u8(different_word(first_word, first_word), false, 4)
                test.pass()
            }
        "#;
        let program = parse_program(Path::new("game.ezra"), source).unwrap();
        let asm = emit_ez80_assembly(&program).unwrap();
        let run = run_assembly_test(&asm, 8_000).unwrap();

        assert!(run.halted, "{asm}");
        assert_eq!(run.result_code, 0, "{asm}");
    }

    #[test]
    fn emits_and_runs_pointer_u24_cast_round_trip() {
        let source = r#"
            global byte: u8 = 0

            fn main() {
                let p: ptr<u8> = &byte
                let raw: u24 = cast<u24>(p)
                let q: ptr<u8> = cast<ptr<u8>>(raw)
                mem.poke8(q, 0x6D)
                test.assert_eq_u8(byte, 0x6D, 1)
                test.assert_eq_u24(raw, cast<u24>(&byte), 2)
                test.pass()
            }
        "#;
        let program = parse_program(Path::new("game.ezra"), source).unwrap();
        let asm = emit_ez80_assembly(&program).unwrap();
        let run = run_assembly_test(&asm, 8_000).unwrap();

        assert!(run.halted, "{asm}");
        assert_eq!(run.result_code, 0, "{asm}");
    }

    #[test]
    fn emits_and_runs_runtime_multiplication() {
        let expected_u8 = 17u8.wrapping_mul(15);
        let expected_u16 = 0x0123u16.wrapping_mul(0x0021);
        let expected_u16_wrap = 0xFFFFu16.wrapping_mul(0xFFFF);
        let expected_u24 = (0x000123u32 * 0x000045) & 0x00FF_FFFF;
        let expected_wrap = (0x00FF00u32 * 0x000101) & 0x00FF_FFFF;
        let source = format!(
            r#"
            struct Accum {{
                wide: u16
                long: u24
            }}

            fn mul8(a: u8, b: u8) -> u8 {{
                return a * b
            }}

            fn mul16(a: u16, b: u16) -> u16 {{
                return a * b
            }}

            fn mul24(a: u24, b: u24) -> u24 {{
                return a * b
            }}

            fn main() {{
                let a: u8 = mul8(17, 15)
                test.assert_eq_u8(a, 0x{expected_u8:02X}, 1)

                let b: u16 = mul16(0x0123, 0x0021)
                test.assert_eq_u16(b, 0x{expected_u16:04X}, 2)

                let b_wrap: u16 = mul16(0xFFFF, 0xFFFF)
                test.assert_eq_u16(b_wrap, 0x{expected_u16_wrap:04X}, 7)

                let c: u24 = mul24(0x000123, 0x000045)
                test.assert_eq_u24(c, 0x{expected_u24:06X}, 3)

                let d: u24 = mul24(0x00FF00, 0x000101)
                test.assert_eq_u24(d, 0x{expected_wrap:06X}, 4)

                let accum: Accum = Accum {{ wide: 3, long: 5 }}
                accum.wide = accum.wide * 7
                accum.long = accum.long * 9
                test.assert_eq_u16(accum.wide, 21, 5)
                test.assert_eq_u24(accum.long, 45, 6)
                test.pass()
            }}
            "#
        );
        let program = parse_program(Path::new("game.ezra"), &source).unwrap();
        let asm = emit_ez80_assembly(&program).unwrap();
        let run = run_assembly_test(&asm, 120_000).unwrap();

        assert!(asm.contains("    call __ezra_mul_u8"), "{asm}");
        assert!(
            asm.contains("__ezra_mul_u8:\n    ld b, a\n    mlt bc\n    ld a, c\n    ret"),
            "{asm}"
        );
        assert!(asm.contains("    call __ezra_mul_u16"), "{asm}");
        assert!(
            asm.contains("__ezra_mul_u16:\n    ld d, h\n    ld e, l\n    ld h, c\n    mlt hl"),
            "{asm}"
        );
        assert!(asm.contains("    call __ezra_mul_u24"), "{asm}");
        assert!(asm.contains("__ezra_mul_u24:"), "{asm}");
        assert!(run.halted, "{asm}");
        assert_eq!(run.result_code, 0, "{asm}");
    }

    #[test]
    fn emits_and_runs_wide_runtime_division_and_modulo() {
        let expected_u16_div = 1000u16 / 17;
        let expected_u16_mod = 1000u16 % 17;
        let expected_u24_div = 0x000123u32 / 5;
        let expected_u24_mod = 0x000123u32 % 5;
        let source = format!(
            r#"
            fn div16(a: u16, b: u16) -> u16 {{
                return a / b
            }}

            fn mod16(a: u16, b: u16) -> u16 {{
                return a % b
            }}

            fn div24(a: u24, b: u24) -> u24 {{
                return a / b
            }}

            fn mod24(a: u24, b: u24) -> u24 {{
                return a % b
            }}

            fn main() {{
                test.assert_eq_u16(div16(1000, 17), {expected_u16_div}, 1)
                test.assert_eq_u16(mod16(1000, 17), {expected_u16_mod}, 2)
                test.assert_eq_u16(div16(1000, 0), 0, 3)
                test.assert_eq_u16(mod16(1000, 0), 0, 4)

                test.assert_eq_u24(div24(0x000123, 5), 0x{expected_u24_div:06X}, 5)
                test.assert_eq_u24(mod24(0x000123, 5), 0x{expected_u24_mod:06X}, 6)
                test.assert_eq_u24(div24(0x000123, 0), 0, 7)
                test.assert_eq_u24(mod24(0x000123, 0), 0, 8)
                test.pass()
            }}
            "#
        );
        let program = parse_program(Path::new("game.ezra"), &source).unwrap();
        let asm = emit_ez80_assembly(&program).unwrap();
        let run = run_assembly_test(&asm, 80_000).unwrap();

        assert!(asm.contains("    call __ezra_div_u16"), "{asm}");
        assert!(asm.contains("    call __ezra_mod_u16"), "{asm}");
        assert!(asm.contains("    call __ezra_div_u24"), "{asm}");
        assert!(asm.contains("    call __ezra_mod_u24"), "{asm}");
        assert!(asm.contains("__ezra_div_u24:"), "{asm}");
        assert!(asm.contains("__ezra_mod_u24:"), "{asm}");
        assert!(run.halted, "{asm}");
        assert_eq!(run.result_code, 0, "{asm}");
    }

    #[test]
    fn constant_division_uses_truncating_semantics() {
        assert_eq!(trunc_div_or_zero(7, 3), 2);
        assert_eq!(trunc_mod_or_zero(7, 3), 1);
        assert_eq!(trunc_div_or_zero(-7, 3), -2);
        assert_eq!(trunc_mod_or_zero(-7, 3), -1);
        assert_eq!(trunc_div_or_zero(7, -3), -2);
        assert_eq!(trunc_mod_or_zero(7, -3), 1);
        assert_eq!(trunc_div_or_zero(-3, 2), -1);
        assert_eq!(trunc_div_or_zero(7, 0), 0);
        assert_eq!(trunc_mod_or_zero(7, 0), 0);
    }

    #[test]
    fn emits_and_runs_generic_hardware_port_examples() {
        let source = r#"
            port PAD1_LO: u8 = 0x01
            port PAD1_HI: u8 = 0x02
            port TI_LCD_CMD: u8 = 0x10
            port TI_LCD_DATA: u8 = 0x11
            port AGON_VDP_DATA: u8 = 0x9B

            fn read_pad_low() -> u8 {
                return in PAD1_LO
            }

            fn ti_lcd_command(cmd: u8) {
                out TI_LCD_CMD, cmd
            }

            fn ti_lcd_data(value: u8) {
                out TI_LCD_DATA, value
            }

            fn agon_vdp_byte(value: u8) {
                out AGON_VDP_DATA, value
            }

            fn main() {
                let pad_lo: u8 = read_pad_low()
                let pad_hi: u8 = in PAD1_HI
                ti_lcd_command(0x2A)
                ti_lcd_data(pad_lo)
                agon_vdp_byte(pad_hi)
                test.assert_eq_u8(pad_lo, 0, 1)
                test.assert_eq_u8(pad_hi, 0, 2)
                test.pass()
            }
        "#;
        let program = parse_program(Path::new("game.ezra"), source).unwrap();
        let asm = emit_ez80_assembly(&program).unwrap();
        let run = run_assembly_test(&asm, 2_000).unwrap();

        assert!(asm.contains("in0 a, (01h)"), "{asm}");
        assert!(asm.contains("in0 a, (02h)"), "{asm}");
        assert!(asm.contains("out0 (10h), a"), "{asm}");
        assert!(asm.contains("out0 (11h), a"), "{asm}");
        assert!(asm.contains("out0 (9Bh), a"), "{asm}");
        assert!(run.halted, "{asm}");
        assert_eq!(run.result_code, 0, "{asm}");
    }

    #[test]
    fn emits_and_runs_default_fantasy_port_map_symbols() {
        let source = r#"
            fn main() {
                let pad2: u8 = in PAD2_LO
                let status: u8 = in EXT_STATUS
                out VIDEO_CMD, VIDEO_SET_MODE
                out EXT_ADDR0, pad2
                out EXT_COMMAND, status
                test.assert_eq_u8(pad2, 0x33, 1)
                test.assert_eq_u8(status, 0x44, 2)
                test.pass()
            }
        "#;
        let program = parse_program(Path::new("game.ezra"), source).unwrap();
        let asm = emit_ez80_assembly(&program).unwrap();
        let run = run_assembly_test_with_options(
            &asm,
            &TestRunOptions {
                instruction_budget: 4_000,
                initial_ports: vec![(0x03, 0x33), (0x17, 0x44)],
                initial_memory: Vec::new(),
                stack_top: EZRA_STACK_TOP.get(),
            },
        )
        .unwrap();

        assert!(asm.contains("in0 a, (03h)"), "{asm}");
        assert!(asm.contains("in0 a, (17h)"), "{asm}");
        assert!(asm.contains("out0 (09h), a"), "{asm}");
        assert!(asm.contains("out0 (10h), a"), "{asm}");
        assert!(asm.contains("out0 (16h), a"), "{asm}");
        assert!(run.halted, "{asm}");
        assert_eq!(run.result_code, 0, "{asm}");
        assert_eq!(run.ports[0x09], 3, "{asm}");
        assert_eq!(run.ports[0x10], 0x33, "{asm}");
        assert_eq!(run.ports[0x16], 0x44, "{asm}");
    }

    #[test]
    fn can_disable_default_sdk_symbols_for_target_specific_hardware() {
        let source = r#"
            const SCREEN: ptr<u8> = 0x040180
            port TI_KEYGROUP: u8 = 0x01
            port AGON_VDP: u8 = 0x9B

            fn main() {
                let keys: u8 = in TI_KEYGROUP
                *(SCREEN) = keys
                out AGON_VDP, *SCREEN
                test.assert_eq_u8(*SCREEN, 0x2C, 1)
                test.pass()
            }
        "#;
        let program = parse_program(Path::new("game.ezra"), source).unwrap();
        let asm = emit_ez80_assembly_with_options(
            &program,
            AssemblyOptions {
                default_sdk_symbols: false,
                ..AssemblyOptions::default()
            },
        )
        .unwrap();
        let run = run_assembly_test_with_options(
            &asm,
            &TestRunOptions {
                instruction_budget: 4_000,
                initial_ports: vec![(0x01, 0x2C)],
                initial_memory: Vec::new(),
                stack_top: EZRA_STACK_TOP.get(),
            },
        )
        .unwrap();

        assert!(asm.contains("in0 a, (01h)"), "{asm}");
        assert!(asm.contains("out0 (9Bh), a"), "{asm}");
        assert!(run.halted, "{asm}");
        assert_eq!(run.result_code, 0, "{asm}");
        assert_eq!(run.ports[0x9B], 0x2C, "{asm}");

        let default_port_source = r#"
            fn main() {
                let pad: u8 = in PAD1_LO
                test.assert_eq_u8(pad, 0, 1)
                test.pass()
            }
        "#;
        let program = parse_program(Path::new("game.ezra"), default_port_source).unwrap();
        let error = emit_ez80_assembly_with_options(
            &program,
            AssemblyOptions {
                default_sdk_symbols: false,
                ..AssemblyOptions::default()
            },
        )
        .unwrap_err();

        assert_eq!(error.message, "unknown port `PAD1_LO`");
    }

    #[test]
    fn emits_and_runs_default_video_audio_base_pointer_symbols() {
        let source = r#"
            fn main() {
                *(VRAM_BASE + 1) = 0x4A;
                *(AUDIO_BASE + 2) = 0x5B;
                test.assert_eq_u8(*(VRAM_BASE + 1), 0x4A, 1)
                test.assert_eq_u8(*(AUDIO_BASE + 2), 0x5B, 2)
                test.pass()
            }
        "#;
        let program = parse_program(Path::new("game.ezra"), source).unwrap();
        let asm = emit_ez80_assembly_with_options(
            &program,
            AssemblyOptions {
                vram_base: Address24::new(0x04_0180),
                audio_base: Address24::new(0x04_0190),
                ..AssemblyOptions::default()
            },
        )
        .unwrap();
        let run = run_assembly_test(&asm, 4_000).unwrap();

        assert!(asm.contains("040180h"), "{asm}");
        assert!(asm.contains("040190h"), "{asm}");
        assert!(run.halted, "{asm}");
        assert_eq!(run.result_code, 0, "{asm}");
    }

    #[test]
    fn emits_and_runs_ti84_plus_ce_style_sdk_modules() {
        let root = std::env::temp_dir().join(format!(
            "ezra_ti84_sdk_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(root.join("ti84")).unwrap();
        let main_path = root.join("game.ezra");
        std::fs::write(
            root.join("ti84/keys.ezra"),
            r#"
            pub port KEY_LO: u8 = 0x01
            pub port KEY_HI: u8 = 0x02

            pub fn scan() -> u16 {
                let lo: u8 = in KEY_LO
                let hi: u8 = in KEY_HI
                return cast<u16>(lo) | (cast<u16>(hi) << 8)
            }
            "#,
        )
        .unwrap();
        std::fs::write(
            root.join("ti84/lcd.ezra"),
            r#"
            pub port LCD_CMD: u8 = 0x10
            pub port LCD_DATA: u8 = 0x11
            pub volatile mmio LCD_SHADOW: ptr<u8> = 0x040240

            pub fn command(value: u8) {
                out LCD_CMD, value
            }

            pub fn write(value: u8) {
                *(LCD_SHADOW) = value
                out LCD_DATA, value
            }
            "#,
        )
        .unwrap();
        std::fs::write(
            &main_path,
            r#"
            import ti84.keys
            import ti84.lcd

            fn main() {
                let keys: u16 = keys.scan()
                lcd.command(0x2A)
                lcd.write(cast<u8>(keys))

                test.assert_eq_u16(keys, 0x1205, 1)
                test.assert_eq_u8(*(lcd.LCD_SHADOW), 0x05, 2)
                test.pass()
            }
            "#,
        )
        .unwrap();

        let program = load_program(&main_path).unwrap();
        let asm = emit_ez80_assembly(&program).unwrap();
        let run = run_assembly_test_with_options(
            &asm,
            &TestRunOptions {
                instruction_budget: 8_000,
                initial_ports: vec![(0x01, 0x05), (0x02, 0x12)],
                initial_memory: Vec::new(),
                stack_top: EZRA_STACK_TOP.get(),
            },
        )
        .unwrap();

        let _ = std::fs::remove_dir_all(&root);
        assert!(asm.contains("_keys_scan:"), "{asm}");
        assert!(asm.contains("_lcd_write:"), "{asm}");
        assert!(asm.contains("in0 a, (01h)"), "{asm}");
        assert!(asm.contains("in0 a, (02h)"), "{asm}");
        assert!(asm.contains("out0 (10h), a"), "{asm}");
        assert!(asm.contains("out0 (11h), a"), "{asm}");
        assert!(run.halted, "{asm}");
        assert_eq!(run.result_code, 0, "{asm}");
    }

    #[test]
    fn emits_and_runs_agon_light_style_sdk_modules() {
        let root = std::env::temp_dir().join(format!(
            "ezra_agon_sdk_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(root.join("agon")).unwrap();
        let main_path = root.join("game.ezra");
        std::fs::write(
            root.join("agon/vdp.ezra"),
            r#"
            pub port VDP_DATA: u8 = 0x9B
            pub volatile mmio VDP_SHADOW: ptr<u8> = 0x040260

            pub fn byte(value: u8) {
                *(VDP_SHADOW) = value
                out VDP_DATA, value
            }
            "#,
        )
        .unwrap();
        std::fs::write(
            root.join("agon/system.ezra"),
            r#"
            pub port STATUS: u8 = 0x17

            pub fn status() -> u8 {
                return in STATUS
            }
            "#,
        )
        .unwrap();
        std::fs::write(
            &main_path,
            r#"
            import agon.vdp
            import agon.system

            fn main() {
                let sys_status: u8 = system.status()
                vdp.byte(sys_status ^ 0xFF)

                test.assert_eq_u8(sys_status, 0xA0, 1)
                test.assert_eq_u8(*(vdp.VDP_SHADOW), 0x5F, 2)
                test.pass()
            }
            "#,
        )
        .unwrap();

        let program = load_program(&main_path).unwrap();
        let asm = emit_ez80_assembly(&program).unwrap();
        let run = run_assembly_test_with_options(
            &asm,
            &TestRunOptions {
                instruction_budget: 6_000,
                initial_ports: vec![(0x17, 0xA0)],
                initial_memory: Vec::new(),
                stack_top: EZRA_STACK_TOP.get(),
            },
        )
        .unwrap();

        let _ = std::fs::remove_dir_all(&root);
        assert!(asm.contains("_vdp_byte:"), "{asm}");
        assert!(asm.contains("_system_status:"), "{asm}");
        assert!(asm.contains("in0 a, (17h)"), "{asm}");
        assert!(asm.contains("out0 (9Bh), a"), "{asm}");
        assert!(run.halted, "{asm}");
        assert_eq!(run.result_code, 0, "{asm}");
    }

    #[test]
    fn emits_and_runs_type_aliases() {
        let source = r#"
            alias subpx = i24
            alias addr = ptr<u8>
            alias byte = u8

            volatile mmio SCRATCH: addr = 0x040180
            global player_x: subpx = 0x000100

            fn add_pos(x: subpx, dx: subpx) -> subpx {
                return x + dx
            }

            fn main() {
                let x: subpx = add_pos(player_x, 0x000080)
                let p: addr = cast<addr>(0x040181)
                let value: byte = 0x37
                mem.poke8(SCRATCH, value)
                mem.poke8(p, mem.peek8(SCRATCH) + 1)
                test.assert_eq_u24(x, 0x000180, 1)
                test.assert_eq_u8(mem.peek8(p), 0x38, 2)
                test.pass()
            }
        "#;
        let program = parse_program(Path::new("game.ezra"), source).unwrap();
        let asm = emit_ez80_assembly(&program).unwrap();
        let run = run_assembly_test(&asm, 4_000).unwrap();

        assert!(run.halted, "{asm}");
        assert_eq!(run.result_code, 0, "{asm}");
    }

    #[test]
    fn emits_and_runs_repeated_volatile_mmio_dereferences() {
        let source = r#"
            volatile mmio STATUS: ptr<u8> = 0x040270
            volatile mmio CONTROL: ptr<u8> = 0x040271

            fn main() {
                *STATUS;
                *STATUS;
                *(CONTROL) = 0x34;
                *(CONTROL) = 0x35;
                test.assert_eq_u8(*STATUS, 0x12, 1)
                test.assert_eq_u8(*CONTROL, 0x35, 2)
                test.pass()
            }
        "#;
        let program = parse_program(Path::new("game.ezra"), source).unwrap();
        let asm = emit_ez80_assembly(&program).unwrap();
        let run = run_assembly_test_with_options(
            &asm,
            &TestRunOptions {
                instruction_budget: 4_000,
                initial_ports: Vec::new(),
                initial_memory: vec![(0x040270, 0x12)],
                stack_top: EZRA_STACK_TOP.get(),
            },
        )
        .unwrap();

        assert!(run.halted, "{asm}");
        assert_eq!(run.result_code, 0, "{asm}");
        assert!(asm.matches("    ld hl, 040270h").count() >= 3, "{asm}");
        assert!(asm.matches("    ld a, (hl)").count() >= 4, "{asm}");
        assert!(asm.matches("    ld (hl), a").count() >= 2, "{asm}");
    }

    #[test]
    fn preserves_order_between_ports_and_volatile_mmio() {
        let source = r#"
            port FIRST: u8 = 0x20
            port SECOND: u8 = 0x21
            port THIRD: u8 = 0x22
            volatile mmio STATUS: ptr<u8> = 0x040270

            fn main() {
                out FIRST, 0x11
                *(STATUS) = 0x22
                out SECOND, *STATUS
                *(STATUS) = 0x33
                out THIRD, *STATUS
                test.assert_eq_u8(*STATUS, 0x33, 1)
                test.pass()
            }
        "#;
        let program = parse_program(Path::new("game.ezra"), source).unwrap();
        let asm = emit_ez80_assembly(&program).unwrap();
        let run = run_assembly_test(&asm, 4_000).unwrap();

        assert!(asm.contains("out0 (20h), a"), "{asm}");
        assert!(asm.contains("out0 (21h), a"), "{asm}");
        assert!(asm.contains("out0 (22h), a"), "{asm}");
        assert!(asm.matches("    ld hl, 040270h").count() >= 4, "{asm}");
        assert!(asm.matches("    ld a, (hl)").count() >= 3, "{asm}");
        assert!(asm.matches("    ld (hl), a").count() >= 2, "{asm}");
        assert!(run.halted, "{asm}");
        assert_eq!(run.result_code, 0, "{asm}");
        assert_eq!(run.ports[0x20], 0x11, "{asm}");
        assert_eq!(run.ports[0x21], 0x22, "{asm}");
        assert_eq!(run.ports[0x22], 0x33, "{asm}");
    }

    #[test]
    fn emits_and_runs_static_arrays() {
        let source = r#"
            global palette: [u8; 4] = [1, 2, 3]
            global words: [u16; 3] = [0x0100, 0x0200]

            fn main() {
                test.assert_eq_u8(palette[0], 1, 1)
                test.assert_eq_u8(palette[3], 0, 2)
                palette[1] = 9
                test.assert_eq_u8(palette[1], 9, 3)

                let local: [u8; 3] = [4, 5, 6]
                local[2] = palette[1] + 1
                test.assert_eq_u8(local[2], 10, 4)

                test.assert_eq_u16(words[0], 0x0100, 5)
                test.assert_eq_u16(words[2], 0, 6)
                words[2] = 0x1234
                test.assert_eq_u16(words[2], 0x1234, 7)

                let p: ptr<u8> = &palette[1]
                mem.poke8(p, 0x44)
                test.assert_eq_u8(mem.peek8(p), 0x44, 8)
                test.assert_eq_u8(palette[1], 0x44, 9)
                test.pass()
            }
        "#;
        let program = parse_program(Path::new("game.ezra"), source).unwrap();
        let asm = emit_ez80_assembly(&program).unwrap();
        let run = run_assembly_test(&asm, 6_000).unwrap();

        assert!(run.halted, "{asm}");
        assert_eq!(run.result_code, 0, "{asm}");
    }

    #[test]
    fn emits_and_runs_array_length_constant_expressions() {
        let source = r#"
            const BASE: u8 = 2
            const EXTRA: u8 = 1
            global bytes: [u8; BASE + EXTRA] = [7, 8, 9]

            fn main() {
                test.assert_eq_u8(bytes[2], 9, 1)
                bytes[BASE] = 11
                test.assert_eq_u8(bytes[2], 11, 2)

                let local: [u16; BASE * 2] = [1, 2, 3, 4]
                test.assert_eq_u16(local[3], 4, 3)
                local[EXTRA + 1] = 0x1234
                test.assert_eq_u16(local[2], 0x1234, 4)
                test.pass()
            }
        "#;
        let program = parse_program(Path::new("game.ezra"), source).unwrap();
        let asm = emit_ez80_assembly(&program).unwrap();
        let run = run_assembly_test(&asm, 8_000).unwrap();

        assert!(run.halted, "{asm}");
        assert_eq!(run.result_code, 0, "{asm}");
    }

    #[test]
    fn emits_and_runs_large_aggregate_storage() {
        let source = r#"
            struct Big {
                padding: [u8; 300]
                tail: u8
            }

            global bytes: [u8; 300] = []
            global big: Big = Big { tail: 7 }

            fn main() {
                bytes[299] = 0xA5
                test.assert_eq_u8(bytes[299], 0xA5, 1)
                test.assert_eq_u8(big.tail, 7, 2)

                big.padding[299] = 0x5A
                test.assert_eq_u8(big.padding[299], 0x5A, 3)

                let local: [u8; 260] = []
                local[259] = 0xC3
                test.assert_eq_u8(local[259], 0xC3, 4)
                test.pass()
            }
        "#;
        let program = parse_program(Path::new("game.ezra"), source).unwrap();
        let asm = emit_ez80_assembly(&program).unwrap();
        let run = run_assembly_test(&asm, 80_000).unwrap();

        assert!(run.halted, "{asm}");
        assert_eq!(run.result_code, 0, "{asm}");
    }

    #[test]
    fn emits_and_runs_arrays_of_structs() {
        let source = r#"
            struct Point {
                x: u8
                y: u16
            }

            global points: [Point; 3] = [
                Point { x: 1, y: 0x0203 },
                Point { x: 4, y: 0x0506 }
            ]

            fn main() {
                let raw: ptr<u8> = cast<ptr<u8>>(&points[0])
                test.assert_eq_u8(mem.peek8(raw + 0), 1, 1)
                test.assert_eq_u8(mem.peek8(raw + 1), 0x03, 2)
                test.assert_eq_u8(mem.peek8(raw + 2), 0x02, 3)
                test.assert_eq_u8(mem.peek8(raw + 3), 4, 4)
                test.assert_eq_u8(mem.peek8(raw + 4), 0x06, 5)
                test.assert_eq_u8(mem.peek8(raw + 5), 0x05, 6)
                test.assert_eq_u8(mem.peek8(raw + 6), 0, 7)
                test.assert_eq_u8(mem.peek8(raw + 7), 0, 8)
                test.assert_eq_u8(mem.peek8(raw + 8), 0, 9)

                let local: [Point; 2] = [Point { x: 7, y: 0x0809 }]
                let local_raw: ptr<u8> = cast<ptr<u8>>(&local[0])
                test.assert_eq_u8(mem.peek8(local_raw + 0), 7, 10)
                test.assert_eq_u8(mem.peek8(local_raw + 1), 0x09, 11)
                test.assert_eq_u8(mem.peek8(local_raw + 2), 0x08, 12)
                test.assert_eq_u8(mem.peek8(local_raw + 3), 0, 13)
                test.assert_eq_u8(mem.peek8(local_raw + 4), 0, 14)
                test.assert_eq_u8(mem.peek8(local_raw + 5), 0, 15)

                let i: u8 = 1
                let second: ptr<u8> = cast<ptr<u8>>(&points[i])
                test.assert_eq_u24(cast<u24>(second), cast<u24>(raw) + 3, 16)
                test.assert_eq_u8(mem.peek8(second + 0), 4, 17)
                test.assert_eq_u8(mem.peek8(second + 1), 0x06, 18)
                test.assert_eq_u8(mem.peek8(second + 2), 0x05, 19)

                points[2] = Point { x: 9, y: 0x0A0B }
                test.assert_eq_u8(mem.peek8(raw + 6), 9, 20)
                test.assert_eq_u8(mem.peek8(raw + 7), 0x0B, 21)
                test.assert_eq_u8(mem.peek8(raw + 8), 0x0A, 22)

                points[i] = Point { x: 0x0C, y: 0x0D0E }
                test.assert_eq_u8(mem.peek8(second + 0), 0x0C, 23)
                test.assert_eq_u8(mem.peek8(second + 1), 0x0E, 24)
                test.assert_eq_u8(mem.peek8(second + 2), 0x0D, 25)
                test.pass()
            }
        "#;
        let program = parse_program(Path::new("game.ezra"), source).unwrap();
        let asm = emit_ez80_assembly(&program).unwrap();
        let run = run_assembly_test(&asm, 10_000).unwrap();

        assert!(run.halted, "{asm}");
        assert_eq!(run.result_code, 0, "{asm}");
    }

    #[test]
    fn emits_and_runs_nested_arrays() {
        let source = r#"
            global grid: [[u8; 3]; 3] = [
                [1, 2, 3],
                [4, 5, 6]
            ]

            fn main() {
                let raw: ptr<u8> = cast<ptr<u8>>(&grid[0])
                test.assert_eq_u8(mem.peek8(raw + 0), 1, 1)
                test.assert_eq_u8(mem.peek8(raw + 1), 2, 2)
                test.assert_eq_u8(mem.peek8(raw + 2), 3, 3)
                test.assert_eq_u8(mem.peek8(raw + 3), 4, 4)
                test.assert_eq_u8(mem.peek8(raw + 4), 5, 5)
                test.assert_eq_u8(mem.peek8(raw + 5), 6, 6)
                test.assert_eq_u8(mem.peek8(raw + 6), 0, 7)
                test.assert_eq_u8(mem.peek8(raw + 7), 0, 8)
                test.assert_eq_u8(mem.peek8(raw + 8), 0, 9)

                let row_index: u8 = 2
                let third: ptr<u8> = cast<ptr<u8>>(&grid[row_index])
                test.assert_eq_u24(cast<u24>(third), cast<u24>(raw) + 6, 10)

                grid[2] = [7, 8, 9]
                test.assert_eq_u8(mem.peek8(third + 0), 7, 11)
                test.assert_eq_u8(mem.peek8(third + 1), 8, 12)
                test.assert_eq_u8(mem.peek8(third + 2), 9, 13)

                grid[row_index] = [10, 11, 12]
                test.assert_eq_u8(mem.peek8(third + 0), 10, 14)
                test.assert_eq_u8(mem.peek8(third + 1), 11, 15)
                test.assert_eq_u8(mem.peek8(third + 2), 12, 16)

                let local: [[u16; 2]; 2] = [[0x0102, 0x0304]]
                let local_raw: ptr<u8> = cast<ptr<u8>>(&local[0])
                test.assert_eq_u8(mem.peek8(local_raw + 0), 0x02, 17)
                test.assert_eq_u8(mem.peek8(local_raw + 1), 0x01, 18)
                test.assert_eq_u8(mem.peek8(local_raw + 2), 0x04, 19)
                test.assert_eq_u8(mem.peek8(local_raw + 3), 0x03, 20)
                test.assert_eq_u8(mem.peek8(local_raw + 4), 0, 21)
                test.assert_eq_u8(mem.peek8(local_raw + 5), 0, 22)
                test.assert_eq_u8(mem.peek8(local_raw + 6), 0, 23)
                test.assert_eq_u8(mem.peek8(local_raw + 7), 0, 24)
                test.pass()
            }
        "#;
        let program = parse_program(Path::new("game.ezra"), source).unwrap();
        let asm = emit_ez80_assembly(&program).unwrap();
        let run = run_assembly_test(&asm, 12_000).unwrap();

        assert!(run.halted, "{asm}");
        assert_eq!(run.result_code, 0, "{asm}");
    }

    #[test]
    fn emits_and_runs_chained_array_and_struct_accesses() {
        let source = r#"
            struct Point {
                x: u8
                y: u16
            }

            struct Packet {
                points: [Point; 2]
            }

            global grid: [[u8; 3]; 2] = [
                [1, 2, 3],
                [4, 5, 6]
            ]

            global packets: [Packet; 2] = [
                Packet {
                    points: [
                        Point { x: 7, y: 0x0809 },
                        Point { x: 10, y: 0x0B0C }
                    ]
                }
            ]

            fn main() {
                let row: u8 = 1
                let col: u8 = 2
                test.assert_eq_u8(grid[row][col], 6, 1)
                grid[row][col] = 0x44
                test.assert_eq_u8(grid[1][2], 0x44, 2)
                grid[row][col] += 1
                test.assert_eq_u8(grid[1][2], 0x45, 3)

                let packet_index: u8 = 0
                let point_index: u8 = 1
                test.assert_eq_u8(packets[packet_index].points[point_index].x, 10, 4)
                test.assert_eq_u16(packets[packet_index].points[point_index].y, 0x0B0C, 5)
                packets[packet_index].points[point_index].x = grid[row][col]
                test.assert_eq_u8(packets[0].points[1].x, 0x45, 6)
                packets[packet_index].points[point_index].y += 1
                test.assert_eq_u16(packets[0].points[1].y, 0x0B0D, 7)

                let x_ptr: ptr<u8> = &packets[packet_index].points[point_index].x
                test.assert_eq_u8(*x_ptr, 0x45, 8)
                test.pass()
            }
        "#;
        let program = parse_program(Path::new("game.ezra"), source).unwrap();
        let asm = emit_ez80_assembly(&program).unwrap();
        let run = run_assembly_test(&asm, 30_000).unwrap();

        assert!(run.halted, "{asm}");
        assert_eq!(run.result_code, 0, "{asm}");
    }

    #[test]
    fn emits_and_runs_structs_with_array_fields() {
        let source = r#"
            struct Packet {
                tag: u8
                bytes: [u8; 3]
                words: [u16; 2]
            }

            global packet: Packet = Packet {
                tag: 0xAA,
                bytes: [1, 2, 3],
                words: [0x0405]
            }

            fn main() {
                let raw: ptr<u8> = cast<ptr<u8>>(&packet)
                test.assert_eq_u8(mem.peek8(raw + 0), 0xAA, 1)
                test.assert_eq_u8(mem.peek8(raw + 1), 1, 2)
                test.assert_eq_u8(mem.peek8(raw + 2), 2, 3)
                test.assert_eq_u8(mem.peek8(raw + 3), 3, 4)
                test.assert_eq_u8(mem.peek8(raw + 4), 0x05, 5)
                test.assert_eq_u8(mem.peek8(raw + 5), 0x04, 6)
                test.assert_eq_u8(mem.peek8(raw + 6), 0, 7)
                test.assert_eq_u8(mem.peek8(raw + 7), 0, 8)

                packet.bytes = [9, 8]
                let bytes: ptr<u8> = cast<ptr<u8>>(&packet.bytes)
                test.assert_eq_u8(mem.peek8(bytes + 0), 9, 9)
                test.assert_eq_u8(mem.peek8(bytes + 1), 8, 10)
                test.assert_eq_u8(mem.peek8(bytes + 2), 0, 11)

                let local: Packet = Packet { tag: 0x55 }
                let local_raw: ptr<u8> = cast<ptr<u8>>(&local)
                test.assert_eq_u8(mem.peek8(local_raw + 0), 0x55, 12)
                test.assert_eq_u8(mem.peek8(local_raw + 1), 0, 13)
                test.assert_eq_u8(mem.peek8(local_raw + 7), 0, 14)
                test.pass()
            }
        "#;
        let program = parse_program(Path::new("game.ezra"), source).unwrap();
        let asm = emit_ez80_assembly(&program).unwrap();
        let run = run_assembly_test(&asm, 10_000).unwrap();

        assert!(run.halted, "{asm}");
        assert_eq!(run.result_code, 0, "{asm}");
    }

    #[test]
    fn emits_and_runs_forward_constants_in_struct_array_fields() {
        let source = r#"
            struct Packet {
                tag: u8
                bytes: [u8; BYTE_LEN]
                words: [u16; WORD_LEN + EXTRA_WORDS]
            }

            global packet: Packet = Packet {
                tag: 0xAA,
                bytes: [1, 2, 3, 4],
                words: [0x0506, 0x0708, 0x090A]
            }

            const BYTE_LEN: u8 = BASE_LEN + 1
            const BASE_LEN: u8 = 3
            const WORD_LEN: u8 = 2
            const EXTRA_WORDS: u8 = 1

            fn main() {
                let raw: ptr<u8> = cast<ptr<u8>>(&packet)
                test.assert_eq_u8(mem.peek8(raw + 0), 0xAA, 1)
                test.assert_eq_u8(mem.peek8(raw + 4), 4, 2)
                test.assert_eq_u8(mem.peek8(raw + 5), 0x06, 3)
                test.assert_eq_u8(mem.peek8(raw + 10), 0x09, 4)
                packet.bytes[2] = 0x55
                packet.words[2] = 0x1234
                test.assert_eq_u8(packet.bytes[2], 0x55, 5)
                test.assert_eq_u16(packet.words[2], 0x1234, 6)
                test.pass()
            }
        "#;
        let program = parse_program(Path::new("game.ezra"), source).unwrap();
        let asm = emit_ez80_assembly(&program).unwrap();
        let run = run_assembly_test(&asm, 16_000).unwrap();

        assert!(run.halted, "{asm}");
        assert_eq!(run.result_code, 0, "{asm}");
    }

    #[test]
    fn emits_and_runs_array_pointer_parameters() {
        let source = r#"
            global bytes: [u8; 3] = [0x11, 0x22, 0x33]

            fn first(values: ptr<[u8; 3]>) -> u8 {
                return mem.peek8(cast<ptr<u8>>(values))
            }

            fn second(values: ptr<[u8; 3]>) -> u8 {
                let raw: ptr<u8> = cast<ptr<u8>>(values)
                return mem.peek8(raw + 1)
            }

            fn main() {
                test.assert_eq_u8(first(&bytes), 0x11, 1)
                test.assert_eq_u8(second(&bytes), 0x22, 2)
                test.pass()
            }
        "#;
        let program = parse_program(Path::new("game.ezra"), source).unwrap();
        let asm = emit_ez80_assembly(&program).unwrap();
        let run = run_assembly_test(&asm, 6_000).unwrap();

        assert!(run.halted, "{asm}");
        assert_eq!(run.result_code, 0, "{asm}");
    }

    #[test]
    fn rejects_array_pointer_decay() {
        let cases = [
            r#"
            global bytes: [u8; 2] = [1, 2]

            fn main() {
                let ptr: ptr<u8> = bytes
                test.pass()
            }
            "#,
            r#"
            global bytes: [u8; 2] = [1, 2]

            fn first(values: ptr<[u8; 2]>) -> u8 {
                let raw: ptr<u8> = cast<ptr<u8>>(values)
                return *raw
            }

            fn main() {
                test.assert_eq_u8(first(bytes), 1, 1)
                test.pass()
            }
            "#,
            r#"
            global bytes: [u8; 2] = [1, 2]
            global dst: [u8; 2] = [0, 0]

            fn main() {
                mem.memcpy(dst, bytes, 2)
                test.pass()
            }
            "#,
        ];

        for source in cases {
            let program = parse_program(Path::new("game.ezra"), source).unwrap();
            let error = emit_ez80_assembly(&program).unwrap_err();

            assert_eq!(error.message, "type mismatch");
        }
    }

    #[test]
    fn emits_and_runs_array_pointer_arithmetic_scale() {
        let source = r#"
            struct Cell {
                x: u8
                y: u16
            }

            fn next_chunk(values: ptr<[u8; 3]>) -> ptr<[u8; 3]> {
                return values + 1
            }

            fn prev_chunk(values: ptr<[u8; 3]>) -> ptr<[u8; 3]> {
                return values - 1
            }

            fn next_cell(values: ptr<Cell>) -> ptr<Cell> {
                return values + 1
            }

            fn prev_cell(values: ptr<Cell>) -> ptr<Cell> {
                return values - 1
            }

            fn main() {
                let chunks: [[u8; 3]; 2] = [[1, 2, 3], [4, 5, 6]]
                let next: ptr<[u8; 3]> = next_chunk(&chunks[0])
                let prev: ptr<[u8; 3]> = prev_chunk(next)
                test.assert_eq_u24(cast<u24>(next), cast<u24>(&chunks[0]) + 3, 1)
                test.assert_eq_u24(cast<u24>(prev), cast<u24>(&chunks[0]), 2)

                let cells: [Cell; 2] = [
                    Cell { x: 1, y: 0x0203 },
                    Cell { x: 4, y: 0x0506 },
                ]
                let second: ptr<Cell> = next_cell(&cells[0])
                let first: ptr<Cell> = prev_cell(second)
                test.assert_eq_u24(cast<u24>(second), cast<u24>(&cells[0]) + 3, 3)
                test.assert_eq_u24(cast<u24>(first), cast<u24>(&cells[0]), 4)
                test.pass()
            }
        "#;
        let program = parse_program(Path::new("game.ezra"), source).unwrap();
        let asm = emit_ez80_assembly(&program).unwrap();
        let run = run_assembly_test(&asm, 8_000).unwrap();

        assert!(run.halted, "{asm}");
        assert_eq!(run.result_code, 0, "{asm}");
    }

    #[test]
    fn emits_and_runs_runtime_array_indexes() {
        let source = r#"
            global bytes: [u8; 4] = [0, 0, 0, 0]
            global words: [u16; 3] = [0, 0, 0]
            global longs: [u24; 2] = [0, 0]

            fn main() {
                let i: u8 = 0
                while i < 4 {
                    bytes[i] = i + 1
                    i += 1
                }
                test.assert_eq_u8(bytes[0], 1, 1)
                test.assert_eq_u8(bytes[3], 4, 2)

                let j: u8 = 0
                while j < 3 {
                    words[j] = cast<u16>(j) + 0x0100
                    j += 1
                }
                test.assert_eq_u16(words[0], 0x0100, 3)
                test.assert_eq_u16(words[2], 0x0102, 4)

                let k: u8 = 0
                while k < 2 {
                    longs[k] = cast<u24>(k) + 0x010000
                    k += 1
                }
                test.assert_eq_u24(longs[0], 0x010000, 5)
                test.assert_eq_u24(longs[1], 0x010001, 6)

                let p: ptr<u8> = &bytes[i - 2]
                mem.poke8(p, 0x7E)
                test.assert_eq_u8(bytes[2], 0x7E, 7)
                test.pass()
            }
        "#;
        let program = parse_program(Path::new("game.ezra"), source).unwrap();
        let asm = emit_ez80_assembly(&program).unwrap();
        let run = run_assembly_test(&asm, 8_000).unwrap();

        assert!(run.halted, "{asm}");
        assert_eq!(run.result_code, 0, "{asm}");
    }

    #[test]
    fn emits_and_runs_compound_indexed_assignments() {
        let source = r#"
            global bytes: [u8; 4] = [1, 2, 3, 4]
            global words: [u16; 3] = [0x0100, 0x0200, 0x0300]
            global longs: [u24; 2] = [0x010000, 0x020000]

            fn main() {
                bytes[1] += 5
                bytes[2] ^= 0x0F
                test.assert_eq_u8(bytes[1], 7, 1)
                test.assert_eq_u8(bytes[2], 12, 2)

                let i: u8 = 3
                bytes[i] -= 2
                test.assert_eq_u8(bytes[3], 2, 3)

                let j: u8 = 1
                words[j] += 0x0010
                words[j] <<= 1
                test.assert_eq_u16(words[1], 0x0420, 4)

                let k: u8 = 0
                longs[k] += 0x000123
                longs[k] &= 0x01FFFF
                test.assert_eq_u24(longs[0], 0x010123, 5)
                test.pass()
            }
        "#;
        let program = parse_program(Path::new("game.ezra"), source).unwrap();
        let asm = emit_ez80_assembly(&program).unwrap();
        let run = run_assembly_test(&asm, 20_000).unwrap();

        assert!(run.halted, "{asm}");
        assert_eq!(run.result_code, 0, "{asm}");
    }

    #[test]
    fn emits_and_runs_pointer_dereferences() {
        let source = r#"
            global bytes: [u8; 4] = [0, 0, 0, 0]
            global words: [u16; 2] = [0, 0]
            global longs: [u24; 2] = [0, 0]

            fn second_byte() -> ptr<u8> {
                return &bytes[2]
            }

            fn main() {
                let p: ptr<u8> = &bytes[0];
                *p = 0x12;
                *(p + 1) = 0x34;
                test.assert_eq_u8(*p, 0x12, 1);
                test.assert_eq_u8(*(p + 1), 0x34, 2);
                *second_byte() = 0x56;
                test.assert_eq_u8(*second_byte(), 0x56, 7);

                let w: ptr<u16> = &words[1];
                *w = 0x5678;
                test.assert_eq_u16(words[1], 0x5678, 3);
                test.assert_eq_u16(*w, 0x5678, 4);

                let l: ptr<u24> = &longs[1];
                *l = 0x010203;
                test.assert_eq_u24(longs[1], 0x010203, 5);
                test.assert_eq_u24(*l, 0x010203, 6);
                test.pass()
            }
        "#;
        let program = parse_program(Path::new("game.ezra"), source).unwrap();
        let asm = emit_ez80_assembly(&program).unwrap();
        let run = run_assembly_test(&asm, 6_000).unwrap();

        assert!(run.halted, "{asm}");
        assert_eq!(run.result_code, 0, "{asm}");
    }

    #[test]
    fn emits_and_runs_aggregate_pointer_assignments() {
        let source = r#"
            struct Pair {
                left: u8
                right: u16
            }

            global bytes: [u8; 3] = [0, 0, 0]
            global pair: Pair = Pair { left: 0, right: 0 }

            fn main() {
                let byte_ptr: ptr<[u8; 3]> = &bytes;
                *(byte_ptr) = [4, 5, 6]
                test.assert_eq_u8(bytes[0], 4, 1)
                test.assert_eq_u8(bytes[2], 6, 2)

                let pair_ptr: ptr<Pair> = &pair;
                *(pair_ptr) = Pair { left: 7, right: 0x0809 }
                test.assert_eq_u8(pair.left, 7, 3)
                test.assert_eq_u16(pair.right, 0x0809, 4)
                test.pass()
            }
        "#;
        let program = parse_program(Path::new("game.ezra"), source).unwrap();
        let asm = emit_ez80_assembly(&program).unwrap();
        let run = run_assembly_test(&asm, 12_000).unwrap();

        assert!(run.halted, "{asm}");
        assert_eq!(run.result_code, 0, "{asm}");
    }

    #[test]
    fn emits_and_runs_aggregate_pointer_reads() {
        let source = r#"
            struct Pair {
                left: u8
                right: u16
            }

            global bytes: [u8; 3] = [4, 5, 6]
            global pair: Pair = Pair { left: 7, right: 0x0809 }

            fn main() {
                let byte_ptr: ptr<[u8; 3]> = &bytes;
                let local_bytes: [u8; 3] = *(byte_ptr)
                test.assert_eq_u8(local_bytes[0], 4, 1)
                test.assert_eq_u8(local_bytes[2], 6, 2)

                let pair_ptr: ptr<Pair> = &pair;
                let local_pair: Pair = *(pair_ptr)
                test.assert_eq_u8(local_pair.left, 7, 3)
                test.assert_eq_u16(local_pair.right, 0x0809, 4)
                test.pass()
            }
        "#;
        let program = parse_program(Path::new("game.ezra"), source).unwrap();
        let asm = emit_ez80_assembly(&program).unwrap();
        let run = run_assembly_test(&asm, 12_000).unwrap();

        assert!(run.halted, "{asm}");
        assert_eq!(run.result_code, 0, "{asm}");
    }

    #[test]
    fn emits_and_runs_stored_aggregate_copies() {
        let source = r#"
            struct Pair {
                left: u8
                right: u16
            }

            struct Packet {
                bytes: [u8; 3]
                pair: Pair
            }

            global source_bytes: [u8; 3] = [1, 2, 3]
            global target_bytes: [u8; 3] = [0, 0, 0]
            global source_pair: Pair = Pair { left: 4, right: 0x0506 }
            global target_pair: Pair = Pair { left: 0, right: 0 }
            global packet: Packet = Packet {
                bytes: [7, 8, 9],
                pair: Pair { left: 10, right: 0x0B0C }
            }

            fn main() {
                target_bytes = source_bytes
                test.assert_eq_u8(target_bytes[0], 1, 1)
                test.assert_eq_u8(target_bytes[2], 3, 2)

                target_pair = source_pair
                test.assert_eq_u8(target_pair.left, 4, 3)
                test.assert_eq_u16(target_pair.right, 0x0506, 4)

                let local_bytes: [u8; 3] = source_bytes
                test.assert_eq_u8(local_bytes[1], 2, 5)

                let local_pair: Pair = target_pair
                test.assert_eq_u8(local_pair.left, 4, 6)
                test.assert_eq_u16(local_pair.right, 0x0506, 7)

                packet.bytes = target_bytes
                test.assert_eq_u8(packet.bytes[0], 1, 8)
                test.assert_eq_u8(packet.bytes[2], 3, 9)

                packet.pair = source_pair
                test.assert_eq_u8(packet.pair.left, 4, 10)
                test.assert_eq_u16(packet.pair.right, 0x0506, 11)
                test.pass()
            }
        "#;
        let program = parse_program(Path::new("game.ezra"), source).unwrap();
        let asm = emit_ez80_assembly(&program).unwrap();
        let run = run_assembly_test(&asm, 16_000).unwrap();

        assert!(run.halted, "{asm}");
        assert_eq!(run.result_code, 0, "{asm}");
    }

    #[test]
    fn emits_and_runs_copied_aggregate_global_initializers() {
        let source = r#"
            struct Pair {
                left: u8
                right: u16
            }

            struct Packet {
                bytes: [u8; 3]
                pair: Pair
            }

            global source_bytes: [u8; 3] = [1, 2, 3]
            global copied_bytes: [u8; 3] = source_bytes
            global source_pair: Pair = Pair { left: 4, right: 0x0506 }
            global copied_pair: Pair = source_pair
            global source_packet: Packet = Packet {
                bytes: [7, 8, 9],
                pair: Pair { left: 10, right: 0x0B0C }
            }
            global copied_packet: Packet = source_packet

            fn main() {
                test.assert_eq_u8(copied_bytes[0], 1, 1)
                test.assert_eq_u8(copied_bytes[2], 3, 2)
                test.assert_eq_u8(copied_pair.left, 4, 3)
                test.assert_eq_u16(copied_pair.right, 0x0506, 4)
                test.assert_eq_u8(copied_packet.bytes[0], 7, 5)
                test.assert_eq_u8(copied_packet.bytes[2], 9, 6)
                test.assert_eq_u8(copied_packet.pair.left, 10, 7)
                test.assert_eq_u16(copied_packet.pair.right, 0x0B0C, 8)
                test.pass()
            }
        "#;
        let program = parse_program(Path::new("game.ezra"), source).unwrap();
        let asm = emit_ez80_assembly(&program).unwrap();
        let run = run_assembly_test(&asm, 16_000).unwrap();

        assert!(run.halted, "{asm}");
        assert_eq!(run.result_code, 0, "{asm}");
    }

    #[test]
    fn emits_and_runs_overlapping_stored_aggregate_copies() {
        let source = r#"
            struct Rows {
                first: [u8; 3]
                second: [u8; 3]
            }

            global rows: Rows = Rows {
                first: [1, 2, 3],
                second: [4, 5, 6]
            }
            global grid: [[u8; 3]; 2] = [
                [7, 8, 9],
                [10, 11, 12]
            ]

            fn main() {
                rows.second = rows.first
                test.assert_eq_u8(rows.second[0], 1, 1)
                test.assert_eq_u8(rows.second[2], 3, 2)

                rows.first = rows.first
                test.assert_eq_u8(rows.first[0], 1, 3)
                test.assert_eq_u8(rows.first[2], 3, 4)

                grid[1] = grid[0]
                test.assert_eq_u8(grid[1][0], 7, 5)
                test.assert_eq_u8(grid[1][2], 9, 6)
                test.pass()
            }
        "#;
        let program = parse_program(Path::new("game.ezra"), source).unwrap();
        let asm = emit_ez80_assembly(&program).unwrap();
        let run = run_assembly_test(&asm, 12_000).unwrap();

        assert!(run.halted, "{asm}");
        assert_eq!(run.result_code, 0, "{asm}");
    }

    #[test]
    fn emits_and_runs_casted_indirect_assignments() {
        let source = r#"
            global bytes: [u8; 2] = [0, 0]
            global word: u16 = 0

            fn main() {
                let wide: u16 = 0x12FE
                bytes[1] = cast<u8>(wide)

                let p: ptr<u16> = &word;
                let small: u8 = 0x34;
                *p = cast<u16>(small);

                test.assert_eq_u8(bytes[1], 0xFE, 1)
                test.assert_eq_u16(word, 0x0034, 2)
                test.pass()
            }
        "#;
        let program = parse_program(Path::new("game.ezra"), source).unwrap();
        let asm = emit_ez80_assembly(&program).unwrap();
        let run = run_assembly_test(&asm, 6_000).unwrap();

        assert!(run.halted, "{asm}");
        assert_eq!(run.result_code, 0, "{asm}");
    }

    #[test]
    fn emits_and_runs_compound_pointer_dereference_assignments() {
        let source = r#"
            global bytes: [u8; 4] = [10, 20, 30, 40]
            global words: [u16; 2] = [0x0100, 0x0200]
            global longs: [u24; 2] = [0x010000, 0x020000]

            fn main() {
                let b: ptr<u8> = &bytes[1];
                *b += 7;
                *(b + 1) &= 0x1F;
                test.assert_eq_u8(bytes[1], 27, 1)
                test.assert_eq_u8(bytes[2], 30, 2)

                let w: ptr<u16> = &words[0];
                *w += 0x0023;
                *(w + 1) >>= 1;
                test.assert_eq_u16(words[0], 0x0123, 3)
                test.assert_eq_u16(words[1], 0x0100, 4)

                let l: ptr<u24> = &longs[0];
                *l += 0x000123;
                *(l + 1) ^= 0x0000FF;
                test.assert_eq_u24(longs[0], 0x010123, 5)
                test.assert_eq_u24(longs[1], 0x0200FF, 6)
                test.pass()
            }
        "#;
        let program = parse_program(Path::new("game.ezra"), source).unwrap();
        let asm = emit_ez80_assembly(&program).unwrap();
        let run = run_assembly_test(&asm, 20_000).unwrap();

        assert!(run.halted, "{asm}");
        assert_eq!(run.result_code, 0, "{asm}");
    }

    #[test]
    fn emits_and_runs_scaled_pointer_arithmetic() {
        let source = r#"
            global bytes: [u8; 4] = [0, 0, 0, 0]
            global words: [u16; 3] = [0, 0, 0]
            global longs: [u24; 3] = [0, 0, 0]

            fn main() {
                let b: ptr<u8> = &bytes[1];
                *(b + 2) = 0x7A;
                test.assert_eq_u8(bytes[3], 0x7A, 1);
                let back_byte: i8 = -1;
                *(b + back_byte) = 0x33;
                test.assert_eq_u8(bytes[0], 0x33, 5);
                test.assert_eq_u24(cast<u24>(b + back_byte), cast<u24>(&bytes[0]), 7);

                let w: ptr<u16> = &words[0];
                *(w + 2) = 0x4567;
                test.assert_eq_u16(words[2], 0x4567, 2);
                *(w + 2 - 1) = 0x1234;
                test.assert_eq_u16(words[1], 0x1234, 3);
                let back_word: i8 = -1;
                *(w + 2 + back_word) = 0x2345;
                test.assert_eq_u16(words[1], 0x2345, 6);
                test.assert_eq_u24(cast<u24>(w + 2 + back_word), cast<u24>(&words[1]), 8);

                let l: ptr<u24> = &longs[0];
                *(l + 2) = 0x010203;
                test.assert_eq_u24(longs[2], 0x010203, 4);
                test.pass()
            }
        "#;
        let program = parse_program(Path::new("game.ezra"), source).unwrap();
        let asm = emit_ez80_assembly(&program).unwrap();
        let run = run_assembly_test(&asm, 8_000).unwrap();

        assert!(run.halted, "{asm}");
        assert_eq!(run.result_code, 0, "{asm}");
    }

    #[test]
    fn emits_and_runs_struct_pointer_arithmetic_scale() {
        let source = r#"
            struct Cell {
                value: u24
                flags: u8
            }

            struct BigCell {
                padding: [u8; 300]
                value: u8
            }

            global cell: Cell = Cell { value: 0x010203, flags: 0x44 }
            global big: BigCell = BigCell { value: 0x99 }

            fn main() {
                let p: ptr<Cell> = &cell
                let q: ptr<Cell> = p + 2
                let r: ptr<Cell> = q - 1
                test.assert_eq_u24(cast<u24>(q), cast<u24>(p) + 8, 1)
                test.assert_eq_u24(cast<u24>(r), cast<u24>(p) + 4, 2)

                let big_p: ptr<BigCell> = &big
                let big_q: ptr<BigCell> = big_p + 1
                let big_r: ptr<BigCell> = big_q - 1
                test.assert_eq_u24(cast<u24>(big_q), cast<u24>(big_p) + 301, 3)
                test.assert_eq_u24(cast<u24>(big_r), cast<u24>(big_p), 4)
                test.pass()
            }
        "#;
        let program = parse_program(Path::new("game.ezra"), source).unwrap();
        let asm = emit_ez80_assembly(&program).unwrap();
        let run = run_assembly_test(&asm, 8_000).unwrap();

        assert!(run.halted, "{asm}");
        assert_eq!(run.result_code, 0, "{asm}");
    }

    #[test]
    fn emits_and_runs_scalar_address_of() {
        let source = r#"
            global byte_value: u8 = 0
            global word_value: u16 = 0
            global long_value: u24 = 0
            global word_ptr: ptr<u16> = &word_value

            fn write_byte(ptr: ptr<u8>, value: u8) {
                *ptr = value
            }

            fn main() {
                let byte_ptr: ptr<u8> = &byte_value;
                write_byte(byte_ptr, 0x5A);
                test.assert_eq_u8(byte_value, 0x5A, 1);
                test.assert_eq_u8(*byte_ptr, 0x5A, 2);

                *word_ptr = 0x1234;
                test.assert_eq_u16(word_value, 0x1234, 3);

                let long_ptr: ptr<u24> = &long_value;
                *long_ptr = 0x010203;
                test.assert_eq_u24(long_value, 0x010203, 4);
                test.pass()
            }
        "#;
        let program = parse_program(Path::new("game.ezra"), source).unwrap();
        let asm = emit_ez80_assembly(&program).unwrap();
        let run = run_assembly_test(&asm, 8_000).unwrap();

        assert!(run.halted, "{asm}");
        assert_eq!(run.result_code, 0, "{asm}");
    }

    #[test]
    fn emits_and_runs_basic_struct_fields() {
        let source = r#"
            struct Entity {
                x: u24
                y: u24
                sprite: u8
                flags: u8
            }

            global player: Entity = Entity {
                x: 0x010000,
                sprite: 3,
            }

            fn main() {
                test.assert_eq_u24(player.x, 0x010000, 1);
                test.assert_eq_u24(player.y, 0, 2);
                test.assert_eq_u8(player.sprite, 3, 3);
                test.assert_eq_u8(player.flags, 0, 4);

                player.y = player.x + 0x000123;
                player.sprite += 4;
                player.flags = 0x80;

                let local: Entity = Entity {
                    x: player.y,
                    y: 0x020000,
                    sprite: player.sprite,
                    flags: player.flags,
                };

                test.assert_eq_u24(player.y, 0x010123, 5);
                test.assert_eq_u8(player.sprite, 7, 6);
                test.assert_eq_u24(local.x, 0x010123, 7);
                test.assert_eq_u24(local.y, 0x020000, 8);
                test.assert_eq_u8(local.sprite, 7, 9);
                test.assert_eq_u8(local.flags, 0x80, 10);
                test.pass()
            }
        "#;
        let program = parse_program(Path::new("game.ezra"), source).unwrap();
        let asm = emit_ez80_assembly(&program).unwrap();
        let run = run_assembly_test(&asm, 12_000).unwrap();

        assert!(run.halted, "{asm}");
        assert_eq!(run.result_code, 0, "{asm}");
    }

    #[test]
    fn emits_and_runs_struct_field_addresses() {
        let source = r#"
            struct Entity {
                x: u24
                sprite: u8
                hp: u16
            }

            global player: Entity = Entity {
                x: 0,
                sprite: 1,
                hp: 100,
            }

            fn write_u24(ptr: ptr<u24>, value: u24) {
                *ptr = value
            }

            fn main() {
                let x_ptr: ptr<u24> = &player.x;
                write_u24(x_ptr, 0x010203);
                test.assert_eq_u24(player.x, 0x010203, 1);
                test.assert_eq_u24(*x_ptr, 0x010203, 2);

                let sprite_ptr: ptr<u8> = &player.sprite;
                *sprite_ptr = 7;
                test.assert_eq_u8(player.sprite, 7, 3);

                let hp_ptr: ptr<u16> = &player.hp;
                *hp_ptr = *hp_ptr + 23;
                test.assert_eq_u16(player.hp, 123, 4);
                test.pass()
            }
        "#;
        let program = parse_program(Path::new("game.ezra"), source).unwrap();
        let asm = emit_ez80_assembly(&program).unwrap();
        let run = run_assembly_test(&asm, 8_000).unwrap();

        assert!(run.halted, "{asm}");
        assert_eq!(run.result_code, 0, "{asm}");
    }

    #[test]
    fn emits_and_runs_inline_embedded_bytes() {
        let source = r#"
            embed palette: bytes = bytes [0x11, 0x22, 0x33] section .rodata align 16
            embed title_text: bytes = text("HI")
            embed title_cstr: bytes = cstr("OK")
            embed blank: bytes = repeat(0x7E, 4)

            global palette_ptr: ptr<u8> = palette.ptr

            fn main() {
                test.assert_eq_u24(palette.len, 3, 1);
                test.assert_eq_u8(*palette_ptr, 0x11, 2);
                test.assert_eq_u8(*(palette.ptr + 1), 0x22, 3);
                test.assert_eq_u8(*(palette.end - 1), 0x33, 4);
                test.assert_eq_u24(cast<ptr24>(palette.ptr), EZRA_RODATA_BASE, 14);

                test.assert_eq_u24(title_text.len, 2, 5);
                test.assert_eq_u8(*(title_text.ptr + 0), 'H', 6);
                test.assert_eq_u8(*(title_text.ptr + 1), 'I', 7);
                test.assert_eq_u24(cast<ptr24>(title_text.ptr), EZRA_ASSET_BASE, 15);

                test.assert_eq_u24(title_cstr.len, 3, 8);
                test.assert_eq_u8(*(title_cstr.ptr + 0), 'O', 9);
                test.assert_eq_u8(*(title_cstr.ptr + 1), 'K', 10);
                test.assert_eq_u8(*(title_cstr.ptr + 2), 0, 11);
                test.assert_eq_u24(cast<ptr24>(title_cstr.ptr), EZRA_ASSET_BASE + 2, 16);

                test.assert_eq_u24(blank.len, 4, 12);
                test.assert_eq_u8(*(blank.ptr + 3), 0x7E, 13);
                test.assert_eq_u24(cast<ptr24>(blank.ptr), EZRA_ASSET_BASE + 5, 17);
                test.pass()
            }
        "#;
        let program = parse_program(Path::new("game.ezra"), source).unwrap();
        let asm = emit_ez80_assembly(&program).unwrap();
        let run = run_assembly_test(&asm, 12_000).unwrap();

        assert!(run.halted, "{asm}");
        assert_eq!(run.result_code, 0, "{asm}");
    }

    #[test]
    fn emits_and_runs_custom_section_embedded_bytes_at_section_base() {
        let source = r#"
            embed banked: bytes = bytes [0xA1, 0xA2] section .bank1 align 256

            fn main() {
                test.assert_eq_u24(cast<ptr24>(banked.ptr), 0x120000, 1)
                test.assert_eq_u8(*(banked.ptr + 0), 0xA1, 2)
                test.assert_eq_u8(*(banked.ptr + 1), 0xA2, 3)
                test.pass()
            }
        "#;
        let program = parse_program(Path::new("game.ezra"), source).unwrap();
        let asm = emit_ez80_assembly_with_options(
            &program,
            AssemblyOptions {
                section_bases: vec![(".bank1".to_owned(), Address24::new(0x12_0000))],
                ..AssemblyOptions::default()
            },
        )
        .unwrap();
        let run = run_assembly_test(&asm, 12_000).unwrap();

        assert!(run.halted, "{asm}");
        assert_eq!(run.result_code, 0, "{asm}");
    }

    #[test]
    fn rejects_writes_to_read_only_embedded_bytes() {
        let cases = [
            (
                r#"
                embed sprite: bytes = bytes [0x11, 0x22]

                fn main() {
                    *(sprite.ptr) = 0x33
                    test.pass()
                }
                "#,
                "embedded object `sprite` is read-only",
            ),
            (
                r#"
                embed sprite: bytes = bytes [0x11, 0x22]

                fn main() {
                    *(sprite.ptr + 1) = 0x33
                    test.pass()
                }
                "#,
                "embedded object `sprite` is read-only",
            ),
            (
                r#"
                embed sprite: bytes = bytes [0x11, 0x22]

                fn main() {
                    let p: ptr<u8> = sprite.ptr;
                    *(p) = 0x33
                    test.pass()
                }
                "#,
                "embedded object `sprite` is read-only",
            ),
            (
                r#"
                embed sprite: bytes = bytes [0x11, 0x22]

                fn main() {
                    let p: ptr<u16> = cast<ptr<u16>>(sprite.ptr + 1);
                    *(p) = 0x3344
                    test.pass()
                }
                "#,
                "embedded object `sprite` is read-only",
            ),
            (
                r#"
                embed sprite: bytes = bytes [0x11, 0x22, 0x33, 0x44]

                fn main() {
                    let p: ptr<u16> = cast<ptr<u16>>(sprite.ptr);
                    let q: ptr<u16> = p + 1;
                    *(q) = 0x5566
                    test.pass()
                }
                "#,
                "embedded object `sprite` is read-only",
            ),
            (
                r#"
                embed sprite: bytes = bytes [0x11, 0x22]
                global sprite_alias: ptr<u8> = sprite.ptr

                fn main() {
                    *(sprite_alias + 1) = 0x33
                    test.pass()
                }
                "#,
                "embedded object `sprite` is read-only",
            ),
            (
                r#"
                embed sprite: bytes = bytes [0x11, 0x22]

                fn main() {
                    let offset: u8 = 1
                    let p: ptr<u8> = sprite.ptr;
                    let q: ptr<u8> = p + offset;
                    *(q) = 0x33
                    test.pass()
                }
                "#,
                "embedded object `sprite` is read-only",
            ),
        ];

        for (source, expected) in cases {
            let program = parse_program(Path::new("game.ezra"), source).unwrap();
            let error = emit_ez80_assembly(&program).unwrap_err();

            assert_eq!(error.message, expected);
        }
    }

    #[test]
    fn allows_reassigned_embedded_pointer_alias_to_mutable_memory() {
        let source = r#"
            embed sprite: bytes = bytes [0x11, 0x22]

            fn main() {
                let p: ptr<u8> = sprite.ptr;
                p = cast<ptr<u8>>(0x040120);
                *(p) = 0x33
                test.assert_eq_u8(*(cast<ptr<u8>>(0x040120)), 0x33, 1)
                test.pass()
            }
        "#;
        let program = parse_program(Path::new("game.ezra"), source).unwrap();
        let asm = emit_ez80_assembly(&program).unwrap();
        let run = run_assembly_test(&asm, 2_000).unwrap();

        assert!(run.halted, "{asm}");
        assert_eq!(run.result_code, 0, "{asm}");
    }

    #[test]
    fn emits_and_runs_file_embedded_bytes() {
        let root = std::env::temp_dir().join(format!(
            "ezra_file_embed_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let assets = root.join("assets");
        std::fs::create_dir_all(&assets).unwrap();
        std::fs::write(assets.join("blob.bin"), [0xA5, 0x5A, 0xC3]).unwrap();
        let source_path = root.join("game.ezra");
        let source = r#"
            embed blob: bytes = file("assets/blob.bin") align 4

            fn main() {
                test.assert_eq_u24(blob.len, 3, 1);
                test.assert_eq_u8(*(blob.ptr + 0), 0xA5, 2);
                test.assert_eq_u8(*(blob.ptr + 1), 0x5A, 3);
                test.assert_eq_u8(*(blob.end - 1), 0xC3, 4);
                test.pass()
            }
        "#;
        let program = parse_program(&source_path, source).unwrap();
        let asm = emit_ez80_assembly(&program).unwrap();
        let run = run_assembly_test(&asm, 12_000).unwrap();

        let _ = std::fs::remove_dir_all(&root);
        assert!(run.halted, "{asm}");
        assert_eq!(run.result_code, 0, "{asm}");
    }

    #[test]
    fn reports_missing_embedded_files() {
        let root = std::env::temp_dir().join(format!(
            "ezra_missing_file_embed_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let source_path = root.join("game.ezra");
        let source = r#"
            embed blob: bytes = file("assets/missing.bin")
            fn main() { test.pass() }
        "#;
        let program = parse_program(&source_path, source).unwrap();
        let error = emit_ez80_assembly(&program).unwrap_err();

        assert_eq!(
            error.message,
            format!(
                "embedded file `{}` not found",
                root.join("assets/missing.bin").display()
            )
        );
    }

    #[test]
    fn emits_and_runs_zero_terminated_string_literals() {
        let source = r#"
            global title: ptr<u8> = "EZ"

            fn same(a: ptr<u8>, b: ptr<u8>) -> bool {
                return a == b
            }

            fn main() {
                let text: ptr<u8> = "OK";
                test.assert_eq_u8(*text, 'O', 1);
                test.assert_eq_u8(*(text + 1), 'K', 2);
                test.assert_eq_u8(*(text + 2), 0, 3);
                test.assert_eq_u8(*title, 'E', 4);
                test.assert_eq_u8(*(title + 1), 'Z', 5);
                test.assert_eq_u8(*(title + 2), 0, 6);
                test.assert_eq_u8(same("OK", "OK"), true, 7);
                test.assert_eq_u24(cast<ptr24>(title), EZRA_RODATA_BASE, 8);
                test.assert_eq_u24(cast<ptr24>(text), EZRA_RODATA_BASE + 3, 9);
                test.pass()
            }
        "#;
        let program = parse_program(Path::new("game.ezra"), source).unwrap();
        let asm = emit_ez80_assembly(&program).unwrap();
        let run = run_assembly_test(&asm, 10_000).unwrap();

        assert!(run.halted, "{asm}");
        assert_eq!(run.result_code, 0, "{asm}");
    }

    #[test]
    fn emits_and_runs_character_literal_escapes() {
        let source = r#"
            fn main() {
                test.assert_eq_u8('\n', 10, 1)
                test.assert_eq_u8('\0', 0, 2)
                test.assert_eq_u8('\t', 9, 3)
                test.assert_eq_u8('\'', 39, 4)
                test.assert_eq_u8('\\', 92, 5)
                test.pass()
            }
        "#;
        let program = parse_program(Path::new("game.ezra"), source).unwrap();
        let asm = emit_ez80_assembly(&program).unwrap();
        let run = run_assembly_test(&asm, 10_000).unwrap();

        assert!(run.halted, "{asm}");
        assert_eq!(run.result_code, 0, "{asm}");
    }

    #[test]
    fn emits_and_runs_const_string_literal_pointers() {
        let source = r#"
            const TITLE: ptr<u8> = "EZ"
            global title_copy: ptr<u8> = TITLE

            fn same(a: ptr<u8>, b: ptr<u8>) -> bool {
                return a == b
            }

            fn main() {
                test.assert_eq_u8(*TITLE, 'E', 1)
                test.assert_eq_u8(*(TITLE + 1), 'Z', 2)
                test.assert_eq_u8(*(TITLE + 2), 0, 3)
                test.assert_eq_u8(*title_copy, 'E', 4)
                test.assert_eq_u8(same(TITLE, "EZ"), true, 5)
                test.pass()
            }
        "#;
        let program = parse_program(Path::new("game.ezra"), source).unwrap();
        let asm = emit_ez80_assembly(&program).unwrap();
        let run = run_assembly_test(&asm, 10_000).unwrap();

        assert!(run.halted, "{asm}");
        assert_eq!(run.result_code, 0, "{asm}");
    }

    #[test]
    fn rejects_writes_to_read_only_string_literals() {
        let cases = [
            r#"
                fn main() {
                    *("OK") = 'N'
                    test.pass()
                }
            "#,
            r#"
                fn main() {
                    let text: ptr<u8> = "OK";
                    *(text + 1) = 'X'
                    test.pass()
                }
            "#,
            r#"
                const TITLE: ptr<u8> = "EZ";

                fn main() {
                    *(TITLE) = 'N'
                    test.pass()
                }
            "#,
            r#"
                global title_copy: ptr<u8> = "EZ";

                fn main() {
                    *(title_copy + 1) = 'X'
                    test.pass()
                }
            "#,
        ];

        for source in cases {
            let program = parse_program(Path::new("game.ezra"), source).unwrap();
            let error = emit_ez80_assembly(&program).unwrap_err();

            assert_eq!(error.message, "string literal is read-only");
        }
    }

    #[test]
    fn allows_reassigned_global_readonly_pointer_aliases_to_mutable_memory() {
        let source = r#"
            embed sprite: bytes = bytes [0x11, 0x22]
            global p: ptr<u8> = sprite.ptr
            global text: ptr<u8> = "OK"

            fn main() {
                p = cast<ptr<u8>>(0x040120);
                text = cast<ptr<u8>>(0x040121);
                *(p) = 0x33;
                *(text) = 0x44
                test.assert_eq_u8(*(cast<ptr<u8>>(0x040120)), 0x33, 1)
                test.assert_eq_u8(*(cast<ptr<u8>>(0x040121)), 0x44, 2)
                test.pass()
            }
        "#;
        let program = parse_program(Path::new("game.ezra"), source).unwrap();
        let asm = emit_ez80_assembly(&program).unwrap();
        let run = run_assembly_test(&asm, 4_000).unwrap();

        assert!(run.halted, "{asm}");
        assert_eq!(run.result_code, 0, "{asm}");
    }

    #[test]
    fn emits_and_runs_debug_str_builtin() {
        let source = r#"
            global title: ptr<u8> = "EZ"

            fn main() {
                debug.str("OK")
                debug.char(' ')
                ezra.debug.str(title)
                test.pass()
            }
        "#;
        let program = parse_program(Path::new("game.ezra"), source).unwrap();
        let asm = emit_ez80_assembly(&program).unwrap();
        let run = run_assembly_test(&asm, 4_000).unwrap();

        assert!(asm.contains("out0 (0Ch), a"), "{asm}");
        assert!(run.halted, "{asm}");
        assert_eq!(run.result_code, 0, "{asm}");
        assert_eq!(run.debug_output, b"OK EZ", "{asm}");
    }

    #[test]
    fn emits_and_runs_debug_hex_builtins() {
        let source = r#"
            fn main() {
                let byte: u8 = 0xAF;
                let word: u16 = 0x1234;
                let addr: u24 = 0x00BEEF;
                debug.hex_u8(byte)
                debug.char(' ')
                ezra.debug.hex_u16(word)
                debug.char(' ')
                debug.hex_u24(addr)
                test.pass()
            }
        "#;
        let program = parse_program(Path::new("game.ezra"), source).unwrap();
        let asm = emit_ez80_assembly(&program).unwrap();
        let run = run_assembly_test(&asm, 8_000).unwrap();

        assert!(asm.contains("srl a"), "{asm}");
        assert!(run.halted, "{asm}");
        assert_eq!(run.result_code, 0, "{asm}");
        assert_eq!(run.debug_output, b"AF 1234 00BEEF", "{asm}");
    }

    #[test]
    fn emits_and_runs_generic_mmio_peek_poke_examples() {
        let source = r#"
            volatile mmio SCRATCH: ptr<u8> = 0x040120
            volatile mmio TI_LCD_BUFFER: ptr<u8> = 0x080000
            volatile mmio AGON_VDP_BUFFER: ptr<u8> = 0x0C0000

            fn ti_write(value: u8) {
                *(TI_LCD_BUFFER) = value;
            }

            fn agon_write(value: u8) {
                *(AGON_VDP_BUFFER) = value;
            }

            fn main() {
                let ptr: ptr<u8> = cast<ptr<u8>>(0x040121);
                *(SCRATCH) = 0x5A;
                *ptr = *SCRATCH + 1;
                ti_write(*ptr);
                agon_write(0xC3);
                test.assert_eq_u8(*SCRATCH, 0x5A, 1);
                test.assert_eq_u8(*ptr, 0x5B, 2);
                test.assert_eq_u8(*TI_LCD_BUFFER, 0x5B, 3);
                test.assert_eq_u8(*AGON_VDP_BUFFER, 0xC3, 4);
                test.pass()
            }
        "#;
        let program = parse_program(Path::new("game.ezra"), source).unwrap();
        let asm = emit_ez80_assembly(&program).unwrap();
        let run = run_assembly_test(&asm, 4_000).unwrap();

        assert!(asm.contains("ld a, (hl)"), "{asm}");
        assert!(asm.contains("ld (hl), a"), "{asm}");
        assert!(run.halted, "{asm}");
        assert_eq!(run.result_code, 0, "{asm}");
    }

    #[test]
    fn emits_full_width_discarded_volatile_mmio_loads() {
        let source = r#"
            volatile mmio STATUS16: ptr<u16> = 0x040180
            volatile mmio STATUS24: ptr<u24> = 0x040190

            fn main() {
                *STATUS16;
                *STATUS24;
                test.pass()
            }
        "#;
        let program = parse_program(Path::new("game.ezra"), source).unwrap();
        let asm = emit_ez80_assembly_with_debug_comments(&program, true).unwrap();
        let run = run_assembly_test(&asm, 4_000).unwrap();
        let status16 = asm
            .split("; source: *STATUS16")
            .nth(1)
            .and_then(|tail| tail.split("; source: *STATUS24").next())
            .unwrap();
        let status24 = asm
            .split("; source: *STATUS24")
            .nth(1)
            .and_then(|tail| tail.split("; source: test.pass()").next())
            .unwrap();

        assert!(status16.contains("    inc hl"), "{asm}");
        assert_eq!(status24.matches("    inc hl").count(), 2, "{asm}");
        assert!(run.halted, "{asm}");
        assert_eq!(run.result_code, 0, "{asm}");
    }

    #[test]
    fn emits_and_runs_wide_comparisons() {
        let source = r#"
            fn main() {
                let a: u16 = 0x0100
                let b: u16 = 0x0200
                test.assert_eq_u8(a < b, 1, 1)
                test.assert_eq_u8(b > a, 1, 2)
                test.assert_eq_u8(a >= b, 0, 3)
                test.assert_eq_u8(a != b, 1, 4)

                let c: u24 = 0x010000
                let d: u24 = 0x010000
                let e: u24 = 0x020000
                test.assert_eq_u8(c == d, 1, 5)
                test.assert_eq_u8(c <= d, 1, 6)
                test.assert_eq_u8(e <= c, 0, 7)

                let count: u8 = 0
                while c < e {
                    c += 0x008000
                    count += 1
                }
                if c >= e {
                    count += 1
                }
                test.assert_eq_u8(count, 3, 8)
                test.pass()
            }
        "#;
        let program = parse_program(Path::new("game.ezra"), source).unwrap();
        let asm = emit_ez80_assembly(&program).unwrap();
        let run = run_assembly_test(&asm, 4_000).unwrap();

        assert!(run.halted, "{asm}");
        assert_eq!(run.result_code, 0, "{asm}");
    }

    #[test]
    fn emits_and_runs_comparisons_with_fitting_untyped_literals() {
        let source = r#"
            const SMALL_NEG: i8 = -2
            const CONST_SIGNED_LT: bool = SMALL_NEG < -1
            const CONST_UNSIGNED_EQ: bool = 5 == 5

            fn main() {
                let a: i8 = -2
                test.assert_eq_u8(a < -1, 1, 1)
                test.assert_eq_u8(a >= -2, 1, 2)

                let b: i16 = -300
                test.assert_eq_u8(b < -1, 1, 3)
                test.assert_eq_u8(-301 <= b, 1, 4)

                let c: i24 = -0x012345
                test.assert_eq_u8(c < -1, 1, 5)
                test.assert_eq_u8(-0x012345 == c, 1, 6)

                let d: u8 = 7
                test.assert_eq_u8(d == 7, 1, 7)
                test.assert_eq_u8(7 <= d, 1, 8)

                test.assert_eq_u8(CONST_SIGNED_LT, 1, 9)
                test.assert_eq_u8(CONST_UNSIGNED_EQ, 1, 10)
                test.pass()
            }
        "#;
        let program = parse_program(Path::new("game.ezra"), source).unwrap();
        let asm = emit_ez80_assembly(&program).unwrap();
        let run = run_assembly_test(&asm, 8_000).unwrap();

        assert!(run.halted, "{asm}");
        assert_eq!(run.result_code, 0, "{asm}");
    }

    #[test]
    fn emits_and_runs_signed_comparisons() {
        let source = r#"
            alias subpx = i24

            fn main() {
                let a: i8 = -1
                let b: i8 = 1
                let c: i8 = -2
                test.assert_eq_u8(a < b, 1, 1)
                test.assert_eq_u8(b > a, 1, 2)
                test.assert_eq_u8(c < a, 1, 3)
                test.assert_eq_u8(a >= c, 1, 4)

                let d: i16 = -300
                let e: i16 = 7
                let f: i16 = -301
                test.assert_eq_u8(d < e, 1, 5)
                test.assert_eq_u8(e <= d, 0, 6)
                test.assert_eq_u8(f <= d, 1, 7)
                test.assert_eq_u8(d != f, 1, 8)

                let g: subpx = -0x010000
                let h: subpx = 0x000100
                let i: subpx = -0x020000
                test.assert_eq_u8(g < h, 1, 9)
                test.assert_eq_u8(h >= g, 1, 10)
                test.assert_eq_u8(i < g, 1, 11)
                test.assert_eq_u8(g == g, 1, 12)

                let min: subpx = -0x800000
                let max: subpx = 0x7FFFFF
                test.assert_eq_u8(min < max, 1, 13)
                test.assert_eq_u8(max > min, 1, 14)
                test.pass()
            }
        "#;
        let program = parse_program(Path::new("game.ezra"), source).unwrap();
        let asm = emit_ez80_assembly(&program).unwrap();
        let run = run_assembly_test(&asm, 6_000).unwrap();

        assert!(run.halted, "{asm}");
        assert_eq!(run.result_code, 0, "{asm}");
    }
}
