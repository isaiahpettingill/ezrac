use crate::compat::{SourcePath, prelude::*};

#[cfg(feature = "std")]
use std::{
    fs,
    path::{Path, PathBuf},
};

use crate::{
    ast::{BinaryOp, Declaration, EmbedSource, Expr, Program, Type, UnaryOp},
    diagnostic::Diagnostic,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Storage {
    pub address: u32,
    pub size: u32,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FieldLayout {
    pub offset: u32,
    pub ty: Type,
    pub size: u32,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StructLayout {
    pub size: u32,
    pub fields: HashMap<String, FieldLayout>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FunctionSignature {
    pub params: Vec<Type>,
    pub return_type: Option<Type>,
    pub argument_slots: Vec<Storage>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EmbedObject {
    pub storage: Storage,
    pub bytes: Vec<u8>,
}

#[derive(Clone, Debug)]
pub struct SemanticModel {
    pointer_bytes: u8,
    max_address: u32,
    next_ram: u32,
    next_rodata: u32,
    next_asset: u32,
    aliases: HashMap<String, Type>,
    pub constants: HashMap<String, i64>,
    pub constant_types: HashMap<String, Type>,
    pub structs: HashMap<String, StructLayout>,
    pub globals: HashMap<String, Storage>,
    pub global_types: HashMap<String, Type>,
    pub mmio: HashMap<String, (u32, Type, bool)>,
    pub embeds: HashMap<String, EmbedObject>,
    pub functions: HashMap<String, FunctionSignature>,
    pub strings: HashMap<String, Storage>,
}

impl SemanticModel {
    pub fn from_program(
        program: &Program,
        pointer_width_bits: u16,
        ram_base: u32,
        rodata_base: u32,
        asset_base: u32,
    ) -> Result<Self, Diagnostic> {
        let pointer_bytes = u8::try_from(pointer_width_bits / 8)
            .map_err(|_| Diagnostic::new("invalid target pointer width"))?;
        if !matches!(pointer_bytes, 2 | 3) {
            return Err(Diagnostic::new(format!(
                "unsupported target pointer width {pointer_width_bits}"
            )));
        }
        let max_address = (1u32 << pointer_width_bits.min(24)) - 1;
        let mut model = Self {
            pointer_bytes,
            max_address,
            next_ram: ram_base,
            next_rodata: rodata_base,
            next_asset: asset_base,
            aliases: HashMap::new(),
            constants: HashMap::new(),
            constant_types: HashMap::new(),
            structs: HashMap::new(),
            globals: HashMap::new(),
            global_types: HashMap::new(),
            mmio: HashMap::new(),
            embeds: HashMap::new(),
            functions: HashMap::new(),
            strings: HashMap::new(),
        };
        model.collect(program)?;
        Ok(model)
    }

    pub const fn pointer_bytes(&self) -> u8 {
        self.pointer_bytes
    }

    pub const fn max_address(&self) -> u32 {
        self.max_address
    }

    pub const fn next_ram_address(&self) -> u32 {
        self.next_ram
    }

    pub fn allocate(&mut self, size: u32) -> Result<Storage, Diagnostic> {
        let storage = allocate_from(&mut self.next_ram, size, 1, self.max_address)?;
        Ok(storage)
    }

    pub fn allocate_type(&mut self, ty: &Type) -> Result<Storage, Diagnostic> {
        self.allocate(self.type_size(ty)?)
    }

    pub fn resolved_type(&self, ty: &Type) -> Result<Type, Diagnostic> {
        self.resolve_type(ty, &mut HashSet::new())
    }

    fn resolve_type(&self, ty: &Type, seen: &mut HashSet<String>) -> Result<Type, Diagnostic> {
        match ty {
            Type::Named(name) if self.aliases.contains_key(name) => {
                if !seen.insert(name.clone()) {
                    return Err(Diagnostic::new(format!("cyclic type alias `{name}`")));
                }
                let resolved = self.resolve_type(&self.aliases[name], seen);
                seen.remove(name);
                resolved
            }
            Type::Named(_) => Ok(ty.clone()),
            Type::Ptr(inner) => Ok(Type::Ptr(Box::new(self.resolve_type(inner, seen)?))),
            Type::Array { element, len } => Ok(Type::Array {
                element: Box::new(self.resolve_type(element, seen)?),
                len: len.clone(),
            }),
        }
    }

    pub fn type_width(&self, ty: &Type) -> Result<u8, Diagnostic> {
        match self.resolved_type(ty)? {
            Type::Named(name) if matches!(name.as_str(), "u8" | "i8" | "bool") => Ok(1),
            Type::Named(name) if matches!(name.as_str(), "u16" | "i16") => Ok(2),
            Type::Named(name) if matches!(name.as_str(), "u24" | "i24" | "ptr24") => Ok(3),
            Type::Ptr(_) => Ok(self.pointer_bytes),
            Type::Named(name) if self.structs.contains_key(&name) => Err(Diagnostic::new(format!(
                "struct `{name}` cannot be used as a scalar value"
            ))),
            Type::Array { .. } => Err(Diagnostic::new("array value cannot be used as a scalar")),
            Type::Named(name) => Err(Diagnostic::new(format!("unknown type `{name}`"))),
        }
    }

    pub fn type_size(&self, ty: &Type) -> Result<u32, Diagnostic> {
        match self.resolved_type(ty)? {
            Type::Array { element, len } => {
                let len = self.const_value(&len)?;
                let len = u32::try_from(len)
                    .map_err(|_| Diagnostic::new("array length must be non-negative"))?;
                self.type_size(&element)?
                    .checked_mul(len)
                    .filter(|size| *size <= self.max_address)
                    .ok_or_else(|| Diagnostic::new("array size exceeds target address space"))
            }
            Type::Named(name) if self.structs.contains_key(&name) => Ok(self.structs[&name].size),
            scalar => Ok(u32::from(self.type_width(&scalar)?)),
        }
    }

    pub fn const_value(&self, expr: &Expr) -> Result<i64, Diagnostic> {
        match expr {
            Expr::Int(value) | Expr::TypedInt(value, _) => Ok(*value),
            Expr::Bool(value) => Ok(i64::from(*value)),
            Expr::Char(value) => Ok(i64::from(*value)),
            Expr::Ident(name) => self
                .constants
                .get(name)
                .copied()
                .ok_or_else(|| Diagnostic::new(format!("`{name}` is not a constant"))),
            Expr::Unary { op, expr } => {
                let value = self.const_value(expr)?;
                Ok(match op {
                    UnaryOp::Neg => value.wrapping_neg(),
                    UnaryOp::BitNot => !value,
                    UnaryOp::Not => i64::from(value == 0),
                })
            }
            Expr::Binary { left, op, right } => {
                let left = self.const_value(left)?;
                let right = self.const_value(right)?;
                Ok(eval_binary(left, *op, right))
            }
            Expr::Cast { expr, .. } => self.const_value(expr),
            Expr::AddressOf(name) => self
                .globals
                .get(name)
                .map(|storage| i64::from(storage.address))
                .ok_or_else(|| Diagnostic::new(format!("unknown global `{name}`"))),
            _ => Err(Diagnostic::new("expression is not constant")),
        }
    }

    pub fn field(&self, ty: &Type, name: &str) -> Result<&FieldLayout, Diagnostic> {
        let Type::Named(struct_name) = self.resolved_type(ty)? else {
            return Err(Diagnostic::new("field access requires a struct"));
        };
        self.structs
            .get(&struct_name)
            .and_then(|layout| layout.fields.get(name))
            .ok_or_else(|| Diagnostic::new(format!("unknown field `{name}` on `{struct_name}`")))
    }

    pub fn intern_string(&mut self, value: &str) -> Result<Storage, Diagnostic> {
        if let Some(storage) = self.strings.get(value) {
            return Ok(*storage);
        }
        let size = u32::try_from(value.len() + 1)
            .map_err(|_| Diagnostic::new("string literal is too large"))?;
        let storage = allocate_from(&mut self.next_rodata, size, 1, self.max_address)?;
        self.strings.insert(value.to_owned(), storage);
        Ok(storage)
    }

    fn collect(&mut self, program: &Program) -> Result<(), Diagnostic> {
        let mut names = HashSet::new();
        for declaration in &program.declarations {
            if let Some(name) = declaration_name(declaration)
                && !names.insert(name.to_owned())
            {
                return Err(Diagnostic::new(format!("duplicate declaration `{name}`")));
            }
            if let Declaration::Alias(alias) = declaration {
                self.aliases.insert(alias.name.clone(), alias.ty.clone());
            }
        }
        let mut pending = program
            .declarations
            .iter()
            .filter_map(|declaration| match declaration {
                Declaration::Const(declaration) => Some(declaration),
                _ => None,
            })
            .collect::<Vec<_>>();
        while !pending.is_empty() {
            let before = pending.len();
            pending.retain(|declaration| {
                let Ok(value) = self.const_value(&declaration.value) else {
                    return true;
                };
                self.constants.insert(declaration.name.clone(), value);
                self.constant_types
                    .insert(declaration.name.clone(), declaration.ty.clone());
                false
            });
            if pending.len() == before {
                break;
            }
        }
        for declaration in &program.declarations {
            if let Declaration::Struct(declaration) = declaration {
                let mut offset = 0;
                let mut fields = HashMap::new();
                for field in &declaration.fields {
                    let size = self.type_size(&field.ty)?;
                    if fields
                        .insert(
                            field.name.clone(),
                            FieldLayout {
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
                    offset = offset
                        .checked_add(size)
                        .filter(|value| *value <= self.max_address)
                        .ok_or_else(|| Diagnostic::new("struct exceeds target address space"))?;
                }
                self.structs.insert(
                    declaration.name.clone(),
                    StructLayout {
                        size: offset,
                        fields,
                    },
                );
            }
        }
        for declaration in &program.declarations {
            if let Declaration::Const(declaration) = declaration {
                if self.constants.contains_key(&declaration.name) {
                    continue;
                }
                let value = self.const_value(&declaration.value)?;
                self.constants.insert(declaration.name.clone(), value);
                self.constant_types
                    .insert(declaration.name.clone(), declaration.ty.clone());
            }
        }
        for declaration in &program.declarations {
            match declaration {
                Declaration::Mmio(declaration) => {
                    let address = u32::try_from(self.const_value(&declaration.value)?)
                        .ok()
                        .filter(|address| *address <= self.max_address)
                        .ok_or_else(|| {
                            Diagnostic::new(format!(
                                "mmio `{}` is outside target address space",
                                declaration.name
                            ))
                        })?;
                    self.mmio.insert(
                        declaration.name.clone(),
                        (address, declaration.ty.clone(), declaration.volatile),
                    );
                    self.constants
                        .insert(declaration.name.clone(), i64::from(address));
                    self.constant_types
                        .insert(declaration.name.clone(), declaration.ty.clone());
                }
                Declaration::Global(declaration) => {
                    let storage = self.allocate_type(&declaration.ty)?;
                    self.globals.insert(declaration.name.clone(), storage);
                    self.global_types
                        .insert(declaration.name.clone(), declaration.ty.clone());
                }
                Declaration::Embed(declaration) => {
                    let bytes = embed_bytes(&declaration.source, &program.source_path, self)?;
                    let align = declaration
                        .align
                        .as_ref()
                        .map(|value| self.const_value(value))
                        .transpose()?
                        .unwrap_or(1);
                    let align = u32::try_from(align)
                        .ok()
                        .filter(|value| value.is_power_of_two())
                        .ok_or_else(|| Diagnostic::new("embed alignment must be a power of two"))?;
                    let size = u32::try_from(bytes.len())
                        .map_err(|_| Diagnostic::new("embedded asset is too large"))?;
                    let storage =
                        allocate_from(&mut self.next_asset, size, align, self.max_address)?;
                    self.embeds
                        .insert(declaration.name.clone(), EmbedObject { storage, bytes });
                    for (suffix, value, ty) in [
                        (
                            "ptr",
                            storage.address,
                            Type::Ptr(Box::new(Type::Named("u8".to_owned()))),
                        ),
                        ("len", storage.size, Type::Named("u24".to_owned())),
                        (
                            "end",
                            storage.address + storage.size,
                            Type::Ptr(Box::new(Type::Named("u8".to_owned()))),
                        ),
                    ] {
                        let name = format!("{}.{suffix}", declaration.name);
                        self.constants.insert(name.clone(), i64::from(value));
                        self.constant_types.insert(name, ty);
                    }
                }
                _ => {}
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
            let mut argument_slots = Vec::new();
            for param in params {
                argument_slots.push(self.allocate_type(&param.ty)?);
            }
            self.functions.insert(
                name.clone(),
                FunctionSignature {
                    params: params.iter().map(|param| param.ty.clone()).collect(),
                    return_type: return_type.clone(),
                    argument_slots,
                },
            );
        }
        collect_strings(program, self)?;
        Ok(())
    }
}

fn allocate_from(
    cursor: &mut u32,
    size: u32,
    align: u32,
    max_address: u32,
) -> Result<Storage, Diagnostic> {
    let address = cursor
        .checked_add(align - 1)
        .map(|value| value & !(align - 1))
        .ok_or_else(|| Diagnostic::new("storage address overflow"))?;
    let next = address
        .checked_add(size)
        .filter(|next| *next <= max_address.saturating_add(1))
        .ok_or_else(|| Diagnostic::new("storage exceeds target address space"))?;
    *cursor = next;
    Ok(Storage { address, size })
}

fn declaration_name(declaration: &Declaration) -> Option<&str> {
    match declaration {
        Declaration::Const(value) => Some(&value.name),
        Declaration::Alias(value) => Some(&value.name),
        Declaration::Port(value) => Some(&value.name),
        Declaration::Mmio(value) => Some(&value.name),
        Declaration::Embed(value) => Some(&value.name),
        Declaration::Global(value) => Some(&value.name),
        Declaration::Struct(value) => Some(&value.name),
        Declaration::ExternAsmFunction(value) => Some(&value.name),
        Declaration::Function(value) => Some(&value.name),
        Declaration::Cfg { declaration, .. } => declaration_name(declaration),
        Declaration::Import(_) => None,
    }
}

fn eval_binary(left: i64, op: BinaryOp, right: i64) -> i64 {
    match op {
        BinaryOp::Mul => left.wrapping_mul(right),
        BinaryOp::Div => left.checked_div(right).unwrap_or(0),
        BinaryOp::Mod => left.checked_rem(right).unwrap_or(0),
        BinaryOp::Add => left.wrapping_add(right),
        BinaryOp::Sub => left.wrapping_sub(right),
        BinaryOp::Shl => left.checked_shl(right as u32).unwrap_or(0),
        BinaryOp::Shr => ((left as u64).checked_shr(right as u32).unwrap_or(0)) as i64,
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
    }
}

fn embed_bytes(
    source: &EmbedSource,
    source_path: &SourcePath,
    model: &SemanticModel,
) -> Result<Vec<u8>, Diagnostic> {
    match source {
        EmbedSource::File(file) => read_embed_file(file, source_path),
        EmbedSource::Bytes(values) => values
            .iter()
            .map(|value| {
                u8::try_from(model.const_value(value)?)
                    .map_err(|_| Diagnostic::new("embedded byte is outside u8 range"))
            })
            .collect(),
        EmbedSource::Text(value) => Ok(value.as_bytes().to_vec()),
        EmbedSource::CStr(value) => Ok(value.bytes().chain(core::iter::once(0)).collect()),
        EmbedSource::Repeat { value, len } => {
            let value = u8::try_from(model.const_value(value)?)
                .map_err(|_| Diagnostic::new("embedded byte is outside u8 range"))?;
            let len = usize::try_from(model.const_value(len)?)
                .map_err(|_| Diagnostic::new("embedded repeat length is invalid"))?;
            Ok(vec![value; len])
        }
    }
}

#[cfg(feature = "std")]
fn read_embed_file(file: &str, source_path: &SourcePath) -> Result<Vec<u8>, Diagnostic> {
    let relative = source_path
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join(file);
    let mut candidates = vec![relative];
    if let Some(root) = source_path
        .ancestors()
        .find(|path| path.join("Ezra.toml").is_file())
    {
        candidates.push(root.join(file));
    }
    candidates
        .into_iter()
        .find_map(|path| fs::read(&path).ok().map(|bytes| (path, bytes)))
        .map(|(_, bytes)| bytes)
        .ok_or_else(|| {
            Diagnostic::new(format!(
                "failed to read embedded file `{}`",
                PathBuf::from(file).display()
            ))
        })
}

#[cfg(all(feature = "no-std", not(feature = "std")))]
fn read_embed_file(file: &str, _source_path: &SourcePath) -> Result<Vec<u8>, Diagnostic> {
    Err(Diagnostic::new(format!(
        "embedded file `{file}` is unavailable without a host filesystem"
    )))
}

fn collect_strings(program: &Program, model: &mut SemanticModel) -> Result<(), Diagnostic> {
    fn collect_expr(value: &Expr, output: &mut Vec<String>) {
        match value {
            Expr::String(value) => output.push(value.clone()),
            Expr::Array(values) => values.iter().for_each(|value| collect_expr(value, output)),
            Expr::Index { index, .. }
            | Expr::AddressOfIndex { index, .. }
            | Expr::Deref(index)
            | Expr::Unary { expr: index, .. }
            | Expr::Cast { expr: index, .. } => collect_expr(index, output),
            Expr::StructInit { fields, .. } => fields
                .iter()
                .for_each(|(_, value)| collect_expr(value, output)),
            Expr::Call { args, .. } => args.iter().for_each(|value| collect_expr(value, output)),
            Expr::Binary { left, right, .. } => {
                collect_expr(left, output);
                collect_expr(right, output);
            }
            _ => {}
        }
    }
    fn collect_stmts(body: &[crate::ast::Stmt], output: &mut Vec<String>) {
        for stmt in body {
            match stmt {
                crate::ast::Stmt::Let { value, .. }
                | crate::ast::Stmt::Assign { value, .. }
                | crate::ast::Stmt::Return(Some(value))
                | crate::ast::Stmt::Out { value, .. }
                | crate::ast::Stmt::Expr(value) => collect_expr(value, output),
                crate::ast::Stmt::If {
                    condition,
                    then_body,
                    else_body,
                } => {
                    collect_expr(condition, output);
                    collect_stmts(then_body, output);
                    collect_stmts(else_body, output);
                }
                crate::ast::Stmt::While { condition, body } => {
                    collect_expr(condition, output);
                    collect_stmts(body, output);
                }
                crate::ast::Stmt::Loop { body } => collect_stmts(body, output),
                _ => {}
            }
        }
    }
    let mut values = Vec::new();
    for declaration in &program.declarations {
        match declaration {
            Declaration::Const(value) => collect_expr(&value.value, &mut values),
            Declaration::Global(value) => collect_expr(&value.value, &mut values),
            Declaration::Function(value) => collect_stmts(&value.body, &mut values),
            _ => {}
        }
    }
    for value in values {
        model.intern_string(&value)?;
    }
    Ok(())
}
