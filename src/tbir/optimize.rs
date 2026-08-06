use crate::{
    ast::{
        AccessPath, AccessSegment, AssignOp, BinaryOp, Declaration, Expr, Function, Place, Program,
        Stmt, Type, UnaryOp,
    },
    compat::prelude::*,
    target::CpuFamily,
};

use super::{
    TbirAccess, TbirObjectKind, TbirOptimizationDecision, TbirOptimizationKind,
    TbirOptimizationOutcome, TbirOptimizationReport,
    provenance::OptimizationContext,
    range::{RangeAnalysis, ValueFacts},
};

#[path = "demand.rs"]
mod demand;

const COMPTIME_MAX_STEPS: usize = 4096;
const COMPTIME_MAX_CALL_DEPTH: usize = 64;
const COMPTIME_MAX_ARRAY_ELEMENTS: usize = 4096;

#[derive(Clone)]
struct ComptimeConstant {
    value: Expr,
    ty: Type,
    disabled: bool,
}

#[derive(Clone, Default)]
struct ComptimeContext {
    constants: HashMap<String, ComptimeConstant>,
    functions: HashMap<String, Function>,
    mutable_globals: HashSet<String>,
    ports: HashSet<String>,
    mmio: HashSet<String>,
}

impl ComptimeContext {
    fn from_program(program: &Program) -> Self {
        fn collect(declarations: &[Declaration], context: &mut ComptimeContext) {
            for declaration in declarations {
                match declaration {
                    Declaration::Cfg { declaration, .. }
                    | Declaration::Bank { declaration, .. } => {
                        collect(core::slice::from_ref(declaration), context);
                    }
                    Declaration::Const(constant) => {
                        context.constants.insert(
                            constant.name.clone(),
                            ComptimeConstant {
                                value: constant.value.clone(),
                                ty: constant.ty.clone(),
                                disabled: has_named_attr(&constant.attrs, "no-comptime"),
                            },
                        );
                    }
                    Declaration::Function(function) => {
                        context
                            .functions
                            .insert(function.name.clone(), function.clone());
                    }
                    Declaration::Global(global) => {
                        context.mutable_globals.insert(global.name.clone());
                    }
                    Declaration::Port(port) => {
                        context.ports.insert(port.name.clone());
                    }
                    Declaration::Mmio(mmio) => {
                        context.mmio.insert(mmio.name.clone());
                    }
                    _ => {}
                }
            }
        }

        let mut context = Self::default();
        collect(&program.declarations, &mut context);
        context
    }

    fn is_comptime_call(&self, expr: &Expr) -> Option<String> {
        let Expr::Call { path, .. } = expr else {
            return None;
        };
        let name = path.last()?.as_str();
        let function = self.functions.get(name)?;
        (has_attr(function, "comptime") && !has_attr(function, "no-comptime"))
            .then_some(name.to_owned())
    }

    fn references_enabled_constant(&self, expr: &Expr) -> bool {
        match expr {
            Expr::Ident(name) | Expr::Index { name, .. } => self
                .constants
                .get(name)
                .is_some_and(|constant| !constant.disabled),
            Expr::Access(path) => self
                .constants
                .get(&path.root)
                .is_some_and(|constant| !constant.disabled),
            Expr::Array(values) => values
                .iter()
                .any(|value| self.references_enabled_constant(value)),
            Expr::Unary { expr, .. }
            | Expr::Cast { expr, .. }
            | Expr::Deref(expr)
            | Expr::BankedPointer { pointer: expr, .. } => self.references_enabled_constant(expr),
            Expr::Binary { left, right, .. } => {
                self.references_enabled_constant(left) || self.references_enabled_constant(right)
            }
            Expr::Call { args, .. } => args.iter().any(|arg| self.references_enabled_constant(arg)),
            Expr::StructInit { fields, .. } => fields
                .iter()
                .any(|(_, value)| self.references_enabled_constant(value)),
            Expr::AddressOfIndex { index, .. } => self.references_enabled_constant(index),
            Expr::AddressOfAccess(path) => path.segments.iter().any(|segment| match segment {
                AccessSegment::Index(index) => self.references_enabled_constant(index),
                AccessSegment::Field(_) => false,
            }),
            Expr::Int(_)
            | Expr::TypedInt(_, _)
            | Expr::Bool(_)
            | Expr::Char(_)
            | Expr::String(_)
            | Expr::In(_)
            | Expr::Field { .. }
            | Expr::AddressOfField { .. }
            | Expr::AddressOf(_) => false,
        }
    }

    fn evaluate(&self, expr: &Expr) -> Result<Expr, ComptimeFailure> {
        Evaluator {
            context: self,
            locals: HashMap::new(),
            constant_stack: Vec::new(),
            call_stack: Vec::new(),
            steps: 0,
        }
        .eval_expr(expr)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ComptimeFailure {
    UnknownInput,
    MutableGlobal,
    Port,
    Mmio,
    SideEffect,
    Loop,
    Pointer,
    InlineAsm,
    Recursion,
    EvaluationLimit,
    UnsupportedCall,
    UnsupportedBody,
    Aggregate,
    OutOfBounds,
}

impl ComptimeFailure {
    fn reason(self) -> &'static str {
        match self {
            Self::UnknownInput => "not all inputs are known",
            Self::MutableGlobal => "mutable globals are not comptime",
            Self::Port => "ports are not supported",
            Self::Mmio => "MMIO is not supported",
            Self::SideEffect => "side effects are not supported",
            Self::Loop => "loops are not supported",
            Self::Pointer => "pointers are not supported",
            Self::InlineAsm => "inline asm is not supported",
            Self::Recursion => "recursion is not supported",
            Self::EvaluationLimit => "evaluation limit exceeded",
            Self::UnsupportedCall => "called function is not @comptime",
            Self::UnsupportedBody => "function body is not supported",
            Self::Aggregate => "aggregate values are not supported",
            Self::OutOfBounds => "constant array index is out of bounds",
        }
    }
}

#[derive(Clone)]
enum EvalFlow {
    Continue,
    Return(Expr),
}

struct Evaluator<'a> {
    context: &'a ComptimeContext,
    locals: HashMap<String, Expr>,
    constant_stack: Vec<String>,
    call_stack: Vec<String>,
    steps: usize,
}

impl Evaluator<'_> {
    fn step(&mut self) -> Result<(), ComptimeFailure> {
        self.steps = self.steps.saturating_add(1);
        if self.steps > COMPTIME_MAX_STEPS {
            Err(ComptimeFailure::EvaluationLimit)
        } else {
            Ok(())
        }
    }

    fn eval_expr(&mut self, expr: &Expr) -> Result<Expr, ComptimeFailure> {
        self.step()?;
        match expr {
            Expr::Int(_) | Expr::TypedInt(_, _) | Expr::Bool(_) | Expr::Char(_) => Ok(expr.clone()),
            Expr::Ident(name) => {
                if let Some(value) = self.locals.get(name) {
                    return Ok(value.clone());
                }
                if let Some(constant) = self.context.constants.get(name).cloned() {
                    if constant.disabled {
                        return Err(ComptimeFailure::UnknownInput);
                    }
                    if self.constant_stack.iter().any(|item| item == name) {
                        return Err(ComptimeFailure::Recursion);
                    }
                    self.constant_stack.push(name.clone());
                    let result = self.eval_expr(&constant.value);
                    self.constant_stack.pop();
                    let result = result?;
                    return self.complete_constant_value(result, &constant.ty);
                }
                if self.context.mutable_globals.contains(name) {
                    return Err(ComptimeFailure::MutableGlobal);
                }
                if self.context.ports.contains(name) {
                    return Err(ComptimeFailure::Port);
                }
                if self.context.mmio.contains(name) {
                    return Err(ComptimeFailure::Mmio);
                }
                Err(ComptimeFailure::UnknownInput)
            }
            Expr::Array(values) => {
                if values.len() > COMPTIME_MAX_ARRAY_ELEMENTS {
                    return Err(ComptimeFailure::EvaluationLimit);
                }
                Ok(Expr::Array(
                    values
                        .iter()
                        .map(|value| self.eval_expr(value))
                        .collect::<Result<Vec<_>, _>>()?,
                ))
            }
            Expr::Index { name, index } => {
                let base = self.eval_expr(&Expr::Ident(name.clone()))?;
                let index = self.eval_index(index)?;
                self.index_value(base, index)
            }
            Expr::Access(path) => {
                let mut value = self.eval_expr(&Expr::Ident(path.root.clone()))?;
                for segment in &path.segments {
                    match segment {
                        AccessSegment::Index(index) => {
                            let index = self.eval_index(index)?;
                            value = self.index_value(value, index)?;
                        }
                        AccessSegment::Field(_) => return Err(ComptimeFailure::Aggregate),
                    }
                }
                Ok(value)
            }
            Expr::Unary { op, expr } => {
                let value = self.eval_expr(expr)?;
                self.eval_unary(*op, value)
            }
            Expr::Binary { left, op, right } => {
                let left = self.eval_expr(left)?;
                if matches!(op, BinaryOp::And | BinaryOp::Or) {
                    let left_truth = literal_truth(&left).ok_or(ComptimeFailure::UnknownInput)?;
                    if (*op == BinaryOp::And && !left_truth) || (*op == BinaryOp::Or && left_truth)
                    {
                        return Ok(Expr::Bool(left_truth));
                    }
                }
                let right = self.eval_expr(right)?;
                eval_binary_literals(&left, *op, &right)
            }
            Expr::Cast { ty, expr } => {
                let value = self.eval_expr(expr)?;
                self.eval_cast(ty, value)
            }
            Expr::Call { path, args } => self.eval_call(path, args),
            Expr::String(_) => Err(ComptimeFailure::Pointer),
            Expr::In(_) => Err(ComptimeFailure::Port),
            Expr::AddressOf(_)
            | Expr::AddressOfIndex { .. }
            | Expr::AddressOfField { .. }
            | Expr::AddressOfAccess(_)
            | Expr::Deref(_)
            | Expr::BankedPointer { .. } => Err(ComptimeFailure::Pointer),
            Expr::Field { .. } | Expr::StructInit { .. } => Err(ComptimeFailure::Aggregate),
        }
    }

    fn eval_index(&mut self, expr: &Expr) -> Result<usize, ComptimeFailure> {
        let value = self.eval_expr(expr)?;
        let value = literal_int(&value).ok_or(ComptimeFailure::UnknownInput)?;
        usize::try_from(value).map_err(|_| ComptimeFailure::OutOfBounds)
    }

    fn complete_constant_value(&mut self, value: Expr, ty: &Type) -> Result<Expr, ComptimeFailure> {
        if type_contains_pointer(ty) {
            return Err(ComptimeFailure::Pointer);
        }
        let Type::Array { element, len } = ty else {
            return Ok(value);
        };
        let len = self.eval_index(len)?;
        if len > COMPTIME_MAX_ARRAY_ELEMENTS {
            return Err(ComptimeFailure::EvaluationLimit);
        }
        let Expr::Array(values) = value else {
            return Err(ComptimeFailure::Aggregate);
        };
        if values.len() > len {
            return Err(ComptimeFailure::OutOfBounds);
        }
        let mut completed = Vec::with_capacity(len);
        for index in 0..len {
            let value = values.get(index).cloned().unwrap_or(zero_value(element)?);
            completed.push(self.complete_constant_value(value, element)?);
        }
        Ok(Expr::Array(completed))
    }

    fn index_value(&self, value: Expr, index: usize) -> Result<Expr, ComptimeFailure> {
        let Expr::Array(values) = value else {
            return Err(ComptimeFailure::Aggregate);
        };
        values
            .get(index)
            .cloned()
            .ok_or(ComptimeFailure::OutOfBounds)
    }

    fn eval_unary(&self, op: UnaryOp, value: Expr) -> Result<Expr, ComptimeFailure> {
        match (op, value) {
            (UnaryOp::Not, Expr::Bool(value)) => Ok(Expr::Bool(!value)),
            (UnaryOp::Neg, Expr::Int(value)) => Ok(Expr::Int(value.wrapping_neg())),
            (UnaryOp::Neg, Expr::TypedInt(value, ty)) => {
                Ok(typed_integer(value.wrapping_neg(), ty))
            }
            (UnaryOp::BitNot, Expr::Int(value)) => Ok(Expr::Int(!value)),
            (UnaryOp::BitNot, Expr::TypedInt(value, ty)) => Ok(typed_integer(!value, ty)),
            _ => Err(ComptimeFailure::UnsupportedBody),
        }
    }

    fn eval_cast(&self, ty: &Type, value: Expr) -> Result<Expr, ComptimeFailure> {
        if type_contains_pointer(ty) {
            return Err(ComptimeFailure::Pointer);
        }
        if matches!(ty, Type::Named(name) if name == "bool") {
            return Ok(Expr::Bool(
                literal_truth(&value).ok_or(ComptimeFailure::UnknownInput)?,
            ));
        }
        let value = literal_int(&value).ok_or(ComptimeFailure::UnsupportedBody)?;
        Ok(typed_integer(value, ty.clone()))
    }

    fn eval_call(&mut self, path: &[String], args: &[Expr]) -> Result<Expr, ComptimeFailure> {
        let name = path.last().ok_or(ComptimeFailure::UnsupportedCall)?;
        let function = self
            .context
            .functions
            .get(name)
            .ok_or(ComptimeFailure::UnsupportedCall)?
            .clone();
        if !has_attr(&function, "comptime") || has_attr(&function, "no-comptime") {
            return Err(ComptimeFailure::UnsupportedCall);
        }
        if function.return_type.is_none() {
            return Err(ComptimeFailure::UnsupportedBody);
        }
        if function.params.len() != args.len()
            || function
                .params
                .iter()
                .any(|param| type_contains_pointer(&param.ty))
            || function
                .return_type
                .as_ref()
                .is_some_and(type_contains_pointer)
        {
            return Err(ComptimeFailure::Pointer);
        }
        if self.call_stack.len() >= COMPTIME_MAX_CALL_DEPTH
            || self.call_stack.iter().any(|item| item == name)
        {
            return Err(if self.call_stack.len() >= COMPTIME_MAX_CALL_DEPTH {
                ComptimeFailure::EvaluationLimit
            } else {
                ComptimeFailure::Recursion
            });
        }
        let values = args
            .iter()
            .map(|arg| self.eval_expr(arg))
            .collect::<Result<Vec<_>, _>>()?;
        if values
            .iter()
            .any(|value| matches!(value, Expr::Array(_) | Expr::StructInit { .. }))
        {
            return Err(ComptimeFailure::Aggregate);
        }
        let caller_locals = core::mem::replace(
            &mut self.locals,
            function
                .params
                .iter()
                .zip(values)
                .map(|(param, value)| (param.name.clone(), value))
                .collect(),
        );
        self.call_stack.push(name.clone());
        let result = self.eval_stmts(&function.body);
        self.call_stack.pop();
        self.locals = caller_locals;
        match result? {
            EvalFlow::Return(value) => Ok(value),
            EvalFlow::Continue => Err(ComptimeFailure::UnsupportedBody),
        }
    }

    fn eval_stmts(&mut self, stmts: &[Stmt]) -> Result<EvalFlow, ComptimeFailure> {
        for stmt in stmts {
            self.step()?;
            match stmt {
                Stmt::Let { name, value, .. } => {
                    let value = self.eval_expr(value)?;
                    if matches!(value, Expr::Array(_) | Expr::StructInit { .. }) {
                        return Err(ComptimeFailure::Aggregate);
                    }
                    self.locals.insert(name.clone(), value);
                }
                Stmt::LetTwo { .. } | Stmt::ReturnTwo { .. } => {
                    return Err(ComptimeFailure::UnsupportedBody);
                }
                Stmt::If {
                    condition,
                    then_body,
                    else_body,
                } => {
                    let condition = self.eval_expr(condition)?;
                    let truth = literal_truth(&condition).ok_or(ComptimeFailure::UnknownInput)?;
                    let flow = if truth {
                        self.eval_stmts(then_body)?
                    } else {
                        self.eval_stmts(else_body)?
                    };
                    if matches!(flow, EvalFlow::Return(_)) {
                        return Ok(flow);
                    }
                }
                Stmt::Return(Some(value)) => {
                    let value = self.eval_expr(value)?;
                    if matches!(value, Expr::Array(_) | Expr::StructInit { .. }) {
                        return Err(ComptimeFailure::Aggregate);
                    }
                    return Ok(EvalFlow::Return(value));
                }
                Stmt::Return(None) => return Err(ComptimeFailure::UnsupportedBody),
                Stmt::Expr(value) => {
                    self.eval_expr(value)?;
                }
                Stmt::Assign { .. } => return Err(ComptimeFailure::SideEffect),
                Stmt::While { .. } | Stmt::Loop { .. } | Stmt::Break | Stmt::Continue => {
                    return Err(ComptimeFailure::Loop);
                }
                Stmt::Asm { .. } => return Err(ComptimeFailure::InlineAsm),
                Stmt::Out { .. } => return Err(ComptimeFailure::Port),
            }
        }
        Ok(EvalFlow::Continue)
    }
}

