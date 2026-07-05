use std::{
    collections::{HashMap, HashSet},
    fs,
    path::Path,
};

use crate::{
    ast::{
        AssignOp, BinaryOp, Declaration, EmbedSource, Expr, FieldDecl, Function, Place, Program,
        Stmt, Type, UnaryOp,
    },
    diagnostic::Diagnostic,
};

const VAR_BASE: u32 = 0x04_0000;

pub fn emit_ez80_assembly(program: &Program) -> Result<String, Diagnostic> {
    let symbols = Symbols::from_program(program)?;
    let main = program
        .main_function()
        .ok_or_else(|| Diagnostic::new("missing required `fn main()`"))?;

    let mut emitter = Emitter {
        symbols,
        out: String::new(),
        label_counter: 0,
        scopes: Vec::new(),
        scope_types: Vec::new(),
        string_literals: HashMap::new(),
        loop_stack: Vec::new(),
        return_type_stack: Vec::new(),
        return_value_stack: Vec::new(),
        function_name_stack: Vec::new(),
        function_frame_stack: Vec::new(),
    };
    emitter.emit_prelude();
    emitter.emit_embed_initializers();
    emitter.emit_global_initializers(program)?;
    emitter.emit_start_tail();
    emitter.emit_function(main)?;
    for declaration in &program.declarations {
        let Declaration::Function(function) = declaration else {
            continue;
        };
        if function.name != "main" {
            emitter.emit_function(function)?;
        }
    }
    Ok(emitter.out)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct Variable {
    addr: u32,
    size: u32,
    element_size: Option<u8>,
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

struct Symbols {
    constants: HashMap<String, i64>,
    constant_types: HashMap<String, Type>,
    aliases: HashMap<String, Type>,
    structs: HashMap<String, StructLayout>,
    embeds: HashMap<String, EmbedObject>,
    ports: HashMap<String, u8>,
    globals: HashMap<String, Variable>,
    global_types: HashMap<String, Type>,
    functions: HashMap<String, FunctionSig>,
    next_addr: u32,
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
    size: u8,
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
    fn from_program(program: &Program) -> Result<Self, Diagnostic> {
        let mut symbols = Self {
            constants: sdk_constants(),
            constant_types: HashMap::new(),
            aliases: HashMap::new(),
            structs: HashMap::new(),
            embeds: HashMap::new(),
            ports: sdk_ports(),
            globals: HashMap::new(),
            global_types: HashMap::new(),
            functions: HashMap::new(),
            next_addr: VAR_BASE,
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
                let layout = symbols.build_struct_layout(&decl.fields)?;
                symbols.structs.insert(decl.name.clone(), layout);
            }
        }

        for declaration in &program.declarations {
            let (name, params, return_type) = match declaration {
                Declaration::Function(function) => {
                    (&function.name, &function.params, &function.return_type)
                }
                Declaration::ExternAsmFunction(function) => {
                    (&function.name, &function.params, &function.return_type)
                }
                _ => continue,
            };
            for param in params {
                symbols.type_width(&param.ty)?;
            }
            let param_widths = params
                .iter()
                .map(|param| symbols.type_width(&param.ty))
                .collect::<Result<Vec<_>, _>>()?;
            let uses_arg_slots = params.len() <= 3
                && param_widths.get(2).is_some_and(|third| third.bytes() != 1)
                && param_widths
                    .get(1)
                    .is_some_and(|second| second.bytes() == 1);
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
        }

        for declaration in &program.declarations {
            match declaration {
                Declaration::Const(decl) => {
                    symbols.validate_const_expr_arithmetic_compatibility(&decl.value)?;
                    let value = symbols.eval_i64(&decl.value)?;
                    symbols.validate_value_for_type(value, &decl.ty)?;
                    symbols.constants.insert(decl.name.clone(), value);
                    symbols
                        .constant_types
                        .insert(decl.name.clone(), decl.ty.clone());
                }
                Declaration::Port(decl) => {
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
                    let align = decl
                        .align
                        .as_ref()
                        .map(|expr| symbols.eval_i64(expr))
                        .transpose()?
                        .unwrap_or(1);
                    if align <= 0 || (align & (align - 1)) != 0 {
                        return Err(Diagnostic::new(format!(
                            "embed `{}` alignment {align} is not a positive power of two",
                            decl.name
                        )));
                    }
                    if let Some(original) = module_alias_original_name(&decl.name) {
                        if let Some(embed) = symbols.embeds.get(original).cloned() {
                            symbols.register_embed_properties(
                                &decl.name,
                                embed.variable,
                                embed.variable.len.unwrap_or(0),
                            );
                            continue;
                        }
                    }
                    let bytes = symbols.embed_bytes(&decl.source, &program.source_path)?;
                    symbols.align_next_addr(align as u32);
                    let variable = symbols.alloc_array(ValueWidth::U8.bytes(), bytes.len() as u32);
                    symbols.register_embed_properties(&decl.name, variable, bytes.len() as u32);
                    symbols
                        .embeds
                        .insert(decl.name.clone(), EmbedObject { variable, bytes });
                }
                Declaration::Global(decl) => {
                    if let Some(original) = module_alias_original_name(&decl.name) {
                        if let Some(variable) = symbols.globals.get(original).copied() {
                            symbols.globals.insert(decl.name.clone(), variable);
                            if let Some(ty) = symbols.global_types.get(original).cloned() {
                                symbols.global_types.insert(decl.name.clone(), ty);
                            }
                            continue;
                        }
                    }
                    let variable = symbols.alloc_storage(&decl.ty)?;
                    symbols.globals.insert(decl.name.clone(), variable);
                    symbols
                        .global_types
                        .insert(decl.name.clone(), decl.ty.clone());
                }
                Declaration::Struct(_) => {}
                _ => {}
            }
        }

        Ok(symbols)
    }

    fn alloc_var(&mut self, size: u8) -> Variable {
        let variable = Variable {
            addr: self.next_addr,
            size: size as u32,
            element_size: None,
            len: None,
        };
        self.next_addr += size as u32;
        variable
    }

    fn alloc_array(&mut self, element_size: u8, len: u32) -> Variable {
        let size = element_size as u32 * len;
        let variable = Variable {
            addr: self.next_addr,
            size,
            element_size: Some(element_size),
            len: Some(len),
        };
        self.next_addr += size;
        variable
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
                    Diagnostic::new(format!(
                        "failed to read embedded file `{}`: {error}",
                        resolved.display()
                    ))
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

    fn build_struct_layout(&self, fields: &[FieldDecl]) -> Result<StructLayout, Diagnostic> {
        let mut offset = 0u32;
        let mut layout_fields = HashMap::new();
        for field in fields {
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
                    return Err(Diagnostic::new(format!(
                        "type `{name}` is parsed but not implemented in assembly codegen yet"
                    )));
                };
                self.type_width(alias)
            }
            Type::Ptr(_) => Ok(ValueWidth::U24),
            Type::Array { .. } => Err(Diagnostic::new(
                "array storage codegen is not implemented yet",
            )),
        }
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

    fn type_size(&self, ty: &Type) -> Result<u8, Diagnostic> {
        match self.resolved_type(ty)? {
            Type::Array { .. } => Err(Diagnostic::new(
                "array value cannot be used where scalar storage size is required",
            )),
            Type::Named(name) if self.structs.contains_key(&name) => {
                let size = self.structs[&name].size;
                if size > u8::MAX as u32 {
                    return Err(Diagnostic::new(format!(
                        "struct `{name}` size {size} exceeds current storage limit"
                    )));
                }
                Ok(size as u8)
            }
            scalar => Ok(self.type_width(&scalar)?.bytes()),
        }
    }

    fn alloc_storage(&mut self, ty: &Type) -> Result<Variable, Diagnostic> {
        match self.resolved_type(ty)? {
            Type::Array { element, len } => {
                let element_size = self.type_width(&element)?.bytes();
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

    fn array_len(&self, text: &str) -> Result<u32, Diagnostic> {
        let value = if let Some(value) = self.constants.get(text).copied() {
            value
        } else {
            parse_int_text(text)?
        };
        if value < 0 {
            return Err(Diagnostic::new(format!("array length {value} is negative")));
        }
        Ok(value as u32)
    }

    fn eval_i64(&self, expr: &Expr) -> Result<i64, Diagnostic> {
        match expr {
            Expr::Int(value) => Ok(*value),
            Expr::Char(value) => Ok(*value as i64),
            Expr::Bool(value) => Ok(i64::from(*value)),
            Expr::Ident(name) => self
                .constants
                .get(name)
                .copied()
                .ok_or_else(|| Diagnostic::new(format!("unknown constant `{name}`"))),
            Expr::Unary { op, expr } => {
                let value = self.eval_i64(expr)?;
                Ok(match op {
                    UnaryOp::Neg => -value,
                    UnaryOp::BitNot => !value,
                    UnaryOp::Not => i64::from(value == 0),
                })
            }
            Expr::Binary { left, op, right } => {
                let left = self.eval_i64(left)?;
                let right = self.eval_i64(right)?;
                Ok(match op {
                    BinaryOp::Mul => left * right,
                    BinaryOp::Div => trunc_div_or_zero(left, right),
                    BinaryOp::Mod => trunc_mod_or_zero(left, right),
                    BinaryOp::Add => left + right,
                    BinaryOp::Sub => left - right,
                    BinaryOp::Shl => const_shl_or_zero(left, right),
                    BinaryOp::Shr => const_shr_or_zero(left, right),
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
            Expr::Cast { expr, .. } => self.eval_i64(expr),
            Expr::Array(_)
            | Expr::Index { .. }
            | Expr::AddressOfIndex { .. }
            | Expr::AddressOfField { .. }
            | Expr::AddressOf(_)
            | Expr::Field { .. }
            | Expr::StructInit { .. }
            | Expr::Deref(_)
            | Expr::In(_)
            | Expr::Call { .. }
            | Expr::String(_) => Err(Diagnostic::new(format!(
                "expression `{expr:?}` is not a compile-time integer"
            ))),
        }
    }

    fn validate_const_expr_arithmetic_compatibility(&self, expr: &Expr) -> Result<(), Diagnostic> {
        match expr {
            Expr::Binary { left, right, .. } => {
                self.validate_const_expr_arithmetic_compatibility(left)?;
                self.validate_const_expr_arithmetic_compatibility(right)?;
                self.validate_const_binary_operand_types(left, right)?;
            }
            Expr::Unary { expr, .. } | Expr::Cast { expr, .. } | Expr::Deref(expr) => {
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

    fn validate_const_binary_operand_types(
        &self,
        left: &Expr,
        right: &Expr,
    ) -> Result<(), Diagnostic> {
        if expr_is_untyped_literal(left) || expr_is_untyped_literal(right) {
            return Ok(());
        }

        let left_type = self.resolved_type(&self.const_expr_type(left)?)?;
        let right_type = self.resolved_type(&self.const_expr_type(right)?)?;
        if type_is_bool(&left_type) || type_is_bool(&right_type) {
            return Err(Diagnostic::new("type mismatch"));
        }
        if type_is_signed(&left_type) != type_is_signed(&right_type) {
            return Err(Diagnostic::new("signed/unsigned mix without cast"));
        }
        Ok(())
    }

    fn const_expr_type(&self, expr: &Expr) -> Result<Type, Diagnostic> {
        match expr {
            Expr::Ident(name) => {
                if let Some(ty) = self.constant_types.get(name) {
                    Ok(ty.clone())
                } else if let Some(value) = self.constants.get(name).copied() {
                    Ok(int_value_type(value))
                } else {
                    Err(Diagnostic::new(format!("unknown constant `{name}`")))
                }
            }
            Expr::Int(value) => Ok(int_value_type(*value)),
            Expr::Char(_) => Ok(Type::Named("u8".to_owned())),
            Expr::Bool(_) => Ok(Type::Named("bool".to_owned())),
            Expr::Unary { expr, op } => match op {
                UnaryOp::Not => Ok(Type::Named("bool".to_owned())),
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
            Expr::Array(_)
            | Expr::Index { .. }
            | Expr::AddressOfIndex { .. }
            | Expr::AddressOfField { .. }
            | Expr::AddressOf(_)
            | Expr::Field { .. }
            | Expr::StructInit { .. }
            | Expr::Deref(_)
            | Expr::In(_)
            | Expr::Call { .. } => Err(Diagnostic::new(
                "expression is not supported in a constant declaration",
            )),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct LoopLabels {
    continue_label: String,
    break_label: String,
}

struct Emitter {
    symbols: Symbols,
    out: String,
    label_counter: usize,
    scopes: Vec<HashMap<String, Variable>>,
    scope_types: Vec<HashMap<String, Type>>,
    string_literals: HashMap<String, Variable>,
    loop_stack: Vec<LoopLabels>,
    return_type_stack: Vec<Option<Type>>,
    return_value_stack: Vec<bool>,
    function_name_stack: Vec<String>,
    function_frame_stack: Vec<bool>,
}

impl Emitter {
    fn emit_prelude(&mut self) {
        self.line("; generated by ezrac scaffold");
        self.line("; target: eZ80 ADL mode");
        self.line("section .text");
        self.line("__ezra_start:");
        self.line("    ld sp, 0F00000h");
    }

    fn emit_start_tail(&mut self) {
        self.line("    call _main");
        self.line("__ezra_exit:");
        self.line("    jp __ezra_exit");
        self.line("");
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
            if variable.element_size.is_some() {
                self.emit_array_initializer(variable, &decl.value)?;
            } else if self.is_struct_type(&decl.ty)? {
                self.emit_struct_initializer(variable, &decl.ty, &decl.value)?;
            } else {
                self.emit_expr_to_type(&decl.value, &decl.ty)?;
                self.emit_store_width(variable);
            }
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
                    ValueWidth::U8.bytes(),
                ));
            }
        }
    }

    fn emit_function(&mut self, function: &Function) -> Result<(), Diagnostic> {
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
            && !block_guarantees_value_return(&function.body)
        {
            return Err(Diagnostic::new(format!(
                "missing return value in function `{}`",
                function.name
            )));
        }
        self.line(&format!("{}:", function_label(&function.name)));
        self.scopes.push(HashMap::new());
        self.scope_types.push(HashMap::new());
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
        if !naked {
            if interrupt {
                if !function.params.is_empty() {
                    return Err(Diagnostic::new(format!(
                        "interrupt function `{}` cannot take parameters",
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
        for stmt in &function.body {
            self.emit_stmt(stmt)?;
        }
        self.function_frame_stack.pop();
        self.function_name_stack.pop();
        self.return_value_stack.pop();
        self.return_type_stack.pop();
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
    }

    fn emit_interrupt_epilogue(&mut self) {
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
            let variable = self.symbols.alloc_var(width.bytes());
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

    fn emit_stmt(&mut self, stmt: &Stmt) -> Result<(), Diagnostic> {
        match stmt {
            Stmt::Let { name, ty, value } => {
                if self.name_in_current_function(name) {
                    return Err(Diagnostic::new(format!(
                        "local `{name}` shadows an existing name"
                    )));
                }
                let variable = self.symbols.alloc_storage(ty)?;
                self.current_scope_mut().insert(name.clone(), variable);
                self.current_scope_types_mut()
                    .insert(name.clone(), ty.clone());
                if variable.element_size.is_some() {
                    self.emit_array_initializer(variable, value)?;
                } else if self.is_struct_type(ty)? {
                    self.emit_struct_initializer(variable, ty, value)?;
                } else {
                    self.emit_expr_to_type(value, ty)?;
                    self.emit_store_width(variable);
                }
            }
            Stmt::Assign { target, op, value } => {
                self.emit_assignment(target, *op, value)?;
            }
            Stmt::Out { port, value } => {
                let port = self.port(port)?;
                self.emit_expr_to_a(value)?;
                self.emit_out_a(port);
            }
            Stmt::Expr(Expr::Call { path, args }) => self.emit_call(path, args)?,
            Stmt::Expr(expr) => {
                self.emit_expr_to_a(expr)?;
            }
            Stmt::If {
                condition,
                then_body,
                else_body,
            } => {
                let else_label = self.next_label("else");
                let end_label = self.next_label("endif");
                self.emit_expr_to_a(condition)?;
                self.line("    or a");
                self.line(&format!("    jp z, {else_label}"));
                for stmt in then_body {
                    self.emit_stmt(stmt)?;
                }
                self.line(&format!("    jp {end_label}"));
                self.line(&format!("{else_label}:"));
                for stmt in else_body {
                    self.emit_stmt(stmt)?;
                }
                self.line(&format!("{end_label}:"));
            }
            Stmt::While { condition, body } => {
                let start_label = self.next_label("while");
                let end_label = self.next_label("endwhile");
                self.loop_stack.push(LoopLabels {
                    continue_label: start_label.clone(),
                    break_label: end_label.clone(),
                });
                self.line(&format!("{start_label}:"));
                self.emit_expr_to_a(condition)?;
                self.line("    or a");
                self.line(&format!("    jp z, {end_label}"));
                for stmt in body {
                    self.emit_stmt(stmt)?;
                }
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
                for stmt in body {
                    self.emit_stmt(stmt)?;
                }
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
                self.line("    ret");
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
                self.line("    ret");
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
        validate_inline_asm_clobbers(clobbers, lines)?;

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
                let value = self.symbols.eval_i64(&Expr::Ident(input.name.clone()))?;
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
                AssignOp::Shl => self.emit_wide_assignment_shift(variable, BinaryOp::Shl, value)?,
                AssignOp::Shr => self.emit_wide_assignment_shift(variable, BinaryOp::Shr, value)?,
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
                AssignOp::Shl => self.emit_wide_assignment_shift(variable, BinaryOp::Shl, value)?,
                AssignOp::Shr => self.emit_wide_assignment_shift(variable, BinaryOp::Shr, value)?,
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
                self.emit_load_a(variable);
                self.emit_shift_a_by_expr(BinaryOp::Shl, value)?;
            }
            AssignOp::Shr => {
                self.emit_load_a(variable);
                self.emit_shift_a_by_expr(BinaryOp::Shr, value)?;
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
                let variable = self.variable(name)?;
                if op == AssignOp::Set {
                    if let Some(ty) = self.variable_type(name) {
                        self.validate_expr_assignable_to_type(value, ty)?;
                    }
                }
                self.emit_assignment_value(variable, op, value)?;
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
                }
                self.emit_assignment_value(variable, op, value)?;
                self.emit_store_width(variable);
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
        if values.len() as u32 > len {
            return Err(Diagnostic::new(format!(
                "array initializer has {} values but array length is {len}",
                values.len()
            )));
        }
        for index in 0..len {
            let element = scalar_var(variable.addr + index * element_size as u32, element_size);
            if let Some(value) = values.get(index as usize) {
                self.emit_expr_to_width(value, element.width()?)?;
            } else {
                self.line(match element_size {
                    1 => "    ld a, 00h",
                    2 | 3 => "    ld hl, 000000h",
                    _ => unreachable!("unsupported array element size"),
                });
            }
            self.emit_store_width(element);
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
            let field_var = scalar_var(variable.addr + field.offset, field.size);
            self.emit_expr_to_width(field_value, field_var.width()?)?;
            self.emit_store_width(field_var);
        }

        for (field_name, field) in &layout.fields {
            if initialized.contains_key(field_name) {
                continue;
            }
            let field_var = scalar_var(variable.addr + field.offset, field.size);
            match field.size {
                1 => self.line("    ld a, 00h"),
                2 | 3 => self.line("    ld hl, 000000h"),
                _ => unreachable!("unsupported struct field size"),
            }
            self.emit_store_width(field_var);
        }
        Ok(())
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
    ) -> Result<(), Diagnostic> {
        let temp = self.symbols.alloc_var(variable.width()?.bytes());
        self.emit_load_width(variable);
        self.emit_store_width(temp);
        self.emit_shift_memory_by_expr(temp, op, value)?;
        self.emit_load_width(temp);
        Ok(())
    }

    fn emit_call(&mut self, path: &[String], args: &[Expr]) -> Result<(), Diagnostic> {
        match path_text(path).as_str() {
            "test.pass" | "ezra.test.pass" => {
                self.emit_out(0x0D, 0);
                self.emit_out(0x0E, 1);
            }
            "test.fail" | "ezra.test.fail" => {
                let expr = args.first().cloned().unwrap_or(Expr::Int(1));
                self.emit_expr_to_a(&expr)?;
                self.emit_out_a(0x0D);
                self.emit_out(0x0E, 1);
            }
            "test.assert_eq_u8" | "ezra.test.assert_eq_u8" => {
                if args.len() != 3 {
                    return Err(Diagnostic::new(
                        "test.assert_eq_u8 requires three arguments",
                    ));
                }
                let ok = self.next_label("assert_ok");
                self.emit_expr_to_a(&args[0])?;
                self.line("    ld b, a");
                self.emit_expr_to_a(&args[1])?;
                self.line("    ld c, a");
                self.line("    ld a, b");
                self.line("    cp c");
                self.line(&format!("    jp z, {ok}"));
                self.emit_expr_to_a(&args[2])?;
                self.emit_out_a(0x0D);
                self.emit_out(0x0E, 1);
                self.line(&format!("{ok}:"));
            }
            "test.assert_eq_u16" | "ezra.test.assert_eq_u16" => {
                if args.len() != 3 {
                    return Err(Diagnostic::new(
                        "test.assert_eq_u16 requires three arguments",
                    ));
                }
                let ok = self.next_label("assert_ok");
                self.emit_expr_to_hl(&args[0], ValueWidth::U16)?;
                self.line("    push hl");
                self.emit_expr_to_hl(&args[1], ValueWidth::U16)?;
                self.line("    pop bc");
                self.line("    or a");
                self.line("    sbc hl, bc");
                self.line(&format!("    jp z, {ok}"));
                self.emit_expr_to_a(&args[2])?;
                self.emit_out_a(0x0D);
                self.emit_out(0x0E, 1);
                self.line(&format!("{ok}:"));
            }
            "test.assert_eq_u24" | "ezra.test.assert_eq_u24" => {
                if args.len() != 3 {
                    return Err(Diagnostic::new(
                        "test.assert_eq_u24 requires three arguments",
                    ));
                }
                let ok = self.next_label("assert_ok");
                self.emit_expr_to_hl(&args[0], ValueWidth::U24)?;
                self.line("    push hl");
                self.emit_expr_to_hl(&args[1], ValueWidth::U24)?;
                self.line("    pop bc");
                self.line("    or a");
                self.line("    sbc hl, bc");
                self.line(&format!("    jp z, {ok}"));
                self.emit_expr_to_a(&args[2])?;
                self.emit_out_a(0x0D);
                self.emit_out(0x0E, 1);
                self.line(&format!("{ok}:"));
            }
            "debug.char" | "ezra.debug.char" => {
                let expr = args
                    .first()
                    .ok_or_else(|| Diagnostic::new("debug.char requires one argument"))?;
                self.emit_expr_to_a(expr)?;
                self.emit_out_a(0x0C);
            }
            "mem.poke8" | "ezra.mem.poke8" => {
                self.emit_mem_poke8(args)?;
            }
            path => self.emit_user_call(path, args)?,
        }
        Ok(())
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
            let temp = self.symbols.alloc_var(width.bytes());
            self.emit_expr_to_type(arg, ty)?;
            self.emit_store_width(temp);
            temps.push(temp);
        }

        if sig.uses_arg_slots {
            for (temp, slot) in temps.iter().copied().zip(sig.arg_slots.iter().copied()) {
                self.emit_load_width(temp);
                self.emit_store_width(slot);
            }
            self.line(&format!("    call {}", function_label(name)));
            return Ok(());
        }

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
        if let Ok(value) = self.symbols.eval_i64(expr) {
            let value = self.value_for_type(value, ty, width)?;
            match width {
                ValueWidth::U8 => self.line(&format!("    ld a, {value:02X}h")),
                ValueWidth::U16 | ValueWidth::U24 => self.line(&format!("    ld hl, {value:06X}h")),
            }
            return Ok(());
        }
        self.emit_expr_to_width(expr, width)
    }

    fn emit_cast_to_type(&mut self, expr: &Expr, ty: &Type) -> Result<(), Diagnostic> {
        self.validate_cast(expr, ty)?;
        let width = self.symbols.type_width(ty)?;
        if let Ok(value) = self.symbols.eval_i64(expr) {
            let bits = u32::from(width.bytes()) * 8;
            let mask = (1_i128 << bits) - 1;
            let value = ((value as i128) & mask) as u32;
            match width {
                ValueWidth::U8 => self.line(&format!("    ld a, {value:02X}h")),
                ValueWidth::U16 | ValueWidth::U24 => self.line(&format!("    ld hl, {value:06X}h")),
            }
            return Ok(());
        }
        let source_width = self.expr_width(expr)?;
        match width {
            ValueWidth::U8 => {
                if source_width == ValueWidth::U8 {
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
                } else {
                    self.emit_expr_to_hl(expr, source_width)?;
                }
            }
        }
        Ok(())
    }

    fn validate_cast(&self, expr: &Expr, target: &Type) -> Result<(), Diagnostic> {
        let source_type = self.symbols.resolved_type(&self.expr_type(expr)?)?;
        let target_type = self.symbols.resolved_type(target)?;
        match (&source_type, &target_type) {
            (Type::Ptr(_), Type::Ptr(_)) => Ok(()),
            (Type::Ptr(_), Type::Named(name)) if name == "u24" => Ok(()),
            (Type::Ptr(_), Type::Named(_)) => {
                Err(Diagnostic::new("pointer-to-integer casts produce u24"))
            }
            (Type::Named(name), Type::Ptr(_)) if name == "u24" => Ok(()),
            (Type::Named(_), Type::Ptr(_)) => {
                Err(Diagnostic::new("integer-to-pointer casts require u24"))
            }
            _ => Ok(()),
        }
    }

    fn emit_expr_to_hl(&mut self, expr: &Expr, width: ValueWidth) -> Result<(), Diagnostic> {
        match expr {
            Expr::Ident(name) => {
                if let Some(variable) = self.variable_opt(name) {
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
            Expr::Int(_) | Expr::Char(_) | Expr::Bool(_) => {
                let value = self.value_for_width(expr, width)?;
                self.line(&format!("    ld hl, {:06X}h", value));
            }
            Expr::Cast { expr, ty } => self.emit_cast_to_type(expr, ty)?,
            Expr::Unary { op, expr } => self.emit_unary_to_hl(*op, expr, width)?,
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
                        self.emit_mul_to_width(left, right, width)?;
                        return Ok(());
                    }
                    self.emit_expr_to_hl(left, width)?;
                    self.line("    push hl");
                    self.emit_expr_to_hl(right, width)?;
                    self.line("    pop bc");
                    self.emit_wide_op_with_left_in_bc(*op, width)?;
                }
                BinaryOp::Shl | BinaryOp::Shr => {
                    let temp = self.symbols.alloc_var(width.bytes());
                    self.emit_expr_to_hl(left, width)?;
                    self.emit_store_width(temp);
                    self.emit_shift_memory_by_expr(temp, *op, right)?;
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
        let temp = self.symbols.alloc_var(ValueWidth::U16.bytes());
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
            (BinaryOp::Add, Some(scale), _) => {
                self.emit_expr_to_hl(left, ValueWidth::U24)?;
                self.line("    push hl");
                self.emit_scaled_offset_to_hl(right, scale)?;
                self.line("    pop bc");
                self.line("    add hl, bc");
                Ok(true)
            }
            (BinaryOp::Add, None, Some(scale)) => {
                self.emit_expr_to_hl(right, ValueWidth::U24)?;
                self.line("    push hl");
                self.emit_scaled_offset_to_hl(left, scale)?;
                self.line("    pop bc");
                self.line("    add hl, bc");
                Ok(true)
            }
            (BinaryOp::Sub, Some(scale), None) => {
                self.emit_expr_to_hl(left, ValueWidth::U24)?;
                self.line("    push hl");
                self.emit_scaled_offset_to_hl(right, scale)?;
                self.line("    ex de, hl");
                self.line("    pop hl");
                self.line("    or a");
                self.line("    sbc hl, de");
                Ok(true)
            }
            _ => Ok(false),
        }
    }

    fn emit_scaled_offset_to_hl(&mut self, expr: &Expr, scale: u8) -> Result<(), Diagnostic> {
        self.emit_expr_to_hl(expr, ValueWidth::U24)?;
        match scale {
            1 => {}
            _ => {
                let base = self.symbols.alloc_var(ValueWidth::U24.bytes());
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
        let right = self.symbols.alloc_var(width.bytes());
        self.emit_store_width(right);
        self.line("    push bc");
        self.line("    pop hl");
        let left = self.symbols.alloc_var(width.bytes());
        self.emit_store_width(left);
        let result = self.symbols.alloc_var(width.bytes());

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
                if let Some(variable) = self.variable_opt(name) {
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
            Expr::Deref(ptr) => {
                self.emit_deref_to_a(ptr)?;
            }
            Expr::Int(_) | Expr::Char(_) | Expr::Bool(_) => {
                let value = self.u8(expr)?;
                self.line(&format!("    ld a, {:02X}h", value));
            }
            Expr::Cast { expr, ty } => self.emit_cast_to_type(expr, ty)?,
            Expr::Unary { op, expr } => self.emit_unary_to_a(*op, expr)?,
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
        self.emit_expr_to_hl(&args[0], ValueWidth::U24)?;
        self.line("    ld a, (hl)");
        Ok(())
    }

    fn emit_mem_poke8(&mut self, args: &[Expr]) -> Result<(), Diagnostic> {
        if args.len() != 2 {
            return Err(Diagnostic::new("mem.poke8 requires two arguments"));
        }
        let addr = self.symbols.alloc_var(ValueWidth::U24.bytes());
        let value = self.symbols.alloc_var(ValueWidth::U8.bytes());
        self.emit_expr_to_hl(&args[0], ValueWidth::U24)?;
        self.emit_store_hl(addr);
        self.emit_expr_to_a(&args[1])?;
        self.emit_store_a(value);
        self.emit_load_hl(addr);
        self.emit_load_a(value);
        self.line("    ld (hl), a");
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

        let len = value
            .len()
            .checked_add(1)
            .ok_or_else(|| Diagnostic::new("string literal is too large"))?;
        if len > u32::MAX as usize {
            return Err(Diagnostic::new("string literal is too large"));
        }

        let variable = self.symbols.alloc_array(ValueWidth::U8.bytes(), len as u32);
        for (offset, byte) in value.bytes().chain(std::iter::once(0)).enumerate() {
            self.line(&format!("    ld a, {byte:02X}h"));
            self.emit_store_a(scalar_var(
                variable.addr + offset as u32,
                ValueWidth::U8.bytes(),
            ));
        }
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
                let result = self.symbols.alloc_var(width.bytes());
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
        let width = self.symbols.type_width(&pointee_type)?;

        let addr = self.symbols.alloc_var(ValueWidth::U24.bytes());
        self.emit_expr_to_hl(ptr, ValueWidth::U24)?;
        self.emit_store_hl(addr);

        if op != AssignOp::Set {
            let current = self.symbols.alloc_var(width.bytes());
            self.emit_load_hl(addr);
            self.emit_load_pointed_width_into(current);
            let stored = self.symbols.alloc_var(width.bytes());
            self.emit_assignment_value(current, op, value)?;
            self.emit_store_width(stored);
            self.emit_load_hl(addr);
            self.emit_store_var_to_pointed_width(stored);
            return Ok(());
        }

        self.validate_expr_assignable_to_type(value, &pointee_type)?;
        let stored = self.symbols.alloc_var(width.bytes());
        self.emit_expr_to_width(value, width)?;
        self.emit_store_width(stored);
        self.emit_load_hl(addr);
        self.emit_store_var_to_pointed_width(stored);
        Ok(())
    }

    fn emit_binary_expr(
        &mut self,
        left: &Expr,
        op: BinaryOp,
        right: &Expr,
    ) -> Result<(), Diagnostic> {
        if is_comparison(op) {
            let width = self.expr_width(left)?.max(self.expr_width(right)?);
            if width != ValueWidth::U8 {
                self.emit_wide_comparison(left, op, right, width)?;
                return Ok(());
            }
        }
        if matches!(op, BinaryOp::Shl | BinaryOp::Shr) {
            self.emit_expr_to_a(left)?;
            self.emit_shift_a_by_expr(op, right)?;
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
            self.emit_mul_to_width(left, right, ValueWidth::U8)?;
            return Ok(());
        }

        if matches!(
            op,
            BinaryOp::Add | BinaryOp::Sub | BinaryOp::BitAnd | BinaryOp::BitOr | BinaryOp::BitXor
        ) {
            self.ensure_binary_arithmetic_operands_compatible(left, right)?;
        }
        self.emit_expr_to_a(left)?;
        self.line("    ld b, a");
        self.emit_expr_to_a(right)?;
        self.line("    ld c, a");
        self.line("    ld a, b");
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
            BinaryOp::And | BinaryOp::Or => self.emit_logical(op),
            BinaryOp::Div | BinaryOp::Mod | BinaryOp::Shl | BinaryOp::Shr => {
                return Err(Diagnostic::new(format!(
                    "binary operator `{op:?}` is not implemented in u8 codegen yet"
                )));
            }
            BinaryOp::Mul => unreachable!("multiplication handled before u8 binary dispatch"),
        }
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

    fn emit_u8_div_mod(
        &mut self,
        left: &Expr,
        right: &Expr,
        op: BinaryOp,
    ) -> Result<(), Diagnostic> {
        let dividend = self.symbols.alloc_var(1);
        let divisor = self.symbols.alloc_var(1);
        let quotient = self.symbols.alloc_var(1);
        let loop_label = self.next_label("div_loop");
        let zero_label = self.next_label("div_zero");
        let done_label = self.next_label("div_done");

        self.emit_expr_to_a(left)?;
        self.emit_store_a(dividend);
        self.emit_expr_to_a(right)?;
        self.emit_store_a(divisor);
        self.line("    or a");
        self.line(&format!("    jp z, {zero_label}"));
        self.line("    xor a");
        self.emit_store_a(quotient);
        self.line(&format!("{loop_label}:"));
        self.emit_load_a(dividend);
        self.line("    ld b, a");
        self.emit_load_a(divisor);
        self.line("    ld c, a");
        self.line("    ld a, b");
        self.line("    cp c");
        self.line(&format!("    jp c, {done_label}"));
        self.line("    sub c");
        self.emit_store_a(dividend);
        self.emit_load_a(quotient);
        self.line("    ld b, a");
        self.line("    ld a, 01h");
        self.line("    add a, b");
        self.emit_store_a(quotient);
        self.line(&format!("    jp {loop_label}"));
        self.line(&format!("{zero_label}:"));
        self.line("    xor a");
        self.emit_store_a(dividend);
        self.line("    xor a");
        self.emit_store_a(quotient);
        self.line("    xor a");
        self.line(&format!("    jp {done_label}"));
        self.line(&format!("{done_label}:"));
        match op {
            BinaryOp::Div => self.emit_load_a(quotient),
            BinaryOp::Mod => self.emit_load_a(dividend),
            _ => unreachable!("not a division op"),
        }
        Ok(())
    }

    fn emit_mul_to_width(
        &mut self,
        left: &Expr,
        right: &Expr,
        width: ValueWidth,
    ) -> Result<(), Diagnostic> {
        let left_var = self.symbols.alloc_var(width.bytes());
        let counter = self.symbols.alloc_var(width.bytes());
        let result = self.symbols.alloc_var(width.bytes());
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
        let dividend = self.symbols.alloc_var(width.bytes());
        let divisor = self.symbols.alloc_var(width.bytes());
        let quotient = self.symbols.alloc_var(width.bytes());
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
        let dividend = self.symbols.alloc_var(width.bytes());
        let divisor = self.symbols.alloc_var(width.bytes());
        let quotient = self.symbols.alloc_var(width.bytes());
        let quotient_negative = self.symbols.alloc_var(ValueWidth::U8.bytes());
        let remainder_negative = self.symbols.alloc_var(ValueWidth::U8.bytes());
        let loop_label = self.next_label("sdiv_loop");
        let zero_label = self.next_label("sdiv_zero");
        let done_label = self.next_label("sdiv_done");
        let quotient_positive_label = self.next_label("sdiv_q_positive");
        let remainder_positive_label = self.next_label("sdiv_r_positive");

        self.emit_expr_to_width(left, width)?;
        self.emit_store_width(dividend);
        self.emit_expr_to_width(right, width)?;
        self.emit_store_width(divisor);
        self.emit_jump_if_memory_zero(divisor, &zero_label);
        self.emit_zero_variable(quotient);
        self.emit_zero_variable(quotient_negative);
        self.emit_zero_variable(remainder_negative);

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

    fn emit_zero_variable(&mut self, variable: Variable) {
        match variable.size {
            1 => self.line("    xor a"),
            2 | 3 => self.line("    ld hl, 000000h"),
            _ => unreachable!("unsupported variable size {}", variable.size),
        }
        self.emit_store_width(variable);
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

    fn emit_shift_a(&mut self, op: BinaryOp, count: u8) -> Result<(), Diagnostic> {
        for _ in 0..count {
            match op {
                BinaryOp::Shl => self.line("    add a, a"),
                BinaryOp::Shr => self.line("    srl a"),
                _ => unreachable!("not a shift op"),
            }
        }
        Ok(())
    }

    fn emit_shift_a_by_expr(&mut self, op: BinaryOp, count: &Expr) -> Result<(), Diagnostic> {
        if let Some(count) = self.maybe_const_shift_count(count)? {
            return self.emit_shift_a(op, count);
        }
        let temp = self.symbols.alloc_var(ValueWidth::U8.bytes());
        self.emit_store_a(temp);
        self.emit_expr_to_a(count)?;
        self.line("    ld b, a");
        self.emit_shift_memory_dynamic(temp, op)?;
        self.emit_load_a(temp);
        Ok(())
    }

    fn emit_shift_memory(
        &mut self,
        variable: Variable,
        op: BinaryOp,
        count: u8,
    ) -> Result<(), Diagnostic> {
        for _ in 0..count {
            match op {
                BinaryOp::Shl => self.emit_shift_memory_left_once(variable),
                BinaryOp::Shr => self.emit_shift_memory_right_once(variable),
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
    ) -> Result<(), Diagnostic> {
        if let Some(count) = self.maybe_const_shift_count(count)? {
            return self.emit_shift_memory(variable, op, count);
        }
        self.emit_expr_to_a(count)?;
        self.line("    ld b, a");
        self.emit_shift_memory_dynamic(variable, op)
    }

    fn emit_shift_memory_dynamic(
        &mut self,
        variable: Variable,
        op: BinaryOp,
    ) -> Result<(), Diagnostic> {
        let loop_label = self.next_label("shift_loop");
        let done_label = self.next_label("shift_done");
        self.line(&format!("{loop_label}:"));
        self.line("    ld a, b");
        self.line("    or a");
        self.line(&format!("    jp z, {done_label}"));
        match op {
            BinaryOp::Shl => self.emit_shift_memory_left_once(variable),
            BinaryOp::Shr => self.emit_shift_memory_right_once(variable),
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

    fn emit_shift_memory_right_once(&mut self, variable: Variable) {
        for offset in (0..variable.size).rev() {
            let addr = variable.addr + offset as u32;
            self.line(&format!("    ld a, ({addr:06X}h)"));
            if offset == variable.size - 1 {
                self.line("    srl a");
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
                let value = self.symbols.alloc_var(width.bytes());
                self.emit_store_width(value);
                let result = self.symbols.alloc_var(width.bytes());
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

    fn emit_logical(&mut self, op: BinaryOp) {
        match op {
            BinaryOp::And => {
                let false_label = self.next_label("and_false");
                let end_label = self.next_label("and_end");
                self.line("    or a");
                self.line(&format!("    jp z, {false_label}"));
                self.line("    ld a, c");
                self.line("    or a");
                self.line(&format!("    jp z, {false_label}"));
                self.line("    ld a, 01h");
                self.line(&format!("    jp {end_label}"));
                self.line(&format!("{false_label}:"));
                self.line("    ld a, 00h");
                self.line(&format!("{end_label}:"));
            }
            BinaryOp::Or => {
                let true_label = self.next_label("or_true");
                let end_label = self.next_label("or_end");
                self.line("    or a");
                self.line(&format!("    jp nz, {true_label}"));
                self.line("    ld a, c");
                self.line("    or a");
                self.line(&format!("    jp nz, {true_label}"));
                self.line("    ld a, 00h");
                self.line(&format!("    jp {end_label}"));
                self.line(&format!("{true_label}:"));
                self.line("    ld a, 01h");
                self.line(&format!("{end_label}:"));
            }
            _ => unreachable!("not logical"),
        }
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

    fn array_element_variable(&self, name: &str, index: &Expr) -> Result<Variable, Diagnostic> {
        let (array, element_size, len) = self.array_info(name)?;
        let index_value = self.symbols.eval_i64(index)?;
        if index_value < 0 || index_value as u32 >= len {
            return Err(Diagnostic::new(format!(
                "array index {index_value} is out of bounds for `{name}` length {len}"
            )));
        }
        Ok(scalar_var(
            array.addr + index_value as u32 * element_size as u32,
            element_size,
        ))
    }

    fn array_info(&self, name: &str) -> Result<(Variable, u8, u32), Diagnostic> {
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

    fn is_struct_type(&self, ty: &Type) -> Result<bool, Diagnostic> {
        match self.symbols.resolved_type(ty)? {
            Type::Named(name) => Ok(self.symbols.structs.contains_key(&name)),
            _ => Ok(false),
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
        Ok(scalar_var(base_variable.addr + field.offset, field.size))
    }

    fn field_type(&self, base: &str, field: &str) -> Result<Type, Diagnostic> {
        let key = format!("{base}.{field}");
        if let Some(ty) = self.variable_type(&key) {
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

    fn array_element_type(&self, name: &str) -> Result<Type, Diagnostic> {
        let Some(ty) = self.variable_type(name) else {
            return Err(Diagnostic::new(format!("unknown array `{name}`")));
        };
        match self.symbols.resolved_type(ty)? {
            Type::Array { element, .. } => Ok(*element),
            _ => Err(Diagnostic::new(format!("`{name}` is not an array"))),
        }
    }

    fn pointer_pointee_size(&self, expr: &Expr) -> Result<Option<u8>, Diagnostic> {
        match self.expr_type(expr) {
            Ok(ty) => match self.symbols.resolved_type(&ty)? {
                Type::Ptr(inner) => Ok(Some(self.symbols.type_size(&inner)?)),
                _ => Ok(None),
            },
            Err(_) => Ok(None),
        }
    }

    fn emit_array_element_address(&mut self, name: &str, index: &Expr) -> Result<(), Diagnostic> {
        if let Ok(element) = self.array_element_variable(name, index) {
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
            _ => unreachable!("unsupported array element size"),
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
        if let Ok(element) = self.array_element_variable(name, index) {
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
                let result = self.symbols.alloc_var(element_size);
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
        if let Ok(element) = self.array_element_variable(name, index) {
            if op == AssignOp::Set {
                let ty = self.array_element_type(name)?;
                self.validate_expr_assignable_to_type(value, &ty)?;
            }
            self.emit_assignment_value(element, op, value)?;
            self.emit_store_width(element);
            return Ok(());
        }

        let (_, element_size, _) = self.array_info(name)?;
        let addr = self.symbols.alloc_var(ValueWidth::U24.bytes());
        self.emit_array_element_address(name, index)?;
        self.emit_store_hl(addr);

        let width = scalar_var(0, element_size).width()?;
        if op != AssignOp::Set {
            let current = self.symbols.alloc_var(element_size);
            self.emit_load_hl(addr);
            self.emit_load_pointed_width_into(current);
            let stored = self.symbols.alloc_var(element_size);
            self.emit_assignment_value(current, op, value)?;
            self.emit_store_width(stored);
            self.emit_load_hl(addr);
            self.emit_store_var_to_pointed_width(stored);
            return Ok(());
        }

        if op == AssignOp::Set {
            let ty = self.array_element_type(name)?;
            self.validate_expr_assignable_to_type(value, &ty)?;
        }
        let stored = self.symbols.alloc_var(element_size);
        self.emit_expr_to_width(value, width)?;
        self.emit_store_width(stored);
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

    fn value_for_type(&self, value: i64, ty: &Type, width: ValueWidth) -> Result<u32, Diagnostic> {
        let resolved = self.symbols.resolved_type(ty)?;
        self.symbols.validate_value_for_type(value, &resolved)?;
        let bits = u32::from(width.bytes()) * 8;
        let mask = (1_i128 << bits) - 1;
        Ok(((value as i128) & mask) as u32)
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
            Expr::AddressOfIndex { name, .. } => {
                Ok(Type::Ptr(Box::new(self.array_element_type(name)?)))
            }
            Expr::AddressOfField { base, field } => {
                Ok(Type::Ptr(Box::new(self.field_type(base, field)?)))
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
            Expr::Call { path, .. } => self
                .symbols
                .functions
                .get(&path_text(path))
                .and_then(|sig| sig.return_type.clone())
                .ok_or_else(|| Diagnostic::new(format!("unknown function `{}`", path_text(path)))),
            Expr::Unary { expr, op } => match op {
                UnaryOp::Not => Ok(Type::Named("bool".to_owned())),
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
            Expr::AddressOfIndex { .. } => Ok(ValueWidth::U24),
            Expr::AddressOfField { .. } => Ok(ValueWidth::U24),
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
            Expr::Call { path, .. } => self
                .symbols
                .functions
                .get(&path_text(path))
                .map(|sig| sig.return_width)
                .ok_or_else(|| Diagnostic::new(format!("unknown function `{}`", path_text(path)))),
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

    fn maybe_const_shift_count(&self, expr: &Expr) -> Result<Option<u8>, Diagnostic> {
        match self.symbols.eval_i64(expr) {
            Ok(value) => self.validate_shift_count(value).map(Some),
            Err(_) => Ok(None),
        }
    }

    fn validate_shift_count(&self, value: i64) -> Result<u8, Diagnostic> {
        if !(0..=24).contains(&value) {
            return Err(Diagnostic::new(format!(
                "shift count {value} is outside supported range 0..=24"
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
        if expr_is_untyped_literal(left) || expr_is_untyped_literal(right) {
            return Ok(());
        }

        if matches!(left_type, Type::Ptr(_)) || matches!(right_type, Type::Ptr(_)) {
            return Ok(());
        }

        if type_is_signed(&left_type) != type_is_signed(&right_type) {
            return Err(Diagnostic::new("signed/unsigned mix without cast"));
        }
        Ok(())
    }

    fn validate_expr_assignable_to_type(
        &self,
        expr: &Expr,
        target: &Type,
    ) -> Result<(), Diagnostic> {
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

    fn validate_expr_arithmetic_compatibility(&self, expr: &Expr) -> Result<(), Diagnostic> {
        match expr {
            Expr::Binary { left, op, right } => {
                self.validate_expr_arithmetic_compatibility(left)?;
                self.validate_expr_arithmetic_compatibility(right)?;
                if matches!(
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
            Expr::Unary { expr, .. } | Expr::Cast { expr, .. } | Expr::Deref(expr) => {
                self.validate_expr_arithmetic_compatibility(expr)?;
            }
            Expr::Index { index, .. } | Expr::AddressOfIndex { index, .. } => {
                self.validate_expr_arithmetic_compatibility(index)?;
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

fn parse_int_text(text: &str) -> Result<i64, Diagnostic> {
    let digits = text
        .trim_end_matches("u8")
        .trim_end_matches("i8")
        .trim_end_matches("u16")
        .trim_end_matches("i16")
        .trim_end_matches("u24")
        .trim_end_matches("i24");
    if let Some(hex) = digits.strip_prefix("0x") {
        i64::from_str_radix(hex, 16)
    } else if let Some(bin) = digits.strip_prefix("0b") {
        i64::from_str_radix(bin, 2)
    } else {
        digits.parse()
    }
    .map_err(|_| Diagnostic::new(format!("invalid integer literal `{text}`")))
}

fn const_shl_or_zero(left: i64, right: i64) -> i64 {
    if !(0..64).contains(&right) {
        0
    } else {
        left.wrapping_shl(right as u32)
    }
}

fn const_shr_or_zero(left: i64, right: i64) -> i64 {
    if !(0..64).contains(&right) {
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

fn scalar_var(addr: u32, size: u8) -> Variable {
    Variable {
        addr,
        size: size as u32,
        element_size: None,
        len: None,
    }
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

fn has_attr(function: &Function, attr: &str) -> bool {
    function.attrs.iter().any(|candidate| candidate == attr)
}

fn block_guarantees_value_return(stmts: &[Stmt]) -> bool {
    stmts.iter().any(stmt_guarantees_value_return)
}

fn stmt_guarantees_value_return(stmt: &Stmt) -> bool {
    match stmt {
        Stmt::Return(Some(_)) => true,
        Stmt::If {
            then_body,
            else_body,
            ..
        } if !else_body.is_empty() => {
            block_guarantees_value_return(then_body) && block_guarantees_value_return(else_body)
        }
        _ => false,
    }
}

fn type_display(ty: &Type) -> String {
    match ty {
        Type::Named(name) => name.clone(),
        Type::Ptr(inner) => format!("ptr<{}>", type_display(inner)),
        Type::Array { element, len } => format!("[{}; {len}]", type_display(element)),
    }
}

fn type_is_signed(ty: &Type) -> bool {
    matches!(ty, Type::Named(name) if matches!(name.as_str(), "i8" | "i16" | "i24"))
}

fn type_is_bool(ty: &Type) -> bool {
    matches!(ty, Type::Named(name) if name == "bool")
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

fn validate_inline_asm_clobbers(clobbers: &[String], lines: &[String]) -> Result<(), Diagnostic> {
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
    }
    Ok(())
}

fn asm_clobbers_include(clobbers: &[String], name: &str) -> bool {
    clobbers.iter().any(|clobber| clobber == name)
}

fn asm_line_uses_ports(line: &str) -> bool {
    asm_line_mentions_word(line, "out")
        || asm_line_mentions_word(line, "out0")
        || asm_line_mentions_word(line, "in")
        || asm_line_mentions_word(line, "in0")
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

fn sdk_constants() -> HashMap<String, i64> {
    HashMap::from([
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
        ("AUDIO_SUBMIT_BUFFER".to_owned(), 1),
        ("AUDIO_STOP".to_owned(), 2),
    ])
}

fn sdk_ports() -> HashMap<String, u8> {
    HashMap::from([
        ("PAD1_LO".to_owned(), 0x01),
        ("PAD1_HI".to_owned(), 0x02),
        ("VIDEO_CMD".to_owned(), 0x09),
        ("AUDIO_CMD".to_owned(), 0x0A),
        ("DEBUG_CHAR".to_owned(), 0x0C),
        ("TEST_RESULT".to_owned(), 0x0D),
        ("TEST_HALT".to_owned(), 0x0E),
    ])
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use crate::{compile::load_program, parser::parse_program, vm::run_assembly_test};

    use super::*;

    #[test]
    fn emits_test_pass_ports() {
        let program = parse_program(Path::new("game.ezra"), "fn main() { test.pass() }").unwrap();
        let asm = emit_ez80_assembly(&program).unwrap();

        assert!(asm.contains("out0 (0Dh), a"));
        assert!(asm.contains("out0 (0Eh), a"));
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

        assert_eq!(error.message, "parameter `value` shadows an existing name");
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
        ];

        for source in cases {
            let program = parse_program(Path::new("game.ezra"), source).unwrap();
            let error = emit_ez80_assembly(&program).unwrap_err();

            assert_eq!(error.message, "signed/unsigned mix without cast");
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
                "integer-to-pointer casts require u24",
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
                "pointer-to-integer casts produce u24",
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
        let source = r#"
            fn answer() -> u8 {
                let value: u8 = 1
            }

            fn main() { test.pass() }
        "#;
        let program = parse_program(Path::new("game.ezra"), source).unwrap();
        let error = emit_ez80_assembly(&program).unwrap_err();

        assert_eq!(error.message, "missing return value in function `answer`");
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
                return base + count
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
        let source = r#"
            fn add_four(a: u8, b: u8, c: u8, d: u8) -> u8 {
                return a + b + c + d
            }

            fn wide_third(a: u24, b: u8, c: u24) -> u24 {
                return a + b + c
            }

            fn main() {
                test.assert_eq_u8(add_four(1, 2, 3, 4), 10, 1)
                test.assert_eq_u24(wide_third(0x000100, 5, 0x000020), 0x000125, 2)
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
    fn emits_and_runs_inline_asm_reg8_and_imm_placeholders() {
        let source = r#"
            const DEBUG_PORT: u8 = 0x0C

            fn main() {
                let ch: u8 = 0x43
                asm volatile(in DEBUG_PORT: u8 as imm, in ch: u8 as reg8, clobber ports) {
                    "out0 ({DEBUG_PORT}), {ch}"
                }
                test.pass()
            }
        "#;
        let program = parse_program(Path::new("game.ezra"), source).unwrap();
        let asm = emit_ez80_assembly(&program).unwrap();
        let run = run_assembly_test(&asm, 4_000).unwrap();

        assert!(asm.contains("    out0 (0Ch), a"), "{asm}");
        assert!(run.halted, "{asm}");
        assert_eq!(run.result_code, 0, "{asm}");
        assert_eq!(run.debug_output, b"C", "{asm}");
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
    fn rejects_unknown_inline_asm_operand_placeholder() {
        let source = r#"
            fn main() {
                asm volatile {
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
                test.assert_eq_u8(pair.lo, 3, 1)
                test.assert_eq_u8(pair.hi, 4, 2)
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
            "pub global bytes: [u8; 3] = [1, 2, 3]\n",
        )
        .unwrap();
        std::fs::write(
            &main_path,
            r#"
            import lib.state
            fn main() {
                test.assert_eq_u8(state.bytes[1], 2, 1)
                state.bytes[2] = state.bytes[1] + 5
                test.assert_eq_u8(bytes[2], 7, 2)
                let ptr: ptr<u8> = &state.bytes[0]
                test.assert_eq_u8(*(ptr + 2), 7, 3)
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
        assert_eq!(run.debug_output, b"P", "{asm}");
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
        ];

        for (source, expected) in cases {
            let program = parse_program(Path::new("game.ezra"), source).unwrap();
            let error = emit_ez80_assembly(&program).unwrap_err();

            assert_eq!(error.message, expected);
        }
    }

    #[test]
    fn emits_and_runs_naked_asm_functions_without_epilogue() {
        let source = r#"
            naked fn raw_debug() {
                asm volatile(clobber ports) {
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
        assert!(irq.contains("    pop af"), "{asm}");
        assert!(irq.contains("    reti"), "{asm}");
        assert!(run.halted, "{asm}");
        assert_eq!(run.result_code, 0, "{asm}");
        assert_eq!(run.debug_output, b"I", "{asm}");
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
            extern asm fn raw_status(port: u8) -> u8

            fn main() {
                let value: u8 = raw_status(0x17)
                debug.char(value)
                test.pass()
            }
        "#;
        let program = parse_program(Path::new("game.ezra"), source).unwrap();
        let asm = emit_ez80_assembly(&program).unwrap();

        assert!(asm.contains("    call _raw_status"), "{asm}");
        assert!(!asm.contains("_raw_status:"), "{asm}");
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
                test.assert_eq_u8(!0, 1, 3)
                test.assert_eq_u8(!a, 0, 4)

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
    fn emits_and_runs_constant_shift_ops() {
        let expected_u8_assign = 0x12u8.wrapping_shl(2) >> 1;
        let expected_u8_expr = 0x81u8 >> 3;
        let expected_u16_expr = 0x1234u16.wrapping_shl(3) >> 2;
        let expected_u16_assign = 0x00F0u16.wrapping_shl(4) >> 3;
        let expected_u24_expr = ((0x010203u32 << 4) & 0x00FF_FFFF) >> 3;
        let expected_u24_assign = ((0x000F00u32 << 5) & 0x00FF_FFFF) >> 2;
        let source = format!(
            r#"
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
        let source = format!(
            r#"
            alias subpx = i24

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

                let c: i16 = -300
                let d: i16 = 7
                test.assert_eq_u16(div16(c, d), 0x{expected_i16_div:04X}, 5)
                test.assert_eq_u16(mod16(c, d), 0x{expected_i16_mod:04X}, 6)

                let e: subpx = -0x012345
                let f: subpx = 17
                test.assert_eq_u24(div24(e, f), 0x{expected_i24_div:06X}, 7)
                test.assert_eq_u24(mod24(e, f), 0x{expected_i24_mod:06X}, 8)
                test.pass()
            }}
            "#
        );
        let program = parse_program(Path::new("game.ezra"), &source).unwrap();
        let asm = emit_ez80_assembly(&program).unwrap();
        let run = run_assembly_test(&asm, 300_000).unwrap();

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

            fn main() {
                let wide: u16 = cast<u16>(0x12)
                let narrow: u8 = cast<u8>(0x1234)
                let assigned: u8 = 0
                assigned = cast<u8>(0x01FE)
                test.assert_eq_u16(wide, 0x0012, 1)
                test.assert_eq_u8(narrow, 0x34, 2)
                test.assert_eq_u8(assigned, 0xFE, 3)
                test.assert_eq_u8(low_byte(0xABCD), 0xCD, 4)
                test.assert_eq_u16(widen(0x7A), 0x007A, 5)
                test.assert_eq_u16(WIDE, 0x0112, 6)
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

            fn main() {
                let p: ptr<u8> = &bytes[0];
                *p = 0x12;
                *(p + 1) = 0x34;
                test.assert_eq_u8(*p, 0x12, 1);
                test.assert_eq_u8(*(p + 1), 0x34, 2);

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
                let b: ptr<u8> = &bytes[0];
                *(b + 2) = 0x7A;
                test.assert_eq_u8(bytes[2], 0x7A, 1);

                let w: ptr<u16> = &words[0];
                *(w + 2) = 0x4567;
                test.assert_eq_u16(words[2], 0x4567, 2);
                *(w + 2 - 1) = 0x1234;
                test.assert_eq_u16(words[1], 0x1234, 3);

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

            global cell: Cell = Cell { value: 0x010203, flags: 0x44 }

            fn main() {
                let p: ptr<Cell> = &cell
                let q: ptr<Cell> = p + 2
                let r: ptr<Cell> = q - 1
                test.assert_eq_u24(cast<u24>(q), cast<u24>(p) + 8, 1)
                test.assert_eq_u24(cast<u24>(r), cast<u24>(p) + 4, 2)
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

                test.assert_eq_u24(title_text.len, 2, 5);
                test.assert_eq_u8(*(title_text.ptr + 0), 'H', 6);
                test.assert_eq_u8(*(title_text.ptr + 1), 'I', 7);

                test.assert_eq_u24(title_cstr.len, 3, 8);
                test.assert_eq_u8(*(title_cstr.ptr + 0), 'O', 9);
                test.assert_eq_u8(*(title_cstr.ptr + 1), 'K', 10);
                test.assert_eq_u8(*(title_cstr.ptr + 2), 0, 11);

                test.assert_eq_u24(blank.len, 4, 12);
                test.assert_eq_u8(*(blank.ptr + 3), 0x7E, 13);
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
    fn emits_and_runs_generic_mmio_peek_poke_examples() {
        let source = r#"
            volatile mmio SCRATCH: ptr<u8> = 0x040120
            volatile mmio TI_LCD_BUFFER: ptr<u8> = 0x080000
            volatile mmio AGON_VDP_BUFFER: ptr<u8> = 0x0C0000

            fn ti_write(value: u8) {
                mem.poke8(TI_LCD_BUFFER, value)
            }

            fn agon_write(value: u8) {
                mem.poke8(AGON_VDP_BUFFER, value)
            }

            fn main() {
                let ptr: ptr<u8> = cast<ptr<u8>>(0x040121)
                mem.poke8(SCRATCH, 0x5A)
                mem.poke8(ptr, mem.peek8(SCRATCH) + 1)
                ti_write(mem.peek8(ptr))
                agon_write(0xC3)
                test.assert_eq_u8(mem.peek8(SCRATCH), 0x5A, 1)
                test.assert_eq_u8(mem.peek8(ptr), 0x5B, 2)
                test.assert_eq_u8(mem.peek8(TI_LCD_BUFFER), 0x5B, 3)
                test.assert_eq_u8(mem.peek8(AGON_VDP_BUFFER), 0xC3, 4)
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
}
