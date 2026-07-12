use std::{
    collections::{HashMap, HashSet},
    path::Path,
};

use crate::{
    ast::{
        AccessPath, AccessSegment, BinaryOp, Declaration, EmbedSource, Expr, FieldDecl, Function,
        Program, Type, UnaryOp,
    },
    diagnostic::Diagnostic,
    target::Address24,
};

use super::{
    AssemblyOptions, access_path_summary, addr24, alloc_from_cursor, assigned_names_in_program,
    collect_const_address_roots, collect_const_dependency_names, const_access_name,
    const_shl_or_zero, const_shr_or_zero, declaration_name, expr_is_untyped_literal,
    find_const_declaration, function_declaration_name, function_label, has_attr, int_value_type,
    is_comparison, is_inlinable_function, is_raw_address_type, module_alias_original_name,
    ptr_u8_type, read_embed_file, reserved_function_label, scalar_var, sdk_constant_types,
    sdk_constants, sdk_ports, section_cursor, trunc_div_or_zero, trunc_mod_or_zero, type_display,
    type_is_bool, type_is_signed, validate_comparison_types, validate_integer_unary_operand_type,
    validate_shift_count_integer_type, validate_shift_operand_type,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct Variable {
    pub(super) addr: u32,
    pub(super) size: u32,
    pub(super) element_size: Option<u32>,
    pub(super) len: Option<u32>,
}

impl Variable {
    pub(super) fn width(self) -> Result<ValueWidth, Diagnostic> {
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
pub(super) enum ValueWidth {
    U8,
    U16,
    U24,
}

impl ValueWidth {
    pub(super) fn bytes(self) -> u8 {
        match self {
            Self::U8 => 1,
            Self::U16 => 2,
            Self::U24 => 3,
        }
    }

    pub(super) fn max(self, other: Self) -> Self {
        match (self, other) {
            (Self::U24, _) | (_, Self::U24) => Self::U24,
            (Self::U16, _) | (_, Self::U16) => Self::U16,
            (Self::U8, Self::U8) => Self::U8,
        }
    }
}

#[derive(Clone)]
pub(super) struct Symbols {
    pub(super) constants: HashMap<String, i64>,
    pub(super) constant_types: HashMap<String, Type>,
    evaluating_constants: HashSet<String>,
    aliases: HashMap<String, Type>,
    pub(super) structs: HashMap<String, StructLayout>,
    pub(super) embeds: HashMap<String, EmbedObject>,
    pub(super) string_literals: HashMap<String, Variable>,
    pub(super) ports: HashMap<String, u8>,
    pub(super) globals: HashMap<String, Variable>,
    pub(super) global_types: HashMap<String, Type>,
    pub(super) readonly_global_pointer_aliases: HashMap<String, u32>,
    pub(super) functions: HashMap<String, FunctionSig>,
    pub(super) inline_functions: HashMap<String, Function>,
    next_addr: u32,
    asset_next_addr: u32,
    rodata_next_addr: u32,
    section_next_addrs: Vec<(String, u32)>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct EmbedObject {
    pub(super) variable: Variable,
    pub(super) bytes: Vec<u8>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct StructLayout {
    pub(super) size: u32,
    pub(super) fields: HashMap<String, StructField>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct StructField {
    pub(super) offset: u32,
    pub(super) ty: Type,
    pub(super) size: u32,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct FunctionSig {
    pub(super) arity: usize,
    pub(super) params: Vec<ValueWidth>,
    pub(super) param_types: Vec<Type>,
    pub(super) arg_slots: Vec<Variable>,
    pub(super) uses_arg_slots: bool,
    pub(super) stack_arg_offsets: Vec<Option<u8>>,
    pub(super) stack_arg_bytes: u8,
    pub(super) return_width: ValueWidth,
    pub(super) return_type: Option<Type>,
    pub(super) is_interrupt: bool,
}

impl Symbols {
    pub(super) fn from_program(
        program: &Program,
        options: AssemblyOptions,
    ) -> Result<Self, Diagnostic> {
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

        let mut function_labels = HashMap::new();
        for declaration in &program.declarations {
            let Some(name) = function_declaration_name(declaration) else {
                continue;
            };
            let label = function_label(name);
            if reserved_function_label(&label) {
                return Err(Diagnostic::new(format!(
                    "function `{name}` emits reserved assembly label `{label}`"
                )));
            }
            if let Some(existing) = function_labels.insert(label.clone(), name.to_owned()) {
                return Err(Diagnostic::new(format!(
                    "function `{name}` emits assembly label `{label}` already used by function `{existing}`"
                )));
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
            let (name, params, return_type, extern_asm, is_interrupt) = match declaration {
                Declaration::Function(function) => (
                    &function.name,
                    &function.params,
                    &function.return_type,
                    false,
                    has_attr(function, "interrupt"),
                ),
                Declaration::ExternAsmFunction(function) => (
                    &function.name,
                    &function.params,
                    &function.return_type,
                    true,
                    false,
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
                    is_interrupt,
                },
            );
            if let Declaration::Function(function) = declaration
                && is_inlinable_function(function)
            {
                symbols
                    .inline_functions
                    .insert(function.name.clone(), function.clone());
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
                    let value_type =
                        symbols.resolved_type(&symbols.const_expr_type(&decl.value)?)?;
                    if type_is_bool(&value_type) {
                        return Err(Diagnostic::new(format!(
                            "port `{}` value must be an integer constant",
                            decl.name
                        )));
                    }
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
                    let value_type =
                        symbols.resolved_type(&symbols.const_expr_type(&decl.value)?)?;
                    if type_is_bool(&value_type) || matches!(value_type, Type::Ptr(_)) {
                        return Err(Diagnostic::new(format!(
                            "mmio `{}` address must be an integer constant",
                            decl.name
                        )));
                    }
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

    pub(super) fn alloc_var<S: Into<u32>>(&mut self, size: S) -> Variable {
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

    pub(super) fn intern_string_literal(&mut self, value: &str) -> Result<Variable, Diagnostic> {
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
        if let Some(original) = module_alias_original_name(&decl.name)
            && let Some(embed) = self.embeds.get(original).cloned()
        {
            self.register_embed_properties(
                &decl.name,
                embed.variable,
                embed.variable.len.unwrap_or(0),
            );
            return Ok(());
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
        if let Some(original) = module_alias_original_name(&decl.name)
            && let Some(variable) = self.globals.get(original).copied()
        {
            self.globals.insert(decl.name.clone(), variable);
            if let Some(ty) = self.global_types.get(original).cloned() {
                self.global_types.insert(decl.name.clone(), ty);
            }
            return Ok(());
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
            EmbedSource::File(path) => read_embed_file(path, source_path),
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
            offset += size;
        }
        Ok(StructLayout {
            size: offset,
            fields: layout_fields,
        })
    }

    pub(super) fn type_width(&self, ty: &Type) -> Result<ValueWidth, Diagnostic> {
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
            Type::Array { .. } => Err(Diagnostic::new("array value cannot be used as a scalar")),
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

    pub(super) fn validate_value_for_type(&self, value: i64, ty: &Type) -> Result<(), Diagnostic> {
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

    pub(super) fn resolved_type(&self, ty: &Type) -> Result<Type, Diagnostic> {
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

    pub(super) fn type_size(&self, ty: &Type) -> Result<u32, Diagnostic> {
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

    pub(super) fn alloc_storage(&mut self, ty: &Type) -> Result<Variable, Diagnostic> {
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

    pub(super) fn storage_at(&self, addr: u32, ty: &Type) -> Result<Variable, Diagnostic> {
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

    pub(super) fn const_array_element_variable(
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

    pub(super) fn const_access_variable(&self, path: &AccessPath) -> Result<Variable, Diagnostic> {
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

    pub(super) fn array_element_type(&self, name: &str) -> Result<Type, Diagnostic> {
        let Some(ty) = self.global_types.get(name) else {
            return Err(Diagnostic::new(format!("unknown array `{name}`")));
        };
        match self.resolved_type(ty)? {
            Type::Array { element, .. } => Ok(*element),
            _ => Err(Diagnostic::new(format!("`{name}` is not an array"))),
        }
    }

    pub(super) fn field_type(&self, base: &str, field: &str) -> Result<Type, Diagnostic> {
        let Some(ty) = self.global_types.get(base) else {
            return Err(Diagnostic::new(format!("unknown variable `{base}`")));
        };
        self.const_field_info(ty, field).map(|field| field.ty)
    }

    pub(super) fn access_type(&self, path: &AccessPath) -> Result<Type, Diagnostic> {
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

    pub(super) fn array_len(&self, expr: &Expr) -> Result<u32, Diagnostic> {
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

    pub(super) fn eval_i64(&self, expr: &Expr) -> Result<i64, Diagnostic> {
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

    pub(super) fn const_cast_value(&self, value: i64, ty: &Type) -> Result<i64, Diagnostic> {
        self.wrap_value_for_type(value, ty)
    }

    pub(super) fn wrap_value_for_type(&self, value: i64, ty: &Type) -> Result<i64, Diagnostic> {
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

    pub(super) fn embed_property_value(&self, name: &str) -> Option<i64> {
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

    pub(super) fn readonly_embed_name_for_addr(&self, addr: u32) -> Option<&str> {
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

    pub(super) fn readonly_string_literal_for_addr(&self, addr: u32) -> Option<&str> {
        self.readonly_string_literal_for_range(u64::from(addr), u64::from(addr) + 1)
    }

    pub(super) fn readonly_string_literal_for_range(&self, start: u64, end: u64) -> Option<&str> {
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