fn has_named_attr(attrs: &[String], name: &str) -> bool {
    attrs.iter().any(|attr| attr == name)
}

fn type_contains_pointer(ty: &Type) -> bool {
    match ty {
        Type::Ptr(_) | Type::Function { .. } => true,
        Type::Array { element, .. } => type_contains_pointer(element),
        Type::Named(name) if name == "ptr" => true,
        Type::Named(_) => false,
    }
}

fn zero_value(ty: &Type) -> Result<Expr, ComptimeFailure> {
    match ty {
        Type::Named(name) if name == "bool" => Ok(Expr::Bool(false)),
        Type::Named(name) if name == "char" => Ok(Expr::Char(0)),
        Type::Named(name)
            if matches!(
                name.as_str(),
                "u8" | "i8" | "u16" | "i16" | "u24" | "i24" | "u32" | "i32"
            ) =>
        {
            Ok(Expr::TypedInt(0, ty.clone()))
        }
        Type::Array { .. } => Ok(Expr::Array(Vec::new())),
        Type::Ptr(_) | Type::Function { .. } => Err(ComptimeFailure::Pointer),
        Type::Named(_) => Err(ComptimeFailure::Aggregate),
    }
}

fn literal_int(expr: &Expr) -> Option<i64> {
    match expr {
        Expr::Int(value) | Expr::TypedInt(value, _) => Some(*value),
        Expr::Bool(value) => Some(i64::from(*value)),
        Expr::Char(value) => Some(i64::from(*value)),
        _ => None,
    }
}

fn literal_truth(expr: &Expr) -> Option<bool> {
    match expr {
        Expr::Bool(value) => Some(*value),
        Expr::Int(value) | Expr::TypedInt(value, _) => Some(*value != 0),
        Expr::Char(value) => Some(*value != 0),
        _ => None,
    }
}

fn typed_integer(value: i64, ty: Type) -> Expr {
    let Some(bits) = integer_bits(&ty) else {
        return Expr::Int(value);
    };
    let mask = (1_i64 << bits) - 1;
    let raw = value & mask;
    let value = if matches!(&ty, Type::Named(name) if name.starts_with('i'))
        && raw & (1_i64 << (bits - 1)) != 0
    {
        raw - (mask + 1)
    } else {
        raw
    };
    Expr::TypedInt(value, ty)
}

fn integer_bits(ty: &Type) -> Option<u32> {
    match ty {
        Type::Named(name) if matches!(name.as_str(), "u8" | "i8") => Some(8),
        Type::Named(name) if matches!(name.as_str(), "u16" | "i16") => Some(16),
        Type::Named(name) if matches!(name.as_str(), "u24" | "i24") => Some(24),
        Type::Named(name) if matches!(name.as_str(), "u32" | "i32") => Some(32),
        _ => None,
    }
}

fn eval_binary_literals(left: &Expr, op: BinaryOp, right: &Expr) -> Result<Expr, ComptimeFailure> {
    if let (Expr::Bool(left), Expr::Bool(right)) = (left, right) {
        return match op {
            BinaryOp::And => Ok(Expr::Bool(*left && *right)),
            BinaryOp::Or => Ok(Expr::Bool(*left || *right)),
            BinaryOp::Eq => Ok(Expr::Bool(left == right)),
            BinaryOp::Ne => Ok(Expr::Bool(left != right)),
            _ => Err(ComptimeFailure::UnsupportedBody),
        };
    }
    let left_value = literal_int(left).ok_or(ComptimeFailure::UnsupportedBody)?;
    let right_value = literal_int(right).ok_or(ComptimeFailure::UnsupportedBody)?;
    let value = match op {
        BinaryOp::Mul => left_value.wrapping_mul(right_value),
        BinaryOp::Div => left_value.checked_div(right_value).unwrap_or(0),
        BinaryOp::Mod => left_value.checked_rem(right_value).unwrap_or(0),
        BinaryOp::Add => left_value.wrapping_add(right_value),
        BinaryOp::Sub => left_value.wrapping_sub(right_value),
        BinaryOp::Shl => left_value.checked_shl(right_value as u32).unwrap_or(0),
        BinaryOp::Shr => {
            let count = right_value as u32;
            match left {
                Expr::TypedInt(_, ty) if matches!(ty, Type::Named(name) if name.starts_with('i')) =>
                {
                    let bits = integer_bits(ty).ok_or(ComptimeFailure::UnsupportedBody)?;
                    if count >= bits {
                        if left_value < 0 { -1 } else { 0 }
                    } else {
                        left_value >> count
                    }
                }
                _ => left_value.checked_shr(count).unwrap_or(0),
            }
        }
        BinaryOp::Lt => return Ok(Expr::Bool(left_value < right_value)),
        BinaryOp::Le => return Ok(Expr::Bool(left_value <= right_value)),
        BinaryOp::Gt => return Ok(Expr::Bool(left_value > right_value)),
        BinaryOp::Ge => return Ok(Expr::Bool(left_value >= right_value)),
        BinaryOp::Eq => return Ok(Expr::Bool(left_value == right_value)),
        BinaryOp::Ne => return Ok(Expr::Bool(left_value != right_value)),
        BinaryOp::BitAnd => left_value & right_value,
        BinaryOp::BitXor => left_value ^ right_value,
        BinaryOp::BitOr => left_value | right_value,
        BinaryOp::And => return Ok(Expr::Bool(left_value != 0 && right_value != 0)),
        BinaryOp::Or => return Ok(Expr::Bool(left_value != 0 || right_value != 0)),
    };
    let ty = match left {
        Expr::TypedInt(_, ty) => Some(ty.clone()),
        _ => match right {
            Expr::TypedInt(_, ty) => Some(ty.clone()),
            _ => None,
        },
    };
    Ok(ty.map_or(Expr::Int(value), |ty| typed_integer(value, ty)))
}

pub fn optimize_program(program: &Program, cpu: CpuFamily) -> (Program, TbirOptimizationReport) {
    optimize_program_with_context(program, cpu, &OptimizationContext::default())
}

pub fn optimize_program_with_context(
    program: &Program,
    cpu: CpuFamily,
    context: &OptimizationContext,
) -> (Program, TbirOptimizationReport) {
    let mut program = program.clone();
    let comptime = ComptimeContext::from_program(&program);
    let mut report = TbirOptimizationReport::default();
    // Keep the stage order visible: later passes rely on the safety facts and
    // normalized expressions produced by earlier stages.
    scalar_simplify_program(&mut program, &mut report, true, &comptime);
    hoist_pure_loop_invariants_program(&mut program, &mut report);
    local_propagation_and_cse_program(&mut program, context, &mut report);
    scalar_simplify_program(&mut program, &mut report, false, &comptime);
    known_bits_program(&mut program);
    hoist_named_memory_reads_program(&mut program, context, &mut report);
    expand_inline_functions(&mut program, context, &mut report, &comptime);
    demand::apply_program(&mut program);
    remove_unused_pure_lets_program(&mut program);
    run_tail_passes(&mut program, cpu, &mut report);
    (program, report)
}

fn functions(program: &Program) -> Vec<&Function> {
    fn collect<'a>(declarations: &'a [Declaration], output: &mut Vec<&'a Function>) {
        for declaration in declarations {
            match declaration {
                Declaration::Function(function) => output.push(function),
                Declaration::Cfg { declaration, .. } | Declaration::Bank { declaration, .. } => {
                    collect(core::slice::from_ref(declaration), output)
                }
                _ => {}
            }
        }
    }
    let mut output = Vec::new();
    collect(&program.declarations, &mut output);
    output
}

fn inline_function_names(program: &Program) -> HashSet<String> {
    functions(program)
        .into_iter()
        .filter(|function| has_attr(function, "inline") && !has_attr(function, "no-comptime"))
        .map(|function| function.name.clone())
        .collect()
}

fn expand_inline_functions(
    program: &mut Program,
    context: &OptimizationContext,
    report: &mut TbirOptimizationReport,
    comptime: &ComptimeContext,
) {
    let all_functions = functions(program);
    let mut graph = HashMap::new();
    for function in &all_functions {
        let mut calls = HashSet::new();
        collect_calls(&function.body, &mut calls);
        graph.insert(function.name.clone(), calls);
    }

    let mut approved = HashMap::new();
    let mut rejected = Vec::new();
    for function in all_functions {
        if !has_attr(function, "inline") {
            continue;
        }
        let reason = inline_rejection(function, &graph);
        if let Some(reason) = reason {
            rejected.push((function.name.clone(), reason));
        } else {
            approved.insert(function.name.clone(), function.clone());
        }
    }
    for (name, reason) in rejected {
        decision(
            report,
            TbirOptimizationKind::Inline,
            None,
            &name,
            Some(reason),
        );
    }

    let mut used_names = HashSet::new();
    for function in functions(program) {
        used_names.insert(function.name.clone());
        for param in &function.params {
            used_names.insert(param.name.clone());
        }
        collect_local_names(&function.body, &mut used_names);
    }
    let mut expander = InlineExpander {
        approved: &approved,
        used_names,
        next_temp: 0,
        counts: HashMap::new(),
        active: Vec::new(),
    };
    expand_inline_declarations(&mut program.declarations, &mut expander);

    let expanded_any = !expander.counts.is_empty();
    let mut counts: Vec<_> = expander.counts.into_iter().collect();
    counts.sort_by(|left, right| left.0.cmp(&right.0));
    for (name, count) in counts {
        decision(report, TbirOptimizationKind::Inline, None, &name, None);
        if let Some(last) = report.decisions.last_mut() {
            last.reason = format!("approved; transformed {count} call(s)");
        }
    }

    if expanded_any {
        let mut cleanup_report = TbirOptimizationReport::default();
        scalar_simplify_program(program, &mut cleanup_report, false, comptime);
        local_propagation_and_cse_program(program, context, &mut cleanup_report);
        scalar_simplify_program(program, &mut cleanup_report, false, comptime);
    }
}

fn inline_rejection<'a>(
    function: &Function,
    graph: &HashMap<String, HashSet<String>>,
) -> Option<&'a str> {
    if has_attr(function, "no-comptime") {
        Some("no-comptime attribute")
    } else if has_attr(function, "naked") {
        Some("naked function")
    } else if has_attr(function, "interrupt") {
        Some("interrupt function")
    } else if graph
        .get(&function.name)
        .is_some_and(|calls| calls.contains(&function.name))
    {
        Some("direct recursion")
    } else if graph.get(&function.name).is_some_and(|calls| {
        calls
            .iter()
            .any(|callee| reachable(callee, &function.name, graph, &mut HashSet::new()))
    }) {
        Some("mutual recursion")
    } else if function.body.iter().any(stmt_has_inline_forbidden_control) {
        Some("inline assembly or control exit")
    } else if inline_return_body(function).is_none() && inline_void_body(function).is_none() {
        Some("unsupported body shape")
    } else {
        None
    }
}

fn stmt_has_inline_forbidden_control(stmt: &Stmt) -> bool {
    match stmt {
        Stmt::Asm { .. } | Stmt::Break | Stmt::Continue => true,
        Stmt::If {
            then_body,
            else_body,
            ..
        } => then_body
            .iter()
            .chain(else_body)
            .any(stmt_has_inline_forbidden_control),
        Stmt::While { body, .. } | Stmt::Loop { body } => {
            body.iter().any(stmt_has_inline_forbidden_control)
        }
        _ => false,
    }
}

struct InlineExpander<'a> {
    approved: &'a HashMap<String, Function>,
    used_names: HashSet<String>,
    next_temp: usize,
    counts: HashMap<String, usize>,
    active: Vec<String>,
}

fn expand_inline_declarations(declarations: &mut [Declaration], expander: &mut InlineExpander<'_>) {
    for declaration in declarations {
        match declaration {
            Declaration::Function(function) => {
                function.body = expander.expand_stmts(core::mem::take(&mut function.body));
            }
            Declaration::Cfg { declaration, .. } | Declaration::Bank { declaration, .. } => {
                expand_inline_declarations(core::slice::from_mut(declaration), expander);
            }
            _ => {}
        }
    }
}

impl InlineExpander<'_> {
    fn unique(&mut self, kind: &str) -> String {
        loop {
            let name = format!("__tbir_inline_{kind}_{}", self.next_temp);
            self.next_temp += 1;
            if self.used_names.insert(name.clone()) {
                return name;
            }
        }
    }

    fn expand_stmts(&mut self, stmts: Vec<Stmt>) -> Vec<Stmt> {
        let mut output = Vec::new();
        for stmt in stmts {
            self.expand_stmt(stmt, &mut output);
        }
        output
    }

    fn expand_stmt(&mut self, stmt: Stmt, output: &mut Vec<Stmt>) {
        match stmt {
            Stmt::Let { name, ty, value } => {
                let value = self.expand_expr(value, output);
                output.push(Stmt::Let { name, ty, value });
            }
            Stmt::Assign { target, op, value } => {
                let target = self.expand_place(target, output);
                let value = self.expand_expr(value, output);
                output.push(Stmt::Assign { target, op, value });
            }
            Stmt::Return(value) => {
                let value = value.map(|value| self.expand_expr(value, output));
                output.push(Stmt::Return(value));
            }
            Stmt::Out { port, value } => {
                let value = self.expand_expr(value, output);
                output.push(Stmt::Out { port, value });
            }
            Stmt::Expr(Expr::Call { path, args }) if self.is_approved_void(&path) => {
                self.expand_void_call(path, args, output);
            }
            Stmt::Expr(value) => {
                let value = self.expand_expr(value, output);
                output.push(Stmt::Expr(value));
            }
            Stmt::If {
                condition,
                then_body,
                else_body,
            } => {
                let condition = self.expand_expr(condition, output);
                output.push(Stmt::If {
                    condition,
                    then_body: self.expand_stmts(then_body),
                    else_body: self.expand_stmts(else_body),
                });
            }
            Stmt::While { condition, body } => output.push(Stmt::While {
                condition,
                body: self.expand_stmts(body),
            }),
            Stmt::Loop { body } => output.push(Stmt::Loop {
                body: self.expand_stmts(body),
            }),
            stmt => output.push(stmt),
        }
    }

    fn is_approved_void(&self, path: &[String]) -> bool {
        path.last()
            .and_then(|name| self.approved.get(name))
            .is_some_and(|f| f.return_type.is_none())
    }

    fn expand_void_call(&mut self, path: Vec<String>, args: Vec<Expr>, output: &mut Vec<Stmt>) {
        let Some(name) = path.last().cloned() else {
            return;
        };
        let Some(function) = self.approved.get(&name).cloned() else {
            output.push(Stmt::Expr(Expr::Call { path, args }));
            return;
        };
        if self.active.contains(&name) || args.len() != function.params.len() {
            output.push(Stmt::Expr(Expr::Call { path, args }));
            return;
        }
        let (bindings, rename) = self.bind_arguments(&function, args, output);
        output.extend(bindings);
        let body = inline_void_body(&function).unwrap_or(&[]).to_vec();
        self.active.push(name.clone());
        output.extend(self.expand_stmts(rename_stmts(body, &rename)));
        self.active.pop();
        *self.counts.entry(name).or_default() += 1;
    }

    fn expand_expr(&mut self, expr: Expr, prefix: &mut Vec<Stmt>) -> Expr {
        match expr {
            Expr::Call { path, args } => {
                let mut expanded_args = Vec::new();
                for arg in args {
                    expanded_args.push(self.expand_expr(arg, prefix));
                }
                let Some(name) = path.last().cloned() else {
                    return Expr::Call {
                        path,
                        args: expanded_args,
                    };
                };
                let Some(function) = self.approved.get(&name).cloned() else {
                    return Expr::Call {
                        path,
                        args: expanded_args,
                    };
                };
                let Some(return_ty) = function.return_type.clone() else {
                    return Expr::Call {
                        path,
                        args: expanded_args,
                    };
                };
                if self.active.contains(&name) || expanded_args.len() != function.params.len() {
                    return Expr::Call {
                        path,
                        args: expanded_args,
                    };
                }
                let (bindings, rename) = self.bind_arguments(&function, expanded_args, prefix);
                prefix.extend(bindings);
                let (body, result) = inline_return_body(&function).expect("approved value inline");
                self.active.push(name.clone());
                prefix.extend(self.expand_stmts(rename_stmts(body.to_vec(), &rename)));
                let result = self.expand_expr(rename_expr(result, &rename), prefix);
                self.active.pop();
                let result_name = self.unique("result");
                prefix.push(Stmt::Let {
                    name: result_name.clone(),
                    ty: return_ty,
                    value: result,
                });
                *self.counts.entry(name).or_default() += 1;
                Expr::Ident(result_name)
            }
            Expr::Array(values) => Expr::Array(
                values
                    .into_iter()
                    .map(|v| self.expand_expr(v, prefix))
                    .collect(),
            ),
            Expr::Index { name, index } => Expr::Index {
                name,
                index: Box::new(self.expand_expr(*index, prefix)),
            },
            Expr::AddressOfIndex { name, index } => Expr::AddressOfIndex {
                name,
                index: Box::new(self.expand_expr(*index, prefix)),
            },
            Expr::Deref(expr) => Expr::Deref(Box::new(self.expand_expr(*expr, prefix))),
            Expr::BankedPointer { pointer, bank } => Expr::BankedPointer {
                pointer: Box::new(self.expand_expr(*pointer, prefix)),
                bank,
            },
            Expr::Unary { op, expr } => Expr::Unary {
                op,
                expr: Box::new(self.expand_expr(*expr, prefix)),
            },
            Expr::Cast { ty, expr } => Expr::Cast {
                ty,
                expr: Box::new(self.expand_expr(*expr, prefix)),
            },
            Expr::Binary { left, op, right } => {
                let left = Box::new(self.expand_expr(*left, prefix));
                let right = if matches!(op, BinaryOp::And | BinaryOp::Or) {
                    right
                } else {
                    Box::new(self.expand_expr(*right, prefix))
                };
                Expr::Binary { left, op, right }
            }
            Expr::Access(path) => Expr::Access(self.expand_access(path, prefix)),
            Expr::AddressOfAccess(path) => Expr::AddressOfAccess(self.expand_access(path, prefix)),
            Expr::StructInit { ty, fields } => Expr::StructInit {
                ty,
                fields: fields
                    .into_iter()
                    .map(|(n, v)| (n, self.expand_expr(v, prefix)))
                    .collect(),
            },
            expr => expr,
        }
    }

    fn bind_arguments(
        &mut self,
        function: &Function,
        args: Vec<Expr>,
        prefix: &mut Vec<Stmt>,
    ) -> (Vec<Stmt>, HashMap<String, String>) {
        let mut bindings = Vec::new();
        let mut rename = HashMap::new();
        for (param, arg) in function.params.iter().zip(args) {
            let arg_name = self.unique("arg");
            prefix.push(Stmt::Let {
                name: arg_name.clone(),
                ty: param.ty.clone(),
                value: arg,
            });
            let param_name = self.unique("param");
            bindings.push(Stmt::Let {
                name: param_name.clone(),
                ty: param.ty.clone(),
                value: Expr::Ident(arg_name),
            });
            rename.insert(param.name.clone(), param_name);
        }
        let mut locals = HashSet::new();
        collect_local_names(&function.body, &mut locals);
        for local in locals {
            rename.entry(local).or_insert_with(|| self.unique("local"));
        }
        (bindings, rename)
    }

    fn expand_place(&mut self, place: Place, prefix: &mut Vec<Stmt>) -> Place {
        match place {
            Place::Index { name, index } => Place::Index {
                name,
                index: Box::new(self.expand_expr(*index, prefix)),
            },
            Place::Access(path) => Place::Access(self.expand_access(path, prefix)),
            Place::Deref(expr) => Place::Deref(Box::new(self.expand_expr(*expr, prefix))),
            place => place,
        }
    }

    fn expand_access(&mut self, mut path: AccessPath, prefix: &mut Vec<Stmt>) -> AccessPath {
        for segment in &mut path.segments {
            if let AccessSegment::Index(index) = segment {
                **index = self.expand_expr((**index).clone(), prefix);
            }
        }
        path
    }
}

