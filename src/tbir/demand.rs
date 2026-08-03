use crate::{
    ast::{
        AccessPath, AccessSegment, BinaryOp, Declaration, Expr, Function, Place, Program, Stmt,
        Type, UnaryOp,
    },
    compat::prelude::*,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct Demand {
    bits: u32,
    bytes: u8,
}

impl Demand {
    fn from_bits(bits: u32) -> Self {
        Self {
            bits,
            bytes: bytes_for_bits(bits),
        }
    }

    fn bits(self) -> u32 {
        self.bits
    }

    fn bytes(self) -> u8 {
        self.bytes
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
struct Effects {
    calls: bool,
    memory_reads: bool,
    port_reads: bool,
    address_taken: bool,
    unknown_values: bool,
}

impl Effects {
    fn is_pure(self) -> bool {
        !self.calls
            && !self.memory_reads
            && !self.port_reads
            && !self.address_taken
            && !self.unknown_values
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct DemandReport {
    demanded_bits: u32,
    demanded_bytes: u8,
    input_demands: Vec<Demand>,
    effects: Effects,
}

impl DemandReport {
    #[cfg_attr(not(test), allow(dead_code))]
    fn demanded(&self) -> Demand {
        Demand::from_bits(self.demanded_bits)
    }
}

fn bytes_for_bits(bits: u32) -> u8 {
    let mut bytes = 0;
    let mut byte = 0;
    while byte < 4 {
        if bits & (0xFF << (byte * 8)) != 0 {
            bytes |= 1 << byte;
        }
        byte += 1;
    }
    bytes
}

fn width_mask(bits: u8) -> u32 {
    if bits >= 32 {
        u32::MAX
    } else if bits == 0 {
        0
    } else {
        (1u32 << bits) - 1
    }
}

fn low_prefix(bits: u32) -> u32 {
    if bits == 0 {
        0
    } else {
        let highest = 31 - bits.leading_zeros();
        width_mask((highest + 1) as u8)
    }
}

fn integer_width_bits(ty: &Type) -> Option<u8> {
    match ty {
        Type::Named(name) if matches!(name.as_str(), "u8" | "i8") => Some(8),
        Type::Named(name) if matches!(name.as_str(), "u16" | "i16") => Some(16),
        Type::Named(name) if matches!(name.as_str(), "u24" | "i24") => Some(24),
        Type::Named(name) if matches!(name.as_str(), "u32" | "i32") => Some(32),
        _ => None,
    }
}

fn is_unsigned_integer(ty: &Type) -> bool {
    matches!(
        ty,
        Type::Named(name) if matches!(name.as_str(), "u8" | "u16" | "u24" | "u32")
    )
}

fn unsigned_narrowing_demand(source: &Type, target: &Type) -> Option<Demand> {
    let source_bits = integer_width_bits(source)?;
    let target_bits = integer_width_bits(target)?;
    if !is_unsigned_integer(source) || !is_unsigned_integer(target) || target_bits >= source_bits {
        return None;
    }
    Some(Demand::from_bits(width_mask(target_bits)))
}

fn literal_mask(expr: &Expr) -> Option<u32> {
    match expr {
        Expr::Int(value) if *value >= 0 => Some(*value as u32),
        Expr::TypedInt(value, ty) if *value >= 0 => {
            integer_width_bits(ty).map(|bits| *value as u32 & width_mask(bits))
        }
        _ => None,
    }
}

fn demand_for_expr(expr: &Expr, demanded_bits: u32) -> DemandReport {
    let demanded_bits = match expr {
        Expr::Cast { ty, .. } => {
            integer_width_bits(ty).map_or(demanded_bits, |width| demanded_bits & width_mask(width))
        }
        _ => demanded_bits,
    };
    let demanded = Demand::from_bits(demanded_bits);
    let mut input_demands = Vec::new();
    collect_demands(expr, demanded, &mut input_demands);
    DemandReport {
        demanded_bits: demanded.bits(),
        demanded_bytes: demanded.bytes(),
        input_demands,
        effects: expression_effects(expr),
    }
}

fn collect_demands(expr: &Expr, demanded: Demand, inputs: &mut Vec<Demand>) {
    match expr {
        Expr::Cast { ty, expr } => {
            let bits = integer_width_bits(ty)
                .map(|width| demanded.bits() & width_mask(width))
                .unwrap_or_else(|| demanded.bits());
            collect_demands(expr, Demand::from_bits(bits), inputs);
        }
        Expr::Unary { op, expr } => {
            let bits = match op {
                UnaryOp::Neg => low_prefix(demanded.bits()),
                UnaryOp::BitNot | UnaryOp::Not => demanded.bits(),
            };
            collect_demands(expr, Demand::from_bits(bits), inputs);
        }
        Expr::Binary { left, op, right } => {
            let operand_bits = match op {
                BinaryOp::Add | BinaryOp::Sub | BinaryOp::Mul => low_prefix(demanded.bits()),
                BinaryOp::BitAnd => {
                    literal_mask(right).map_or(demanded.bits(), |mask| demanded.bits() & mask)
                }
                BinaryOp::Shl | BinaryOp::Shr => {
                    if let Some(shift) = literal_mask(right) {
                        if shift >= 32 {
                            0
                        } else if *op == BinaryOp::Shl {
                            demanded.bits() >> shift
                        } else {
                            demanded.bits() << shift
                        }
                    } else {
                        u32::MAX
                    }
                }
                BinaryOp::BitOr | BinaryOp::BitXor => demanded.bits(),
                BinaryOp::Div
                | BinaryOp::Mod
                | BinaryOp::Lt
                | BinaryOp::Le
                | BinaryOp::Gt
                | BinaryOp::Ge
                | BinaryOp::Eq
                | BinaryOp::Ne
                | BinaryOp::And
                | BinaryOp::Or => {
                    if demanded.bits() == 0 {
                        0
                    } else {
                        u32::MAX
                    }
                }
            };
            collect_demands(left, Demand::from_bits(operand_bits), inputs);
            collect_demands(right, Demand::from_bits(operand_bits), inputs);
        }
        Expr::Deref(pointer) => {
            inputs.push(demanded);
            collect_demands(pointer, Demand::from_bits(u32::MAX), inputs);
        }
        Expr::Index { index, .. } | Expr::AddressOfIndex { index, .. } => {
            inputs.push(demanded);
            collect_demands(index, Demand::from_bits(u32::MAX), inputs);
        }
        Expr::Access(path) | Expr::AddressOfAccess(path) => {
            inputs.push(demanded);
            for segment in &path.segments {
                if let AccessSegment::Index(index) = segment {
                    collect_demands(index, Demand::from_bits(u32::MAX), inputs);
                }
            }
        }
        Expr::Array(values) => {
            for value in values {
                collect_demands(value, Demand::from_bits(u32::MAX), inputs);
            }
        }
        Expr::StructInit { fields, .. } => {
            for (_, value) in fields {
                collect_demands(value, Demand::from_bits(u32::MAX), inputs);
            }
        }
        Expr::Call { args, .. } => {
            inputs.push(demanded);
            for arg in args {
                collect_demands(arg, Demand::from_bits(u32::MAX), inputs);
            }
        }
        Expr::BankedPointer { pointer, .. } => {
            inputs.push(demanded);
            collect_demands(pointer, Demand::from_bits(u32::MAX), inputs);
        }
        Expr::AddressOf(_) => inputs.push(demanded),
        Expr::Int(_)
        | Expr::TypedInt(_, _)
        | Expr::Bool(_)
        | Expr::Char(_)
        | Expr::String(_)
        | Expr::Ident(_)
        | Expr::In(_)
        | Expr::Field { .. }
        | Expr::AddressOfField { .. } => inputs.push(demanded),
    }
}

fn expression_effects(expr: &Expr) -> Effects {
    let mut effects = Effects::default();
    collect_expression_effects(expr, &mut effects);
    effects
}

fn collect_expression_effects(expr: &Expr, effects: &mut Effects) {
    match expr {
        Expr::Call { args, .. } => {
            effects.calls = true;
            for arg in args {
                collect_expression_effects(arg, effects);
            }
        }
        Expr::In(_) => effects.port_reads = true,
        Expr::Deref(pointer) => {
            effects.memory_reads = true;
            collect_expression_effects(pointer, effects);
        }
        Expr::Index { index, .. } => {
            effects.memory_reads = true;
            collect_expression_effects(index, effects);
        }
        Expr::Field { .. } | Expr::Access(_) => effects.memory_reads = true,
        Expr::AddressOf(_) | Expr::AddressOfIndex { .. } | Expr::AddressOfField { .. } => {
            effects.address_taken = true;
        }
        Expr::AddressOfAccess(_) => effects.address_taken = true,
        Expr::BankedPointer { pointer, .. }
        | Expr::Unary { expr: pointer, .. }
        | Expr::Cast { expr: pointer, .. } => collect_expression_effects(pointer, effects),
        Expr::Array(values) => {
            for value in values {
                collect_expression_effects(value, effects);
            }
        }
        Expr::StructInit { fields, .. } => {
            for (_, value) in fields {
                collect_expression_effects(value, effects);
            }
        }
        Expr::Binary { left, right, .. } => {
            collect_expression_effects(left, effects);
            collect_expression_effects(right, effects);
        }
        Expr::Int(_)
        | Expr::TypedInt(_, _)
        | Expr::Bool(_)
        | Expr::Char(_)
        | Expr::String(_)
        | Expr::Ident(_) => {}
    }
}

#[derive(Clone, Default)]
struct TypeContext {
    aliases: HashMap<String, Type>,
    named_types: HashMap<String, Type>,
    local_types: HashMap<String, Type>,
    function_returns: HashMap<String, Option<Type>>,
    memory_names: HashSet<String>,
}

impl TypeContext {
    fn from_program(program: &Program) -> Self {
        let mut context = Self::default();
        for declaration in &program.declarations {
            context.collect_declaration(declaration);
        }
        context
    }

    fn collect_declaration(&mut self, declaration: &Declaration) {
        match declaration {
            Declaration::Cfg { declaration, .. } | Declaration::Bank { declaration, .. } => {
                self.collect_declaration(declaration)
            }
            Declaration::Alias(alias) => {
                self.aliases.insert(alias.name.clone(), alias.ty.clone());
            }
            Declaration::Const(value) => {
                self.named_types
                    .insert(value.name.clone(), value.ty.clone());
            }
            Declaration::Port(value) => {
                self.named_types
                    .insert(value.name.clone(), value.ty.clone());
                self.memory_names.insert(value.name.clone());
            }
            Declaration::Mmio(value) => {
                self.named_types
                    .insert(value.name.clone(), value.ty.clone());
                self.memory_names.insert(value.name.clone());
            }
            Declaration::Global(value) => {
                self.named_types
                    .insert(value.name.clone(), value.ty.clone());
                self.memory_names.insert(value.name.clone());
            }
            Declaration::Embed(value) => {
                if let Some(ty) = &value.ty {
                    self.named_types.insert(value.name.clone(), ty.clone());
                }
                self.memory_names.insert(value.name.clone());
            }
            Declaration::Function(function) => {
                self.function_returns
                    .insert(function.name.clone(), function.return_type.clone());
            }
            Declaration::ExternAsmFunction(function) => {
                self.function_returns
                    .insert(function.name.clone(), function.return_type.clone());
            }
            Declaration::Import(_) | Declaration::Struct(_) => {}
        }
    }

    fn resolve_type(&self, ty: &Type) -> Type {
        let mut ty = ty.clone();
        let mut seen = HashSet::new();
        while let Type::Named(name) = &ty {
            let Some(next) = self.aliases.get(name) else {
                break;
            };
            if !seen.insert(name.clone()) {
                break;
            }
            ty = next.clone();
        }
        ty
    }

    fn expr_type(&self, expr: &Expr) -> Option<Type> {
        match expr {
            Expr::Ident(name) => self
                .local_types
                .get(name)
                .or_else(|| self.named_types.get(name))
                .cloned(),
            Expr::Int(value) if *value >= 0 => Some(if *value <= 0xFF {
                Type::Named("u8".to_owned())
            } else if *value <= 0xFFFF {
                Type::Named("u16".to_owned())
            } else {
                Type::Named("u24".to_owned())
            }),
            Expr::Int(_) => None,
            Expr::TypedInt(_, ty) | Expr::Cast { ty, .. } => Some(self.resolve_type(ty)),
            Expr::Bool(_) => Some(Type::Named("bool".to_owned())),
            Expr::Char(_) | Expr::In(_) => Some(Type::Named("u8".to_owned())),
            Expr::String(_) => Some(Type::Ptr(Box::new(Type::Named("u8".to_owned())))),
            Expr::Deref(pointer) => match self.resolve_type(&self.expr_type(pointer)?) {
                Type::Ptr(inner) => Some(*inner),
                _ => None,
            },
            Expr::BankedPointer { pointer, .. } => self.expr_type(pointer),
            Expr::Index { name, .. } => match self.resolve_type(self.named_types.get(name)?) {
                Type::Array { element, .. } => Some(*element),
                _ => None,
            },
            Expr::Call { path, .. } => self
                .function_returns
                .get(path.last()?)
                .and_then(|return_type| return_type.clone()),
            Expr::Unary { op, expr } => match op {
                UnaryOp::Not => Some(Type::Named("bool".to_owned())),
                UnaryOp::Neg | UnaryOp::BitNot => self.expr_type(expr),
            },
            Expr::Binary { left, op, right } => {
                if matches!(
                    op,
                    BinaryOp::Lt
                        | BinaryOp::Le
                        | BinaryOp::Gt
                        | BinaryOp::Ge
                        | BinaryOp::Eq
                        | BinaryOp::Ne
                        | BinaryOp::And
                        | BinaryOp::Or
                ) {
                    return Some(Type::Named("bool".to_owned()));
                }
                let left = self.resolve_type(&self.expr_type(left)?);
                let right = self.resolve_type(&self.expr_type(right)?);
                if matches!(op, BinaryOp::Add | BinaryOp::Sub)
                    && (matches!(left, Type::Ptr(_)) || matches!(right, Type::Ptr(_)))
                {
                    return Some(if matches!(left, Type::Ptr(_)) {
                        left
                    } else {
                        right
                    });
                }
                let left_bits = integer_width_bits(&left)?;
                let right_bits = integer_width_bits(&right)?;
                Some(if left_bits >= right_bits { left } else { right })
            }
            Expr::Field { .. }
            | Expr::AddressOf(_)
            | Expr::AddressOfIndex { .. }
            | Expr::AddressOfField { .. }
            | Expr::Access(_)
            | Expr::AddressOfAccess(_)
            | Expr::Array(_)
            | Expr::StructInit { .. } => None,
        }
    }

    fn effects(&self, expr: &Expr) -> Effects {
        let mut effects = Effects::default();
        self.collect_effects(expr, &mut effects);
        effects
    }

    fn collect_effects(&self, expr: &Expr, effects: &mut Effects) {
        match expr {
            Expr::Ident(name) => {
                if !self.local_types.contains_key(name) {
                    if self.memory_names.contains(name) {
                        effects.memory_reads = true;
                    } else if !self.named_types.contains_key(name) {
                        effects.unknown_values = true;
                    }
                }
            }
            Expr::Call { args, .. } => {
                effects.calls = true;
                for arg in args {
                    self.collect_effects(arg, effects);
                }
            }
            Expr::In(_) => effects.port_reads = true,
            Expr::Deref(pointer) => {
                effects.memory_reads = true;
                self.collect_effects(pointer, effects);
            }
            Expr::Index { index, .. } => {
                effects.memory_reads = true;
                self.collect_effects(index, effects);
            }
            Expr::Field { .. } | Expr::Access(_) => effects.memory_reads = true,
            Expr::AddressOf(_) | Expr::AddressOfIndex { .. } | Expr::AddressOfField { .. } => {
                effects.address_taken = true;
            }
            Expr::AddressOfAccess(_) => effects.address_taken = true,
            Expr::BankedPointer { pointer, .. }
            | Expr::Unary { expr: pointer, .. }
            | Expr::Cast { expr: pointer, .. } => self.collect_effects(pointer, effects),
            Expr::Array(values) => {
                for value in values {
                    self.collect_effects(value, effects);
                }
            }
            Expr::StructInit { fields, .. } => {
                for (_, value) in fields {
                    self.collect_effects(value, effects);
                }
            }
            Expr::Binary { left, right, .. } => {
                self.collect_effects(left, effects);
                self.collect_effects(right, effects);
            }
            Expr::Int(_)
            | Expr::TypedInt(_, _)
            | Expr::Bool(_)
            | Expr::Char(_)
            | Expr::String(_) => {}
        }
    }
}

fn invalid_typed_literal(expr: &Expr) -> bool {
    match expr {
        Expr::TypedInt(value, ty) => {
            let Some(bits) = integer_width_bits(ty) else {
                return false;
            };
            if is_unsigned_integer(ty) {
                *value < 0 || *value as u64 > width_mask(bits) as u64
            } else if matches!(ty, Type::Named(name) if name.starts_with('i')) {
                let min = -(1i64 << (bits - 1));
                let max = (1i64 << (bits - 1)) - 1;
                *value < min || *value > max
            } else {
                false
            }
        }
        Expr::Unary {
            op: UnaryOp::Neg,
            expr,
        } if matches!(expr.as_ref(), Expr::TypedInt(_, _)) => true,
        Expr::Array(values) => values.iter().any(invalid_typed_literal),
        Expr::StructInit { fields, .. } => fields.iter().any(|(_, value)| invalid_typed_literal(value)),
        Expr::Index { index, .. }
        | Expr::AddressOfIndex { index, .. }
        | Expr::Deref(index)
        | Expr::BankedPointer { pointer: index, .. }
        | Expr::Unary { expr: index, .. }
        | Expr::Cast { expr: index, .. } => invalid_typed_literal(index),
        Expr::Access(path) | Expr::AddressOfAccess(path) => path.segments.iter().any(|segment| {
            matches!(segment, AccessSegment::Index(index) if invalid_typed_literal(index))
        }),
        Expr::Binary { left, right, .. } => {
            invalid_typed_literal(left) || invalid_typed_literal(right)
        }
        Expr::Call { args, .. } => args.iter().any(invalid_typed_literal),
        Expr::Int(_)
        | Expr::Bool(_)
        | Expr::Char(_)
        | Expr::String(_)
        | Expr::Ident(_)
        | Expr::In(_)
        | Expr::Field { .. }
        | Expr::AddressOf(_)
        | Expr::AddressOfField { .. } => false,
    }
}

fn is_untyped_nonnegative_literal(expr: &Expr) -> bool {
    matches!(expr, Expr::Int(value) if *value >= 0)
}

fn narrow_arithmetic(target: &Type, expr: &Expr, context: &TypeContext) -> Option<Expr> {
    let target = context.resolve_type(target);
    let target_bits = integer_width_bits(&target)?;
    if !is_unsigned_integer(&target) {
        return None;
    }
    let source_type = context.resolve_type(&context.expr_type(expr)?);
    let source_bits = integer_width_bits(&source_type)?;
    if !is_unsigned_integer(&source_type) || target_bits >= source_bits {
        return None;
    }
    let demand = unsigned_narrowing_demand(&source_type, &target)?;
    let Expr::Binary { left, op, right } = expr else {
        return None;
    };
    if !matches!(op, BinaryOp::Add | BinaryOp::Mul)
        || invalid_typed_literal(expr)
        || !context.effects(expr).is_pure()
    {
        return None;
    }

    let left_type = context.resolve_type(&context.expr_type(left)?);
    let right_type = context.resolve_type(&context.expr_type(right)?);
    let source_operand = |ty: &Type, value: &Expr| {
        is_unsigned_integer(ty)
            && (integer_width_bits(ty) == Some(source_bits)
                || is_untyped_nonnegative_literal(value))
    };
    if !source_operand(&left_type, left) || !source_operand(&right_type, right) {
        return None;
    }

    let report = demand_for_expr(expr, demand.bits());
    if report.effects != Effects::default()
        || report.input_demands.len() != 2
        || report
            .input_demands
            .iter()
            .any(|input| input.bits() != demand.bits())
    {
        return None;
    }

    let narrow_operand = |value: &Expr| -> Option<Expr> {
        let value_type = context.resolve_type(&context.expr_type(value)?);
        let value_bits = integer_width_bits(&value_type)?;
        if !is_unsigned_integer(&value_type) {
            return None;
        }
        if value_bits > target_bits {
            Some(Expr::Cast {
                ty: target.clone(),
                expr: Box::new(value.clone()),
            })
        } else {
            Some(value.clone())
        }
    };

    Some(Expr::Binary {
        left: Box::new(narrow_operand(left)?),
        op: *op,
        right: Box::new(narrow_operand(right)?),
    })
}

fn visit_expr(expr: Expr, context: &TypeContext) -> Expr {
    match expr {
        Expr::Array(values) => Expr::Array(
            values
                .into_iter()
                .map(|value| visit_expr(value, context))
                .collect(),
        ),
        Expr::Index { name, index } => Expr::Index {
            name,
            index: Box::new(visit_expr(*index, context)),
        },
        Expr::AddressOfIndex { name, index } => Expr::AddressOfIndex {
            name,
            index: Box::new(visit_expr(*index, context)),
        },
        Expr::Access(mut path) => {
            visit_access(&mut path, context);
            Expr::Access(path)
        }
        Expr::AddressOfAccess(mut path) => {
            visit_access(&mut path, context);
            Expr::AddressOfAccess(path)
        }
        Expr::StructInit { ty, fields } => Expr::StructInit {
            ty,
            fields: fields
                .into_iter()
                .map(|(name, value)| (name, visit_expr(value, context)))
                .collect(),
        },
        Expr::Deref(value) => Expr::Deref(Box::new(visit_expr(*value, context))),
        Expr::BankedPointer { pointer, bank } => Expr::BankedPointer {
            pointer: Box::new(visit_expr(*pointer, context)),
            bank,
        },
        Expr::Call { path, args } => Expr::Call {
            path,
            args: args
                .into_iter()
                .map(|arg| visit_expr(arg, context))
                .collect(),
        },
        Expr::Unary { op, expr } => Expr::Unary {
            op,
            expr: Box::new(visit_expr(*expr, context)),
        },
        Expr::Binary { left, op, right } => Expr::Binary {
            left: Box::new(visit_expr(*left, context)),
            op,
            right: Box::new(visit_expr(*right, context)),
        },
        Expr::Cast { ty, expr } => {
            let expr = visit_expr(*expr, context);
            let expr = narrow_arithmetic(&ty, &expr, context).unwrap_or(expr);
            Expr::Cast {
                ty,
                expr: Box::new(expr),
            }
        }
        other => other,
    }
}

fn visit_access(path: &mut AccessPath, context: &TypeContext) {
    for segment in &mut path.segments {
        if let AccessSegment::Index(index) = segment {
            *index = Box::new(visit_expr((**index).clone(), context));
        }
    }
}

fn visit_place(place: Place, context: &TypeContext) -> Place {
    match place {
        Place::Index { name, index } => Place::Index {
            name,
            index: Box::new(visit_expr(*index, context)),
        },
        Place::Access(mut path) => {
            visit_access(&mut path, context);
            Place::Access(path)
        }
        Place::Deref(value) => Place::Deref(Box::new(visit_expr(*value, context))),
        Place::Ident(_) | Place::Field { .. } => place,
    }
}

fn visit_stmts(stmts: &mut [Stmt], context: &mut TypeContext) {
    for stmt in stmts {
        match stmt {
            Stmt::Let { name, ty, value } => {
                *value = visit_expr(core::mem::replace(value, Expr::Int(0)), context);
                context.local_types.insert(name.clone(), ty.clone());
            }
            Stmt::Assign { target, value, .. } => {
                *target = visit_place(
                    core::mem::replace(target, Place::Ident(String::new())),
                    context,
                );
                *value = visit_expr(core::mem::replace(value, Expr::Int(0)), context);
            }
            Stmt::If {
                condition,
                then_body,
                else_body,
            } => {
                *condition = visit_expr(core::mem::replace(condition, Expr::Bool(false)), context);
                let mut then_context = context.clone();
                let mut else_context = context.clone();
                visit_stmts(then_body, &mut then_context);
                visit_stmts(else_body, &mut else_context);
            }
            Stmt::While { condition, body } => {
                *condition = visit_expr(core::mem::replace(condition, Expr::Bool(false)), context);
                let mut body_context = context.clone();
                visit_stmts(body, &mut body_context);
            }
            Stmt::Loop { body } => {
                let mut body_context = context.clone();
                visit_stmts(body, &mut body_context);
            }
            Stmt::Return(Some(value)) | Stmt::Expr(value) => {
                *value = visit_expr(core::mem::replace(value, Expr::Int(0)), context);
            }
            Stmt::Out { value, .. } => {
                *value = visit_expr(core::mem::replace(value, Expr::Int(0)), context);
            }
            Stmt::Asm { .. } | Stmt::Return(None) | Stmt::Break | Stmt::Continue => {}
        }
    }
}

fn visit_function(function: &mut Function, base: &TypeContext) {
    let mut context = base.clone();
    for param in &function.params {
        context
            .local_types
            .insert(param.name.clone(), param.ty.clone());
    }
    visit_stmts(&mut function.body, &mut context);
}

fn visit_declarations(declarations: &mut [Declaration], base: &TypeContext) {
    for declaration in declarations {
        match declaration {
            Declaration::Cfg { declaration, .. } | Declaration::Bank { declaration, .. } => {
                visit_declarations(core::slice::from_mut(declaration), base)
            }
            Declaration::Function(function) => visit_function(function, base),
            Declaration::Const(value) => {
                value.value = visit_expr(core::mem::replace(&mut value.value, Expr::Int(0)), base)
            }
            Declaration::Port(value) => {
                value.value = visit_expr(core::mem::replace(&mut value.value, Expr::Int(0)), base)
            }
            Declaration::Mmio(value) => {
                value.value = visit_expr(core::mem::replace(&mut value.value, Expr::Int(0)), base)
            }
            Declaration::Global(value) => {
                value.value = visit_expr(core::mem::replace(&mut value.value, Expr::Int(0)), base)
            }
            Declaration::Embed(_)
            | Declaration::Import(_)
            | Declaration::Alias(_)
            | Declaration::Struct(_)
            | Declaration::ExternAsmFunction(_) => {}
        }
    }
}

pub(crate) fn apply_program(program: &mut Program) {
    let context = TypeContext::from_program(program);
    visit_declarations(&mut program.declarations, &context);
}

#[cfg(test)]
mod tests;
