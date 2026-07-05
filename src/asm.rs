use std::{collections::HashMap, fs, path::Path};

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
        return_stack: Vec::new(),
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
            symbols.functions.insert(
                name.clone(),
                FunctionSig {
                    arity: params.len(),
                    params: params
                        .iter()
                        .map(|param| symbols.type_width(&param.ty))
                        .collect::<Result<Vec<_>, _>>()?,
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
                    let value = symbols.eval_i64(&decl.value)?;
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
                    let bytes = symbols.embed_bytes(&decl.source, &program.source_path)?;
                    symbols.align_next_addr(align as u32);
                    let variable = symbols.alloc_array(ValueWidth::U8.bytes(), bytes.len() as u32);
                    symbols.register_embed_properties(&decl.name, variable, bytes.len() as u32);
                    symbols
                        .embeds
                        .insert(decl.name.clone(), EmbedObject { variable, bytes });
                }
                Declaration::Global(decl) => {
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
    return_stack: Vec<ValueWidth>,
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
                self.emit_expr_to_width(&decl.value, variable.width()?)?;
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
        let naked = function.attrs.iter().any(|attr| attr == "naked");
        if naked {
            for stmt in &function.body {
                if !matches!(stmt, Stmt::Asm { .. }) {
                    return Err(Diagnostic::new(format!(
                        "naked function `{}` may contain only asm blocks",
                        function.name
                    )));
                }
            }
        }
        self.line(&format!("_{}:", function.name));
        self.scopes.push(HashMap::new());
        self.scope_types.push(HashMap::new());
        self.return_stack.push(
            function
                .return_type
                .as_ref()
                .map(|ty| self.symbols.type_width(ty))
                .transpose()?
                .unwrap_or(ValueWidth::U8),
        );
        if !naked {
            self.bind_params(function)?;
        }
        for stmt in &function.body {
            self.emit_stmt(stmt)?;
        }
        self.return_stack.pop();
        self.scope_types.pop();
        self.scopes.pop();
        if naked {
            return Ok(());
        }
        if function.name == "main" {
            self.line("    jp __ezra_exit");
        } else {
            self.line("    ret");
        }
        Ok(())
    }

    fn bind_params(&mut self, function: &Function) -> Result<(), Diagnostic> {
        if function.params.len() > 3 {
            return Err(Diagnostic::new(format!(
                "function `{}` has {} parameters; current codegen supports at most 3 register parameters",
                function.name,
                function.params.len()
            )));
        }

        for (index, param) in function.params.iter().enumerate() {
            let width = self.symbols.type_width(&param.ty)?;
            let variable = self.symbols.alloc_var(width.bytes());
            self.current_scope_mut()
                .insert(param.name.clone(), variable);
            self.current_scope_types_mut()
                .insert(param.name.clone(), param.ty.clone());
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
                let variable = self.symbols.alloc_storage(ty)?;
                self.current_scope_mut().insert(name.clone(), variable);
                self.current_scope_types_mut()
                    .insert(name.clone(), ty.clone());
                if variable.element_size.is_some() {
                    self.emit_array_initializer(variable, value)?;
                } else if self.is_struct_type(ty)? {
                    self.emit_struct_initializer(variable, ty, value)?;
                } else {
                    self.emit_expr_to_width(value, variable.width()?)?;
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
            Stmt::Return(None) => self.line("    ret"),
            Stmt::Return(Some(expr)) => {
                self.emit_expr_to_width(expr, self.current_return_width())?;
                self.line("    ret");
            }
            Stmt::Asm { volatile, lines } => {
                if *volatile {
                    self.line("    ; asm volatile");
                } else {
                    self.line("    ; asm");
                }
                for line in lines {
                    self.line(&format!("    {line}"));
                }
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
                self.emit_shift_a(BinaryOp::Shl, self.const_shift_count(value)?)?;
            }
            AssignOp::Shr => {
                self.emit_load_a(variable);
                self.emit_shift_a(BinaryOp::Shr, self.const_shift_count(value)?)?;
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
                self.emit_assignment_value(variable, op, value)?;
                self.emit_store_width(variable);
            }
            Place::Index { name, index } => {
                self.emit_index_assignment(name, index, op, value)?;
            }
            Place::Field { base, field } => {
                let variable = self.field_variable(base, field)?;
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
        let count = self.const_shift_count(value)?;
        let temp = self.symbols.alloc_var(variable.width()?.bytes());
        self.emit_load_width(variable);
        self.emit_store_width(temp);
        self.emit_shift_memory(temp, op, count)?;
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
            path if path.contains('.') => {
                self.line(&format!("    call _{}", path.replace('.', "_")))
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
        if args.len() > 3 {
            return Err(Diagnostic::new(format!(
                "function `{name}` has {} arguments; current codegen supports at most 3 register arguments",
                args.len()
            )));
        }

        let mut temps = Vec::with_capacity(args.len());
        for (index, arg) in args.iter().enumerate() {
            let width = sig.params[index];
            let temp = self.symbols.alloc_var(width.bytes());
            self.emit_expr_to_width(arg, width)?;
            self.emit_store_width(temp);
            temps.push(temp);
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
        self.line(&format!("    call _{name}"));
        Ok(())
    }

    fn emit_expr_to_width(&mut self, expr: &Expr, width: ValueWidth) -> Result<(), Diagnostic> {
        match width {
            ValueWidth::U8 => self.emit_expr_to_a(expr),
            ValueWidth::U16 | ValueWidth::U24 => self.emit_expr_to_hl(expr, width),
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
            Expr::Cast { expr, .. } => match self.value_for_width(expr, width) {
                Ok(value) => self.line(&format!("    ld hl, {:06X}h", value)),
                Err(_) => self.emit_expr_to_width(expr, width)?,
            },
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
                    let count = self.const_shift_count(right)?;
                    let temp = self.symbols.alloc_var(width.bytes());
                    self.emit_expr_to_hl(left, width)?;
                    self.emit_store_width(temp);
                    self.emit_shift_memory(temp, *op, count)?;
                    self.emit_load_width(temp);
                }
                BinaryOp::Div | BinaryOp::Mod => {
                    self.emit_div_mod_to_width(left, right, *op, width)?;
                    return Ok(());
                }
                _ => {
                    return Err(Diagnostic::new(format!(
                        "binary operator `{op:?}` is not implemented in wide codegen yet"
                    )));
                }
            },
            Expr::Call { path, args } if path.len() == 1 => {
                self.emit_user_call(&path[0], args)?;
            }
            Expr::Array(_) | Expr::StructInit { .. } | Expr::In(_) | Expr::Call { .. } => {
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
            2 => self.line("    add hl, hl"),
            3 => {
                self.line("    push hl");
                self.line("    add hl, hl");
                self.line("    pop bc");
                self.line("    add hl, bc");
            }
            _ => {
                return Err(Diagnostic::new(format!(
                    "pointer arithmetic scale {scale} is not implemented yet"
                )));
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
            Expr::Cast { expr, .. } => match self.u8(expr) {
                Ok(value) => self.line(&format!("    ld a, {:02X}h", value)),
                Err(_) => self.emit_expr_to_a(expr)?,
            },
            Expr::Unary { op, expr } => self.emit_unary_to_a(*op, expr)?,
            Expr::Binary { left, op, right } => self.emit_binary_expr(left, *op, right)?,
            Expr::Call { path, args }
                if matches!(path_text(path).as_str(), "mem.peek8" | "ezra.mem.peek8") =>
            {
                self.emit_mem_peek8(args)?;
            }
            Expr::Call { path, args } if path.len() == 1 => {
                self.emit_user_call(&path[0], args)?;
            }
            Expr::AddressOfIndex { .. }
            | Expr::AddressOfField { .. }
            | Expr::AddressOf(_)
            | Expr::Array(_)
            | Expr::StructInit { .. }
            | Expr::Call { .. }
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
        let width = match self.symbols.resolved_type(&self.expr_type(ptr)?)? {
            Type::Ptr(inner) => self.symbols.type_width(&inner)?,
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
            let count = self.const_shift_count(right)?;
            self.emit_expr_to_a(left)?;
            self.emit_shift_a(op, count)?;
            return Ok(());
        }
        if matches!(op, BinaryOp::Div | BinaryOp::Mod) {
            self.emit_u8_div_mod(left, right, op)?;
            return Ok(());
        }
        if op == BinaryOp::Mul {
            self.emit_mul_to_width(left, right, ValueWidth::U8)?;
            return Ok(());
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
        if variable.element_size.is_some() {
            return Err(Diagnostic::new(format!(
                "array `{name}` does not decay to a pointer; use `&{name}[0]`"
            )));
        }
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
                match self.symbols.resolved_type(ty)? {
                    Type::Array { .. } => Err(Diagnostic::new(format!(
                        "array `{name}` does not decay to a pointer; use `&{name}[0]`"
                    ))),
                    scalar => Ok(Type::Ptr(Box::new(scalar))),
                }
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
            Expr::Call { path, .. } if path.len() == 1 => self
                .symbols
                .functions
                .get(&path[0])
                .and_then(|sig| sig.return_type.clone())
                .ok_or_else(|| Diagnostic::new(format!("unknown function `{}`", path[0]))),
            Expr::Call { path, .. }
                if matches!(path_text(path).as_str(), "mem.peek8" | "ezra.mem.peek8") =>
            {
                Ok(Type::Named("u8".to_owned()))
            }
            Expr::Call { .. } => Ok(Type::Named("u8".to_owned())),
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
            Expr::Call { path, .. } if path.len() == 1 => self
                .symbols
                .functions
                .get(&path[0])
                .map(|sig| sig.return_width)
                .ok_or_else(|| Diagnostic::new(format!("unknown function `{}`", path[0]))),
            Expr::Call { .. } => Ok(ValueWidth::U8),
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

    fn const_shift_count(&self, expr: &Expr) -> Result<u8, Diagnostic> {
        let value = self.symbols.eval_i64(expr)?;
        if !(0..=24).contains(&value) {
            return Err(Diagnostic::new(format!(
                "shift count {value} is outside supported range 0..=24"
            )));
        }
        Ok(value as u8)
    }

    fn current_return_width(&self) -> ValueWidth {
        *self
            .return_stack
            .last()
            .expect("function return width exists during emission")
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

fn scalar_var(addr: u32, size: u8) -> Variable {
    Variable {
        addr,
        size: size as u32,
        element_size: None,
        len: None,
    }
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

    use crate::{parser::parse_program, vm::run_assembly_test};

    use super::*;

    #[test]
    fn emits_test_pass_ports() {
        let program = parse_program(Path::new("game.ezra"), "fn main() { test.pass() }").unwrap();
        let asm = emit_ez80_assembly(&program).unwrap();

        assert!(asm.contains("out0 (0Dh), a"));
        assert!(asm.contains("out0 (0Eh), a"));
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
                asm volatile {
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
        assert!(asm.contains("    ld a, 0x41"));
        assert!(run.halted, "{asm}");
        assert_eq!(run.result_code, 0, "{asm}");
        assert_eq!(run.debug_output, b"A", "{asm}");
    }

    #[test]
    fn emits_and_runs_naked_asm_functions_without_epilogue() {
        let source = r#"
            naked fn raw_debug() {
                asm volatile {
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