fn renamed(name: String, names: &HashMap<String, String>) -> String {
    names.get(&name).cloned().unwrap_or(name)
}

fn rename_stmts(stmts: Vec<Stmt>, names: &HashMap<String, String>) -> Vec<Stmt> {
    stmts
        .into_iter()
        .map(|stmt| match stmt {
            Stmt::Let { name, ty, value } => Stmt::Let {
                name: renamed(name, names),
                ty,
                value: rename_expr(value, names),
            },
            Stmt::Assign { target, op, value } => Stmt::Assign {
                target: rename_place(target, names),
                op,
                value: rename_expr(value, names),
            },
            Stmt::If {
                condition,
                then_body,
                else_body,
            } => Stmt::If {
                condition: rename_expr(condition, names),
                then_body: rename_stmts(then_body, names),
                else_body: rename_stmts(else_body, names),
            },
            Stmt::While { condition, body } => Stmt::While {
                condition: rename_expr(condition, names),
                body: rename_stmts(body, names),
            },
            Stmt::Loop { body } => Stmt::Loop {
                body: rename_stmts(body, names),
            },
            Stmt::Return(value) => Stmt::Return(value.map(|v| rename_expr(v, names))),
            Stmt::Out { port, value } => Stmt::Out {
                port,
                value: rename_expr(value, names),
            },
            Stmt::Expr(value) => Stmt::Expr(rename_expr(value, names)),
            Stmt::Asm {
                volatile,
                inputs,
                outputs,
                clobbers,
                lines,
            } => {
                let inputs = inputs
                    .into_iter()
                    .map(|mut input| {
                        input.name = renamed(input.name, names);
                        input
                    })
                    .collect();
                let outputs = outputs
                    .into_iter()
                    .map(|mut output| {
                        output.name = renamed(output.name, names);
                        output
                    })
                    .collect();
                let lines = lines
                    .into_iter()
                    .map(|mut line| {
                        for (old, new) in names {
                            line = line.replace(&format!("{{{old}}}"), &format!("{{{new}}}"));
                        }
                        line
                    })
                    .collect();
                Stmt::Asm {
                    volatile,
                    inputs,
                    outputs,
                    clobbers,
                    lines,
                }
            }
            stmt => stmt,
        })
        .collect()
}

fn rename_place(place: Place, names: &HashMap<String, String>) -> Place {
    match place {
        Place::Ident(name) => Place::Ident(renamed(name, names)),
        Place::Index { name, index } => Place::Index {
            name: renamed(name, names),
            index: Box::new(rename_expr(*index, names)),
        },
        Place::Field { base, field } => Place::Field {
            base: renamed(base, names),
            field,
        },
        Place::Access(path) => Place::Access(rename_access(path, names)),
        Place::Deref(expr) => Place::Deref(Box::new(rename_expr(*expr, names))),
    }
}

fn rename_access(mut path: AccessPath, names: &HashMap<String, String>) -> AccessPath {
    path.root = renamed(path.root, names);
    for segment in &mut path.segments {
        if let AccessSegment::Index(index) = segment {
            **index = rename_expr((**index).clone(), names);
        }
    }
    path
}

fn rename_expr(expr: Expr, names: &HashMap<String, String>) -> Expr {
    match expr {
        Expr::Ident(name) => Expr::Ident(renamed(name, names)),
        Expr::Index { name, index } => Expr::Index {
            name: renamed(name, names),
            index: Box::new(rename_expr(*index, names)),
        },
        Expr::Field { base, field } => Expr::Field {
            base: renamed(base, names),
            field,
        },
        Expr::AddressOfIndex { name, index } => Expr::AddressOfIndex {
            name: renamed(name, names),
            index: Box::new(rename_expr(*index, names)),
        },
        Expr::AddressOfField { base, field } => Expr::AddressOfField {
            base: renamed(base, names),
            field,
        },
        Expr::Access(path) => Expr::Access(rename_access(path, names)),
        Expr::AddressOfAccess(path) => Expr::AddressOfAccess(rename_access(path, names)),
        Expr::AddressOf(name) => Expr::AddressOf(renamed(name, names)),
        Expr::Array(values) => {
            Expr::Array(values.into_iter().map(|v| rename_expr(v, names)).collect())
        }
        Expr::StructInit { ty, fields } => Expr::StructInit {
            ty,
            fields: fields
                .into_iter()
                .map(|(n, v)| (n, rename_expr(v, names)))
                .collect(),
        },
        Expr::Deref(expr) => Expr::Deref(Box::new(rename_expr(*expr, names))),
        Expr::BankedPointer { pointer, bank } => Expr::BankedPointer {
            pointer: Box::new(rename_expr(*pointer, names)),
            bank,
        },
        Expr::Call { path, args } => Expr::Call {
            path,
            args: args.into_iter().map(|v| rename_expr(v, names)).collect(),
        },
        Expr::Unary { op, expr } => Expr::Unary {
            op,
            expr: Box::new(rename_expr(*expr, names)),
        },
        Expr::Binary { left, op, right } => Expr::Binary {
            left: Box::new(rename_expr(*left, names)),
            op,
            right: Box::new(rename_expr(*right, names)),
        },
        Expr::Cast { ty, expr } => Expr::Cast {
            ty,
            expr: Box::new(rename_expr(*expr, names)),
        },
        expr => expr,
    }
}

fn reachable(
    current: &str,
    target: &str,
    graph: &HashMap<String, HashSet<String>>,
    visited: &mut HashSet<String>,
) -> bool {
    if current == target {
        return true;
    }
    if !visited.insert(current.to_owned()) {
        return false;
    }
    graph.get(current).is_some_and(|next| {
        next.iter()
            .any(|name| reachable(name, target, graph, visited))
    })
}

fn collect_calls(stmts: &[Stmt], calls: &mut HashSet<String>) {
    for stmt in stmts {
        match stmt {
            Stmt::Let { value, .. }
            | Stmt::Assign { value, .. }
            | Stmt::Return(Some(value))
            | Stmt::Out { value, .. }
            | Stmt::Expr(value) => collect_expr_calls(value, calls),
            Stmt::If {
                condition,
                then_body,
                else_body,
            } => {
                collect_expr_calls(condition, calls);
                collect_calls(then_body, calls);
                collect_calls(else_body, calls);
            }
            Stmt::While { condition, body } => {
                collect_expr_calls(condition, calls);
                collect_calls(body, calls);
            }
            Stmt::Loop { body } => collect_calls(body, calls),
            _ => {}
        }
    }
}

fn collect_expr_calls(expr: &Expr, calls: &mut HashSet<String>) {
    match expr {
        Expr::Call { path, args } => {
            if let Some(name) = path.last() {
                calls.insert(name.clone());
            }
            for arg in args {
                collect_expr_calls(arg, calls);
            }
        }
        Expr::Array(values) => {
            for value in values {
                collect_expr_calls(value, calls);
            }
        }
        Expr::Index { index, .. }
        | Expr::AddressOfIndex { index, .. }
        | Expr::Deref(index)
        | Expr::BankedPointer { pointer: index, .. }
        | Expr::Unary { expr: index, .. }
        | Expr::Cast { expr: index, .. } => collect_expr_calls(index, calls),
        Expr::Access(path) | Expr::AddressOfAccess(path) => {
            for segment in &path.segments {
                if let AccessSegment::Index(index) = segment {
                    collect_expr_calls(index, calls);
                }
            }
        }
        Expr::StructInit { fields, .. } => {
            for (_, value) in fields {
                collect_expr_calls(value, calls);
            }
        }
        Expr::Binary { left, right, .. } => {
            collect_expr_calls(left, calls);
            collect_expr_calls(right, calls);
        }
        _ => {}
    }
}

#[derive(Clone)]
struct FunctionFacts {
    attrs: Vec<String>,
    params: Vec<crate::ast::Param>,
    return_type: Option<Type>,
}

fn run_tail_passes(program: &mut Program, cpu: CpuFamily, report: &mut TbirOptimizationReport) {
    let facts: HashMap<String, FunctionFacts> = functions(program)
        .into_iter()
        .map(|function| {
            (
                function.name.clone(),
                FunctionFacts {
                    attrs: function.attrs.clone(),
                    params: function.params.clone(),
                    return_type: function.return_type.clone(),
                },
            )
        })
        .collect();
    decide_tail_calls_in_declarations(&mut program.declarations, cpu, &facts, report);
}

fn decide_tail_calls_in_declarations(
    declarations: &mut [Declaration],
    cpu: CpuFamily,
    facts: &HashMap<String, FunctionFacts>,
    report: &mut TbirOptimizationReport,
) {
    for declaration in declarations {
        match declaration {
            Declaration::Cfg { declaration, .. } | Declaration::Bank { declaration, .. } => {
                decide_tail_calls_in_declarations(
                    core::slice::from_mut(declaration),
                    cpu,
                    facts,
                    report,
                )
            }
            Declaration::Function(function) => {
                let mut targets = Vec::new();
                collect_tail_targets(
                    &function.body,
                    &function.name,
                    function.return_type.is_none(),
                    &mut targets,
                );
                let mut seen = HashSet::new();
                for callee in targets {
                    if !seen.insert(callee.clone()) {
                        continue;
                    }
                    if callee == function.name {
                        let reason = tail_recursion_rejection(function);
                        decision(
                            report,
                            TbirOptimizationKind::TailRecursion,
                            Some(&function.name),
                            &callee,
                            reason,
                        );
                        if reason.is_none() {
                            rewrite_tail_recursion(function);
                        }
                    } else {
                        let reason = sibling_tail_rejection(function, &callee, cpu, facts);
                        decision(
                            report,
                            TbirOptimizationKind::TailCall,
                            Some(&function.name),
                            &callee,
                            reason,
                        );
                    }
                }
            }
            _ => {}
        }
    }
}

fn collect_tail_targets(
    stmts: &[Stmt],
    function_name: &str,
    allow_void_tail: bool,
    targets: &mut Vec<String>,
) {
    let mut reachable = true;
    for (index, stmt) in stmts.iter().enumerate() {
        if !reachable {
            break;
        }
        match stmt {
            Stmt::Return(Some(Expr::Call { path, .. })) => {
                if let Some(name) = path.last() {
                    targets.push(name.clone());
                }
            }
            Stmt::Expr(Expr::Call { path, .. })
                if allow_void_tail
                    && index + 1 == stmts.len()
                    && path.last().is_some_and(|name| name == function_name) =>
            {
                targets.push(function_name.to_owned());
            }
            Stmt::If {
                then_body,
                else_body,
                ..
            } => {
                let branch_is_tail = allow_void_tail && index + 1 == stmts.len();
                collect_tail_targets(then_body, function_name, branch_is_tail, targets);
                collect_tail_targets(else_body, function_name, branch_is_tail, targets);
            }
            Stmt::While { .. } | Stmt::Loop { .. } => {}
            _ => {}
        }
        reachable = stmt_can_fall_through(stmt);
    }
}

fn block_can_fall_through(stmts: &[Stmt]) -> bool {
    let mut reachable = true;
    for stmt in stmts {
        if !reachable {
            return false;
        }
        reachable = stmt_can_fall_through(stmt);
    }
    reachable
}

fn stmt_can_fall_through(stmt: &Stmt) -> bool {
    match stmt {
        Stmt::Return(_) | Stmt::Break | Stmt::Continue => false,
        Stmt::If {
            then_body,
            else_body,
            ..
        } => block_can_fall_through(then_body) || block_can_fall_through(else_body),
        Stmt::Loop { body } => block_can_reach_break(body),
        _ => true,
    }
}

fn block_can_reach_break(stmts: &[Stmt]) -> bool {
    let mut reachable = true;
    for stmt in stmts {
        if !reachable {
            break;
        }
        if matches!(stmt, Stmt::Break) {
            return true;
        }
        if let Stmt::If {
            then_body,
            else_body,
            ..
        } = stmt
            && (block_can_reach_break(then_body) || block_can_reach_break(else_body))
        {
            return true;
        }
        reachable = stmt_can_fall_through(stmt);
    }
    false
}

fn tail_recursion_rejection(function: &Function) -> Option<&'static str> {
    if has_attr(function, "naked") {
        Some("naked function")
    } else if has_attr(function, "interrupt") {
        Some("interrupt function")
    } else if function.params.len() > 3 {
        Some("too many parameters")
    } else if uses_argument_slots(&function.params) {
        Some("argument slots required")
    } else {
        None
    }
}

fn sibling_tail_rejection(
    caller: &Function,
    callee: &str,
    cpu: CpuFamily,
    facts: &HashMap<String, FunctionFacts>,
) -> Option<&'static str> {
    if !matches!(
        cpu,
        CpuFamily::Ez80 | CpuFamily::Z80 | CpuFamily::Z80N | CpuFamily::Z180
    ) {
        return Some("target does not support sibling tail calls");
    }
    if has_attr(caller, "naked") {
        return Some("naked caller");
    }
    if has_attr(caller, "interrupt") {
        return Some("interrupt caller");
    }
    let Some(callee) = facts.get(callee) else {
        return Some("callee is not a function");
    };
    if callee.attrs.iter().any(|attr| attr == "naked") {
        return Some("naked callee");
    }
    if callee.attrs.iter().any(|attr| attr == "interrupt") {
        return Some("interrupt callee");
    }
    if caller.return_type != callee.return_type {
        return Some("return type mismatch");
    }
    if caller.params.len() > 3 || callee.params.len() > 3 {
        return Some("too many parameters");
    }
    if uses_argument_slots(&caller.params) || uses_argument_slots(&callee.params) {
        return Some("argument slots required");
    }
    None
}

fn uses_argument_slots(params: &[crate::ast::Param]) -> bool {
    params.len() >= 3 && is_byte_type(&params[1].ty) && !is_byte_type(&params[2].ty)
}

