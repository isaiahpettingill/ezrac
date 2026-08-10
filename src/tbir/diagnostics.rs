use alloc::boxed::Box;

use crate::{
    ast::{
        AccessPath, AccessSegment, BinaryOp, Declaration, Expr, Place, Program, Stmt, Type, UnaryOp,
    },
    compat::prelude::*,
    diagnostic::Diagnostic,
    intrinsics::{CATALOG, IntrinsicOperation, MemIntrinsic, ResultCount},
    target::{Address24, CpuFamily, memory_model_for_cpu},
};

#[derive(Clone, Debug)]
struct ReturnSignature {
    first: Option<Type>,
    second: Option<Type>,
}

#[derive(Clone, Debug, Default)]
struct IntrinsicValidationContext {
    aliases: HashMap<String, Type>,
    structs: HashMap<String, HashMap<String, Type>>,
    values: HashMap<String, Type>,
    constants: HashMap<String, i64>,
    constant_expressions: HashMap<String, Expr>,
    functions: HashMap<String, ReturnSignature>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum IntrinsicUse {
    Scalar,
    Pair,
    Statement,
}

/// Validates the restricted 0/1/2-result calling convention before target codegen.
///
/// Intrinsics are catalog entries, not declared functions. Validate them before
/// the older declared-function pass so their result metadata is available to
/// scalar expression typing and two-place bindings.
pub fn validate_multi_value_returns(program: &Program) -> Result<(), Diagnostic> {
    let context = collect_intrinsic_context(program);
    validate_intrinsic_declarations(program, &context)?;
    validate_user_multi_value_returns(program)
}

fn collect_intrinsic_context(program: &Program) -> IntrinsicValidationContext {
    fn collect(declaration: &Declaration, context: &mut IntrinsicValidationContext) {
        match declaration {
            Declaration::Alias(alias) => {
                context.aliases.insert(alias.name.clone(), alias.ty.clone());
            }
            Declaration::Struct(structure) => {
                context.structs.insert(
                    structure.name.clone(),
                    structure
                        .fields
                        .iter()
                        .map(|field| (field.name.clone(), field.ty.clone()))
                        .collect(),
                );
            }
            Declaration::Const(constant) => {
                context
                    .values
                    .insert(constant.name.clone(), constant.ty.clone());
                context
                    .constant_expressions
                    .insert(constant.name.clone(), constant.value.clone());
            }
            Declaration::Global(global) => {
                context
                    .values
                    .insert(global.name.clone(), global.ty.clone());
            }
            Declaration::Port(port) => {
                context.values.insert(port.name.clone(), port.ty.clone());
            }
            Declaration::Mmio(mmio) => {
                context.values.insert(mmio.name.clone(), mmio.ty.clone());
            }
            Declaration::Embed(embed) => {
                if let Some(ty) = &embed.ty {
                    context.values.insert(embed.name.clone(), ty.clone());
                }
            }
            Declaration::Function(function) => {
                context.functions.insert(
                    function.name.clone(),
                    ReturnSignature {
                        first: function.return_type.clone(),
                        second: function.second_return_type.clone(),
                    },
                );
            }
            Declaration::ExternAsmFunction(function) => {
                context.functions.insert(
                    function.name.clone(),
                    ReturnSignature {
                        first: function.return_type.clone(),
                        second: function.second_return_type.clone(),
                    },
                );
            }
            Declaration::Cfg { declaration, .. } | Declaration::Bank { declaration, .. } => {
                collect(declaration, context);
            }
            Declaration::Import(_) => {}
        }
    }

    let mut context = IntrinsicValidationContext::default();
    for declaration in &program.declarations {
        collect(declaration, &mut context);
    }

    for _ in 0..context.constant_expressions.len().saturating_add(1) {
        let before = context.constants.len();
        for (name, expression) in &context.constant_expressions {
            if let Some(value) = known_constant(expression, &context.constants) {
                context.constants.insert(name.clone(), value);
            }
        }
        if context.constants.len() == before {
            break;
        }
    }
    context
}

fn resolve_intrinsic_type(ty: &Type, aliases: &HashMap<String, Type>) -> Type {
    fn resolve(ty: &Type, aliases: &HashMap<String, Type>, seen: &mut HashSet<String>) -> Type {
        match ty {
            Type::Named(name) if aliases.contains_key(name) && seen.insert(name.clone()) => {
                let resolved = resolve(&aliases[name], aliases, seen);
                seen.remove(name);
                resolved
            }
            Type::Ptr(inner) => Type::Ptr(Box::new(resolve(inner, aliases, seen))),
            Type::Function {
                params,
                return_type,
            } => Type::Function {
                params: params
                    .iter()
                    .map(|param| resolve(param, aliases, seen))
                    .collect(),
                return_type: return_type
                    .as_ref()
                    .map(|value| Box::new(resolve(value, aliases, seen))),
            },
            Type::Array { element, len } => Type::Array {
                element: Box::new(resolve(element, aliases, seen)),
                len: len.clone(),
            },
            _ => ty.clone(),
        }
    }

    resolve(ty, aliases, &mut HashSet::new())
}

fn integer_literal_type(value: i64) -> Type {
    if (0..=0xFF).contains(&value) {
        Type::Named("u8".to_owned())
    } else if (0..=0xFFFF).contains(&value) {
        Type::Named("u16".to_owned())
    } else {
        Type::Named("u24".to_owned())
    }
}

fn known_constant(expr: &Expr, constants: &HashMap<String, i64>) -> Option<i64> {
    match expr {
        Expr::Int(value) | Expr::TypedInt(value, _) => Some(*value),
        Expr::Bool(value) => Some(i64::from(*value)),
        Expr::Char(value) => Some(i64::from(*value)),
        Expr::Ident(name) => constants.get(name).copied(),
        Expr::Field { base, field } => constants.get(&format!("{base}.{field}")).copied(),
        Expr::Access(path) => access_constant(path, constants),
        Expr::Unary { op, expr } => {
            let value = known_constant(expr, constants)?;
            Some(match op {
                UnaryOp::Neg => value.wrapping_neg(),
                UnaryOp::BitNot => !value,
                UnaryOp::Not => i64::from(value == 0),
            })
        }
        Expr::Binary { left, op, right } => {
            let left = known_constant(left, constants)?;
            let right = known_constant(right, constants)?;
            Some(match op {
                BinaryOp::Mul => left.wrapping_mul(right),
                BinaryOp::Div => left.checked_div(right)?,
                BinaryOp::Mod => left.checked_rem(right)?,
                BinaryOp::Add => left.wrapping_add(right),
                BinaryOp::Sub => left.wrapping_sub(right),
                BinaryOp::Shl => left.wrapping_shl(right as u32),
                BinaryOp::Shr => left.wrapping_shr(right as u32),
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
        Expr::Cast { expr, .. } => known_constant(expr, constants),
        _ => None,
    }
}

fn access_constant(path: &AccessPath, constants: &HashMap<String, i64>) -> Option<i64> {
    let mut name = path.root.clone();
    for segment in &path.segments {
        let AccessSegment::Field(field) = segment else {
            return None;
        };
        name.push('.');
        name.push_str(field);
    }
    constants.get(&name).copied()
}

fn is_untyped_intrinsic_literal(expr: &Expr) -> bool {
    match expr {
        Expr::Int(_) | Expr::Char(_) => true,
        Expr::Unary {
            op: UnaryOp::Neg,
            expr,
        } => matches!(expr.as_ref(), Expr::Int(_)),
        _ => false,
    }
}

fn intrinsic_argument_hint(name: &str, index: usize) -> Option<Type> {
    let operation = CATALOG.lookup(name)?.operation;
    let type_name = match (operation, index) {
        (IntrinsicOperation::Mem(operation), 2)
            if matches!(
                operation,
                MemIntrinsic::CopyNonoverlapping
                    | MemIntrinsic::Move
                    | MemIntrinsic::Fill
                    | MemIntrinsic::Compare
            ) =>
        {
            "u24"
        }
        (IntrinsicOperation::Mem(MemIntrinsic::FindByte), 1) => "u24",
        (IntrinsicOperation::Mem(MemIntrinsic::Fill), 1)
        | (IntrinsicOperation::Mem(MemIntrinsic::FindByte), 2)
        | (IntrinsicOperation::Mem(MemIntrinsic::Poke8), 1) => "u8",
        (IntrinsicOperation::Mem(MemIntrinsic::StoreLe16 | MemIntrinsic::StoreBe16), 1) => "u16",
        (IntrinsicOperation::Mem(MemIntrinsic::StoreLe24 | MemIntrinsic::StoreBe24), 1) => "u24",
        _ => return None,
    };
    Some(Type::Named(type_name.to_owned()))
}

fn access_type(
    path: &AccessPath,
    context: &IntrinsicValidationContext,
    locals: &HashMap<String, Type>,
) -> Option<Type> {
    let mut ty = locals
        .get(&path.root)
        .or_else(|| context.values.get(&path.root))
        .cloned();
    let mut qualified = path.root.clone();
    for segment in &path.segments {
        match segment {
            AccessSegment::Field(field) => {
                qualified.push('.');
                qualified.push_str(field);
                if let Some(value) = context.values.get(&qualified) {
                    ty = Some(value.clone());
                    continue;
                }
                let Type::Named(struct_name) =
                    resolve_intrinsic_type(ty.as_ref()?, &context.aliases)
                else {
                    return None;
                };
                ty = context.structs.get(&struct_name)?.get(field).cloned();
            }
            AccessSegment::Index(_) => {
                let Type::Array { element, .. } =
                    resolve_intrinsic_type(ty.as_ref()?, &context.aliases)
                else {
                    return None;
                };
                ty = Some(*element);
            }
        }
    }
    ty.map(|ty| resolve_intrinsic_type(&ty, &context.aliases))
}

fn intrinsic_expr_type(
    expr: &Expr,
    context: &IntrinsicValidationContext,
    locals: &HashMap<String, Type>,
) -> Option<Type> {
    match expr {
        Expr::Ident(name) => locals
            .get(name)
            .or_else(|| context.values.get(name))
            .map(|ty| resolve_intrinsic_type(ty, &context.aliases)),
        Expr::Int(value) => Some(integer_literal_type(*value)),
        Expr::TypedInt(_, ty) | Expr::Cast { ty, .. } => {
            Some(resolve_intrinsic_type(ty, &context.aliases))
        }
        Expr::Bool(_) => Some(Type::Named("bool".to_owned())),
        Expr::Char(_) | Expr::In(_) => Some(Type::Named("u8".to_owned())),
        Expr::String(_) => Some(Type::Ptr(Box::new(Type::Named("u8".to_owned())))),
        Expr::Index { name, .. } => {
            let Type::Array { element, .. } = resolve_intrinsic_type(
                locals.get(name).or_else(|| context.values.get(name))?,
                &context.aliases,
            ) else {
                return None;
            };
            Some(*element)
        }
        Expr::Field { base, field } => context
            .values
            .get(&format!("{base}.{field}"))
            .map(|ty| resolve_intrinsic_type(ty, &context.aliases))
            .or_else(|| {
                let ty = locals.get(base).or_else(|| context.values.get(base))?;
                let Type::Named(struct_name) = resolve_intrinsic_type(ty, &context.aliases) else {
                    return None;
                };
                context
                    .structs
                    .get(&struct_name)?
                    .get(field)
                    .map(|ty| resolve_intrinsic_type(ty, &context.aliases))
            }),
        Expr::AddressOf(name) => Some(Type::Ptr(Box::new(intrinsic_expr_type(
            &Expr::Ident(name.clone()),
            context,
            locals,
        )?))),
        Expr::AddressOfIndex { name, .. } => Some(Type::Ptr(Box::new(intrinsic_expr_type(
            &Expr::Index {
                name: name.clone(),
                index: Box::new(Expr::Int(0)),
            },
            context,
            locals,
        )?))),
        Expr::AddressOfField { base, field } => Some(Type::Ptr(Box::new(intrinsic_expr_type(
            &Expr::Field {
                base: base.clone(),
                field: field.clone(),
            },
            context,
            locals,
        )?))),
        Expr::Access(path) => access_type(path, context, locals),
        Expr::AddressOfAccess(path) => {
            Some(Type::Ptr(Box::new(access_type(path, context, locals)?)))
        }
        Expr::Deref(expr) => match resolve_intrinsic_type(
            &intrinsic_expr_type(expr, context, locals)?,
            &context.aliases,
        ) {
            Type::Ptr(inner) => Some(*inner),
            _ => None,
        },
        Expr::StructInit { ty, .. } => Some(Type::Named(ty.clone())),
        Expr::BankedPointer { pointer, .. } => intrinsic_expr_type(pointer, context, locals),
        Expr::Call { path, args } => {
            let name = path.join(".");
            if CATALOG.lookup(&name).is_some() {
                let types = intrinsic_argument_types(name.as_str(), args, context, locals)?;
                let resolution = CATALOG.validate_types(name.as_str(), &types).ok()?;
                return (resolution.result_types.len() == 1)
                    .then(|| resolution.result_types[0].clone());
            }
            let signature = context
                .functions
                .get(&name)
                .or_else(|| path.last().and_then(|name| context.functions.get(name)))?;
            (signature.second.is_none())
                .then(|| signature.first.clone())
                .flatten()
                .map(|ty| resolve_intrinsic_type(&ty, &context.aliases))
        }
        Expr::Unary { op, expr } => match op {
            UnaryOp::Not => Some(Type::Named("bool".to_owned())),
            UnaryOp::Neg | UnaryOp::BitNot => intrinsic_expr_type(expr, context, locals),
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
                Some(Type::Named("bool".to_owned()))
            } else {
                intrinsic_expr_type(left, context, locals)
                    .or_else(|| intrinsic_expr_type(right, context, locals))
            }
        }
        Expr::Array(_) => None,
    }
}

fn intrinsic_argument_types(
    name: &str,
    args: &[Expr],
    context: &IntrinsicValidationContext,
    locals: &HashMap<String, Type>,
) -> Option<Vec<Type>> {
    args.iter()
        .enumerate()
        .map(|(index, arg)| {
            if is_untyped_intrinsic_literal(arg) {
                intrinsic_argument_hint(name, index)
                    .or_else(|| intrinsic_expr_type(arg, context, locals))
            } else {
                intrinsic_expr_type(arg, context, locals)
            }
            .map(|ty| resolve_intrinsic_type(&ty, &context.aliases))
        })
        .collect()
}

fn validate_intrinsic_call(
    path: &[String],
    args: &[Expr],
    context: &IntrinsicValidationContext,
    locals: &HashMap<String, Type>,
    constants: &HashMap<String, i64>,
) -> Result<Option<Vec<Type>>, Diagnostic> {
    let name = path.join(".");
    let Some(descriptor) = CATALOG.lookup(&name) else {
        return Ok(None);
    };
    if args.len() != descriptor.argument_count {
        return Err(Diagnostic::new(format!(
            "intrinsic `{}` expects {} arguments, got {}",
            descriptor.canonical_name,
            descriptor.argument_count,
            args.len()
        )));
    }
    let Some(types) = intrinsic_argument_types(&name, args, context, locals) else {
        return Ok(None);
    };
    let constants = args
        .iter()
        .map(|arg| known_constant(arg, constants))
        .collect::<Vec<_>>();
    let resolution = CATALOG
        .validate_types_with_constants(&name, &types, &constants)
        .map_err(|error| Diagnostic::new(error.to_string()))?;
    Ok(Some(resolution.result_types))
}

fn is_intrinsic_call(expr: &Expr) -> bool {
    matches!(
        expr,
        Expr::Call { path, .. } if CATALOG.lookup(&path.join(".")).is_some()
    )
}

fn contains_intrinsic_call(expr: &Expr) -> bool {
    match expr {
        Expr::Call { args, .. } => is_intrinsic_call(expr) || args.iter().any(contains_intrinsic_call),
        Expr::Index { index, .. }
        | Expr::AddressOfIndex { index, .. }
        | Expr::Deref(index)
        | Expr::BankedPointer { pointer: index, .. }
        | Expr::Unary { expr: index, .. }
        | Expr::Cast { expr: index, .. } => contains_intrinsic_call(index),
        Expr::Access(path) | Expr::AddressOfAccess(path) => path.segments.iter().any(|segment| {
            matches!(segment, AccessSegment::Index(index) if contains_intrinsic_call(index))
        }),
        Expr::Array(values) => values.iter().any(contains_intrinsic_call),
        Expr::StructInit { fields, .. } => fields.iter().any(|(_, value)| contains_intrinsic_call(value)),
        Expr::Binary { left, right, .. } => {
            contains_intrinsic_call(left) || contains_intrinsic_call(right)
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
        | Expr::AddressOfField { .. } => false,
    }
}

fn is_paired_intrinsic(name: &str) -> bool {
    matches!(
        CATALOG.lookup(name).map(|descriptor| descriptor.operation),
        Some(
            IntrinsicOperation::Int(crate::intrinsics::IntIntrinsic::Divmod)
                | IntrinsicOperation::Int(crate::intrinsics::IntIntrinsic::AddCarry)
                | IntrinsicOperation::Int(crate::intrinsics::IntIntrinsic::SubBorrow)
                | IntrinsicOperation::Int(crate::intrinsics::IntIntrinsic::FullMul)
                | IntrinsicOperation::Mem(MemIntrinsic::FindByte)
        )
    )
}

fn validate_intrinsic_expr(
    expr: &Expr,
    context: &IntrinsicValidationContext,
    locals: &HashMap<String, Type>,
    constants: &HashMap<String, i64>,
    use_context: IntrinsicUse,
) -> Result<Option<Vec<Type>>, Diagnostic> {
    match expr {
        Expr::Call { path, args } => {
            for arg in args {
                validate_intrinsic_expr(arg, context, locals, constants, IntrinsicUse::Scalar)?;
            }
            let Some(descriptor) = CATALOG.lookup(&path.join(".")) else {
                return Ok(None);
            };
            let name = descriptor.canonical_name;
            match descriptor.result_count {
                ResultCount::Two if !matches!(use_context, IntrinsicUse::Pair) => {
                    return Err(Diagnostic::new(format!(
                        "two-result intrinsic `{name}` may only be used in a two-place binding or returned directly"
                    )));
                }
                ResultCount::Two if !is_paired_intrinsic(name) => {
                    return Err(Diagnostic::new(format!(
                        "intrinsic `{name}` is not supported as a paired call"
                    )));
                }
                _ if matches!(use_context, IntrinsicUse::Pair)
                    && descriptor.result_count != ResultCount::Two =>
                {
                    return Err(Diagnostic::new(format!(
                        "intrinsic `{name}` does not return two values for a paired binding"
                    )));
                }
                ResultCount::Zero if matches!(use_context, IntrinsicUse::Scalar) => {
                    return Err(Diagnostic::new(format!(
                        "zero-result intrinsic `{name}` cannot be used as a value"
                    )));
                }
                _ => {}
            }
            validate_intrinsic_call(path, args, context, locals, constants)
        }
        Expr::Index { index, .. }
        | Expr::AddressOfIndex { index, .. }
        | Expr::Deref(index)
        | Expr::BankedPointer { pointer: index, .. }
        | Expr::Unary { expr: index, .. }
        | Expr::Cast { expr: index, .. } => {
            validate_intrinsic_expr(index, context, locals, constants, IntrinsicUse::Scalar)
        }
        Expr::Access(path) | Expr::AddressOfAccess(path) => {
            for segment in &path.segments {
                if let AccessSegment::Index(index) = segment {
                    validate_intrinsic_expr(
                        index,
                        context,
                        locals,
                        constants,
                        IntrinsicUse::Scalar,
                    )?;
                }
            }
            Ok(None)
        }
        Expr::Array(values) => {
            for value in values {
                validate_intrinsic_expr(value, context, locals, constants, IntrinsicUse::Scalar)?;
            }
            Ok(None)
        }
        Expr::StructInit { fields, .. } => {
            for (_, value) in fields {
                validate_intrinsic_expr(value, context, locals, constants, IntrinsicUse::Scalar)?;
            }
            Ok(None)
        }
        Expr::Binary { left, right, .. } => {
            validate_intrinsic_expr(left, context, locals, constants, IntrinsicUse::Scalar)?;
            validate_intrinsic_expr(right, context, locals, constants, IntrinsicUse::Scalar)?;
            Ok(None)
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
        | Expr::AddressOfField { .. } => Ok(None),
    }
}

fn validate_intrinsic_place(
    place: &Place,
    context: &IntrinsicValidationContext,
    locals: &HashMap<String, Type>,
    constants: &HashMap<String, i64>,
) -> Result<(), Diagnostic> {
    match place {
        Place::Index { index, .. } | Place::Deref(index) => {
            validate_intrinsic_expr(index, context, locals, constants, IntrinsicUse::Scalar)?;
        }
        Place::Access(path) => {
            for segment in &path.segments {
                if let AccessSegment::Index(index) = segment {
                    validate_intrinsic_expr(
                        index,
                        context,
                        locals,
                        constants,
                        IntrinsicUse::Scalar,
                    )?;
                }
            }
        }
        Place::Ident(_) | Place::Field { .. } => {}
    }
    Ok(())
}

fn validate_intrinsic_type(
    function: &str,
    value_name: &str,
    value: &Expr,
    expected: &Type,
    context: &IntrinsicValidationContext,
    locals: &HashMap<String, Type>,
) -> Result<(), Diagnostic> {
    let Some(actual) = intrinsic_expr_type(value, context, locals) else {
        return Ok(());
    };
    let expected = resolve_intrinsic_type(expected, &context.aliases);
    if actual != expected {
        return Err(Diagnostic::new(format!(
            "type mismatch in function `{function}` for {value_name}: expected `{expected:?}`, got `{actual:?}`"
        )));
    }
    Ok(())
}

fn validate_intrinsic_stmts(
    function: &str,
    returns: &ReturnSignature,
    stmts: &[Stmt],
    locals: &HashMap<String, Type>,
    constants: &HashMap<String, i64>,
    context: &IntrinsicValidationContext,
) -> Result<(), Diagnostic> {
    let mut locals = locals.clone();
    let mut constants = constants.clone();
    for stmt in stmts {
        match stmt {
            Stmt::Let { name, ty, value } => {
                validate_intrinsic_expr(value, context, &locals, &constants, IntrinsicUse::Scalar)?;
                if contains_intrinsic_call(value) {
                    validate_intrinsic_type(function, "let binding", value, ty, context, &locals)?;
                }
                if let Some(value) = known_constant(value, &constants) {
                    constants.insert(name.clone(), value);
                }
                locals.insert(name.clone(), ty.clone());
            }
            Stmt::LetTwo {
                first_name,
                first_ty,
                second_name,
                second_ty,
                value,
            } => {
                let result = validate_intrinsic_expr(
                    value,
                    context,
                    &locals,
                    &constants,
                    IntrinsicUse::Pair,
                )?;
                if let Expr::Call { path, .. } = value
                    && CATALOG.lookup(&path.join(".")).is_some()
                {
                    if let Some(types) = result {
                        let expected_first = resolve_intrinsic_type(first_ty, &context.aliases);
                        let expected_second = resolve_intrinsic_type(second_ty, &context.aliases);
                        if types.len() != 2
                            || types[0] != expected_first
                            || types[1] != expected_second
                        {
                            return Err(Diagnostic::new(format!(
                                "type mismatch in two-result binding `{first_name}, {second_name}` for intrinsic `{}`: expected `{expected_first:?}, {expected_second:?}`, got {:?}",
                                path.join("."),
                                types
                            )));
                        }
                    }
                }
                locals.insert(first_name.clone(), first_ty.clone());
                locals.insert(second_name.clone(), second_ty.clone());
            }
            Stmt::Assign { target, value, .. } => {
                validate_intrinsic_place(target, context, &locals, &constants)?;
                validate_intrinsic_expr(value, context, &locals, &constants, IntrinsicUse::Scalar)?;
            }
            Stmt::If {
                condition,
                then_body,
                else_body,
            } => {
                validate_intrinsic_expr(
                    condition,
                    context,
                    &locals,
                    &constants,
                    IntrinsicUse::Scalar,
                )?;
                validate_intrinsic_stmts(
                    function, returns, then_body, &locals, &constants, context,
                )?;
                validate_intrinsic_stmts(
                    function, returns, else_body, &locals, &constants, context,
                )?;
            }
            Stmt::While { condition, body } => {
                validate_intrinsic_expr(
                    condition,
                    context,
                    &locals,
                    &constants,
                    IntrinsicUse::Scalar,
                )?;
                validate_intrinsic_stmts(function, returns, body, &locals, &constants, context)?;
            }
            Stmt::Loop { body } => {
                validate_intrinsic_stmts(function, returns, body, &locals, &constants, context)?;
            }
            Stmt::Return(Some(value)) => {
                if let Expr::Call { path, .. } = value
                    && let Some(descriptor) = CATALOG.lookup(&path.join("."))
                {
                    if descriptor.result_count == ResultCount::Two {
                        let result = validate_intrinsic_expr(
                            value,
                            context,
                            &locals,
                            &constants,
                            IntrinsicUse::Pair,
                        )?;
                        if returns.second.is_none() {
                            return Err(Diagnostic::new(format!(
                                "two-result intrinsic `{}` cannot be returned from a single-result function",
                                path.join(".")
                            )));
                        }
                        if let Some(types) = result {
                            if types.len() == 2
                                && (returns
                                    .first
                                    .as_ref()
                                    .map(|ty| resolve_intrinsic_type(ty, &context.aliases))
                                    != Some(types[0].clone())
                                    || returns
                                        .second
                                        .as_ref()
                                        .map(|ty| resolve_intrinsic_type(ty, &context.aliases))
                                        != Some(types[1].clone()))
                            {
                                return Err(Diagnostic::new(format!(
                                    "type mismatch returning two values from intrinsic `{}` in function `{function}`",
                                    path.join(".")
                                )));
                            }
                        }
                    } else {
                        validate_intrinsic_expr(
                            value,
                            context,
                            &locals,
                            &constants,
                            IntrinsicUse::Scalar,
                        )?;
                        if returns.second.is_some() {
                            return Err(Diagnostic::new(format!(
                                "function `{function}` must return two values"
                            )));
                        }
                        if let Some(expected) = &returns.first {
                            validate_intrinsic_type(
                                function,
                                "return value",
                                value,
                                expected,
                                context,
                                &locals,
                            )?;
                        }
                    }
                } else {
                    validate_intrinsic_expr(
                        value,
                        context,
                        &locals,
                        &constants,
                        IntrinsicUse::Scalar,
                    )?;
                    if returns.second.is_none()
                        && let Some(expected) = &returns.first
                        && contains_intrinsic_call(value)
                    {
                        validate_intrinsic_type(
                            function,
                            "return value",
                            value,
                            expected,
                            context,
                            &locals,
                        )?;
                    }
                }
            }
            Stmt::ReturnTwo { first, second } => {
                validate_intrinsic_expr(first, context, &locals, &constants, IntrinsicUse::Scalar)?;
                validate_intrinsic_expr(
                    second,
                    context,
                    &locals,
                    &constants,
                    IntrinsicUse::Scalar,
                )?;
                if let (Some(expected_first), Some(expected_second)) =
                    (&returns.first, &returns.second)
                {
                    if contains_intrinsic_call(first) {
                        validate_intrinsic_type(
                            function,
                            "first return value",
                            first,
                            expected_first,
                            context,
                            &locals,
                        )?;
                    }
                    if contains_intrinsic_call(second) {
                        validate_intrinsic_type(
                            function,
                            "second return value",
                            second,
                            expected_second,
                            context,
                            &locals,
                        )?;
                    }
                }
            }
            Stmt::Out { value, .. } => {
                validate_intrinsic_expr(value, context, &locals, &constants, IntrinsicUse::Scalar)?;
            }
            Stmt::Expr(value) => {
                validate_intrinsic_expr(
                    value,
                    context,
                    &locals,
                    &constants,
                    IntrinsicUse::Statement,
                )?;
            }
            Stmt::Break | Stmt::Continue | Stmt::Return(None) | Stmt::Asm { .. } => {}
        }
    }
    Ok(())
}

fn validate_intrinsic_declarations(
    program: &Program,
    context: &IntrinsicValidationContext,
) -> Result<(), Diagnostic> {
    fn visit(
        declaration: &Declaration,
        context: &IntrinsicValidationContext,
    ) -> Result<(), Diagnostic> {
        match declaration {
            Declaration::Function(function) => {
                let locals = function
                    .params
                    .iter()
                    .map(|param| (param.name.clone(), param.ty.clone()))
                    .collect();
                let returns = ReturnSignature {
                    first: function.return_type.clone(),
                    second: function.second_return_type.clone(),
                };
                validate_intrinsic_stmts(
                    &function.name,
                    &returns,
                    &function.body,
                    &locals,
                    &context.constants,
                    context,
                )?;
            }
            Declaration::Cfg { declaration, .. } | Declaration::Bank { declaration, .. } => {
                visit(declaration, context)?;
            }
            Declaration::Const(constant) => {
                validate_intrinsic_expr(
                    &constant.value,
                    context,
                    &HashMap::new(),
                    &context.constants,
                    IntrinsicUse::Scalar,
                )?;
            }
            Declaration::Global(global) => {
                validate_intrinsic_expr(
                    &global.value,
                    context,
                    &HashMap::new(),
                    &context.constants,
                    IntrinsicUse::Scalar,
                )?;
            }
            Declaration::Port(port) => {
                validate_intrinsic_expr(
                    &port.value,
                    context,
                    &HashMap::new(),
                    &context.constants,
                    IntrinsicUse::Scalar,
                )?;
            }
            Declaration::Mmio(mmio) => {
                validate_intrinsic_expr(
                    &mmio.value,
                    context,
                    &HashMap::new(),
                    &context.constants,
                    IntrinsicUse::Scalar,
                )?;
            }
            _ => {}
        }
        Ok(())
    }

    for declaration in &program.declarations {
        visit(declaration, context)?;
    }
    Ok(())
}

/// Validates declared functions' existing 0/1/2-result calling convention.
///
/// Two-result calls are deliberately kept out of general expressions. They can
/// only feed a two-place binding or be returned directly from a two-result
/// function. Both result types must be primitive scalar types.
fn validate_user_multi_value_returns(program: &Program) -> Result<(), Diagnostic> {
    let mut aliases = HashMap::new();
    let mut structs = HashSet::new();
    let mut functions = HashMap::new();

    fn collect(
        declaration: &Declaration,
        aliases: &mut HashMap<String, Type>,
        structs: &mut HashSet<String>,
        functions: &mut HashMap<String, ReturnSignature>,
    ) {
        match declaration {
            Declaration::Alias(alias) => {
                aliases.insert(alias.name.clone(), alias.ty.clone());
            }
            Declaration::Struct(structure) => {
                structs.insert(structure.name.clone());
            }
            Declaration::Function(function) => {
                functions.insert(
                    function.name.clone(),
                    ReturnSignature {
                        first: function.return_type.clone(),
                        second: function.second_return_type.clone(),
                    },
                );
            }
            Declaration::ExternAsmFunction(function) => {
                functions.insert(
                    function.name.clone(),
                    ReturnSignature {
                        first: function.return_type.clone(),
                        second: function.second_return_type.clone(),
                    },
                );
            }
            Declaration::Cfg { declaration, .. } | Declaration::Bank { declaration, .. } => {
                collect(declaration, aliases, structs, functions);
            }
            _ => {}
        }
    }

    for declaration in &program.declarations {
        collect(declaration, &mut aliases, &mut structs, &mut functions);
    }

    fn resolve_alias(ty: &Type, aliases: &HashMap<String, Type>) -> Type {
        let mut current = ty;
        let mut seen = HashSet::new();
        while let Type::Named(name) = current {
            let Some(alias) = aliases.get(name) else {
                break;
            };
            if !seen.insert(name) {
                break;
            }
            current = alias;
        }
        current.clone()
    }

    fn validate_scalar_return_type(
        function: &str,
        ordinal: &str,
        ty: &Type,
        aliases: &HashMap<String, Type>,
        structs: &HashSet<String>,
    ) -> Result<(), Diagnostic> {
        let resolved = resolve_alias(ty, aliases);
        match &resolved {
            Type::Named(name)
                if matches!(
                    name.as_str(),
                    "u8" | "i8"
                        | "u16"
                        | "i16"
                        | "u20"
                        | "i20"
                        | "u24"
                        | "i24"
                        | "u32"
                        | "i32"
                        | "bool"
                        | "char"
                ) =>
            {
                Ok(())
            }
            Type::Array { .. } => Err(Diagnostic::new(format!(
                "function `{function}` {ordinal} return type is an array; arrays are not supported in multi-value returns"
            ))),
            Type::Named(name) if structs.contains(name) => Err(Diagnostic::new(format!(
                "function `{function}` {ordinal} return type is struct `{name}`; structs are not supported in multi-value returns"
            ))),
            Type::Named(name) if name == "bytes" => Err(Diagnostic::new(format!(
                "function `{function}` {ordinal} return type `bytes` is a byte buffer; byte buffers are not supported in multi-value returns"
            ))),
            _ => Err(Diagnostic::new(format!(
                "function `{function}` {ordinal} return type `{resolved:?}` is not a primitive scalar"
            ))),
        }
    }

    fn call_signature<'a>(
        expr: &'a Expr,
        functions: &'a HashMap<String, ReturnSignature>,
    ) -> Option<(&'a str, &'a ReturnSignature)> {
        let Expr::Call { path, .. } = expr else {
            return None;
        };
        let name = path.last()?;
        Some((name, functions.get(name)?))
    }

    fn return_count(signature: &ReturnSignature) -> usize {
        usize::from(signature.first.is_some()) + usize::from(signature.second.is_some())
    }

    fn type_matches(
        actual: Option<Type>,
        expected: &Type,
        aliases: &HashMap<String, Type>,
    ) -> bool {
        actual.is_some_and(|actual| {
            resolve_alias(&actual, aliases) == resolve_alias(expected, aliases)
        })
    }

    fn expr_type(
        expr: &Expr,
        locals: &HashMap<String, Type>,
        functions: &HashMap<String, ReturnSignature>,
        aliases: &HashMap<String, Type>,
    ) -> Option<Type> {
        match expr {
            Expr::Ident(name) => locals.get(name).cloned(),
            Expr::TypedInt(_, ty) | Expr::Cast { ty, .. } => Some(resolve_alias(ty, aliases)),
            Expr::Bool(_) => Some(Type::Named("bool".to_owned())),
            Expr::Char(_) => Some(Type::Named("u8".to_owned())),
            Expr::Call { path, .. } => functions
                .get(path.last()?)
                .and_then(|signature| signature.second.is_none().then(|| signature.first.clone()))
                .flatten(),
            Expr::Unary { op, expr } => match op {
                crate::ast::UnaryOp::Not => Some(Type::Named("bool".to_owned())),
                crate::ast::UnaryOp::Neg | crate::ast::UnaryOp::BitNot => {
                    expr_type(expr, locals, functions, aliases)
                }
            },
            Expr::Binary { left, op, right } => {
                if matches!(
                    op,
                    crate::ast::BinaryOp::Lt
                        | crate::ast::BinaryOp::Le
                        | crate::ast::BinaryOp::Gt
                        | crate::ast::BinaryOp::Ge
                        | crate::ast::BinaryOp::Eq
                        | crate::ast::BinaryOp::Ne
                        | crate::ast::BinaryOp::And
                        | crate::ast::BinaryOp::Or
                ) {
                    Some(Type::Named("bool".to_owned()))
                } else {
                    expr_type(left, locals, functions, aliases)
                        .or_else(|| expr_type(right, locals, functions, aliases))
                }
            }
            Expr::Index { .. }
            | Expr::Field { .. }
            | Expr::Access(_)
            | Expr::Deref(_)
            | Expr::BankedPointer { .. }
            | Expr::AddressOf(_)
            | Expr::AddressOfIndex { .. }
            | Expr::AddressOfField { .. }
            | Expr::AddressOfAccess(_)
            | Expr::Array(_)
            | Expr::StructInit { .. }
            | Expr::Int(_)
            | Expr::String(_)
            | Expr::In(_) => None,
        }
    }

    fn validate_expr(
        expr: &Expr,
        locals: &HashMap<String, Type>,
        functions: &HashMap<String, ReturnSignature>,
        aliases: &HashMap<String, Type>,
        allow_two_result_call: bool,
    ) -> Result<(), Diagnostic> {
        match expr {
            Expr::Call { path, args } => {
                for arg in args {
                    validate_expr(arg, locals, functions, aliases, false)?;
                }
                if let Some(signature) = functions.get(path.last().unwrap_or(&String::new()))
                    && signature.second.is_some()
                    && !allow_two_result_call
                {
                    return Err(Diagnostic::new(format!(
                        "two-result call `{}` may only be used in a two-place binding or returned directly",
                        path.join(".")
                    )));
                }
            }
            Expr::Index { index, .. }
            | Expr::AddressOfIndex { index, .. }
            | Expr::Deref(index)
            | Expr::BankedPointer { pointer: index, .. }
            | Expr::Unary { expr: index, .. }
            | Expr::Cast { expr: index, .. } => {
                validate_expr(index, locals, functions, aliases, false)?;
            }
            Expr::Access(path) | Expr::AddressOfAccess(path) => {
                for segment in &path.segments {
                    if let AccessSegment::Index(index) = segment {
                        validate_expr(index, locals, functions, aliases, false)?;
                    }
                }
            }
            Expr::Array(values) => {
                for value in values {
                    validate_expr(value, locals, functions, aliases, false)?;
                }
            }
            Expr::StructInit { fields, .. } => {
                for (_, value) in fields {
                    validate_expr(value, locals, functions, aliases, false)?;
                }
            }
            Expr::Binary { left, right, .. } => {
                validate_expr(left, locals, functions, aliases, false)?;
                validate_expr(right, locals, functions, aliases, false)?;
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

    fn validate_expected_type(
        function: &str,
        value_name: &str,
        value: &Expr,
        expected: &Type,
        locals: &HashMap<String, Type>,
        functions: &HashMap<String, ReturnSignature>,
        aliases: &HashMap<String, Type>,
    ) -> Result<(), Diagnostic> {
        if let Some(actual) = expr_type(value, locals, functions, aliases)
            && !type_matches(Some(actual.clone()), expected, aliases)
        {
            return Err(Diagnostic::new(format!(
                "type mismatch in function `{function}` for {value_name}: expected `{expected:?}`, got `{actual:?}`"
            )));
        }
        Ok(())
    }

    fn validate_stmts(
        function: &str,
        returns: &ReturnSignature,
        stmts: &[Stmt],
        locals: &HashMap<String, Type>,
        functions: &HashMap<String, ReturnSignature>,
        aliases: &HashMap<String, Type>,
        structs: &HashSet<String>,
    ) -> Result<(), Diagnostic> {
        let mut locals = locals.clone();
        for stmt in stmts {
            match stmt {
                Stmt::Let { name, ty, value } => {
                    validate_expr(value, &locals, functions, aliases, false)?;
                    locals.insert(name.clone(), ty.clone());
                }
                Stmt::LetTwo {
                    first_name,
                    first_ty,
                    second_name,
                    second_ty,
                    value,
                } => {
                    if is_intrinsic_call(value) {
                        validate_expr(value, &locals, functions, aliases, true)?;
                        locals.insert(first_name.clone(), first_ty.clone());
                        locals.insert(second_name.clone(), second_ty.clone());
                        continue;
                    }
                    let Some((callee, signature)) = call_signature(value, functions) else {
                        return Err(Diagnostic::new(
                            "two-place binding requires a direct two-result call",
                        ));
                    };
                    if signature.second.is_none() {
                        return Err(Diagnostic::new(format!(
                            "call `{callee}` does not return two values for a two-place binding"
                        )));
                    }
                    validate_expr(value, &locals, functions, aliases, true)?;
                    validate_expected_type(
                        function,
                        &format!("first result binding `{first_name}`"),
                        value,
                        signature.first.as_ref().unwrap(),
                        &locals,
                        functions,
                        aliases,
                    )?;
                    if let Some(actual) = &signature.first
                        && resolve_alias(actual, aliases) != resolve_alias(first_ty, aliases)
                    {
                        return Err(Diagnostic::new(format!(
                            "type mismatch in two-result binding `{first_name}`: expected `{actual:?}`, got `{first_ty:?}`"
                        )));
                    }
                    if let Some(actual) = &signature.second
                        && resolve_alias(actual, aliases) != resolve_alias(second_ty, aliases)
                    {
                        return Err(Diagnostic::new(format!(
                            "type mismatch in two-result binding `{second_name}`: expected `{actual:?}`, got `{second_ty:?}`"
                        )));
                    }
                    locals.insert(first_name.clone(), first_ty.clone());
                    locals.insert(second_name.clone(), second_ty.clone());
                }
                Stmt::Assign { target, value, .. } => {
                    validate_place(target, &locals, functions, aliases)?;
                    validate_expr(value, &locals, functions, aliases, false)?;
                }
                Stmt::If {
                    condition,
                    then_body,
                    else_body,
                } => {
                    validate_expr(condition, &locals, functions, aliases, false)?;
                    validate_stmts(
                        function, returns, then_body, &locals, functions, aliases, structs,
                    )?;
                    validate_stmts(
                        function, returns, else_body, &locals, functions, aliases, structs,
                    )?;
                }
                Stmt::While { condition, body } => {
                    validate_expr(condition, &locals, functions, aliases, false)?;
                    validate_stmts(
                        function, returns, body, &locals, functions, aliases, structs,
                    )?;
                }
                Stmt::Loop { body } => {
                    validate_stmts(
                        function, returns, body, &locals, functions, aliases, structs,
                    )?;
                }
                Stmt::Return(None) => {
                    if return_count(returns) != 0 {
                        return Err(Diagnostic::new(format!(
                            "function `{function}` must return {} value{}",
                            return_count(returns),
                            if return_count(returns) == 1 { "" } else { "s" }
                        )));
                    }
                }
                Stmt::Return(Some(value)) => {
                    if is_intrinsic_call(value) {
                        let allow_two_result_call = matches!(
                            value,
                            Expr::Call { path, .. }
                                if CATALOG
                                    .lookup(&path.join("."))
                                    .is_some_and(|descriptor| descriptor.result_count == ResultCount::Two)
                        );
                        validate_expr(value, &locals, functions, aliases, allow_two_result_call)?;
                        continue;
                    }
                    if let Some((callee, signature)) = call_signature(value, functions)
                        && signature.second.is_some()
                    {
                        if returns.second.is_none() {
                            return Err(Diagnostic::new(format!(
                                "two-result call `{callee}` cannot be returned from a single-result function"
                            )));
                        }
                        validate_expr(value, &locals, functions, aliases, true)?;
                        if signature
                            .first
                            .as_ref()
                            .map(|ty| resolve_alias(ty, aliases))
                            != returns.first.as_ref().map(|ty| resolve_alias(ty, aliases))
                            || signature
                                .second
                                .as_ref()
                                .map(|ty| resolve_alias(ty, aliases))
                                != returns.second.as_ref().map(|ty| resolve_alias(ty, aliases))
                        {
                            return Err(Diagnostic::new(format!(
                                "type mismatch returning two values from `{callee}` in function `{function}`"
                            )));
                        }
                    } else {
                        validate_expr(value, &locals, functions, aliases, false)?;
                        if returns.second.is_some() {
                            return Err(Diagnostic::new(format!(
                                "function `{function}` must return two values"
                            )));
                        }
                        let Some(expected) = &returns.first else {
                            return Err(Diagnostic::new(format!(
                                "function `{function}` cannot return a value"
                            )));
                        };
                        validate_expected_type(
                            function,
                            "return value",
                            value,
                            expected,
                            &locals,
                            functions,
                            aliases,
                        )?;
                    }
                }
                Stmt::ReturnTwo { first, second } => {
                    let (Some(expected_first), Some(expected_second)) =
                        (&returns.first, &returns.second)
                    else {
                        return Err(Diagnostic::new(format!(
                            "function `{function}` does not return two values"
                        )));
                    };
                    validate_expr(first, &locals, functions, aliases, false)?;
                    validate_expr(second, &locals, functions, aliases, false)?;
                    validate_expected_type(
                        function,
                        "first return value",
                        first,
                        expected_first,
                        &locals,
                        functions,
                        aliases,
                    )?;
                    validate_expected_type(
                        function,
                        "second return value",
                        second,
                        expected_second,
                        &locals,
                        functions,
                        aliases,
                    )?;
                }
                Stmt::Out { value, .. } | Stmt::Expr(value) => {
                    validate_expr(value, &locals, functions, aliases, false)?;
                }
                Stmt::Break | Stmt::Continue | Stmt::Asm { .. } => {}
            }
        }
        let _ = structs;
        Ok(())
    }

    fn validate_place(
        place: &crate::ast::Place,
        locals: &HashMap<String, Type>,
        functions: &HashMap<String, ReturnSignature>,
        aliases: &HashMap<String, Type>,
    ) -> Result<(), Diagnostic> {
        match place {
            crate::ast::Place::Index { index, .. } | crate::ast::Place::Deref(index) => {
                validate_expr(index, locals, functions, aliases, false)
            }
            crate::ast::Place::Access(path) => {
                for segment in &path.segments {
                    if let AccessSegment::Index(index) = segment {
                        validate_expr(index, locals, functions, aliases, false)?;
                    }
                }
                Ok(())
            }
            crate::ast::Place::Ident(_) | crate::ast::Place::Field { .. } => Ok(()),
        }
    }

    fn validate_declarations(
        declarations: &[Declaration],
        functions: &HashMap<String, ReturnSignature>,
        aliases: &HashMap<String, Type>,
        structs: &HashSet<String>,
    ) -> Result<(), Diagnostic> {
        for declaration in declarations {
            match declaration {
                Declaration::Function(function) => {
                    let signature = functions.get(&function.name).unwrap();
                    if signature.second.is_some() {
                        validate_scalar_return_type(
                            &function.name,
                            "first",
                            signature.first.as_ref().unwrap(),
                            aliases,
                            structs,
                        )?;
                        validate_scalar_return_type(
                            &function.name,
                            "second",
                            signature.second.as_ref().unwrap(),
                            aliases,
                            structs,
                        )?;
                    }
                    let locals = function
                        .params
                        .iter()
                        .map(|param| (param.name.clone(), param.ty.clone()))
                        .collect();
                    validate_stmts(
                        &function.name,
                        signature,
                        &function.body,
                        &locals,
                        functions,
                        aliases,
                        structs,
                    )?;
                }
                Declaration::ExternAsmFunction(function) => {
                    let signature = functions.get(&function.name).unwrap();
                    if let Some(first) = &signature.first
                        && signature.second.is_some()
                    {
                        validate_scalar_return_type(
                            &function.name,
                            "first",
                            first,
                            aliases,
                            structs,
                        )?;
                        validate_scalar_return_type(
                            &function.name,
                            "second",
                            signature.second.as_ref().unwrap(),
                            aliases,
                            structs,
                        )?;
                    }
                }
                Declaration::Cfg { declaration, .. } | Declaration::Bank { declaration, .. } => {
                    validate_declarations(
                        core::slice::from_ref(declaration),
                        functions,
                        aliases,
                        structs,
                    )?;
                }
                _ => {}
            }
        }
        Ok(())
    }

    validate_declarations(&program.declarations, &functions, &aliases, &structs)
}

pub fn validate_program(program: &crate::ast::Program, cpu: CpuFamily) -> Result<(), Diagnostic> {
    validate_multi_value_returns(program)?;
    validate_inline_asm_operand_classes(program)?;
    let supports_port_io = cpu.capabilities().supports_port_io;
    let address_width = memory_model_for_cpu(cpu)
        .map(|memory| memory.address_width_bits)
        .unwrap_or(24);
    let max_address = if address_width >= 24 {
        Address24::MAX as i64
    } else {
        (1i64 << address_width) - 1
    };
    for declaration in &program.declarations {
        match declaration {
            Declaration::Port(port) => {
                if !supports_port_io {
                    return Err(Diagnostic::new(format!(
                        "target CPU `{}` does not support separate port I/O; declare `{}` as mmio instead",
                        cpu.as_str(),
                        port.name
                    )));
                }
                if let Some(value) = literal_int(&port.value)
                    && !(0..=0xFF).contains(&value)
                {
                    return Err(Diagnostic::new(format!(
                        "port `{}` value 0x{value:X} is outside the 8-bit port range for target CPU `{}`",
                        port.name,
                        cpu.as_str()
                    )));
                }
            }
            Declaration::Mmio(mmio) => {
                if let Some(value) = literal_int(&mmio.value)
                    && !(0..=max_address).contains(&value)
                {
                    return Err(Diagnostic::new(format!(
                        "mmio `{}` address 0x{value:X} is outside the {}{}-bit address space",
                        mmio.name,
                        if cpu == CpuFamily::Ez80 { "eZ80 " } else { "" },
                        address_width
                    )));
                }
            }
            Declaration::Function(function) if !supports_port_io => {
                validate_no_port_stmts(&function.body, cpu)?;
            }
            Declaration::Cfg { declaration, .. } | Declaration::Bank { declaration, .. } => {
                validate_program(
                    &crate::ast::Program {
                        source_path: program.source_path.clone(),
                        source_text: None,
                        source_units: Vec::new(),
                        declarations: vec![(**declaration).clone()],
                    },
                    cpu,
                )?;
            }
            _ => {}
        }
    }
    Ok(())
}

fn validate_no_port_stmts(stmts: &[Stmt], cpu: CpuFamily) -> Result<(), Diagnostic> {
    for stmt in stmts {
        match stmt {
            Stmt::Out { port, .. } => return port_io_error(cpu, port),
            Stmt::Let { value, .. }
            | Stmt::LetTwo { value, .. }
            | Stmt::Assign { value, .. }
            | Stmt::Return(Some(value))
            | Stmt::Expr(value) => validate_no_port_expr(value, cpu)?,
            Stmt::ReturnTwo { first, second } => {
                validate_no_port_expr(first, cpu)?;
                validate_no_port_expr(second, cpu)?;
            }
            Stmt::If {
                condition,
                then_body,
                else_body,
            } => {
                validate_no_port_expr(condition, cpu)?;
                validate_no_port_stmts(then_body, cpu)?;
                validate_no_port_stmts(else_body, cpu)?;
            }
            Stmt::While { condition, body } => {
                validate_no_port_expr(condition, cpu)?;
                validate_no_port_stmts(body, cpu)?;
            }
            Stmt::Loop { body } => validate_no_port_stmts(body, cpu)?,
            Stmt::Break | Stmt::Continue | Stmt::Return(None) | Stmt::Asm { .. } => {}
        }
    }
    Ok(())
}

fn validate_no_port_expr(expr: &Expr, cpu: CpuFamily) -> Result<(), Diagnostic> {
    match expr {
        Expr::In(port) => return port_io_error(cpu, port),
        Expr::Array(values) => {
            for value in values {
                validate_no_port_expr(value, cpu)?;
            }
        }
        Expr::Index { index, .. }
        | Expr::AddressOfIndex { index, .. }
        | Expr::Deref(index)
        | Expr::BankedPointer { pointer: index, .. }
        | Expr::Unary { expr: index, .. }
        | Expr::Cast { expr: index, .. } => validate_no_port_expr(index, cpu)?,
        Expr::Access(path) | Expr::AddressOfAccess(path) => {
            for segment in &path.segments {
                if let AccessSegment::Index(index) = segment {
                    validate_no_port_expr(index, cpu)?;
                }
            }
        }
        Expr::StructInit { fields, .. } => {
            for (_, value) in fields {
                validate_no_port_expr(value, cpu)?;
            }
        }
        Expr::Call { args, .. } => {
            for arg in args {
                validate_no_port_expr(arg, cpu)?;
            }
        }
        Expr::Binary { left, right, .. } => {
            validate_no_port_expr(left, cpu)?;
            validate_no_port_expr(right, cpu)?;
        }
        Expr::Int(_)
        | Expr::TypedInt(_, _)
        | Expr::Bool(_)
        | Expr::Char(_)
        | Expr::String(_)
        | Expr::Ident(_)
        | Expr::Field { .. }
        | Expr::AddressOfField { .. }
        | Expr::AddressOf(_) => {}
    }
    Ok(())
}

fn port_io_error<T>(cpu: CpuFamily, port: &str) -> Result<T, Diagnostic> {
    Err(Diagnostic::new(format!(
        "target CPU `{}` does not support separate port I/O `{port}`; use mmio instead",
        cpu.as_str()
    )))
}

fn validate_inline_asm_operand_classes(program: &crate::ast::Program) -> Result<(), Diagnostic> {
    let mut aliases = HashMap::new();
    fn collect_aliases(declaration: &Declaration, aliases: &mut HashMap<String, Type>) {
        match declaration {
            Declaration::Alias(alias) => {
                aliases.insert(alias.name.clone(), alias.ty.clone());
            }
            Declaration::Cfg { declaration, .. } | Declaration::Bank { declaration, .. } => {
                collect_aliases(declaration, aliases)
            }
            _ => {}
        }
    }
    for declaration in &program.declarations {
        collect_aliases(declaration, &mut aliases);
    }

    fn resolved_type(
        ty: &Type,
        aliases: &HashMap<String, Type>,
        seen: &mut HashSet<String>,
    ) -> Result<Type, Diagnostic> {
        match ty {
            Type::Named(name) if aliases.contains_key(name) => {
                if !seen.insert(name.clone()) {
                    return Err(Diagnostic::new(format!("cyclic type alias `{name}`")));
                }
                let resolved = resolved_type(&aliases[name], aliases, seen);
                seen.remove(name);
                resolved
            }
            Type::Ptr(inner) => Ok(Type::Ptr(Box::new(resolved_type(inner, aliases, seen)?))),
            Type::Function {
                params,
                return_type,
            } => Ok(Type::Function {
                params: params
                    .iter()
                    .map(|param| resolved_type(param, aliases, seen))
                    .collect::<Result<Vec<_>, _>>()?,
                return_type: return_type
                    .as_ref()
                    .map(|return_type| resolved_type(return_type, aliases, seen).map(Box::new))
                    .transpose()?,
            }),
            Type::Array { element, len } => Ok(Type::Array {
                element: Box::new(resolved_type(element, aliases, seen)?),
                len: len.clone(),
            }),
            Type::Named(_) => Ok(ty.clone()),
        }
    }

    fn validate_operand(
        ty: &Type,
        class: &str,
        aliases: &HashMap<String, Type>,
    ) -> Result<(), Diagnostic> {
        let resolved = resolved_type(ty, aliases, &mut HashSet::new())?;
        let valid = match &resolved {
            Type::Named(name) if matches!(name.as_str(), "u8" | "i8" | "bool") => {
                matches!(class, "reg8" | "mem" | "imm")
            }
            Type::Named(name) if matches!(name.as_str(), "u16" | "i16") => {
                matches!(class, "reg16" | "mem" | "imm")
            }
            Type::Named(name) if matches!(name.as_str(), "u20" | "i20" | "u24" | "i24" | "ptr") => {
                matches!(class, "reg24" | "mem" | "imm")
            }
            Type::Ptr(_) | Type::Function { .. } => {
                matches!(class, "reg16" | "reg24" | "mem" | "imm")
            }
            Type::Named(_) | Type::Array { .. } => matches!(class, "mem" | "imm"),
        };
        if valid {
            Ok(())
        } else {
            Err(Diagnostic::new(format!(
                "inline asm operand class `{class}` is incompatible with type `{resolved:?}`"
            )))
        }
    }

    fn validate_stmts(stmts: &[Stmt], aliases: &HashMap<String, Type>) -> Result<(), Diagnostic> {
        for stmt in stmts {
            match stmt {
                Stmt::Asm {
                    inputs, outputs, ..
                } => {
                    for input in inputs {
                        validate_operand(&input.ty, &input.class, aliases)?;
                    }
                    for output in outputs {
                        validate_operand(&output.ty, &output.class, aliases)?;
                    }
                }
                Stmt::If {
                    then_body,
                    else_body,
                    ..
                } => {
                    validate_stmts(then_body, aliases)?;
                    validate_stmts(else_body, aliases)?;
                }
                Stmt::While { body, .. } | Stmt::Loop { body } => {
                    validate_stmts(body, aliases)?;
                }
                _ => {}
            }
        }
        Ok(())
    }

    fn validate_declaration(
        declaration: &Declaration,
        aliases: &HashMap<String, Type>,
    ) -> Result<(), Diagnostic> {
        match declaration {
            Declaration::Function(function) => validate_stmts(&function.body, aliases),
            Declaration::Cfg { declaration, .. } | Declaration::Bank { declaration, .. } => {
                validate_declaration(declaration, aliases)
            }
            _ => Ok(()),
        }
    }

    for declaration in &program.declarations {
        validate_declaration(declaration, &aliases)?;
    }
    Ok(())
}

fn literal_int(expr: &Expr) -> Option<i64> {
    match expr {
        Expr::Int(value) | Expr::TypedInt(value, _) => Some(*value),
        _ => None,
    }
}

#[cfg(test)]
mod tests;