fn is_byte_type(ty: &Type) -> bool {
    matches!(
        ty,
        Type::Named(name) if matches!(name.as_str(), "u8" | "i8" | "bool" | "char")
    )
}

fn rewrite_tail_recursion(function: &mut Function) {
    let mut used_names = HashSet::new();
    for param in &function.params {
        used_names.insert(param.name.clone());
    }
    collect_local_names(&function.body, &mut used_names);
    let mut next_temp = 0usize;
    let allow_void_tail = function.return_type.is_none();
    let (mut body, rewritten) = rewrite_tail_stmts(
        core::mem::take(&mut function.body),
        &function.name,
        &function.params,
        &mut used_names,
        &mut next_temp,
        allow_void_tail,
    );
    function.body = if rewritten {
        if allow_void_tail && block_can_fall_through(&body) {
            body.push(Stmt::Break);
        }
        vec![Stmt::Loop { body }]
    } else {
        body
    };
}

fn rewrite_tail_stmts(
    stmts: Vec<Stmt>,
    function_name: &str,
    params: &[crate::ast::Param],
    used_names: &mut HashSet<String>,
    next_temp: &mut usize,
    allow_void_tail: bool,
) -> (Vec<Stmt>, bool) {
    let mut output = Vec::new();
    let mut any_rewritten = false;
    let mut reachable = true;
    let statement_count = stmts.len();
    for (index, stmt) in stmts.into_iter().enumerate() {
        if !reachable {
            output.push(stmt);
            continue;
        }
        let can_fall_through = stmt_can_fall_through(&stmt);
        match stmt {
            Stmt::Return(Some(Expr::Call { path, args }))
                if path.last().is_some_and(|name| name == function_name)
                    && args.len() == params.len() =>
            {
                rewrite_tail_call_args(args, params, used_names, next_temp, &mut output);
                output.push(Stmt::Continue);
                any_rewritten = true;
            }
            Stmt::Expr(Expr::Call { path, args })
                if allow_void_tail
                    && index + 1 == statement_count
                    && path.last().is_some_and(|name| name == function_name)
                    && args.len() == params.len() =>
            {
                rewrite_tail_call_args(args, params, used_names, next_temp, &mut output);
                output.push(Stmt::Continue);
                any_rewritten = true;
            }
            Stmt::If {
                condition,
                then_body,
                else_body,
            } => {
                let branch_is_tail = allow_void_tail && index + 1 == statement_count;
                let (then_body, then_rewritten) = rewrite_tail_stmts(
                    then_body,
                    function_name,
                    params,
                    used_names,
                    next_temp,
                    branch_is_tail,
                );
                let (else_body, else_rewritten) = rewrite_tail_stmts(
                    else_body,
                    function_name,
                    params,
                    used_names,
                    next_temp,
                    branch_is_tail,
                );
                any_rewritten |= then_rewritten || else_rewritten;
                output.push(Stmt::If {
                    condition,
                    then_body,
                    else_body,
                });
            }
            stmt => output.push(stmt),
        }
        reachable = can_fall_through;
    }
    (output, any_rewritten)
}

fn rewrite_tail_call_args(
    args: Vec<Expr>,
    params: &[crate::ast::Param],
    used_names: &mut HashSet<String>,
    next_temp: &mut usize,
    output: &mut Vec<Stmt>,
) {
    let mut temps = Vec::new();
    for (arg, param) in args.into_iter().zip(params) {
        let name = unique_temp_name(used_names, next_temp);
        output.push(Stmt::Let {
            name: name.clone(),
            ty: param.ty.clone(),
            value: arg,
        });
        temps.push(name);
    }
    for (param, temp) in params.iter().zip(temps) {
        output.push(Stmt::Assign {
            target: Place::Ident(param.name.clone()),
            op: AssignOp::Set,
            value: Expr::Ident(temp),
        });
    }
}

fn unique_temp_name(used_names: &mut HashSet<String>, next_temp: &mut usize) -> String {
    loop {
        let name = format!("__tbir_tail_arg_{}", *next_temp);
        *next_temp += 1;
        if used_names.insert(name.clone()) {
            return name;
        }
    }
}

fn collect_local_names(stmts: &[Stmt], names: &mut HashSet<String>) {
    for stmt in stmts {
        match stmt {
            Stmt::Let { name, .. } => {
                names.insert(name.clone());
            }
            Stmt::LetTwo {
                first_name,
                second_name,
                ..
            } => {
                names.insert(first_name.clone());
                names.insert(second_name.clone());
            }
            Stmt::If {
                then_body,
                else_body,
                ..
            } => {
                collect_local_names(then_body, names);
                collect_local_names(else_body, names);
            }
            Stmt::While { body, .. } | Stmt::Loop { body } => collect_local_names(body, names),
            _ => {}
        }
    }
}

fn decision(
    report: &mut TbirOptimizationReport,
    kind: TbirOptimizationKind,
    caller: Option<&str>,
    callee: &str,
    rejection: Option<&str>,
) {
    report.decisions.push(TbirOptimizationDecision {
        kind,
        caller: caller.map(str::to_owned),
        callee: callee.to_owned(),
        outcome: if rejection.is_some() {
            TbirOptimizationOutcome::Rejected
        } else {
            TbirOptimizationOutcome::Applied
        },
        reason: rejection.unwrap_or("approved").to_owned(),
    });
}

fn has_attr(function: &Function, attr: &str) -> bool {
    function.attrs.iter().any(|candidate| candidate == attr)
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
        _ => false,
    }
}

fn local_propagation_and_cse_program(
    program: &mut Program,
    context: &OptimizationContext,
    report: &mut TbirOptimizationReport,
) {
    fn visit(
        declarations: &mut [Declaration],
        context: &OptimizationContext,
        report: &mut TbirOptimizationReport,
    ) {
        for declaration in declarations {
            match declaration {
                Declaration::Function(function) => {
                    let mut assigned = HashSet::new();
                    collect_assigned_names(&function.body, &mut assigned);
                    collect_address_taken_names(&function.body, &mut assigned);
                    let mut use_counts = HashMap::new();
                    collect_name_uses_stmts(&function.body, &mut use_counts);
                    let mut values = HashMap::new();
                    let mut available = Vec::new();
                    function.body = propagate_block(
                        core::mem::take(&mut function.body),
                        &assigned,
                        &use_counts,
                        &mut values,
                        &mut available,
                        context,
                        report,
                    );
                }
                Declaration::Cfg { declaration, .. } | Declaration::Bank { declaration, .. } => {
                    visit(core::slice::from_mut(declaration), context, report)
                }
                _ => {}
            }
        }
    }
    visit(&mut program.declarations, context, report);
}

fn collect_assigned_names(stmts: &[Stmt], assigned: &mut HashSet<String>) {
    for stmt in stmts {
        match stmt {
            Stmt::Assign {
                target: Place::Ident(name),
                ..
            } => {
                assigned.insert(name.clone());
            }
            Stmt::LetTwo {
                first_name,
                second_name,
                ..
            } => {
                assigned.insert(first_name.clone());
                assigned.insert(second_name.clone());
            }
            Stmt::If {
                then_body,
                else_body,
                ..
            } => {
                collect_assigned_names(then_body, assigned);
                collect_assigned_names(else_body, assigned);
            }
            Stmt::While { body, .. } | Stmt::Loop { body } => {
                collect_assigned_names(body, assigned)
            }
            _ => {}
        }
    }
}

fn collect_address_taken_names(stmts: &[Stmt], names: &mut HashSet<String>) {
    for stmt in stmts {
        match stmt {
            Stmt::Let { value, .. }
            | Stmt::LetTwo { value, .. }
            | Stmt::Return(Some(value))
            | Stmt::Out { value, .. }
            | Stmt::Expr(value) => collect_address_taken_names_in_expr(value, names),
            Stmt::ReturnTwo { first, second } => {
                collect_address_taken_names_in_expr(first, names);
                collect_address_taken_names_in_expr(second, names);
            }
            Stmt::Assign { target, value, .. } => {
                collect_address_taken_names_in_place(target, names);
                collect_address_taken_names_in_expr(value, names);
            }
            Stmt::If {
                condition,
                then_body,
                else_body,
            } => {
                collect_address_taken_names_in_expr(condition, names);
                collect_address_taken_names(then_body, names);
                collect_address_taken_names(else_body, names);
            }
            Stmt::While { condition, body } => {
                collect_address_taken_names_in_expr(condition, names);
                collect_address_taken_names(body, names);
            }
            Stmt::Loop { body } => collect_address_taken_names(body, names),
            Stmt::Asm { .. } | Stmt::Return(None) | Stmt::Break | Stmt::Continue => {}
        }
    }
}

fn collect_address_taken_names_in_place(place: &Place, names: &mut HashSet<String>) {
    match place {
        Place::Index { index, .. } | Place::Deref(index) => {
            collect_address_taken_names_in_expr(index, names)
        }
        Place::Access(path) => {
            for segment in &path.segments {
                if let AccessSegment::Index(index) = segment {
                    collect_address_taken_names_in_expr(index, names);
                }
            }
        }
        Place::Ident(_) | Place::Field { .. } => {}
    }
}

fn collect_address_taken_names_in_expr(expr: &Expr, names: &mut HashSet<String>) {
    match expr {
        Expr::AddressOf(name) => {
            names.insert(name.clone());
        }
        Expr::AddressOfIndex { name, index } => {
            names.insert(name.clone());
            collect_address_taken_names_in_expr(index, names);
        }
        Expr::AddressOfField { base, .. } => {
            names.insert(base.clone());
        }
        Expr::AddressOfAccess(path) => {
            names.insert(path.root.clone());
            for segment in &path.segments {
                if let AccessSegment::Index(index) = segment {
                    collect_address_taken_names_in_expr(index, names);
                }
            }
        }
        Expr::Array(values) => {
            for value in values {
                collect_address_taken_names_in_expr(value, names);
            }
        }
        Expr::StructInit { fields, .. } => {
            for (_, value) in fields {
                collect_address_taken_names_in_expr(value, names);
            }
        }
        Expr::Deref(value)
        | Expr::BankedPointer { pointer: value, .. }
        | Expr::Unary { expr: value, .. }
        | Expr::Cast { expr: value, .. } => collect_address_taken_names_in_expr(value, names),
        Expr::Binary { left, right, .. } => {
            collect_address_taken_names_in_expr(left, names);
            collect_address_taken_names_in_expr(right, names);
        }
        Expr::Call { args, .. } => {
            for argument in args {
                collect_address_taken_names_in_expr(argument, names);
            }
        }
        Expr::Access(path) => {
            for segment in &path.segments {
                if let AccessSegment::Index(index) = segment {
                    collect_address_taken_names_in_expr(index, names);
                }
            }
        }
        Expr::Index { index, .. } => collect_address_taken_names_in_expr(index, names),
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

fn count_name_use(name: &str, uses: &mut HashMap<String, usize>) {
    *uses.entry(name.to_owned()).or_default() += 1;
}

fn collect_name_uses_stmts(stmts: &[Stmt], uses: &mut HashMap<String, usize>) {
    for stmt in stmts {
        match stmt {
            Stmt::Let { value, .. }
            | Stmt::LetTwo { value, .. }
            | Stmt::Return(Some(value))
            | Stmt::Out { value, .. }
            | Stmt::Expr(value) => collect_name_uses_expr(value, uses),
            Stmt::ReturnTwo { first, second } => {
                collect_name_uses_expr(first, uses);
                collect_name_uses_expr(second, uses);
            }
            Stmt::Assign { target, value, .. } => {
                collect_name_uses_place(target, uses);
                collect_name_uses_expr(value, uses);
            }
            Stmt::If {
                condition,
                then_body,
                else_body,
            } => {
                collect_name_uses_expr(condition, uses);
                collect_name_uses_stmts(then_body, uses);
                collect_name_uses_stmts(else_body, uses);
            }
            Stmt::While { condition, body } => {
                collect_name_uses_expr(condition, uses);
                collect_name_uses_stmts(body, uses);
            }
            Stmt::Loop { body } => collect_name_uses_stmts(body, uses),
            Stmt::Asm {
                inputs, outputs, ..
            } => {
                for input in inputs {
                    count_name_use(&input.name, uses);
                }
                for output in outputs {
                    count_name_use(&output.name, uses);
                }
            }
            Stmt::Return(None) | Stmt::Break | Stmt::Continue => {}
        }
    }
}

fn collect_name_uses_place(place: &Place, uses: &mut HashMap<String, usize>) {
    match place {
        Place::Ident(name) | Place::Field { base: name, .. } => count_name_use(name, uses),
        Place::Index { name, index } => {
            count_name_use(name, uses);
            collect_name_uses_expr(index, uses);
        }
        Place::Access(path) => collect_name_uses_access(path, uses),
        Place::Deref(expr) => collect_name_uses_expr(expr, uses),
    }
}

fn collect_name_uses_access(path: &AccessPath, uses: &mut HashMap<String, usize>) {
    count_name_use(&path.root, uses);
    for segment in &path.segments {
        if let AccessSegment::Index(index) = segment {
            collect_name_uses_expr(index, uses);
        }
    }
}

fn collect_name_uses_expr(expr: &Expr, uses: &mut HashMap<String, usize>) {
    match expr {
        Expr::Ident(name) | Expr::AddressOf(name) => count_name_use(name, uses),
        Expr::Index { name, index } | Expr::AddressOfIndex { name, index } => {
            count_name_use(name, uses);
            collect_name_uses_expr(index, uses);
        }
        Expr::Field { base, .. } | Expr::AddressOfField { base, .. } => count_name_use(base, uses),
        Expr::Access(path) | Expr::AddressOfAccess(path) => collect_name_uses_access(path, uses),
        Expr::Array(values) => {
            for value in values {
                collect_name_uses_expr(value, uses);
            }
        }
        Expr::StructInit { fields, .. } => {
            for (_, value) in fields {
                collect_name_uses_expr(value, uses);
            }
        }
        Expr::Deref(value)
        | Expr::BankedPointer { pointer: value, .. }
        | Expr::Unary { expr: value, .. }
        | Expr::Cast { expr: value, .. } => collect_name_uses_expr(value, uses),
        Expr::Binary { left, right, .. } => {
            collect_name_uses_expr(left, uses);
            collect_name_uses_expr(right, uses);
        }
        Expr::Call { args, .. } => {
            for argument in args {
                collect_name_uses_expr(argument, uses);
            }
        }
        Expr::Int(_)
        | Expr::TypedInt(_, _)
        | Expr::Bool(_)
        | Expr::Char(_)
        | Expr::String(_)
        | Expr::In(_) => {}
    }
}

#[derive(Clone)]
struct PropagatedValue {
    ty: Type,
    expr: Expr,
}

fn propagate_block(
    stmts: Vec<Stmt>,
    assigned: &HashSet<String>,
    use_counts: &HashMap<String, usize>,
    values: &mut HashMap<String, PropagatedValue>,
    available: &mut Vec<(Expr, String)>,
    context: &OptimizationContext,
    report: &mut TbirOptimizationReport,
) -> Vec<Stmt> {
    let mut output = Vec::with_capacity(stmts.len());
    for stmt in stmts {
        let stmt = match stmt {
            Stmt::Let { name, ty, value } => {
                let mut value = substitute_expr(value, values, report);
                value = reuse_available_expr(value, available, &name, context, report);
                let references_memory = expr_references_memory_object(&value, context);
                if is_cse_candidate(&value) && !references_memory {
                    if let Some((_, prior)) = available.iter().find(|(expr, _)| expr == &value) {
                        value = Expr::Ident(prior.clone());
                        report.common_subexpressions += 1;
                        decision(
                            report,
                            TbirOptimizationKind::CommonSubexpression,
                            None,
                            &name,
                            None,
                        );
                    } else if !assigned.contains(&name) {
                        available.push((value.clone(), name.clone()));
                    }
                }
                let may_propagate = is_cheap_to_duplicate(&value)
                    || use_counts.get(&name).copied().unwrap_or(0) <= 1;
                if may_propagate
                    && !assigned.contains(&name)
                    && !contains_licm_temporary(&name)
                    && !contains_licm_temporary_expr(&value)
                    && is_pure_scalar(&value)
                    && !references_memory
                {
                    values.insert(
                        name.clone(),
                        PropagatedValue {
                            ty: ty.clone(),
                            expr: value.clone(),
                        },
                    );
                }
                Stmt::Let { name, ty, value }
            }
            Stmt::LetTwo {
                first_name,
                first_ty,
                second_name,
                second_ty,
                value,
            } => {
                values.remove(&first_name);
                values.remove(&second_name);
                available.clear();
                Stmt::LetTwo {
                    first_name,
                    first_ty,
                    second_name,
                    second_ty,
                    value: substitute_expr(value, values, report),
                }
            }
            Stmt::Assign { target, op, value } => {
                let target = substitute_place(target, values, report);
                let value = substitute_expr(value, values, report);
                available.clear();
                Stmt::Assign { target, op, value }
            }
            Stmt::If {
                condition,
                then_body,
                else_body,
            } => {
                let condition = substitute_expr(condition, values, report);
                let mut then_values = values.clone();
                let mut else_values = values.clone();
                let then_body = propagate_block(
                    then_body,
                    assigned,
                    use_counts,
                    &mut then_values,
                    &mut Vec::new(),
                    context,
                    report,
                );
                let else_body = propagate_block(
                    else_body,
                    assigned,
                    use_counts,
                    &mut else_values,
                    &mut Vec::new(),
                    context,
                    report,
                );
                available.clear();
                Stmt::If {
                    condition,
                    then_body,
                    else_body,
                }
            }
            Stmt::While { condition, body } => {
                let condition = substitute_expr(condition, values, report);
                let mut body_values = values.clone();
                let body = propagate_block(
                    body,
                    assigned,
                    use_counts,
                    &mut body_values,
                    &mut Vec::new(),
                    context,
                    report,
                );
                available.clear();
                Stmt::While { condition, body }
            }
            Stmt::Loop { body } => {
                let mut body_values = values.clone();
                let body = propagate_block(
                    body,
                    assigned,
                    use_counts,
                    &mut body_values,
                    &mut Vec::new(),
                    context,
                    report,
                );
                available.clear();
                Stmt::Loop { body }
            }
            Stmt::Return(value) => Stmt::Return(value.map(|v| substitute_expr(v, values, report))),
            Stmt::ReturnTwo { first, second } => {
                available.clear();
                Stmt::ReturnTwo {
                    first: substitute_expr(first, values, report),
                    second: substitute_expr(second, values, report),
                }
            }
            Stmt::Out { port, value } => {
                available.clear();
                Stmt::Out {
                    port,
                    value: substitute_expr(value, values, report),
                }
            }
            Stmt::Expr(value) => {
                let value = substitute_expr(value, values, report);
                if !is_pure_scalar(&value) {
                    available.clear();
                }
                Stmt::Expr(value)
            }
            Stmt::Asm { ref outputs, .. } => {
                available.clear();
                for output in outputs {
                    values.remove(&output.name);
                }
                stmt
            }
            Stmt::Break | Stmt::Continue => stmt,
        };
        output.push(stmt);
    }
    output
}

fn substitute_place(
    place: Place,
    values: &HashMap<String, PropagatedValue>,
    report: &mut TbirOptimizationReport,
) -> Place {
    match place {
        Place::Index { name, index } => Place::Index {
            name,
            index: Box::new(substitute_expr(*index, values, report)),
        },
        Place::Access(mut path) => {
            for segment in &mut path.segments {
                if let AccessSegment::Index(index) = segment {
                    **index = substitute_expr((**index).clone(), values, report);
                }
            }
            Place::Access(path)
        }
        Place::Deref(expr) => Place::Deref(Box::new(substitute_expr(*expr, values, report))),
        other => other,
    }
}

fn substitute_expr(
    expr: Expr,
    values: &HashMap<String, PropagatedValue>,
    report: &mut TbirOptimizationReport,
) -> Expr {
    match expr {
        Expr::Ident(name) => {
            if let Some(value) = values.get(&name) {
                report.copy_propagations += 1;
                if matches!(
                    value.expr,
                    Expr::Int(_) | Expr::TypedInt(_, _) | Expr::Bool(_) | Expr::Char(_)
                ) {
                    report.constant_propagations += 1;
                }
                decision(
                    report,
                    TbirOptimizationKind::CopyPropagation,
                    None,
                    &name,
                    None,
                );
                match &value.expr {
                    Expr::Int(integer) | Expr::TypedInt(integer, _) => {
                        Expr::TypedInt(*integer, value.ty.clone())
                    }
                    Expr::Unary {
                        op: UnaryOp::Neg,
                        expr,
                    } => match expr.as_ref() {
                        Expr::Int(integer) | Expr::TypedInt(integer, _) => {
                            Expr::TypedInt(-*integer, value.ty.clone())
                        }
                        _ => Expr::Cast {
                            ty: value.ty.clone(),
                            expr: Box::new(value.expr.clone()),
                        },
                    },
                    Expr::Bool(boolean) if value.ty == Type::Named("bool".to_owned()) => {
                        Expr::Bool(*boolean)
                    }
                    Expr::Char(character) if value.ty == Type::Named("char".to_owned()) => {
                        Expr::Char(*character)
                    }
                    _ => Expr::Cast {
                        ty: value.ty.clone(),
                        expr: Box::new(value.expr.clone()),
                    },
                }
            } else {
                Expr::Ident(name)
            }
        }
        Expr::Unary { op, expr } => Expr::Unary {
            op,
            expr: Box::new(substitute_expr(*expr, values, report)),
        },
        Expr::Binary { left, op, right } => Expr::Binary {
            left: Box::new(substitute_expr(*left, values, report)),
            op,
            right: Box::new(substitute_expr(*right, values, report)),
        },
        Expr::Cast { ty, expr } => Expr::Cast {
            ty,
            expr: Box::new(substitute_expr(*expr, values, report)),
        },
        Expr::Array(values_) => Expr::Array(
            values_
                .into_iter()
                .map(|v| substitute_expr(v, values, report))
                .collect(),
        ),
        Expr::StructInit { ty, fields } => Expr::StructInit {
            ty,
            fields: fields
                .into_iter()
                .map(|(n, v)| (n, substitute_expr(v, values, report)))
                .collect(),
        },
        Expr::Index { name, index } => Expr::Index {
            name,
            index: Box::new(substitute_expr(*index, values, report)),
        },
        Expr::AddressOfIndex { name, index } => Expr::AddressOfIndex {
            name,
            index: Box::new(substitute_expr(*index, values, report)),
        },
        Expr::Deref(expr) => Expr::Deref(Box::new(substitute_expr(*expr, values, report))),
        Expr::BankedPointer { pointer, bank } => Expr::BankedPointer {
            pointer: Box::new(substitute_expr(*pointer, values, report)),
            bank,
        },
        Expr::Call { path, args } => Expr::Call {
            path,
            args: args
                .into_iter()
                .map(|v| substitute_expr(v, values, report))
                .collect(),
        },
        other => other,
    }
}

fn reuse_available_expr(
    expr: Expr,
    available: &[(Expr, String)],
    name: &str,
    context: &OptimizationContext,
    report: &mut TbirOptimizationReport,
) -> Expr {
    let expr = match expr {
        Expr::Unary { op, expr } => Expr::Unary {
            op,
            expr: Box::new(reuse_available_expr(
                *expr, available, name, context, report,
            )),
        },
        Expr::Binary { left, op, right } => Expr::Binary {
            left: Box::new(reuse_available_expr(
                *left, available, name, context, report,
            )),
            op,
            right: Box::new(reuse_available_expr(
                *right, available, name, context, report,
            )),
        },
        Expr::Cast { ty, expr } => Expr::Cast {
            ty,
            expr: Box::new(reuse_available_expr(
                *expr, available, name, context, report,
            )),
        },
        other => other,
    };

    if is_cse_candidate(&expr)
        && !expr_references_memory_object(&expr, context)
        && let Some((_, prior)) = available.iter().find(|(prior, _)| prior == &expr)
    {
        report.common_subexpressions += 1;
        decision(
            report,
            TbirOptimizationKind::CommonSubexpression,
            None,
            name,
            None,
        );
        return Expr::Ident(prior.clone());
    }
    expr
}

fn is_cheap_to_duplicate(expr: &Expr) -> bool {
    matches!(
        expr,
        Expr::Int(_) | Expr::TypedInt(_, _) | Expr::Bool(_) | Expr::Char(_) | Expr::Ident(_)
    )
}

fn contains_licm_temporary(name: &str) -> bool {
    name.starts_with("__tbir_licm_")
}

fn contains_licm_temporary_expr(expr: &Expr) -> bool {
    matches!(expr, Expr::Ident(name) if contains_licm_temporary(name))
}

fn is_pure_scalar(expr: &Expr) -> bool {
    match expr {
        Expr::Int(_) | Expr::TypedInt(_, _) | Expr::Bool(_) | Expr::Char(_) | Expr::Ident(_) => {
            true
        }
        Expr::Unary { expr, .. } | Expr::Cast { expr, .. } => is_pure_scalar(expr),
        Expr::Binary { left, right, .. } => is_pure_scalar(left) && is_pure_scalar(right),
        _ => false,
    }
}

fn contains_inline_call(expr: &Expr, inline_functions: &HashSet<String>) -> bool {
    match expr {
        Expr::Call { path, args } => {
            path.last()
                .is_some_and(|name| inline_functions.contains(name))
                || args
                    .iter()
                    .any(|arg| contains_inline_call(arg, inline_functions))
        }
        Expr::Array(values) => values
            .iter()
            .any(|value| contains_inline_call(value, inline_functions)),
        Expr::Index { index, .. } | Expr::AddressOfIndex { index, .. } => {
            contains_inline_call(index, inline_functions)
        }
        Expr::Access(path) | Expr::AddressOfAccess(path) => path.segments.iter().any(|segment| {
            matches!(segment, AccessSegment::Index(index) if contains_inline_call(index, inline_functions))
        }),
        Expr::StructInit { fields, .. } => fields
            .iter()
            .any(|(_, value)| contains_inline_call(value, inline_functions)),
        Expr::Deref(value)
        | Expr::BankedPointer { pointer: value, .. }
        | Expr::Unary { expr: value, .. }
        | Expr::Cast { expr: value, .. } => contains_inline_call(value, inline_functions),
        Expr::Binary { left, right, .. } => {
            contains_inline_call(left, inline_functions)
                || contains_inline_call(right, inline_functions)
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
        | Expr::AddressOf(_) => false,
    }
}

fn expr_references_memory_object(expr: &Expr, context: &OptimizationContext) -> bool {
    match expr {
        Expr::Ident(name)
        | Expr::Index { name, .. }
        | Expr::AddressOfIndex { name, .. }
        | Expr::AddressOf(name) => context.objects.contains_key(name),
        Expr::Field { base, .. } | Expr::AddressOfField { base, .. } => {
            context.objects.contains_key(base)
        }
        Expr::Access(path) | Expr::AddressOfAccess(path) => {
            context.objects.contains_key(&path.root)
                || path.segments.iter().any(|segment| match segment {
                    AccessSegment::Index(index) => expr_references_memory_object(index, context),
                    AccessSegment::Field(_) => false,
                })
        }
        Expr::Unary { expr, .. }
        | Expr::Cast { expr, .. }
        | Expr::Deref(expr)
        | Expr::BankedPointer { pointer: expr, .. } => expr_references_memory_object(expr, context),
        Expr::Binary { left, right, .. } => {
            expr_references_memory_object(left, context)
                || expr_references_memory_object(right, context)
        }
        Expr::Array(values) => values
            .iter()
            .any(|value| expr_references_memory_object(value, context)),
        Expr::StructInit { fields, .. } => fields
            .iter()
            .any(|(_, value)| expr_references_memory_object(value, context)),
        Expr::Call { args, .. } => args
            .iter()
            .any(|arg| expr_references_memory_object(arg, context)),
        Expr::Int(_)
        | Expr::TypedInt(_, _)
        | Expr::Bool(_)
        | Expr::Char(_)
        | Expr::String(_)
        | Expr::In(_) => false,
    }
}

fn is_cse_candidate(expr: &Expr) -> bool {
    is_pure_scalar(expr)
        && !matches!(
            expr,
            Expr::Int(_) | Expr::TypedInt(_, _) | Expr::Bool(_) | Expr::Char(_) | Expr::Ident(_)
        )
}

fn remove_unused_pure_lets_program(program: &mut Program) {
    fn visit(declarations: &mut [Declaration]) {
        for declaration in declarations {
            match declaration {
                Declaration::Function(function) => {
                    remove_unused_pure_lets_block(&mut function.body);
                }
                Declaration::Cfg { declaration, .. } | Declaration::Bank { declaration, .. } => {
                    visit(core::slice::from_mut(declaration));
                }
                _ => {}
            }
        }
    }
    visit(&mut program.declarations);
}

fn remove_unused_pure_lets_block(stmts: &mut Vec<Stmt>) {
    for stmt in stmts.iter_mut() {
        match stmt {
            Stmt::If {
                then_body,
                else_body,
                ..
            } => {
                remove_unused_pure_lets_block(then_body);
                remove_unused_pure_lets_block(else_body);
            }
            Stmt::While { body, .. } | Stmt::Loop { body } => remove_unused_pure_lets_block(body),
            _ => {}
        }
    }

    loop {
        let mut uses = HashMap::new();
        collect_name_uses_stmts(stmts, &mut uses);
        let old_len = stmts.len();
        stmts.retain(|stmt| {
            !matches!(
                stmt,
                Stmt::Let { name, value, .. }
                    if !uses.contains_key(name) && is_effect_free_expr(value)
            )
        });
        if stmts.len() == old_len {
            break;
        }
    }
}

fn is_effect_free_expr(expr: &Expr) -> bool {
    match expr {
        Expr::Int(_)
        | Expr::TypedInt(_, _)
        | Expr::Bool(_)
        | Expr::Char(_)
        | Expr::String(_)
        | Expr::Ident(_)
        | Expr::AddressOf(_)
        | Expr::AddressOfField { .. } => true,
        Expr::Array(values) => values.iter().all(is_effect_free_expr),
        Expr::StructInit { fields, .. } => {
            fields.iter().all(|(_, value)| is_effect_free_expr(value))
        }
        Expr::AddressOfIndex { index, .. } => is_effect_free_expr(index),
        Expr::AddressOfAccess(path) => path.segments.iter().all(|segment| match segment {
            AccessSegment::Index(index) => is_effect_free_expr(index),
            AccessSegment::Field(_) => true,
        }),
        Expr::BankedPointer { pointer, .. }
        | Expr::Unary { expr: pointer, .. }
        | Expr::Cast { expr: pointer, .. } => is_effect_free_expr(pointer),
        Expr::Binary { left, right, .. } => is_effect_free_expr(left) && is_effect_free_expr(right),
        Expr::Index { .. }
        | Expr::Field { .. }
        | Expr::Access(_)
        | Expr::Deref(_)
        | Expr::Call { .. }
        | Expr::In(_) => false,
    }
}

fn hoist_pure_loop_invariants_program(program: &mut Program, report: &mut TbirOptimizationReport) {
    fn visit(declarations: &mut [Declaration], report: &mut TbirOptimizationReport) {
        for declaration in declarations {
            match declaration {
                Declaration::Function(function) => {
                    let mut names: HashSet<String> =
                        function.params.iter().map(|p| p.name.clone()).collect();
                    collect_local_names(&function.body, &mut names);
                    let mut next_temp = 0;
                    let external: HashSet<String> =
                        function.params.iter().map(|p| p.name.clone()).collect();
                    function.body = licm_block(
                        core::mem::take(&mut function.body),
                        &external,
                        &mut names,
                        &mut next_temp,
                        report,
                    );
                }
                Declaration::Cfg { declaration, .. } | Declaration::Bank { declaration, .. } => {
                    visit(core::slice::from_mut(declaration), report)
                }
                _ => {}
            }
        }
    }
    visit(&mut program.declarations, report);
}

fn licm_block(
    stmts: Vec<Stmt>,
    external: &HashSet<String>,
    used_names: &mut HashSet<String>,
    next_temp: &mut usize,
    report: &mut TbirOptimizationReport,
) -> Vec<Stmt> {
    let mut output = Vec::new();
    let mut visible = external.clone();
    for stmt in stmts {
        match stmt {
            Stmt::Let { name, ty, value } => {
                visible.insert(name.clone());
                output.push(Stmt::Let { name, ty, value });
            }
            Stmt::If {
                condition,
                then_body,
                else_body,
            } => {
                let then_body = licm_block(then_body, &visible, used_names, next_temp, report);
                let else_body = licm_block(else_body, &visible, used_names, next_temp, report);
                output.push(Stmt::If {
                    condition,
                    then_body,
                    else_body,
                });
            }
            Stmt::While { condition, body } => {
                let (preheader, body) =
                    hoist_loop_body(body, &visible, used_names, next_temp, report);
                output.extend(preheader);
                output.push(Stmt::While { condition, body });
            }
            Stmt::Loop { body } => {
                let (preheader, body) =
                    hoist_loop_body(body, &visible, used_names, next_temp, report);
                output.extend(preheader);
                output.push(Stmt::Loop { body });
            }
            other => output.push(other),
        }
    }
    output
}

fn hoist_loop_body(
    mut body: Vec<Stmt>,
    external: &HashSet<String>,
    used_names: &mut HashSet<String>,
    next_temp: &mut usize,
    report: &mut TbirOptimizationReport,
) -> (Vec<Stmt>, Vec<Stmt>) {
    let has_exit = body.iter().any(stmt_contains_loop_exit);
    let mut assigned = HashSet::new();
    collect_assigned_names(&body, &mut assigned);
    let mut declared = HashSet::new();
    collect_local_names(&body, &mut declared);
    let mut preheader = Vec::new();
    for stmt in &mut body {
        let Stmt::Let { name, ty, value } = stmt else {
            continue;
        };
        if !is_cse_candidate(value) {
            continue;
        }
        let mut deps = HashSet::new();
        collect_scalar_dependencies(value, &mut deps);
        let rejection = if has_exit {
            Some("loop contains return, break, or continue")
        } else if deps.iter().any(|dep| assigned.contains(dep)) {
            Some("dependency assigned in loop")
        } else if deps
            .iter()
            .any(|dep| declared.contains(dep) || !external.contains(dep))
        {
            Some("dependency declared in loop")
        } else {
            None
        };
        if let Some(reason) = rejection {
            decision(
                report,
                TbirOptimizationKind::LoopInvariantCodeMotion,
                None,
                name,
                Some(reason),
            );
            continue;
        }
        let temp = unique_licm_temp_name(used_names, next_temp);
        preheader.push(Stmt::Let {
            name: temp.clone(),
            ty: ty.clone(),
            value: value.clone(),
        });
        *value = Expr::Ident(temp);
        report.loop_invariants_hoisted += 1;
        decision(
            report,
            TbirOptimizationKind::LoopInvariantCodeMotion,
            None,
            name,
            None,
        );
    }
    let mut nested_external = external.clone();
    nested_external.extend(declared);
    body = licm_block(body, &nested_external, used_names, next_temp, report);
    (preheader, body)
}

fn stmt_contains_loop_exit(stmt: &Stmt) -> bool {
    match stmt {
        Stmt::Return(_) | Stmt::Break | Stmt::Continue => true,
        Stmt::If {
            then_body,
            else_body,
            ..
        } => {
            then_body.iter().any(stmt_contains_loop_exit)
                || else_body.iter().any(stmt_contains_loop_exit)
        }
        Stmt::While { body, .. } | Stmt::Loop { body } => body.iter().any(stmt_contains_loop_exit),
        _ => false,
    }
}

fn collect_scalar_dependencies(expr: &Expr, deps: &mut HashSet<String>) {
    match expr {
        Expr::Ident(name) => {
            deps.insert(name.clone());
        }
        Expr::Unary { expr, .. } | Expr::Cast { expr, .. } => {
            collect_scalar_dependencies(expr, deps)
        }
        Expr::Binary { left, right, .. } => {
            collect_scalar_dependencies(left, deps);
            collect_scalar_dependencies(right, deps);
        }
        _ => {}
    }
}

fn unique_licm_temp_name(used_names: &mut HashSet<String>, next_temp: &mut usize) -> String {
    loop {
        let name = format!("__tbir_licm_{}", *next_temp);
        *next_temp += 1;
        if used_names.insert(name.clone()) {
            return name;
        }
    }
}

fn hoist_named_memory_reads_program(
    program: &mut Program,
    context: &OptimizationContext,
    report: &mut TbirOptimizationReport,
) {
    fn visit(
        declarations: &mut [Declaration],
        context: &OptimizationContext,
        report: &mut TbirOptimizationReport,
    ) {
        for declaration in declarations {
            match declaration {
                Declaration::Function(function) => {
                    let mut names: HashSet<String> = function
                        .params
                        .iter()
                        .map(|param| param.name.clone())
                        .collect();
                    collect_local_names(&function.body, &mut names);
                    let mut next_temp = 0;
                    let external = function
                        .params
                        .iter()
                        .map(|param| param.name.clone())
                        .collect();
                    function.body = memory_licm_block(
                        core::mem::take(&mut function.body),
                        &external,
                        &mut names,
                        &mut next_temp,
                        context,
                        report,
                    );
                }
                Declaration::Cfg { declaration, .. } | Declaration::Bank { declaration, .. } => {
                    visit(core::slice::from_mut(declaration), context, report)
                }
                _ => {}
            }
        }
    }
    visit(&mut program.declarations, context, report);
}

fn memory_licm_block(
    stmts: Vec<Stmt>,
    external: &HashSet<String>,
    used_names: &mut HashSet<String>,
    next_temp: &mut usize,
    context: &OptimizationContext,
    report: &mut TbirOptimizationReport,
) -> Vec<Stmt> {
    let mut output = Vec::new();
    let mut visible = external.clone();
    for stmt in stmts {
        match stmt {
            Stmt::Let { name, ty, value } => {
                visible.insert(name.clone());
                output.push(Stmt::Let { name, ty, value });
            }
            Stmt::If {
                condition,
                then_body,
                else_body,
            } => output.push(Stmt::If {
                condition,
                then_body: memory_licm_block(
                    then_body, &visible, used_names, next_temp, context, report,
                ),
                else_body: memory_licm_block(
                    else_body, &visible, used_names, next_temp, context, report,
                ),
            }),
            Stmt::While { condition, body } => {
                let (preheader, body) =
                    hoist_memory_loop_body(body, &visible, used_names, next_temp, context, report);
                output.extend(preheader);
                output.push(Stmt::While { condition, body });
            }
            Stmt::Loop { body } => {
                let (preheader, body) =
                    hoist_memory_loop_body(body, &visible, used_names, next_temp, context, report);
                output.extend(preheader);
                output.push(Stmt::Loop { body });
            }
            other => output.push(other),
        }
    }
    output
}

fn hoist_memory_loop_body(
    mut body: Vec<Stmt>,
    external: &HashSet<String>,
    used_names: &mut HashSet<String>,
    next_temp: &mut usize,
    context: &OptimizationContext,
    report: &mut TbirOptimizationReport,
) -> (Vec<Stmt>, Vec<Stmt>) {
    let mut assigned = HashSet::new();
    collect_assigned_names(&body, &mut assigned);
    let mut declared = HashSet::new();
    collect_local_names(&body, &mut declared);
    let barrier = memory_loop_barrier(&body, context);
    let mut named_writes = HashSet::new();
    collect_named_writes(&body, &mut named_writes);
    let mut preheader = Vec::new();
    for stmt in &mut body {
        let Stmt::Let { name, ty, value } = stmt else {
            continue;
        };
        let analysis = analyze_memory_initializer(value, context);
        if !analysis.memory_like {
            continue;
        }
        let reason = analysis
            .rejection
            .or_else(|| barrier.clone())
            .or_else(|| {
                analysis
                    .scalar_deps
                    .iter()
                    .any(|dep| assigned.contains(dep))
                    .then(|| "scalar dependency assigned in loop".to_owned())
            })
            .or_else(|| {
                analysis
                    .scalar_deps
                    .iter()
                    .any(|dep| declared.contains(dep) || !external.contains(dep))
                    .then(|| "scalar dependency declared in loop".to_owned())
            })
            .or_else(|| {
                analysis
                    .roots
                    .iter()
                    .any(|root| named_writes.contains(root))
                    .then(|| "same named object written in loop".to_owned())
            });
        if let Some(reason) = reason {
            decision(
                report,
                TbirOptimizationKind::MemoryReadLicm,
                None,
                name,
                Some(&reason),
            );
            continue;
        }
        let temp = unique_memory_licm_temp_name(used_names, next_temp);
        preheader.push(Stmt::Let {
            name: temp.clone(),
            ty: ty.clone(),
            value: value.clone(),
        });
        *value = Expr::Ident(temp);
        report.named_memory_reads_hoisted += 1;
        decision(
            report,
            TbirOptimizationKind::MemoryReadLicm,
            None,
            name,
            None,
        );
    }
    let mut nested_external = external.clone();
    nested_external.extend(declared);
    body = memory_licm_block(
        body,
        &nested_external,
        used_names,
        next_temp,
        context,
        report,
    );
    (preheader, body)
}

#[derive(Default)]
struct MemoryInitializerAnalysis {
    memory_like: bool,
    roots: HashSet<String>,
    scalar_deps: HashSet<String>,
    rejection: Option<String>,
}

fn analyze_memory_initializer(
    expr: &Expr,
    context: &OptimizationContext,
) -> MemoryInitializerAnalysis {
    fn reject(analysis: &mut MemoryInitializerAnalysis, reason: &str) {
        if analysis.rejection.is_none() {
            analysis.rejection = Some(reason.to_owned());
        }
    }
    fn root(name: &str, analysis: &mut MemoryInitializerAnalysis, context: &OptimizationContext) {
        analysis.memory_like = true;
        let Some(object) = context.objects.get(name) else {
            reject(analysis, "unknown named memory object");
            return;
        };
        if object.kind != TbirObjectKind::Global {
            reject(
                analysis,
                match object.kind {
                    TbirObjectKind::Mmio => "MMIO object",
                    TbirObjectKind::Embed => "embedded or read-only object",
                    _ => "non-global named object",
                },
            );
        } else if object.region.is_none() {
            reject(analysis, "object has no known region");
        } else if object.access != TbirAccess::ReadWrite {
            reject(analysis, "object or region is read-only");
        } else if object.volatile {
            reject(analysis, "object or region is volatile");
        } else {
            analysis.roots.insert(name.to_owned());
        }
    }
    fn walk(expr: &Expr, analysis: &mut MemoryInitializerAnalysis, context: &OptimizationContext) {
        match expr {
            Expr::Int(_) | Expr::TypedInt(_, _) | Expr::Bool(_) | Expr::Char(_) => {}
            Expr::Ident(name) if context.objects.contains_key(name) => {
                root(name, analysis, context)
            }
            Expr::Ident(name) => {
                analysis.scalar_deps.insert(name.clone());
            }
            Expr::Index { name, index } => {
                root(name, analysis, context);
                walk(index, analysis, context);
            }
            Expr::Field { base, .. } => root(base, analysis, context),
            Expr::Access(path) => {
                root(&path.root, analysis, context);
                for segment in &path.segments {
                    if let AccessSegment::Index(index) = segment {
                        walk(index, analysis, context);
                    }
                }
            }
            Expr::Unary { expr, .. } | Expr::Cast { expr, .. } => walk(expr, analysis, context),
            Expr::Binary { left, right, .. } => {
                walk(left, analysis, context);
                walk(right, analysis, context);
            }
            Expr::AddressOf(_)
            | Expr::AddressOfIndex { .. }
            | Expr::AddressOfField { .. }
            | Expr::AddressOfAccess(_) => {
                analysis.memory_like = true;
                reject(analysis, "address-taking expression");
            }
            Expr::Deref(_) => {
                analysis.memory_like = true;
                reject(analysis, "pointer dereference");
            }
            Expr::BankedPointer { .. } => {
                analysis.memory_like = true;
                reject(analysis, "banked pointer expression");
            }
            Expr::Call { .. } | Expr::In(_) => {
                analysis.memory_like = true;
                reject(analysis, "effectful expression");
            }
            Expr::String(_) | Expr::Array(_) | Expr::StructInit { .. } => {
                analysis.memory_like = true;
                reject(analysis, "aggregate expression");
            }
        }
    }
    let mut analysis = MemoryInitializerAnalysis::default();
    walk(expr, &mut analysis, context);
    analysis
}

fn unique_memory_licm_temp_name(used_names: &mut HashSet<String>, next: &mut usize) -> String {
    loop {
        let name = format!("__tbir_mem_licm_{}", *next);
        *next += 1;
        if used_names.insert(name.clone()) {
            return name;
        }
    }
}

fn collect_named_writes(stmts: &[Stmt], roots: &mut HashSet<String>) {
    for stmt in stmts {
        match stmt {
            Stmt::Assign { target, .. } => {
                let root = match target {
                    Place::Ident(name) | Place::Index { name, .. } => Some(name),
                    Place::Field { base, .. } => Some(base),
                    Place::Access(path) => Some(&path.root),
                    Place::Deref(_) => None,
                };
                if let Some(root) = root {
                    roots.insert(root.clone());
                }
            }
            Stmt::If {
                then_body,
                else_body,
                ..
            } => {
                collect_named_writes(then_body, roots);
                collect_named_writes(else_body, roots);
            }
            Stmt::While { body, .. } | Stmt::Loop { body } => collect_named_writes(body, roots),
            _ => {}
        }
    }
}

fn memory_loop_barrier(stmts: &[Stmt], context: &OptimizationContext) -> Option<String> {
    for stmt in stmts {
        let reason = match stmt {
            Stmt::Return(_) | Stmt::Break | Stmt::Continue => Some("loop contains explicit exit"),
            Stmt::Asm { .. } => Some("loop contains inline assembly"),
            Stmt::Out { .. } => Some("loop contains port I/O"),
            Stmt::Assign {
                target: Place::Deref(_),
                ..
            } => Some("loop contains pointer dereference"),
            Stmt::Assign { target, .. } if place_is_unknown_write(target, context) => {
                Some("loop contains unknown write")
            }
            _ => None,
        };
        if let Some(reason) = reason {
            return Some(reason.to_owned());
        }
        let expressions: Vec<&Expr> = match stmt {
            Stmt::Let { value, .. }
            | Stmt::Assign { value, .. }
            | Stmt::Expr(value)
            | Stmt::Out { value, .. } => vec![value],
            Stmt::Return(Some(value)) => vec![value],
            Stmt::If { condition, .. } | Stmt::While { condition, .. } => vec![condition],
            _ => Vec::new(),
        };
        for expr in expressions {
            if let Some(reason) = memory_expr_barrier(expr, context) {
                return Some(reason);
            }
        }
        match stmt {
            Stmt::If {
                then_body,
                else_body,
                ..
            } => {
                if let Some(reason) = memory_loop_barrier(then_body, context)
                    .or_else(|| memory_loop_barrier(else_body, context))
                {
                    return Some(reason);
                }
            }
            Stmt::While { body, .. } | Stmt::Loop { body } => {
                if let Some(reason) = memory_loop_barrier(body, context) {
                    return Some(reason);
                }
            }
            _ => {}
        }
    }
    None
}

fn place_is_unknown_write(place: &Place, context: &OptimizationContext) -> bool {
    match place {
        Place::Ident(_) => false,
        Place::Index { name, .. } | Place::Field { base: name, .. } => {
            !context.objects.contains_key(name)
        }
        Place::Access(path) => !context.objects.contains_key(&path.root),
        Place::Deref(_) => true,
    }
}

fn memory_expr_barrier(expr: &Expr, context: &OptimizationContext) -> Option<String> {
    match expr {
        Expr::Call { .. } => Some("loop contains call".to_owned()),
        Expr::In(_) => Some("loop contains port I/O".to_owned()),
        Expr::Deref(_) => Some("loop contains pointer dereference".to_owned()),
        Expr::AddressOf(_)
        | Expr::AddressOfIndex { .. }
        | Expr::AddressOfField { .. }
        | Expr::AddressOfAccess(_) => Some("loop contains address-taking".to_owned()),
        Expr::BankedPointer { .. } => Some("loop contains banked pointer".to_owned()),
        Expr::Ident(name) | Expr::Index { name, .. } => object_access_barrier(name, context),
        Expr::Field { base, .. } => object_access_barrier(base, context),
        Expr::Access(path) => object_access_barrier(&path.root, context),
        Expr::Unary { expr, .. } | Expr::Cast { expr, .. } => memory_expr_barrier(expr, context),
        Expr::Binary { left, right, .. } => {
            memory_expr_barrier(left, context).or_else(|| memory_expr_barrier(right, context))
        }
        Expr::Array(values) => values
            .iter()
            .find_map(|value| memory_expr_barrier(value, context)),
        Expr::StructInit { fields, .. } => fields
            .iter()
            .find_map(|(_, value)| memory_expr_barrier(value, context)),
        _ => None,
    }
}

fn object_access_barrier(name: &str, context: &OptimizationContext) -> Option<String> {
    context
        .objects
        .get(name)
        .and_then(|object| match object.kind {
            TbirObjectKind::Mmio => Some("loop accesses MMIO".to_owned()),
            TbirObjectKind::Embed => Some("loop accesses embedded data".to_owned()),
            _ => None,
        })
}

#[derive(Clone)]
struct KnownBits {
    ty: Type,
    zero: i64,
    one: i64,
}

fn known_bits_program(program: &mut Program) {
    fn visit(declarations: &mut [Declaration]) {
        for declaration in declarations {
            match declaration {
                Declaration::Function(function) => {
                    let mut address_taken = HashSet::new();
                    collect_address_taken_names(&function.body, &mut address_taken);
                    let mut facts = HashMap::new();
                    for param in &function.params {
                        if known_bits_mask(&param.ty).is_some()
                            && !address_taken.contains(&param.name)
                        {
                            facts.insert(
                                param.name.clone(),
                                KnownBits {
                                    ty: param.ty.clone(),
                                    zero: 0,
                                    one: 0,
                                },
                            );
                        }
                    }
                    known_bits_block(
                        &mut function.body,
                        &address_taken,
                        function.return_type.as_ref(),
                        &mut facts,
                    );
                }
                Declaration::Cfg { declaration, .. } | Declaration::Bank { declaration, .. } => {
                    visit(core::slice::from_mut(declaration));
                }
                _ => {}
            }
        }
    }

    visit(&mut program.declarations);
}

fn known_bits_block(
    stmts: &mut [Stmt],
    address_taken: &HashSet<String>,
    return_ty: Option<&Type>,
    facts: &mut HashMap<String, KnownBits>,
) {
    for stmt in stmts {
        match stmt {
            Stmt::Let { name, ty, value } => {
                if known_bits_mask(ty).is_none() {
                    facts.remove(name);
                    continue;
                };
                let (simplified, fact) =
                    known_bits_expr(core::mem::replace(value, Expr::Int(0)), facts, ty);
                *value = simplified;
                if let Some(fact) = fact.filter(|fact| fact.ty == *ty)
                    && !address_taken.contains(name)
                {
                    facts.insert(name.clone(), fact);
                } else {
                    facts.remove(name);
                }
            }
            Stmt::LetTwo {
                first_name,
                second_name,
                ..
            } => {
                facts.remove(first_name);
                facts.remove(second_name);
            }
            Stmt::Assign { target, .. } => match target {
                Place::Ident(name) => {
                    facts.remove(name);
                }
                Place::Index { .. } | Place::Field { .. } | Place::Access(_) | Place::Deref(_) => {
                    facts.clear();
                }
            },
            Stmt::If {
                then_body,
                else_body,
                ..
            } => {
                let mut then_facts = facts.clone();
                let mut else_facts = facts.clone();
                known_bits_block(then_body, address_taken, return_ty, &mut then_facts);
                known_bits_block(else_body, address_taken, return_ty, &mut else_facts);
            }
            Stmt::While { body, .. } | Stmt::Loop { body } => {
                let mut body_facts = facts.clone();
                let mut assigned = HashSet::new();
                collect_assigned_names(body, &mut assigned);
                for name in assigned {
                    body_facts.remove(&name);
                }
                known_bits_block(body, address_taken, return_ty, &mut body_facts);
            }
            Stmt::Asm { outputs, .. } => {
                for output in outputs {
                    facts.remove(&output.name);
                }
            }
            Stmt::Return(Some(value)) => {
                if let Some(return_ty) = return_ty {
                    let (simplified, _) =
                        known_bits_expr(core::mem::replace(value, Expr::Int(0)), facts, return_ty);
                    *value = simplified;
                }
            }
            Stmt::ReturnTwo { .. }
            | Stmt::Expr(_)
            | Stmt::Out { .. }
            | Stmt::Return(None)
            | Stmt::Break
            | Stmt::Continue => {}
        }
    }
}

fn known_bits_expr(
    expr: Expr,
    facts: &HashMap<String, KnownBits>,
    expected_ty: &Type,
) -> (Expr, Option<KnownBits>) {
    let Some(mask) = known_bits_mask(expected_ty) else {
        return (expr, None);
    };
    match expr {
        Expr::Int(value) if known_bits_value_fits(value, expected_ty, mask) => {
            known_bits_constant(value, expected_ty, mask)
        }
        Expr::TypedInt(value, ty)
            if ty == *expected_ty && known_bits_value_fits(value, &ty, mask) =>
        {
            known_bits_constant(value, &ty, mask)
        }
        Expr::Ident(name) => {
            let fact = facts
                .get(&name)
                .filter(|fact| fact.ty == *expected_ty)
                .cloned();
            (Expr::Ident(name), fact)
        }
        Expr::Unary {
            op: UnaryOp::BitNot,
            expr,
        } => {
            let (expr, fact) = known_bits_expr(*expr, facts, expected_ty);
            let fact = fact.map(|fact| KnownBits {
                ty: fact.ty,
                zero: fact.one,
                one: fact.zero,
            });
            known_bits_fold(
                Expr::Unary {
                    op: UnaryOp::BitNot,
                    expr: Box::new(expr),
                },
                fact,
                mask,
            )
        }
        Expr::Binary { left, op, right }
            if matches!(op, BinaryOp::BitAnd | BinaryOp::BitOr | BinaryOp::BitXor) =>
        {
            let (left, left_facts) = known_bits_expr(*left, facts, expected_ty);
            let (right, right_facts) = known_bits_expr(*right, facts, expected_ty);
            let fact = match (left_facts, right_facts) {
                (Some(left), Some(right)) if left.ty == right.ty => Some(match op {
                    BinaryOp::BitAnd => KnownBits {
                        ty: left.ty,
                        zero: left.zero | right.zero,
                        one: left.one & right.one,
                    },
                    BinaryOp::BitOr => KnownBits {
                        ty: left.ty,
                        zero: left.zero & right.zero,
                        one: left.one | right.one,
                    },
                    BinaryOp::BitXor => KnownBits {
                        ty: left.ty,
                        zero: (left.zero & right.zero) | (left.one & right.one),
                        one: (left.zero & right.one) | (left.one & right.zero),
                    },
                    _ => unreachable!(),
                }),
                _ => None,
            };
            known_bits_fold(
                Expr::Binary {
                    left: Box::new(left),
                    op,
                    right: Box::new(right),
                },
                fact,
                mask,
            )
        }
        Expr::Cast { ty, expr } if ty == *expected_ty => {
            let (expr, fact) = known_bits_expr(*expr, facts, expected_ty);
            known_bits_fold(
                Expr::Cast {
                    ty,
                    expr: Box::new(expr),
                },
                fact,
                mask,
            )
        }
        other => range_known_bits(other, facts, expected_ty, mask),
    }
}

fn range_known_bits(
    expr: Expr,
    known_bits: &HashMap<String, KnownBits>,
    expected_ty: &Type,
    mask: i64,
) -> (Expr, Option<KnownBits>) {
    if !range_expression_is_local(&expr, known_bits)
        || range_expression_has_invalid_literal(&expr, expected_ty)
    {
        return (expr, None);
    }

    let mut analysis = RangeAnalysis::new();
    for (name, fact) in known_bits {
        let Some(value) = ValueFacts::from_known_bits(&fact.ty, fact.zero as u64, fact.one as u64)
        else {
            continue;
        };
        analysis.bind(name.clone(), value);
    }
    let value = analysis.analyze(&expr, expected_ty);
    if value.bit_width == 0 || !value.effects.is_pure() {
        return (expr, None);
    }

    let fact = KnownBits {
        ty: expected_ty.clone(),
        zero: value.known_zero as i64 & mask,
        one: value.known_one as i64 & mask,
    };
    known_bits_fold(expr, Some(fact), mask)
}

fn range_expression_has_invalid_literal(expr: &Expr, expected_ty: &Type) -> bool {
    match expr {
        Expr::Int(value) => known_bits_mask(expected_ty)
            .is_some_and(|mask| !known_bits_value_fits(*value, expected_ty, mask)),
        Expr::TypedInt(value, ty) => {
            known_bits_mask(ty).is_some_and(|mask| !known_bits_value_fits(*value, ty, mask))
        }
        Expr::Unary { expr, .. } | Expr::Cast { expr, .. } => {
            range_expression_has_invalid_literal(expr, expected_ty)
        }
        Expr::Binary { left, right, .. } => {
            range_expression_has_invalid_literal(left, expected_ty)
                || range_expression_has_invalid_literal(right, expected_ty)
        }
        _ => false,
    }
}

fn range_expression_is_local(expr: &Expr, known_bits: &HashMap<String, KnownBits>) -> bool {
    match expr {
        Expr::Ident(name) => known_bits.contains_key(name),
        Expr::Int(_) | Expr::TypedInt(_, _) => true,
        Expr::Unary { expr, .. } | Expr::Cast { expr, .. } => {
            range_expression_is_local(expr, known_bits)
        }
        Expr::Binary { left, right, .. } => {
            range_expression_is_local(left, known_bits)
                && range_expression_is_local(right, known_bits)
        }
        Expr::Bool(_)
        | Expr::Char(_)
        | Expr::String(_)
        | Expr::In(_)
        | Expr::Index { .. }
        | Expr::AddressOfIndex { .. }
        | Expr::Field { .. }
        | Expr::AddressOfField { .. }
        | Expr::Access(_)
        | Expr::AddressOfAccess(_)
        | Expr::AddressOf(_)
        | Expr::Deref(_)
        | Expr::BankedPointer { .. }
        | Expr::Array(_)
        | Expr::StructInit { .. }
        | Expr::Call { .. } => false,
    }
}

fn known_bits_value_fits(value: i64, ty: &Type, mask: i64) -> bool {
    match ty {
        Type::Named(name) if name.starts_with('u') => value >= 0 && value <= mask,
        Type::Named(name) if name.starts_with('i') => {
            let bits = mask.count_ones();
            let sign = 1_i64 << (bits - 1);
            value >= -sign && value < sign
        }
        _ => false,
    }
}

fn signed_or_unsigned_literal(value: i64, ty: &Type, mask: i64) -> i64 {
    let value = value & mask;
    if matches!(ty, Type::Named(name) if name.starts_with('i')) {
        let sign = 1_i64 << (mask.count_ones() - 1);
        if value & sign != 0 {
            return value - (mask + 1);
        }
    }
    value
}

fn known_bits_constant(value: i64, ty: &Type, mask: i64) -> (Expr, Option<KnownBits>) {
    let raw = value & mask;
    (
        Expr::TypedInt(signed_or_unsigned_literal(raw, ty, mask), ty.clone()),
        Some(KnownBits {
            ty: ty.clone(),
            zero: (!raw) & mask,
            one: raw,
        }),
    )
}

fn known_bits_fold(expr: Expr, fact: Option<KnownBits>, mask: i64) -> (Expr, Option<KnownBits>) {
    if let Some(fact) = &fact
        && fact.zero | fact.one == mask
    {
        return (
            Expr::TypedInt(
                signed_or_unsigned_literal(fact.one, &fact.ty, mask),
                fact.ty.clone(),
            ),
            Some(fact.clone()),
        );
    }
    (expr, fact)
}

fn known_bits_mask(ty: &Type) -> Option<i64> {
    match ty {
        Type::Named(name) if matches!(name.as_str(), "u8" | "i8") => Some(0xff),
        Type::Named(name) if matches!(name.as_str(), "u16" | "i16") => Some(0xffff),
        Type::Named(name) if matches!(name.as_str(), "u24" | "i24") => Some(0xffffff),
        Type::Named(name) if matches!(name.as_str(), "u32" | "i32") => Some(0xffff_ffff),
        _ => None,
    }
}

fn scalar_simplify_program(
    program: &mut Program,
    report: &mut TbirOptimizationReport,
    count_dead_statements: bool,
    comptime: &ComptimeContext,
) {
    let inline_functions = inline_function_names(program);
    for declaration in &mut program.declarations {
        optimize_declaration(
            declaration,
            report,
            count_dead_statements,
            &inline_functions,
            comptime,
        );
    }
}

fn optimize_declaration(
    declaration: &mut Declaration,
    report: &mut TbirOptimizationReport,
    count_dead_statements: bool,
    inline_functions: &HashSet<String>,
    comptime: &ComptimeContext,
) {
    match declaration {
        Declaration::Cfg { declaration, .. } | Declaration::Bank { declaration, .. } => {
            optimize_declaration(
                declaration,
                report,
                count_dead_statements,
                inline_functions,
                comptime,
            )
        }
        Declaration::Function(function) => optimize_function(
            function,
            report,
            count_dead_statements,
            inline_functions,
            comptime,
        ),
        Declaration::Const(decl) => {
            if !has_named_attr(&decl.attrs, "no-comptime") {
                decl.value = optimize_expr(
                    core::mem::replace(&mut decl.value, Expr::Int(0)),
                    &HashMap::new(),
                    report,
                    inline_functions,
                    comptime,
                );
            }
        }
        Declaration::Port(decl) => {
            decl.value = optimize_expr(
                core::mem::replace(&mut decl.value, Expr::Int(0)),
                &HashMap::new(),
                report,
                inline_functions,
                comptime,
            )
        }
        Declaration::Mmio(decl) => {
            decl.value = optimize_expr(
                core::mem::replace(&mut decl.value, Expr::Int(0)),
                &HashMap::new(),
                report,
                inline_functions,
                comptime,
            )
        }
        Declaration::Global(decl) => {
            decl.value = optimize_expr(
                core::mem::replace(&mut decl.value, Expr::Int(0)),
                &HashMap::new(),
                report,
                inline_functions,
                comptime,
            )
        }
        Declaration::Embed(_)
        | Declaration::Import(_)
        | Declaration::Alias(_)
        | Declaration::Struct(_)
        | Declaration::ExternAsmFunction(_) => {}
    }
}

fn optimize_function(
    function: &mut Function,
    report: &mut TbirOptimizationReport,
    count_dead_statements: bool,
    inline_functions: &HashSet<String>,
    comptime: &ComptimeContext,
) {
    let mut constants = HashMap::new();
    function.body = optimize_stmts(
        core::mem::take(&mut function.body),
        &mut constants,
        report,
        count_dead_statements,
        inline_functions,
        comptime,
    );
}

fn optimize_stmts(
    stmts: Vec<Stmt>,
    constants: &mut HashMap<String, Expr>,
    report: &mut TbirOptimizationReport,
    count_dead_statements: bool,
    inline_functions: &HashSet<String>,
    comptime: &ComptimeContext,
) -> Vec<Stmt> {
    let mut output = Vec::with_capacity(stmts.len());
    let mut terminated = false;
    for stmt in stmts {
        if terminated {
            if count_dead_statements {
                report.dead_statements_marked += 1;
            }
            continue;
        }
        let stmt = optimize_stmt(
            stmt,
            constants,
            report,
            count_dead_statements,
            inline_functions,
            comptime,
        );
        terminated = terminates(&stmt);
        output.push(stmt);
    }
    output
}

fn optimize_stmt(
    stmt: Stmt,
    constants: &mut HashMap<String, Expr>,
    report: &mut TbirOptimizationReport,
    count_dead_statements: bool,
    inline_functions: &HashSet<String>,
    comptime: &ComptimeContext,
) -> Stmt {
    match stmt {
        Stmt::Let { name, ty, value } => {
            let value = type_unsigned_power_of_two_divisor(value, &ty);
            let value = optimize_expr(value, constants, report, inline_functions, comptime);
            Stmt::Let { name, ty, value }
        }
        Stmt::LetTwo {
            first_name,
            first_ty,
            second_name,
            second_ty,
            value,
        } => Stmt::LetTwo {
            first_name,
            first_ty,
            second_name,
            second_ty,
            value: optimize_expr(value, constants, report, inline_functions, comptime),
        },
        Stmt::Assign { target, op, value } => {
            let target = optimize_place(target, constants, report, inline_functions, comptime);
            let value = optimize_expr(value, constants, report, inline_functions, comptime);
            Stmt::Assign { target, op, value }
        }
        Stmt::If {
            condition,
            then_body,
            else_body,
        } => {
            let condition = optimize_expr(condition, constants, report, inline_functions, comptime);
            let mut then_constants = constants.clone();
            let mut else_constants = constants.clone();
            let then_body = optimize_stmts(
                then_body,
                &mut then_constants,
                report,
                count_dead_statements,
                inline_functions,
                comptime,
            );
            let else_body = optimize_stmts(
                else_body,
                &mut else_constants,
                report,
                count_dead_statements,
                inline_functions,
                comptime,
            );
            Stmt::If {
                condition,
                then_body,
                else_body,
            }
        }
        Stmt::While { condition, body } => {
            let condition = optimize_expr(condition, constants, report, inline_functions, comptime);
            let mut body_constants = constants.clone();
            let body = optimize_stmts(
                body,
                &mut body_constants,
                report,
                count_dead_statements,
                inline_functions,
                comptime,
            );
            Stmt::While { condition, body }
        }
        Stmt::Loop { body } => {
            let mut body_constants = constants.clone();
            let body = optimize_stmts(
                body,
                &mut body_constants,
                report,
                count_dead_statements,
                inline_functions,
                comptime,
            );
            Stmt::Loop { body }
        }
        Stmt::Return(value) => Stmt::Return(
            value.map(|value| optimize_expr(value, constants, report, inline_functions, comptime)),
        ),
        Stmt::ReturnTwo { first, second } => Stmt::ReturnTwo {
            first: optimize_expr(first, constants, report, inline_functions, comptime),
            second: optimize_expr(second, constants, report, inline_functions, comptime),
        },
        Stmt::Out { port, value } => Stmt::Out {
            port,
            value: optimize_expr(value, constants, report, inline_functions, comptime),
        },
        Stmt::Expr(value) => {
            let value = optimize_expr(value, constants, report, inline_functions, comptime);
            Stmt::Expr(value)
        }
        Stmt::Asm { .. } => stmt,
        Stmt::Break | Stmt::Continue => stmt,
    }
}

fn optimize_place(
    place: Place,
    constants: &HashMap<String, Expr>,
    report: &mut TbirOptimizationReport,
    inline_functions: &HashSet<String>,
    comptime: &ComptimeContext,
) -> Place {
    match place {
        Place::Index { name, index } => Place::Index {
            name,
            index: Box::new(optimize_expr(
                *index,
                constants,
                report,
                inline_functions,
                comptime,
            )),
        },
        Place::Access(path) => Place::Access(optimize_access(
            path,
            constants,
            report,
            inline_functions,
            comptime,
        )),
        Place::Deref(expr) => Place::Deref(Box::new(optimize_expr(
            *expr,
            constants,
            report,
            inline_functions,
            comptime,
        ))),
        Place::Ident(_) | Place::Field { .. } => place,
    }
}

fn type_unsigned_power_of_two_divisor(expr: Expr, ty: &Type) -> Expr {
    let Expr::Binary {
        left,
        op: op @ (BinaryOp::Div | BinaryOp::Mod),
        right,
    } = expr
    else {
        return expr;
    };
    let right = match *right {
        Expr::Int(value)
            if type_is_unsigned_integer(ty) && value > 1 && (value as u64).is_power_of_two() =>
        {
            Expr::TypedInt(value, ty.clone())
        }
        right => right,
    };
    Expr::Binary {
        left,
        op,
        right: Box::new(right),
    }
}

fn optimize_expr(
    mut expr: Expr,
    constants: &HashMap<String, Expr>,
    report: &mut TbirOptimizationReport,
    inline_functions: &HashSet<String>,
    comptime: &ComptimeContext,
) -> Expr {
    if comptime.is_comptime_call(&expr).is_none()
        && comptime.references_enabled_constant(&expr)
        && let Ok(value) = comptime.evaluate(&expr)
        && value != expr
        && is_comptime_inlineable(&value)
    {
        report.comptime_evaluations += 1;
        return value;
    }

    expr = match expr {
        Expr::Ident(name) => Expr::Ident(name),
        Expr::Array(values) => Expr::Array(
            values
                .into_iter()
                .map(|value| optimize_expr(value, constants, report, inline_functions, comptime))
                .collect(),
        ),
        Expr::Index { name, index } => Expr::Index {
            name,
            index: Box::new(optimize_expr(
                *index,
                constants,
                report,
                inline_functions,
                comptime,
            )),
        },
        Expr::AddressOfIndex { name, index } => Expr::AddressOfIndex {
            name,
            index: Box::new(optimize_expr(
                *index,
                constants,
                report,
                inline_functions,
                comptime,
            )),
        },
        Expr::Access(path) => Expr::Access(optimize_access(
            path,
            constants,
            report,
            inline_functions,
            comptime,
        )),
        Expr::AddressOfAccess(path) => Expr::AddressOfAccess(optimize_access(
            path,
            constants,
            report,
            inline_functions,
            comptime,
        )),
        Expr::StructInit { ty, fields } => Expr::StructInit {
            ty,
            fields: fields
                .into_iter()
                .map(|(name, value)| {
                    (
                        name,
                        optimize_expr(value, constants, report, inline_functions, comptime),
                    )
                })
                .collect(),
        },
        Expr::Deref(value) => Expr::Deref(Box::new(optimize_expr(
            *value,
            constants,
            report,
            inline_functions,
            comptime,
        ))),
        Expr::BankedPointer { pointer, bank } => Expr::BankedPointer {
            pointer: Box::new(optimize_expr(
                *pointer,
                constants,
                report,
                inline_functions,
                comptime,
            )),
            bank,
        },
        Expr::Call { path, args } => Expr::Call {
            path,
            args: args
                .into_iter()
                .map(|arg| optimize_expr(arg, constants, report, inline_functions, comptime))
                .collect(),
        },
        Expr::Unary { op, expr } => {
            let expr = optimize_expr(*expr, constants, report, inline_functions, comptime);
            if op == UnaryOp::BitNot
                && let Expr::Unary {
                    op: UnaryOp::BitNot,
                    expr,
                } = expr
            {
                report.algebraic_simplifications += 1;
                *expr
            } else if let Some(value) = fold_unary(op, &expr) {
                report.constant_folds += 1;
                value
            } else {
                Expr::Unary {
                    op,
                    expr: Box::new(expr),
                }
            }
        }
        Expr::Binary { left, op, right } => {
            let left = optimize_expr(*left, constants, report, inline_functions, comptime);
            let right = optimize_expr(*right, constants, report, inline_functions, comptime);
            if let Some(value) = fold_binary(&left, op, &right) {
                report.constant_folds += 1;
                value
            } else if let Some((value, strength_reduction)) =
                simplify_binary(&left, op, &right, inline_functions)
            {
                if strength_reduction {
                    report.strength_reductions += 1;
                    decision(
                        report,
                        TbirOptimizationKind::StrengthReduction,
                        None,
                        "binary expression",
                        None,
                    );
                } else {
                    report.algebraic_simplifications += 1;
                }
                value
            } else {
                Expr::Binary {
                    left: Box::new(left),
                    op,
                    right: Box::new(right),
                }
            }
        }
        Expr::Cast { ty, expr } => {
            let expr = optimize_expr(*expr, constants, report, inline_functions, comptime);
            let expr = remove_unsigned_narrowing_mask(&ty, expr).unwrap_or_else(|expr| expr);
            Expr::Cast {
                ty,
                expr: Box::new(expr),
            }
        }
        Expr::Int(_)
        | Expr::TypedInt(_, _)
        | Expr::Bool(_)
        | Expr::Char(_)
        | Expr::String(_)
        | Expr::In(_)
        | Expr::Field { .. }
        | Expr::AddressOfField { .. }
        | Expr::AddressOf(_) => expr,
    };
    if let Some(callee) = comptime.is_comptime_call(&expr) {
        match comptime.evaluate(&expr) {
            Ok(value) if value != expr => {
                report.comptime_evaluations += 1;
                decision(report, TbirOptimizationKind::Comptime, None, &callee, None);
                return value;
            }
            Ok(_) => {}
            Err(failure) => {
                report.comptime_rejections += 1;
                decision(
                    report,
                    TbirOptimizationKind::Comptime,
                    None,
                    &callee,
                    Some(failure.reason()),
                );
            }
        }
    }
    expr
}

fn is_comptime_inlineable(expr: &Expr) -> bool {
    matches!(
        expr,
        Expr::Int(_) | Expr::TypedInt(_, _) | Expr::Bool(_) | Expr::Char(_)
    )
}

fn remove_unsigned_narrowing_mask(target: &Type, expr: Expr) -> Result<Expr, Expr> {
    let Expr::Binary {
        left,
        op: BinaryOp::BitAnd,
        right,
    } = expr
    else {
        return Err(expr);
    };
    let Expr::TypedInt(mask, source) = right.as_ref() else {
        return Err(Expr::Binary {
            left,
            op: BinaryOp::BitAnd,
            right,
        });
    };
    let retained_mask = match (source, target) {
        (Type::Named(source), Type::Named(target)) if source == "u16" && target == "u8" => 0xFF,
        (Type::Named(source), Type::Named(target)) if source == "u24" && target == "u8" => 0xFF,
        (Type::Named(source), Type::Named(target)) if source == "u24" && target == "u16" => 0xFFFF,
        _ => {
            return Err(Expr::Binary {
                left,
                op: BinaryOp::BitAnd,
                right,
            });
        }
    };
    if *mask & retained_mask == retained_mask {
        Ok(*left)
    } else {
        Err(Expr::Binary {
            left,
            op: BinaryOp::BitAnd,
            right,
        })
    }
}

fn optimize_access(
    mut path: AccessPath,
    constants: &HashMap<String, Expr>,
    report: &mut TbirOptimizationReport,
    inline_functions: &HashSet<String>,
    comptime: &ComptimeContext,
) -> AccessPath {
    path.segments = path
        .segments
        .into_iter()
        .map(|segment| match segment {
            AccessSegment::Index(index) => AccessSegment::Index(Box::new(optimize_expr(
                *index,
                constants,
                report,
                inline_functions,
                comptime,
            ))),
            AccessSegment::Field(field) => AccessSegment::Field(field),
        })
        .collect();
    path
}

fn simplify_binary(
    left: &Expr,
    op: BinaryOp,
    right: &Expr,
    inline_functions: &HashSet<String>,
) -> Option<(Expr, bool)> {
    let algebraic = |expr| Some((expr, false));
    let strength = |expr| Some((expr, true));
    match (left, op, right) {
        (
            value,
            BinaryOp::Add
            | BinaryOp::Sub
            | BinaryOp::BitOr
            | BinaryOp::BitXor
            | BinaryOp::Shl
            | BinaryOp::Shr,
            value_expr,
        ) if int_value(value_expr) == Some(0) => algebraic(value.clone()),
        (value, BinaryOp::Mul | BinaryOp::Div, value_expr) if int_value(value_expr) == Some(1) => {
            algebraic(value.clone())
        }
        (_, BinaryOp::Div | BinaryOp::Mod, value_expr) if int_value(value_expr) == Some(0) => {
            algebraic(Expr::Int(0))
        }
        (_, BinaryOp::Mul | BinaryOp::BitAnd, value)
            if int_value(value) == Some(0) && is_pure_scalar(left) =>
        {
            algebraic(Expr::Int(0))
        }
        (value, BinaryOp::Mul | BinaryOp::BitAnd, _)
            if int_value(value) == Some(0) && is_pure_scalar(right) =>
        {
            algebraic(Expr::Int(0))
        }
        (value, BinaryOp::Div, divisor) => {
            unsigned_power_of_two(divisor).and_then(|(shift, ty)| {
                strength(typed_shift_expr(value.clone(), BinaryOp::Shr, shift, ty))
            })
        }
        (value, BinaryOp::Mod, divisor) => unsigned_power_of_two(divisor).and_then(|(_, ty)| {
            let mask = int_value(divisor)? - 1;
            strength(Expr::Binary {
                left: Box::new(value.clone()),
                op: BinaryOp::BitAnd,
                right: Box::new(Expr::TypedInt(mask, ty)),
            })
        }),
        (value_expr, BinaryOp::Mul, value) if power_of_two_shift(value_expr).is_some() => {
            power_of_two_shift(value_expr)
                .and_then(|shift| strength(shift_expr(value.clone(), BinaryOp::Shl, shift)))
        }
        (value, BinaryOp::Mul, value_expr) => power_of_two_shift(value_expr)
            .and_then(|shift| strength(shift_expr(value.clone(), BinaryOp::Shl, shift))),
        (value_expr, BinaryOp::Add | BinaryOp::BitOr | BinaryOp::BitXor, value)
            if int_value(value_expr) == Some(0) =>
        {
            algebraic(value.clone())
        }
        (value, BinaryOp::BitAnd, mask) if is_all_ones_mask(mask) => algebraic(value.clone()),
        (value, BinaryOp::BitXor, mask) if is_all_ones_mask(mask) => algebraic(Expr::Unary {
            op: UnaryOp::BitNot,
            expr: Box::new(value.clone()),
        }),
        (
            Expr::Binary {
                left: inner,
                op: inner_op,
                right: inner_constant,
            },
            outer_op,
            outer_constant,
        ) if matches!(
            inner_op,
            BinaryOp::BitAnd | BinaryOp::BitOr | BinaryOp::BitXor
        ) && *inner_op == outer_op =>
        {
            combine_bitwise_constants(inner.as_ref(), *inner_op, inner_constant, outer_constant)
                .map(algebraic)
                .unwrap_or(None)
        }
        (Expr::Bool(true), BinaryOp::And, value)
        | (Expr::Bool(false), BinaryOp::Or, value)
        | (value, BinaryOp::And, Expr::Bool(true))
        | (value, BinaryOp::Or, Expr::Bool(false)) => algebraic(value.clone()),
        (Expr::Bool(false), BinaryOp::And, value)
            if is_pure_scalar(value) || !contains_inline_call(value, inline_functions) =>
        {
            algebraic(Expr::Bool(false))
        }
        (Expr::Bool(true), BinaryOp::Or, value)
            if is_pure_scalar(value) || !contains_inline_call(value, inline_functions) =>
        {
            algebraic(Expr::Bool(true))
        }
        (value, BinaryOp::And, Expr::Bool(false)) if is_pure_scalar(value) => {
            algebraic(Expr::Bool(false))
        }
        (value, BinaryOp::Or, Expr::Bool(true)) if is_pure_scalar(value) => {
            algebraic(Expr::Bool(true))
        }
        _ => None,
    }
}

fn int_value(expr: &Expr) -> Option<i64> {
    match expr {
        Expr::Int(value) | Expr::TypedInt(value, _) => Some(*value),
        _ => None,
    }
}

fn is_all_ones_mask(expr: &Expr) -> bool {
    let Expr::TypedInt(value, ty) = expr else {
        return false;
    };
    typed_integer_mask(ty).is_some_and(|mask| *value == mask)
}

fn combine_bitwise_constants(
    value: &Expr,
    op: BinaryOp,
    inner: &Expr,
    outer: &Expr,
) -> Option<Expr> {
    let (Expr::TypedInt(inner_value, inner_ty), Expr::TypedInt(outer_value, outer_ty)) =
        (inner, outer)
    else {
        return None;
    };
    if inner_ty != outer_ty {
        return None;
    }
    let combined = match op {
        BinaryOp::BitAnd => inner_value & outer_value,
        BinaryOp::BitOr => inner_value | outer_value,
        BinaryOp::BitXor => inner_value ^ outer_value,
        _ => return None,
    };
    Some(Expr::Binary {
        left: Box::new(value.clone()),
        op,
        right: Box::new(Expr::TypedInt(combined, inner_ty.clone())),
    })
}

fn unsigned_power_of_two(expr: &Expr) -> Option<(u32, Type)> {
    let Expr::TypedInt(value, ty) = expr else {
        return None;
    };
    if !type_is_unsigned_integer(ty) || *value <= 1 || !(*value as u64).is_power_of_two() {
        return None;
    }
    Some(((*value as u64).trailing_zeros(), ty.clone()))
}

fn power_of_two_shift(expr: &Expr) -> Option<u32> {
    let value = int_value(expr)?;
    if value > 1 && (value as u64).is_power_of_two() {
        Some((value as u64).trailing_zeros())
    } else {
        None
    }
}

fn shift_expr(value: Expr, op: BinaryOp, shift: u32) -> Expr {
    Expr::Binary {
        left: Box::new(value),
        op,
        right: Box::new(Expr::Int(i64::from(shift))),
    }
}

fn typed_shift_expr(value: Expr, op: BinaryOp, shift: u32, ty: Type) -> Expr {
    Expr::Binary {
        left: Box::new(value),
        op,
        right: Box::new(Expr::TypedInt(i64::from(shift), ty)),
    }
}

fn fold_unary(op: UnaryOp, expr: &Expr) -> Option<Expr> {
    match (op, expr) {
        (UnaryOp::Not, Expr::Bool(value)) => Some(Expr::Bool(!value)),
        (UnaryOp::BitNot, Expr::TypedInt(value, ty)) => {
            let value = if type_is_unsigned_integer(ty) {
                (!value) & typed_integer_mask(ty)?
            } else {
                !value
            };
            Some(Expr::TypedInt(value, ty.clone()))
        }
        _ => None,
    }
}

fn type_is_unsigned_integer(ty: &Type) -> bool {
    matches!(ty, Type::Named(name) if matches!(name.as_str(), "u8" | "u16" | "u24" | "u32"))
}

fn typed_integer_mask(ty: &Type) -> Option<i64> {
    let bits = match ty {
        Type::Named(name) if matches!(name.as_str(), "u8" | "i8") => 8,
        Type::Named(name) if matches!(name.as_str(), "u16" | "i16") => 16,
        Type::Named(name) if matches!(name.as_str(), "u24" | "i24") => 24,
        Type::Named(name) if matches!(name.as_str(), "u32" | "i32") => 32,
        _ => return None,
    };
    Some((1_i64 << bits) - 1)
}

fn fold_binary(left: &Expr, op: BinaryOp, right: &Expr) -> Option<Expr> {
    match (left, right) {
        (Expr::Bool(left), Expr::Bool(right)) => match op {
            BinaryOp::And => Some(Expr::Bool(*left && *right)),
            BinaryOp::Or => Some(Expr::Bool(*left || *right)),
            BinaryOp::Eq => Some(Expr::Bool(left == right)),
            BinaryOp::Ne => Some(Expr::Bool(left != right)),
            _ => None,
        },
        (Expr::Int(left), Expr::Int(right)) => fold_integer_binary(*left, *right, op, None),
        (Expr::TypedInt(left, ty), Expr::TypedInt(right, _)) => {
            fold_integer_binary(*left, *right, op, Some(ty))
        }
        _ => None,
    }
}

fn fold_integer_binary(left: i64, right: i64, op: BinaryOp, ty: Option<&Type>) -> Option<Expr> {
    let value = match op {
        BinaryOp::BitAnd => left & right,
        BinaryOp::BitXor => left ^ right,
        BinaryOp::BitOr => left | right,
        _ => return None,
    };
    Some(match ty {
        Some(ty) => Expr::TypedInt(value, ty.clone()),
        None => Expr::Int(value),
    })
}

fn terminates(stmt: &Stmt) -> bool {
    matches!(stmt, Stmt::Return(_) | Stmt::Break | Stmt::Continue)
}

#[cfg(test)]
mod tests;
