use crate::{
    asm::{
        AssemblyOptions,
        comments::{stmt_summary, with_readability_comments},
        reachability::{RoutineProfile, strip_unreachable_generated_routines},
    },
    ast::{
        AccessPath, AccessSegment, AssignOp, BinaryOp, Declaration, Expr, Function, Place, Program,
        Stmt, Type, UnaryOp,
    },
    compat::prelude::*,
    diagnostic::Diagnostic,
    hir::HirProgram,
    intrinsics::{
        BitsIntrinsic, IntIntrinsic, IntrinsicOperation, IntrinsicResolution, MemIntrinsic,
    },
    regalloc::{
        Location, PhysReg, PhysicalRegister, RegClass, RegUnit, RegisterClass, RegisterUnit,
        SpillClass, SpillClassId, Target,
        source::{SourceLocal, allocate_source_locals},
    },
    target::CpuFamily,
    tbir::{
        TbirProgram,
        model::{SemanticModel, Storage},
    },
};

const FRAME_ADDRESS_BASE: u32 = 0x1_0000;
const FRAME_OFFSET_BIAS: i32 = 0x8000;
const FRAME_SCRATCH_BYTES: u32 = 32;

/// Emits the M6800/M6809 scalar source ABI.
///
/// M6800 functions reserve their frame on S and use X as the frame base. M6809
/// functions save the caller's U on S, use U as the frame base, and leave S for
/// return addresses and outgoing arguments. All compiler-owned storage is in
/// the active frame; globals, MMIO, strings, and embeds keep their model
/// addresses. Scalar returns use A; paired returns use A and B.
pub fn emit_m6800_assembly_with_options(
    program: &Program,
    options: AssemblyOptions,
) -> Result<String, Diagnostic> {
    if options.cpu != CpuFamily::M6800 {
        return Err(Diagnostic::new("M6800 emitter requires an M6800 target"));
    }
    if program.main_function().is_none() {
        return Err(Diagnostic::new("M6800 programs require a `main` function"));
    }
    let hir = HirProgram::from_ast(program)?;
    let (lowered_program, source_comments) = if contains_function_pointer_program(program) {
        (program.clone(), Vec::new())
    } else {
        let tbir = TbirProgram::lower(&hir, program, &options)?;
        (tbir.lowered_program, tbir.source_comments)
    };
    let model = SemanticModel::from_program(
        &lowered_program,
        16,
        options.ram_base.get(),
        options.rodata_base.get(),
        options.asset_base.get(),
    )?;
    Emitter::new(model, CpuFamily::M6800)?
        .emit(&lowered_program)
        .map(|asm| {
            let asm = if options
                .optimization
                .is_enabled(crate::optimization::OptimizationPass::RedundantRegisterCopies)
            {
                crate::asm::copy_cleanup::remove_redundant_register_copies(&asm, options.cpu)
            } else {
                asm
            };
            let asm = if options
                .optimization
                .is_enabled(crate::optimization::OptimizationPass::DeadCodeElimination)
                && !contains_function_pointer_program(program)
            {
                strip_unreachable_generated_routines(&asm, RoutineProfile::M6800)
            } else {
                asm
            };
            with_readability_comments(asm, program, &options, "m6800", &source_comments)
        })
}

#[cfg(feature = "m6809")]
/// Emits the shared scalar source ABI for Motorola M6809.
///
/// M6809 accepts the M6800 accumulator spelling used by this backend while
/// TBIR still sees the concrete M6809 target and applies its normal passes.
pub fn emit_m6809_assembly_with_options(
    program: &Program,
    options: AssemblyOptions,
) -> Result<String, Diagnostic> {
    if options.cpu != CpuFamily::M6809 {
        return Err(Diagnostic::new("M6809 emitter requires an M6809 target"));
    }
    if program.main_function().is_none() {
        return Err(Diagnostic::new("M6809 programs require a `main` function"));
    }
    let hir = HirProgram::from_ast(program)?;
    let (lowered_program, source_comments) = if contains_function_pointer_program(program) {
        (program.clone(), Vec::new())
    } else {
        let tbir = TbirProgram::lower(&hir, program, &options)?;
        (tbir.lowered_program, tbir.source_comments)
    };
    let model = SemanticModel::from_program(
        &lowered_program,
        16,
        options.ram_base.get(),
        options.rodata_base.get(),
        options.asset_base.get(),
    )?;
    Emitter::new(model, CpuFamily::M6809)?
        .emit(&lowered_program)
        .map(|asm| {
            let asm = if options
                .optimization
                .is_enabled(crate::optimization::OptimizationPass::RedundantRegisterCopies)
            {
                crate::asm::copy_cleanup::remove_redundant_register_copies(&asm, options.cpu)
            } else {
                asm
            };
            let asm = if options
                .optimization
                .is_enabled(crate::optimization::OptimizationPass::DeadCodeElimination)
                && !contains_function_pointer_program(program)
            {
                strip_unreachable_generated_routines(&asm, RoutineProfile::M6800)
            } else {
                asm
            };
            let asm = asm.replace(
                "; target: Motorola M6800 scalar RAM ABI",
                "; target: Motorola M6809 scalar RAM ABI",
            );
            with_readability_comments(asm, program, &options, "m6809", &source_comments)
        })
}

#[derive(Clone)]
struct Binding {
    storage: Storage,
    ty: Type,
}

#[derive(Clone)]
struct FunctionFrame {
    locals: HashMap<String, Binding>,
    frame_size: u32,
    r1: Storage,
    multiply_addend: Storage,
    multiply_result: Storage,
    intrinsic_scratch: Storage,
    return_scratch: Storage,
    pointer_scratch: Storage,
}

#[derive(Clone)]
struct LoopLabels {
    continue_label: String,
    break_label: String,
}

struct Emitter {
    model: SemanticModel,
    cpu: CpuFamily,
    out: String,
    labels: usize,
    scopes: Vec<HashMap<String, Binding>>,
    loops: Vec<LoopLabels>,
    return_labels: Vec<String>,
    return_types: Vec<Option<Type>>,
    second_return_types: Vec<Option<Type>>,
    local_plans: Vec<HashMap<String, Binding>>,
    frame_sizes: Vec<u32>,
    r1: Storage,
    multiply_addend: Storage,
    multiply_result: Storage,
    intrinsic_scratch: Storage,
    return_scratch: Storage,
    pointer_scratch: Storage,
    pending_stack_bytes: Vec<u32>,
}

impl Emitter {
    fn new(model: SemanticModel, cpu: CpuFamily) -> Result<Self, Diagnostic> {
        let empty = frame_storage(cpu, 0, 1);
        Ok(Self {
            model,
            cpu,
            out: String::new(),
            labels: 0,
            scopes: Vec::new(),
            loops: Vec::new(),
            return_labels: Vec::new(),
            return_types: Vec::new(),
            second_return_types: Vec::new(),
            local_plans: Vec::new(),
            frame_sizes: Vec::new(),
            r1: empty,
            multiply_addend: empty,
            multiply_result: empty,
            intrinsic_scratch: empty,
            return_scratch: empty,
            pointer_scratch: empty,
            pending_stack_bytes: Vec::new(),
        })
    }

    fn emit(mut self, program: &Program) -> Result<String, Diagnostic> {
        self.line("; generated by ezrac");
        self.line(if self.cpu == CpuFamily::M6800 {
            "; target: Motorola M6800 X-based stack frames"
        } else {
            "; target: Motorola M6809 U/S stack frames"
        });
        self.line("__ezra_start:");
        self.emit_startup_frame(program)?;
        for declaration in &program.declarations {
            if let Declaration::Function(function) = declaration {
                self.emit_function(function)?;
                self.emit_function_pointer_trampoline(function)?;
            }
        }
        Ok(self.out)
    }

    fn emit_startup_frame(&mut self, program: &Program) -> Result<(), Diagnostic> {
        let frame = startup_frame(self.cpu);
        self.activate_frame(&frame);
        self.pending_stack_bytes.push(0);
        self.enter_frame(frame.frame_size);
        self.emit_global_initializers(program)?;
        self.leave_frame(frame.frame_size);
        self.pending_stack_bytes.pop();
        self.deactivate_frame();
        self.line("    jsr _main");
        self.line("__ezra_exit:");
        self.line("    bra __ezra_exit");
        Ok(())
    }

    fn activate_frame(&mut self, frame: &FunctionFrame) {
        self.r1 = frame.r1;
        self.multiply_addend = frame.multiply_addend;
        self.multiply_result = frame.multiply_result;
        self.intrinsic_scratch = frame.intrinsic_scratch;
        self.return_scratch = frame.return_scratch;
        self.pointer_scratch = frame.pointer_scratch;
    }

    fn deactivate_frame(&mut self) {
        let empty = frame_storage(self.cpu, 0, 1);
        self.r1 = empty;
        self.multiply_addend = empty;
        self.multiply_result = empty;
        self.intrinsic_scratch = empty;
        self.return_scratch = empty;
        self.pointer_scratch = empty;
    }

    fn enter_frame(&mut self, frame_size: u32) {
        if self.cpu == CpuFamily::M6800 {
            for _ in 0..frame_size {
                self.line("    des");
            }
            self.line("    tsx");
        } else {
            self.line("    pshs u");
            if frame_size != 0 {
                self.line(&format!("    leas -{},s", frame_size));
            }
            self.line("    tfr s,u");
        }
    }

    fn leave_frame(&mut self, frame_size: u32) {
        if self.cpu == CpuFamily::M6800 {
            self.restore_m6800_frame_pointer();
            for _ in 0..frame_size {
                self.line("    inx");
            }
            self.line("    txs");
        } else {
            if frame_size != 0 {
                self.line(&format!("    leas {},s", frame_size));
            }
            self.line("    puls u");
        }
    }

    fn function_value_type(&self, name: &str) -> Option<Type> {
        self.model
            .functions
            .get(name)
            .filter(|signature| signature.second_return_type.is_none())
            .map(|signature| Type::Function {
                params: signature.params.clone(),
                return_type: signature.return_type.clone().map(Box::new),
            })
    }

    fn emit_function_pointer_value(
        &mut self,
        value: &Expr,
        expected: &Type,
    ) -> Result<(), Diagnostic> {
        let expected = self.model.resolved_type(expected)?;
        let Type::Ptr(inner) = &expected else {
            return Err(Diagnostic::new("expected a function pointer type"));
        };
        if !matches!(inner.as_ref(), Type::Function { .. }) {
            return Err(Diagnostic::new("expected a function pointer type"));
        }
        match value {
            Expr::AddressOf(name) => {
                let Some(function_type) = self.function_value_type(name) else {
                    if self.model.functions.contains_key(name) {
                        return Err(Diagnostic::new(format!(
                            "M6800 function pointer cannot reference two-result function `{name}`"
                        )));
                    }
                    return Err(Diagnostic::new(format!(
                        "unknown function `{name}` in function pointer initializer"
                    )));
                };
                let actual = Type::Ptr(Box::new(function_type));
                if self.model.resolved_type(&actual)? != expected {
                    return Err(Diagnostic::new(format!(
                        "function `{name}` has a different function-pointer type"
                    )));
                }
                if self.cpu == CpuFamily::M6800 {
                    self.ldaa_imm(self.function_pointer_token(name)?);
                } else {
                    self.line(&format!("    ldx #{}", function_pointer_label(name)));
                }
            }
            Expr::Ident(name) => {
                let binding = self.binding(name)?;
                if is_function_pointer_type(&self.model, &binding.ty)? {
                    if self.cpu == CpuFamily::M6800 {
                        self.ldaa(binding.storage.address);
                    } else {
                        self.ldx(binding.storage.address);
                    }
                } else {
                    return Err(Diagnostic::new(format!(
                        "`{name}` is not a function pointer"
                    )));
                }
            }
            Expr::Cast { expr, .. } => self.emit_function_pointer_value(expr, &expected)?,
            _ => {
                return Err(Diagnostic::new(
                    "M6800 function pointers must be initialized from `&function` or another function pointer",
                ));
            }
        }
        Ok(())
    }

    fn emit_function_pointer_trampoline(&mut self, function: &Function) -> Result<(), Diagnostic> {
        if function.second_return_type.is_some() {
            return Ok(());
        }
        self.line(&format!("{}:", function_pointer_label(&function.name)));
        // The trampoline preserves the caller's stack layout. The target
        // function owns the frame and returns directly to the original caller.
        self.line(&format!("    jmp _{}", sanitize_label(&function.name)));
        Ok(())
    }

    fn store_function_pointer(
        &mut self,
        value: &Expr,
        storage: Storage,
        ty: &Type,
    ) -> Result<(), Diagnostic> {
        let expected = self.model.resolved_type(ty)?;
        let Type::Ptr(ref inner) = expected else {
            return Err(Diagnostic::new("expected a function pointer type"));
        };
        if !matches!(**inner, Type::Function { .. }) {
            return Err(Diagnostic::new("expected a function pointer type"));
        }
        if self.cpu == CpuFamily::M6800 {
            self.emit_function_pointer_value(value, ty)?;
            self.staa(storage.address);
            self.clra();
            self.staa(storage_offset(storage, 1).address);
            return Ok(());
        }
        self.emit_function_pointer_value(value, ty)?;
        self.stx(storage.address);
        Ok(())
    }

    fn emit_global_initializers(&mut self, program: &Program) -> Result<(), Diagnostic> {
        let embeds = self.model.embeds.values().cloned().collect::<Vec<_>>();
        for embed in embeds {
            for (offset, byte) in embed.bytes.into_iter().enumerate() {
                self.ldaa_imm(byte);
                self.staa(embed.storage.address + offset as u32);
            }
        }
        let strings = self
            .model
            .strings
            .iter()
            .map(|(value, storage)| (value.clone(), *storage))
            .collect::<Vec<_>>();
        for (value, storage) in strings {
            for (offset, byte) in value.bytes().chain(core::iter::once(0)).enumerate() {
                self.ldaa_imm(byte);
                self.staa(storage.address + offset as u32);
            }
        }
        for declaration in &program.declarations {
            match declaration {
                Declaration::Const(constant) if matches!(constant.ty, Type::Array { .. }) => {
                    let storage = self.model.globals[&constant.name];
                    self.emit_const_array_initializer(storage, &constant.ty, &constant.value)?;
                }
                Declaration::Global(global) => {
                    let storage = self.model.globals[&global.name];
                    if is_function_pointer_type(&self.model, &global.ty)? {
                        self.store_function_pointer(&global.value, storage, &global.ty)?;
                    } else {
                        self.require_scalar(&global.ty, "global")?;
                        self.emit_expr(&global.value, &global.ty)?;
                        self.staa(storage.address);
                    }
                }
                _ => {}
            }
        }
        Ok(())
    }

    fn emit_const_array_initializer(
        &mut self,
        storage: Storage,
        ty: &Type,
        value: &Expr,
    ) -> Result<(), Diagnostic> {
        let Type::Array { element, len } = self.model.resolved_type(ty)? else {
            return Err(Diagnostic::new(
                "const array initializer requires an array type",
            ));
        };
        let len = u32::try_from(self.model.const_value(&len)?)
            .map_err(|_| Diagnostic::new("invalid const array length"))?;
        let Expr::Array(values) = value else {
            return Err(Diagnostic::new(
                "const array initializer requires an array literal",
            ));
        };
        let element_size = self.model.type_size(&element)?;
        for index in 0..len {
            let element_storage = Storage {
                address: storage
                    .address
                    .checked_add(index.checked_mul(element_size).ok_or_else(|| {
                        Diagnostic::new("const array initializer address overflow")
                    })?)
                    .ok_or_else(|| Diagnostic::new("const array initializer address overflow"))?,
                size: element_size,
            };
            if let Some(value) = values.get(index as usize) {
                if matches!(self.model.resolved_type(&element)?, Type::Array { .. }) {
                    self.emit_const_array_initializer(element_storage, &element, value)?;
                } else {
                    self.require_scalar(&element, "const array element")?;
                    self.emit_expr(value, &element)?;
                    self.staa(element_storage.address);
                }
            } else {
                self.ldaa_imm(0);
                self.staa(element_storage.address);
            }
        }
        Ok(())
    }

    fn emit_function(&mut self, function: &Function) -> Result<(), Diagnostic> {
        if function
            .attrs
            .iter()
            .any(|attr| attr == "naked" || attr == "interrupt")
        {
            return Err(Diagnostic::new(format!(
                "M6800 source emitter does not support `{}` functions",
                function.name
            )));
        }
        for param in &function.params {
            self.require_scalar(&param.ty, "function parameter")?;
        }
        if let Some(ty) = &function.return_type {
            self.require_scalar(ty, "function return")?;
        }
        if function.name == "main" && function.second_return_type.is_some() {
            return Err(Diagnostic::new(
                "main cannot return two values because its startup caller has no second-result destination",
            ));
        }
        if function.second_return_type.is_some() && function.return_type.is_none() {
            return Err(Diagnostic::new(format!(
                "two-result function `{}` must have a first return type",
                function.name
            )));
        }
        if let Some(ty) = &function.second_return_type {
            self.require_scalar(ty, "second function return")?;
            if !block_guarantees_two_result_return(&function.body) {
                return Err(Diagnostic::new(format!(
                    "missing two return values in function `{}`",
                    function.name
                )));
            }
        }
        let frame = plan_function_locals(function, &mut self.model, self.cpu)?;
        let return_label = self.next_label(&format!("{}_return", function.name));
        self.line(&format!("_{}:", function.name));
        self.scopes.push(HashMap::new());
        self.return_labels.push(return_label.clone());
        self.return_types.push(function.return_type.clone());
        self.second_return_types
            .push(function.second_return_type.clone());
        self.local_plans.push(frame.locals.clone());
        self.frame_sizes.push(frame.frame_size);
        self.pending_stack_bytes.push(0);
        self.activate_frame(&frame);
        self.enter_frame(frame.frame_size);

        let signature = self.model.functions[&function.name].clone();
        let argument_bytes = signature
            .params
            .iter()
            .map(|ty| self.model.type_size(ty))
            .try_fold(0u32, |total, size| {
                total
                    .checked_add(size?)
                    .ok_or_else(|| Diagnostic::new("M6800/M6809 argument frame is too large"))
            })?;
        let mut consumed = 0u32;
        for (param, ty) in function.params.iter().zip(&signature.params) {
            let size = self.model.type_size(ty)?;
            let offset = frame.frame_size
                + if self.cpu == CpuFamily::M6800 { 2 } else { 4 }
                + argument_bytes
                - consumed
                - size;
            self.bind(
                param.name.clone(),
                frame_storage(
                    self.cpu,
                    i32::try_from(offset).map_err(|_| {
                        Diagnostic::new("M6800/M6809 function parameter frame is too large")
                    })?,
                    size,
                ),
                param.ty.clone(),
            )?;
            consumed += size;
        }
        self.emit_block(&function.body)?;
        self.line(&format!("{return_label}:"));
        self.leave_frame(frame.frame_size);
        self.line("    rts");
        self.pending_stack_bytes.pop();
        self.frame_sizes.pop();
        self.second_return_types.pop();
        self.return_types.pop();
        self.return_labels.pop();
        self.local_plans.pop();
        self.scopes.pop();
        self.deactivate_frame();
        Ok(())
    }

    fn emit_block(&mut self, body: &[Stmt]) -> Result<(), Diagnostic> {
        for stmt in body {
            self.emit_stmt(stmt)?;
        }
        Ok(())
    }

    fn emit_stmt(&mut self, stmt: &Stmt) -> Result<(), Diagnostic> {
        self.line(&format!("    ; source: {}", stmt_summary(stmt)));
        match stmt {
            Stmt::Let { name, ty, value } => {
                let storage = self
                    .local_plans
                    .last()
                    .and_then(|locals| locals.get(name))
                    .map(|binding| binding.storage)
                    .ok_or_else(|| {
                        Diagnostic::new(format!("missing allocation for local `{name}`"))
                    })?;
                self.bind(name.clone(), storage, ty.clone())?;
                if is_function_pointer_type(&self.model, ty)? {
                    self.store_function_pointer(value, storage, ty)?;
                } else {
                    self.require_scalar(ty, "local")?;
                    self.emit_expr(value, ty)?;
                    self.staa(storage.address);
                }
            }
            Stmt::LetTwo {
                first_name,
                first_ty: _,
                second_name,
                second_ty: _,
                value,
            } => {
                let first = self
                    .local_plans
                    .last()
                    .and_then(|locals| locals.get(first_name))
                    .cloned()
                    .ok_or_else(|| {
                        Diagnostic::new(format!("missing allocation for local `{first_name}`"))
                    })?;
                let second = self
                    .local_plans
                    .last()
                    .and_then(|locals| locals.get(second_name))
                    .cloned()
                    .ok_or_else(|| {
                        Diagnostic::new(format!("missing allocation for local `{second_name}`"))
                    })?;
                self.require_scalar(&first.ty, "local")?;
                self.require_scalar(&second.ty, "local")?;
                self.bind(first_name.clone(), first.storage, first.ty.clone())?;
                self.bind(second_name.clone(), second.storage, second.ty.clone())?;
                let Expr::Call { path, args } = value else {
                    return Err(Diagnostic::new(
                        "two-result bindings require a direct two-result call",
                    ));
                };
                self.emit_two_result_call(path, args, &first, &second)?;
            }
            Stmt::Assign { target, op, value } => {
                let Place::Ident(name) = target else {
                    return Err(Diagnostic::new(
                        "M6800 source emitter supports assignment to scalar identifiers only",
                    ));
                };
                let binding = self.binding(name)?;
                if is_function_pointer_type(&self.model, &binding.ty)? {
                    if *op != AssignOp::Set {
                        return Err(Diagnostic::new(
                            "compound assignment is not supported for function pointers",
                        ));
                    }
                    self.store_function_pointer(value, binding.storage, &binding.ty)?;
                    return Ok(());
                }
                self.require_scalar(&binding.ty, "assignment target")?;
                if *op == AssignOp::Set {
                    self.emit_expr(value, &binding.ty)?;
                } else {
                    self.ldaa(binding.storage.address);
                    self.push_a();
                    self.emit_expr(value, &binding.ty)?;
                    self.transfer_a_to_b();
                    self.pull_a();
                    self.emit_assign_op(*op)?;
                }
                self.staa(binding.storage.address);
            }
            Stmt::If {
                condition,
                then_body,
                else_body,
            } => {
                let otherwise = self.next_label("if_else");
                let done = self.next_label("if_end");
                if !self.emit_jump_if_false(condition, &otherwise)? {
                    self.emit_expr(condition, &bool_type())?;
                    self.line("    tsta");
                    self.branch_long("beq", &otherwise);
                }
                self.emit_block(then_body)?;
                self.line(&format!("    jmp {done}"));
                self.line(&format!("{otherwise}:"));
                self.emit_block(else_body)?;
                self.line(&format!("{done}:"));
            }
            Stmt::While { condition, body } => {
                let check = self.next_label("while_check");
                let done = self.next_label("while_end");
                self.loops.push(LoopLabels {
                    continue_label: check.clone(),
                    break_label: done.clone(),
                });
                self.line(&format!("{check}:"));
                if !self.emit_jump_if_false(condition, &done)? {
                    self.emit_expr(condition, &bool_type())?;
                    self.line("    tsta");
                    self.branch_long("beq", &done);
                }
                self.emit_block(body)?;
                self.line(&format!("    jmp {check}"));
                self.line(&format!("{done}:"));
                self.loops.pop();
            }
            Stmt::Loop { body } => {
                let again = self.next_label("loop_body");
                let done = self.next_label("loop_end");
                self.loops.push(LoopLabels {
                    continue_label: again.clone(),
                    break_label: done.clone(),
                });
                self.line(&format!("{again}:"));
                self.emit_block(body)?;
                self.line(&format!("    jmp {again}"));
                self.line(&format!("{done}:"));
                self.loops.pop();
            }
            Stmt::Break => {
                let label = self
                    .loops
                    .last()
                    .ok_or_else(|| Diagnostic::new("break outside loop"))?
                    .break_label
                    .clone();
                self.line(&format!("    jmp {label}"));
            }
            Stmt::Continue => {
                let label = self
                    .loops
                    .last()
                    .ok_or_else(|| Diagnostic::new("continue outside loop"))?
                    .continue_label
                    .clone();
                self.line(&format!("    jmp {label}"));
            }
            Stmt::Return(value) => {
                if self
                    .second_return_types
                    .last()
                    .and_then(Clone::clone)
                    .is_some()
                {
                    return Err(Diagnostic::new(
                        "two-result function must use `return first, second`",
                    ));
                }
                let expected = self.return_types.last().cloned().flatten();
                match (value, expected) {
                    (Some(value), Some(ty)) => self.emit_expr(value, &ty)?,
                    (Some(_), None) => {
                        return Err(Diagnostic::new("value return in void function"));
                    }
                    (None, Some(_)) => {
                        return Err(Diagnostic::new("M6800 scalar function must return a value"));
                    }
                    (None, None) => {}
                }
                let label = self
                    .return_labels
                    .last()
                    .expect("function return label")
                    .clone();
                self.line(&format!("    jmp {label}"));
            }
            Stmt::ReturnTwo { first, second } => self.emit_return_two(first, second)?,
            Stmt::Expr(expr) => {
                // A void call is meaningful as a statement even though it has no expression type.
                if matches!(expr, Expr::Call { .. }) {
                    self.emit_expr(expr, &u8_type())?;
                } else {
                    self.emit_expr(expr, &self.expr_type(expr)?)?;
                }
            }
            Stmt::Asm {
                volatile,
                inputs,
                outputs,
                clobbers,
                lines,
            } => self.inline_asm(*volatile, inputs, outputs, clobbers, lines)?,
            Stmt::Out { .. } => {
                return Err(Diagnostic::new(
                    "M6800 source emitter does not support port I/O",
                ));
            }
        }
        Ok(())
    }

    fn emit_expr(&mut self, expr: &Expr, expected: &Type) -> Result<(), Diagnostic> {
        if is_function_pointer_type(&self.model, expected)? {
            return self.emit_function_pointer_value(expr, expected);
        }
        if let Expr::Call { path, args } = expr {
            if let Some(resolution) = self.resolve_intrinsic_call(path, args)? {
                return self.emit_intrinsic(resolution, args, expected);
            }
        }
        self.require_scalar(expected, "expression")?;
        match expr {
            Expr::Int(value) | Expr::TypedInt(value, _) => self.ldaa_imm(*value as u8),
            Expr::Bool(value) => self.ldaa_imm(u8::from(*value)),
            Expr::Char(value) => self.ldaa_imm(*value),
            Expr::Ident(name) => {
                if let Ok(binding) = self.binding(name) {
                    self.require_scalar(&binding.ty, "identifier")?;
                    self.ldaa(binding.storage.address);
                } else if let Some(value) = self.model.constants.get(name) {
                    self.ldaa_imm(*value as u8);
                } else {
                    return Err(Diagnostic::new(format!("unknown value `{name}`")));
                }
            }
            Expr::Index { name, index } => self.emit_array_index(name, index)?,
            Expr::Deref(pointer) => {
                self.emit_pointer_to_x(pointer)?;
                self.line("    ldaa 0,x");
                self.restore_m6800_frame_pointer();
            }
            Expr::Unary { op, expr } => {
                self.emit_expr(expr, expected)?;
                match op {
                    UnaryOp::Neg => self.line("    nega"),
                    UnaryOp::BitNot => self.line("    coma"),
                    UnaryOp::Not => self.bool_not(),
                }
            }
            Expr::Binary { left, op, right } => {
                if matches!(op, BinaryOp::And | BinaryOp::Or) {
                    return self.emit_logical(left, *op, right);
                }
                let operand_ty = if is_comparison(*op) {
                    self.expr_type(left)?
                } else {
                    expected.clone()
                };
                self.require_scalar(&operand_ty, "binary operand")?;
                if matches!(op, BinaryOp::Shl | BinaryOp::Shr)
                    && let Some(count) = self.constant_shift_count(right)
                {
                    self.emit_expr(left, &operand_ty)?;
                    self.emit_constant_shift(*op, count, &operand_ty)?;
                    return Ok(());
                }
                self.emit_expr(left, &operand_ty)?;
                self.push_a();
                self.emit_expr(right, &operand_ty)?;
                self.staa(self.r1.address);
                self.pull_a();
                self.emit_binary(*op)?;
            }
            Expr::Call { path, args } => self.emit_call(path, args, expected)?,
            Expr::Cast { ty, expr } => {
                self.require_scalar(ty, "cast")?;
                self.emit_expr(expr, ty)?;
            }
            Expr::BankedPointer { pointer, .. } => self.emit_expr(pointer, expected)?,
            Expr::String(_)
            | Expr::Array(_)
            | Expr::StructInit { .. }
            | Expr::Field { .. }
            | Expr::AddressOfIndex { .. }
            | Expr::AddressOfField { .. }
            | Expr::Access(_)
            | Expr::AddressOfAccess(_)
            | Expr::AddressOf(_)
            | Expr::In(_) => {
                return Err(Diagnostic::new(
                    "M6800 source emitter supports scalar u8/bool expressions only",
                ));
            }
        }
        Ok(())
    }

    fn emit_array_index(&mut self, name: &str, index: &Expr) -> Result<(), Diagnostic> {
        let binding = self.binding(name)?;
        let array_ty = self.model.resolved_type(&binding.ty)?;
        let Type::Array { element, len } = array_ty else {
            return Err(Diagnostic::new("indexing requires an array"));
        };
        let element = *element;
        let element_size = self.model.type_size(&element)?;
        if element_size != 1 {
            return Err(Diagnostic::new(
                "M6800 array indexing supports one-byte elements only",
            ));
        }
        if let Ok(index_value) = self.model.const_value(index) {
            let length = self.model.const_value(&len)?;
            if index_value < 0 || index_value >= length {
                return Err(Diagnostic::new(format!(
                    "array index {index_value} is out of bounds for `{name}` length {length}"
                )));
            }
            let address = binding
                .storage
                .address
                .checked_add(
                    u32::try_from(index_value)
                        .map_err(|_| Diagnostic::new("array index offset overflow"))?,
                )
                .ok_or_else(|| Diagnostic::new("array index offset overflow"))?;
            self.ldaa(address);
            return Ok(());
        }

        self.emit_expr(index, &u8_type())?;
        let base = u16::try_from(binding.storage.address)
            .map_err(|_| Diagnostic::new("M6800 array is outside the 16-bit address space"))?;
        self.line(&format!("    ldx #{base:04X}h"));
        let loop_label = self.next_label("array_index_loop");
        let done = self.next_label("array_index_done");
        self.line(&format!("{loop_label}:"));
        self.line("    tsta");
        self.branch_long("beq", &done);
        self.increment_x();
        self.line("    deca");
        self.line(&format!("    bra {loop_label}"));
        self.line(&format!("{done}:"));
        self.line("    ldaa 0,x");
        Ok(())
    }

    fn emit_call(
        &mut self,
        path: &[String],
        args: &[Expr],
        expected: &Type,
    ) -> Result<(), Diagnostic> {
        if let Some(resolution) = self.resolve_intrinsic_call(path, args)? {
            return self.emit_intrinsic(resolution, args, expected);
        }
        let name = path.join(".");
        let direct_signature = self
            .model
            .functions
            .get(&name)
            .or_else(|| path.last().and_then(|n| self.model.functions.get(n)))
            .cloned();
        let (signature, indirect_binding) = if let Some(signature) = direct_signature {
            (signature, None)
        } else {
            if path.len() != 1 {
                return Err(Diagnostic::new(format!("unknown function `{name}`")));
            }
            let binding = self.binding(&path[0])?;
            let pointer_type = self.model.resolved_type(&binding.ty)?;
            let Type::Ptr(inner) = pointer_type.clone() else {
                return Err(Diagnostic::new(format!(
                    "M6800 function pointer call requires `ptr<fn(...)>`, got `{pointer_type:?}`"
                )));
            };
            let Type::Function {
                params,
                return_type,
            } = *inner
            else {
                return Err(Diagnostic::new(format!(
                    "M6800 function pointer call requires `ptr<fn(...)>`, got `{pointer_type:?}`"
                )));
            };
            (
                crate::tbir::model::FunctionSignature {
                    params,
                    return_type: return_type.map(|ty| *ty),
                    second_return_type: None,
                    argument_slots: Vec::new(),
                },
                Some(binding),
            )
        };
        if signature.second_return_type.is_some() {
            return Err(Diagnostic::new(format!(
                "two-result function `{name}` requires a two-result binding"
            )));
        }
        if signature.params.len() != args.len() {
            return Err(Diagnostic::new(format!(
                "function `{name}` expects {} arguments, got {}",
                signature.params.len(),
                args.len()
            )));
        }
        let base_pending = self.pending_stack_bytes.last().copied().unwrap_or(0);
        for (arg, ty) in args.iter().zip(&signature.params) {
            if is_function_pointer_type(&self.model, ty)? {
                self.push_function_pointer_argument(arg, ty)?;
            } else {
                self.require_scalar(ty, "function argument")?;
                self.emit_expr(arg, ty)?;
                self.push_argument_a();
            }
        }
        if let Some(binding) = indirect_binding {
            if self.cpu == CpuFamily::M6800 {
                self.emit_m6800_function_pointer_call(&binding)?;
            } else {
                self.ldx(binding.storage.address);
                self.line("    jsr 0,x");
            }
        } else {
            self.line(&format!(
                "    jsr _{}",
                sanitize_label(path.last().unwrap_or(&name))
            ));
        }
        let argument_bytes = signature
            .params
            .iter()
            .map(|ty| self.model.type_size(ty))
            .try_fold(0u32, |total, size| {
                total
                    .checked_add(size?)
                    .ok_or_else(|| Diagnostic::new("M6800/M6809 argument frame is too large"))
            })?;
        self.pop_call_arguments(argument_bytes);
        if let Some(pending) = self.pending_stack_bytes.last_mut() {
            *pending = base_pending;
        }
        self.restore_m6800_frame_pointer();
        if let Some(return_type) = signature.return_type {
            self.require_scalar(&return_type, "function return")?;
        } else if expected != &bool_type() && expected != &u8_type() {
            return Err(Diagnostic::new("void M6800 function used as a value"));
        }
        Ok(())
    }

    fn push_argument_a(&mut self) {
        if self.cpu == CpuFamily::M6800 {
            self.line("    psha");
        } else {
            self.line("    pshs a");
        }
        if let Some(pending) = self.pending_stack_bytes.last_mut() {
            *pending += 1;
        }
    }

    fn push_function_pointer_argument(
        &mut self,
        value: &Expr,
        ty: &Type,
    ) -> Result<(), Diagnostic> {
        if self.cpu == CpuFamily::M6809 {
            self.emit_function_pointer_value(value, ty)?;
            self.line("    pshs x");
            if let Some(pending) = self.pending_stack_bytes.last_mut() {
                *pending += 2;
            }
            return Ok(());
        }
        self.ldaa_imm(0);
        self.push_argument_a();
        self.emit_function_pointer_value(value, ty)?;
        self.push_argument_a();
        Ok(())
    }

    fn emit_m6800_function_pointer_call(&mut self, binding: &Binding) -> Result<(), Diagnostic> {
        let pointer_type = self.model.resolved_type(&binding.ty)?;
        let Type::Ptr(inner) = pointer_type else {
            return Err(Diagnostic::new("expected a function pointer"));
        };
        let targets = self.function_pointer_targets(&inner)?;
        self.ldaa(binding.storage.address);
        let done = self.next_label("function_pointer_call_done");
        let invalid = self.next_label("function_pointer_call_invalid");
        let dispatch = targets
            .iter()
            .map(|(_, name)| (name.clone(), self.next_label("function_pointer_call")))
            .collect::<Vec<_>>();
        for ((token, _), (_, label)) in targets.iter().zip(&dispatch) {
            self.line(&format!("    cmpa #{token:02X}h"));
            self.branch_long("beq", label);
        }
        self.line(&format!("    bra {invalid}"));
        for ((_, name), (_, label)) in targets.iter().zip(&dispatch) {
            self.line(&format!("{label}:"));
            self.line(&format!("    jsr _{}", sanitize_label(name)));
            self.line(&format!("    bra {done}"));
        }
        self.line(&format!("{invalid}:"));
        self.line(&format!("    bra {invalid}"));
        self.line(&format!("{done}:"));
        Ok(())
    }

    fn function_pointer_targets(
        &self,
        function_type: &Type,
    ) -> Result<Vec<(u8, String)>, Diagnostic> {
        let Type::Function {
            params,
            return_type,
        } = self.model.resolved_type(function_type)?
        else {
            return Err(Diagnostic::new("expected a function type"));
        };
        let mut names = self.model.functions.keys().cloned().collect::<Vec<_>>();
        names.sort();
        let mut targets = Vec::new();
        for name in names {
            let signature = &self.model.functions[&name];
            if signature.second_return_type.is_some()
                || signature.params.len() != params.len()
                || !signature
                    .params
                    .iter()
                    .zip(&params)
                    .all(|(actual, expected)| {
                        self.model.resolved_type(actual).ok()
                            == self.model.resolved_type(expected).ok()
                    })
            {
                continue;
            }
            let return_matches = match (&signature.return_type, return_type.as_deref()) {
                (None, None) => true,
                (Some(actual), Some(expected)) => {
                    self.model.resolved_type(actual).ok() == self.model.resolved_type(expected).ok()
                }
                _ => false,
            };
            if return_matches {
                targets.push((self.function_pointer_token(&name)?, name));
            }
        }
        if targets.is_empty() {
            return Err(Diagnostic::new(
                "M6800 function pointer has no matching target function",
            ));
        }
        Ok(targets)
    }

    fn function_pointer_token(&self, name: &str) -> Result<u8, Diagnostic> {
        let mut names = self.model.functions.keys().cloned().collect::<Vec<_>>();
        names.sort();
        let index = names
            .iter()
            .position(|candidate| candidate == name)
            .ok_or_else(|| Diagnostic::new(format!("unknown function `{name}`")))?;
        u8::try_from(index + 1)
            .map_err(|_| Diagnostic::new("M6800 program has too many function-pointer targets"))
    }

    fn pop_call_arguments(&mut self, bytes: u32) {
        if self.cpu == CpuFamily::M6800 {
            for _ in 0..bytes {
                self.line("    ins");
            }
        } else if bytes != 0 {
            self.line(&format!("    leas {},s", bytes));
        }
    }

    fn emit_intrinsic(
        &mut self,
        resolution: IntrinsicResolution,
        args: &[Expr],
        expected: &Type,
    ) -> Result<(), Diagnostic> {
        if let Some(result_ty) = resolution.result_types.first() {
            let expected = self.model.resolved_type(expected)?;
            let result_ty = self.model.resolved_type(result_ty)?;
            if expected != result_ty {
                return Err(Diagnostic::new(format!(
                    "intrinsic result has type {result_ty:?}, expected {expected:?}"
                )));
            }
        }
        match resolution.descriptor.operation {
            IntrinsicOperation::Bits(operation) => self.emit_bits_intrinsic(operation, args),
            IntrinsicOperation::Int(operation) => self.emit_int_intrinsic(operation, args),
            IntrinsicOperation::Mem(operation) => self.emit_mem_intrinsic(operation, args),
        }
    }

    fn resolve_intrinsic_call(
        &self,
        path: &[String],
        args: &[Expr],
    ) -> Result<Option<IntrinsicResolution>, Diagnostic> {
        let name = path.join(".");
        if crate::intrinsics::CATALOG.lookup(&name).is_none() {
            return Ok(None);
        }
        let mut argument_types = Vec::with_capacity(args.len());
        let mut constants = Vec::with_capacity(args.len());
        for arg in args {
            argument_types.push(self.model.resolved_type(&self.expr_type(arg)?)?);
            constants.push(self.model.const_value(arg).ok());
        }
        crate::intrinsics::CATALOG
            .validate_types_with_constants(&name, &argument_types, &constants)
            .map(Some)
            .map_err(|error| Diagnostic::new(error.to_string()))
    }

    fn emit_bits_intrinsic(
        &mut self,
        operation: BitsIntrinsic,
        args: &[Expr],
    ) -> Result<(), Diagnostic> {
        match operation {
            BitsIntrinsic::RotateLeft | BitsIntrinsic::RotateRight => {
                self.require_arg_count(args, 2, "rotate")?;
                self.require_u8_bit_value(&args[0])?;
                self.emit_expr(&args[0], &u8_type())?;
                self.staa(self.scratch(12));
                self.emit_expr(&args[1], &u8_type())?;
                self.line("    anda #07h");
                self.staa(self.scratch(13));
                let loop_label = self.next_label("intrinsic_rotate_loop");
                let done = self.next_label("intrinsic_rotate_done");
                self.line(&format!("{loop_label}:"));
                self.ldaa(self.scratch(13));
                self.line(&format!("    beq {done}"));
                self.ldaa(self.scratch(12));
                if operation == BitsIntrinsic::RotateLeft {
                    self.line("    asla");
                    let no_carry = self.next_label("intrinsic_rotate_no_carry");
                    self.line(&format!("    bcc {no_carry}"));
                    self.line("    oraa #01h");
                    self.line(&format!("{no_carry}:"));
                } else {
                    self.line("    lsra");
                    let no_carry = self.next_label("intrinsic_rotate_no_carry");
                    self.line(&format!("    bcc {no_carry}"));
                    self.line("    oraa #80h");
                    self.line(&format!("{no_carry}:"));
                }
                self.staa(self.scratch(12));
                self.ldaa(self.scratch(13));
                self.line("    deca");
                self.staa(self.scratch(13));
                self.line(&format!("    bra {loop_label}"));
                self.line(&format!("{done}:"));
                self.ldaa(self.scratch(12));
            }
            BitsIntrinsic::Test
            | BitsIntrinsic::Set
            | BitsIntrinsic::Clear
            | BitsIntrinsic::Toggle => {
                self.require_arg_count(args, 2, "bit intrinsic")?;
                self.require_u8_bit_value(&args[0])?;
                let bit = self.intrinsic_constant(args, 1, "bit index")?;
                let bit = u8::try_from(bit)
                    .ok()
                    .filter(|bit| *bit < 8)
                    .ok_or_else(|| Diagnostic::new("bit index is outside the input width"))?;
                self.emit_expr(&args[0], &u8_type())?;
                let mask = 1_u8 << bit;
                match operation {
                    BitsIntrinsic::Test => {
                        self.line(&format!("    bita #{mask:02X}h"));
                        self.emit_bool_from_branch("bne");
                    }
                    BitsIntrinsic::Set => self.line(&format!("    oraa #{mask:02X}h")),
                    BitsIntrinsic::Clear => self.line(&format!("    anda #{:02X}h", !mask)),
                    BitsIntrinsic::Toggle => self.line(&format!("    eora #{mask:02X}h")),
                    _ => unreachable!(),
                }
            }
            BitsIntrinsic::Extract => {
                self.require_arg_count(args, 3, "bits.extract")?;
                self.require_u8_bit_value(&args[0])?;
                let offset = self.intrinsic_constant(args, 1, "bit-range offset")?;
                let width = self.intrinsic_constant(args, 2, "bit-range width")?;
                let (offset, width) = self.validate_bit_range(offset, width)?;
                self.emit_expr(&args[0], &u8_type())?;
                for _ in 0..offset {
                    self.line("    lsra");
                }
                self.line(&format!("    anda #{:02X}h", (1_u16 << width) - 1));
            }
            BitsIntrinsic::Insert => {
                self.require_arg_count(args, 4, "bits.insert")?;
                self.require_u8_bit_value(&args[0])?;
                self.require_u8_bit_value(&args[1])?;
                let offset = self.intrinsic_constant(args, 2, "bit-range offset")?;
                let width = self.intrinsic_constant(args, 3, "bit-range width")?;
                let (offset, width) = self.validate_bit_range(offset, width)?;
                self.emit_expr(&args[0], &u8_type())?;
                self.staa(self.scratch(12));
                self.emit_expr(&args[1], &u8_type())?;
                self.line(&format!("    anda #{:02X}h", (1_u16 << width) - 1));
                for _ in 0..offset {
                    self.line("    asla");
                }
                self.staa(self.scratch(13));
                self.ldaa(self.scratch(12));
                let field_mask = (((1_u16 << width) - 1) << offset) as u8;
                self.line(&format!("    anda #{:02X}h", !field_mask));
                self.memory_instruction("oraa", self.scratch(13));
            }
            BitsIntrinsic::ByteSwap => {
                return Err(Diagnostic::new(
                    "M6800/M6809 bits.byte_swap requires at least u16",
                ));
            }
            BitsIntrinsic::Reverse => self.emit_reverse_bits(args)?,
            BitsIntrinsic::CountOnes
            | BitsIntrinsic::LeadingZeros
            | BitsIntrinsic::TrailingZeros => self.emit_bit_count(operation, args)?,
        }
        Ok(())
    }

    fn emit_reverse_bits(&mut self, args: &[Expr]) -> Result<(), Diagnostic> {
        self.require_arg_count(args, 1, "bits.reverse")?;
        self.require_u8_bit_value(&args[0])?;
        self.emit_expr(&args[0], &u8_type())?;
        self.staa(self.scratch(12));
        self.ldaa_imm(0);
        self.staa(self.scratch(13));
        self.ldaa_imm(8);
        self.staa(self.scratch(14));
        let loop_label = self.next_label("intrinsic_reverse_loop");
        self.line(&format!("{loop_label}:"));
        self.ldaa(self.scratch(12));
        self.line("    lsra");
        self.staa(self.scratch(12));
        self.ldab(self.scratch(13));
        self.line("    rolb");
        self.stab(self.scratch(13));
        self.ldaa(self.scratch(14));
        self.line("    deca");
        self.staa(self.scratch(14));
        self.line(&format!("    bne {loop_label}"));
        self.ldaa(self.scratch(13));
        Ok(())
    }

    fn emit_bit_count(
        &mut self,
        operation: BitsIntrinsic,
        args: &[Expr],
    ) -> Result<(), Diagnostic> {
        self.require_arg_count(args, 1, "bit count")?;
        self.require_u8_bit_value(&args[0])?;
        self.emit_expr(&args[0], &u8_type())?;
        self.staa(self.scratch(12));
        self.ldaa_imm(0);
        self.staa(self.scratch(13));
        self.ldaa_imm(8);
        self.staa(self.scratch(14));
        let loop_label = self.next_label("intrinsic_count_loop");
        let found = self.next_label("intrinsic_count_found");
        self.line(&format!("{loop_label}:"));
        self.ldaa(self.scratch(12));
        match operation {
            BitsIntrinsic::CountOnes | BitsIntrinsic::TrailingZeros => self.line("    lsra"),
            BitsIntrinsic::LeadingZeros => self.line("    asla"),
            _ => unreachable!(),
        }
        self.staa(self.scratch(12));
        self.line(&format!("    bcs {found}"));
        self.ldaa(self.scratch(13));
        self.line("    inca");
        self.staa(self.scratch(13));
        self.ldaa(self.scratch(14));
        self.line("    deca");
        self.staa(self.scratch(14));
        self.line(&format!("    bne {loop_label}"));
        self.line(&format!("{found}:"));
        self.ldaa(self.scratch(13));
        Ok(())
    }

    fn emit_int_intrinsic(
        &mut self,
        operation: IntIntrinsic,
        args: &[Expr],
    ) -> Result<(), Diagnostic> {
        match operation {
            IntIntrinsic::WideningMul => Err(Diagnostic::new(
                "M6800/M6809 widening multiplication returns an unsupported 16-bit scalar",
            )),
            IntIntrinsic::MulHigh => {
                self.emit_u8_full_mul(args)?;
                self.transfer_b_to_a();
                Ok(())
            }
            IntIntrinsic::SaturatingAdd | IntIntrinsic::SaturatingSub => {
                self.emit_saturating(args, operation == IntIntrinsic::SaturatingSub)
            }
            IntIntrinsic::Divmod
            | IntIntrinsic::AddCarry
            | IntIntrinsic::SubBorrow
            | IntIntrinsic::FullMul => Err(Diagnostic::new(
                "two-result integer intrinsic requires a two-result binding",
            )),
        }
    }

    fn emit_u8_full_mul(&mut self, args: &[Expr]) -> Result<(), Diagnostic> {
        self.require_arg_count(args, 2, "int.full_mul")?;
        self.require_u8_integer(&args[0])?;
        self.require_u8_integer(&args[1])?;
        self.emit_expr(&args[0], &u8_type())?;
        self.staa(self.scratch(12));
        self.emit_expr(&args[1], &u8_type())?;
        self.staa(self.scratch(13));
        let signed = matches!(
            self.model.resolved_type(&self.expr_type(&args[0])?)?,
            Type::Named(name) if name == "i8"
        );
        if signed {
            self.prepare_u8_magnitude(12, 18);
            self.prepare_u8_magnitude(13, 19);
        }
        if self.cpu == CpuFamily::M6809 {
            self.transfer_a_to_b();
            self.ldaa(self.scratch(12));
            self.line("    mul");
            self.staa(self.scratch(13));
            self.transfer_b_to_a();
            self.ldab(self.scratch(13));
        } else {
            self.staa(self.scratch(13));
            self.ldaa_imm(0);
            self.staa(self.scratch(14));
            self.staa(self.scratch(15));
            self.ldaa(self.scratch(12));
            self.staa(self.scratch(16));
            self.ldaa_imm(0);
            self.staa(self.scratch(17));
            self.ldab(self.scratch(13));
            self.ldaa_imm(8);
            self.staa(self.scratch(22));
            let loop_label = self.next_label("intrinsic_mul_loop");
            let skip = self.next_label("intrinsic_mul_skip");
            self.line(&format!("{loop_label}:"));
            self.line("    bitb #01h");
            self.branch_long("beq", &skip);
            self.ldaa(self.scratch(14));
            self.memory_instruction("adda", self.scratch(16));
            self.staa(self.scratch(14));
            self.ldaa(self.scratch(15));
            self.memory_instruction("adca", self.scratch(17));
            self.staa(self.scratch(15));
            self.line(&format!("{skip}:"));
            self.ldaa(self.scratch(16));
            self.line("    asla");
            self.staa(self.scratch(16));
            self.ldaa(self.scratch(17));
            self.line("    adca #00h");
            self.staa(self.scratch(17));
            self.line("    lsrb");
            self.ldaa(self.scratch(22));
            self.line("    deca");
            self.staa(self.scratch(22));
            self.line(&format!("    bne {loop_label}"));
            self.ldaa(self.scratch(14));
            self.ldab(self.scratch(15));
        }
        if signed {
            self.staa(self.scratch(20));
            self.stab(self.scratch(21));
            self.ldaa(self.scratch(18));
            self.memory_instruction("eora", self.scratch(19));
            let signed_done = self.next_label("intrinsic_mul_signed_done");
            self.line(&format!("    beq {signed_done}"));
            self.ldaa(self.scratch(20));
            self.line("    coma");
            self.line("    adda #01h");
            self.staa(self.scratch(20));
            let high_no_carry = self.next_label("intrinsic_mul_high_no_carry");
            self.line(&format!("    bcc {high_no_carry}"));
            self.ldaa(self.scratch(21));
            self.line("    coma");
            self.line("    adda #01h");
            self.staa(self.scratch(21));
            let high_done = self.next_label("intrinsic_mul_high_done");
            self.line(&format!("    bra {high_done}"));
            self.line(&format!("{high_no_carry}:"));
            self.ldaa(self.scratch(21));
            self.line("    coma");
            self.staa(self.scratch(21));
            self.line(&format!("{high_done}:"));
            self.line(&format!("{signed_done}:"));
            self.ldaa(self.scratch(20));
            self.ldab(self.scratch(21));
        }
        Ok(())
    }

    fn prepare_u8_magnitude(&mut self, value_offset: u32, sign_offset: u32) {
        let positive = self.next_label("intrinsic_magnitude_positive");
        self.ldaa(self.scratch(value_offset));
        self.line(&format!("    bpl {positive}"));
        self.line("    nega");
        self.staa(self.scratch(value_offset));
        self.ldaa_imm(1);
        self.staa(self.scratch(sign_offset));
        let done = self.next_label("intrinsic_magnitude_done");
        self.line(&format!("    bra {done}"));
        self.line(&format!("{positive}:"));
        self.ldaa_imm(0);
        self.staa(self.scratch(sign_offset));
        self.line(&format!("{done}:"));
        self.ldaa(self.scratch(value_offset));
    }

    fn emit_saturating(&mut self, args: &[Expr], subtract: bool) -> Result<(), Diagnostic> {
        self.require_arg_count(args, 2, "saturating arithmetic")?;
        self.require_u8_integer(&args[0])?;
        self.require_u8_integer(&args[1])?;
        let signed = matches!(self.model.resolved_type(&self.expr_type(&args[0])?)?, Type::Named(name) if name == "i8");
        self.emit_expr(&args[0], &u8_type())?;
        self.staa(self.scratch(12));
        self.emit_expr(&args[1], &u8_type())?;
        self.staa(self.scratch(13));
        self.ldaa(self.scratch(12));
        if subtract {
            self.memory_instruction("suba", self.scratch(13));
        } else {
            self.memory_instruction("adda", self.scratch(13));
        }
        let done = self.next_label("intrinsic_sat_done");
        let clamp = self.next_label("intrinsic_sat_clamp");
        if signed {
            self.branch_long("bvc", &done);
            self.ldaa(self.scratch(12));
            self.line(&format!("    bmi {clamp}"));
            self.ldaa_imm(0x7F);
            self.line(&format!("    bra {done}"));
            self.line(&format!("{clamp}:"));
            self.ldaa_imm(0x80);
        } else if subtract {
            self.branch_long("bcc", &done);
            self.ldaa_imm(0);
        } else {
            self.branch_long("bcc", &done);
            self.ldaa_imm(0xFF);
        }
        self.line(&format!("{done}:"));
        Ok(())
    }

    fn emit_intrinsic_pair(
        &mut self,
        resolution: IntrinsicResolution,
        args: &[Expr],
        first: &Binding,
        second: &Binding,
    ) -> Result<(), Diagnostic> {
        if resolution.result_types.len() != 2 {
            return Err(Diagnostic::new("intrinsic does not produce two results"));
        }
        if self.model.resolved_type(&first.ty)?
            != self.model.resolved_type(&resolution.result_types[0])?
            || self.model.resolved_type(&second.ty)?
                != self.model.resolved_type(&resolution.result_types[1])?
        {
            return Err(Diagnostic::new(
                "intrinsic result types do not match the two-result binding",
            ));
        }
        match resolution.descriptor.operation {
            IntrinsicOperation::Int(IntIntrinsic::FullMul) => self.emit_u8_full_mul(args)?,
            IntrinsicOperation::Int(IntIntrinsic::Divmod) => self.emit_divmod(args)?,
            IntrinsicOperation::Int(IntIntrinsic::AddCarry) => {
                self.emit_add_sub_carry(args, false)?
            }
            IntrinsicOperation::Int(IntIntrinsic::SubBorrow) => {
                self.emit_add_sub_carry(args, true)?
            }
            IntrinsicOperation::Mem(MemIntrinsic::FindByte) => {
                return Err(Diagnostic::new(
                    "M6800/M6809 find_byte cannot return a pointer in this ABI",
                ));
            }
            _ => {
                return Err(Diagnostic::new(
                    "intrinsic is not a supported paired operation",
                ));
            }
        }
        self.stab(second.storage.address);
        self.staa(first.storage.address);
        Ok(())
    }

    fn emit_divmod(&mut self, args: &[Expr]) -> Result<(), Diagnostic> {
        self.require_arg_count(args, 2, "int.divmod")?;
        self.require_u8_integer(&args[0])?;
        self.require_u8_integer(&args[1])?;
        let signed = matches!(self.model.resolved_type(&self.expr_type(&args[0])?)?, Type::Named(name) if name == "i8");
        self.emit_expr(&args[0], &u8_type())?;
        self.staa(self.scratch(12));
        self.emit_expr(&args[1], &u8_type())?;
        self.staa(self.scratch(13));
        self.ldaa(self.scratch(12));
        let left_positive = self.next_label("intrinsic_div_left_positive");
        if signed {
            self.line(&format!("    bpl {left_positive}"));
            self.line("    nega");
            self.staa(self.scratch(12));
            self.ldaa_imm(1);
            self.staa(self.scratch(14));
            self.line(&format!("    bra {left_positive}_sign_done"));
            self.line(&format!("{left_positive}:"));
            self.ldaa_imm(0);
            self.staa(self.scratch(14));
            self.line(&format!("{left_positive}_sign_done:"));
        } else {
            self.staa(self.scratch(12));
            self.ldaa_imm(0);
            self.staa(self.scratch(14));
        }
        self.ldaa(self.scratch(13));
        let right_positive = self.next_label("intrinsic_div_right_positive");
        if signed {
            self.line(&format!("    bpl {right_positive}"));
            self.line("    nega");
            self.staa(self.scratch(13));
            self.ldaa_imm(1);
            self.staa(self.scratch(15));
            self.line(&format!("    bra {right_positive}_sign_done"));
            self.line(&format!("{right_positive}:"));
            self.ldaa_imm(0);
            self.staa(self.scratch(15));
            self.line(&format!("{right_positive}_sign_done:"));
        } else {
            self.staa(self.scratch(13));
            self.ldaa_imm(0);
            self.staa(self.scratch(15));
        }
        let zero = self.next_label("intrinsic_div_zero");
        self.ldaa(self.scratch(13));
        self.line(&format!("    beq {zero}"));
        self.ldaa(self.scratch(12));
        self.line("    clrb");
        self.ldaa_imm(8);
        self.staa(self.scratch(17));
        let loop_label = self.next_label("intrinsic_div_loop");
        let no_sub = self.next_label("intrinsic_div_no_sub");
        self.line(&format!("{loop_label}:"));
        self.line("    asla");
        self.line("    rolb");
        self.memory_instruction("cmpb", self.scratch(13));
        self.branch_long("blo", &no_sub);
        self.memory_instruction("subb", self.scratch(13));
        self.line("    oraa #01h");
        self.line(&format!("{no_sub}:"));
        self.ldaa(self.scratch(17));
        self.line("    deca");
        self.staa(self.scratch(17));
        self.line(&format!("    bne {loop_label}"));
        if signed {
            let q_positive = self.next_label("intrinsic_div_q_positive");
            self.staa(self.scratch(16));
            self.ldaa(self.scratch(14));
            self.memory_instruction("eora", self.scratch(15));
            self.line(&format!("    beq {q_positive}"));
            self.ldaa(self.scratch(16));
            self.line("    nega");
            self.staa(self.scratch(16));
            self.line(&format!("{q_positive}:"));
            self.ldaa(self.scratch(14));
            let r_positive = self.next_label("intrinsic_div_r_positive");
            self.line(&format!("    beq {r_positive}"));
            self.line("    negb");
            self.line(&format!("{r_positive}:"));
            self.ldaa(self.scratch(16));
        }
        let done = self.next_label("intrinsic_div_done");
        self.line(&format!("    bra {done}"));
        self.line(&format!("{zero}:"));
        self.clra();
        self.clrb();
        self.line(&format!("{done}:"));
        Ok(())
    }

    fn emit_add_sub_carry(&mut self, args: &[Expr], subtract: bool) -> Result<(), Diagnostic> {
        self.require_arg_count(
            args,
            3,
            if subtract {
                "int.sub_borrow"
            } else {
                "int.add_carry"
            },
        )?;
        self.require_u8_integer(&args[0])?;
        self.require_u8_integer(&args[1])?;
        self.emit_expr(&args[0], &u8_type())?;
        self.staa(self.scratch(12));
        self.emit_expr(&args[1], &u8_type())?;
        self.staa(self.scratch(13));
        self.emit_expr(&args[2], &bool_type())?;
        self.staa(self.scratch(14));
        self.ldaa(self.scratch(12));
        if subtract {
            self.memory_instruction("suba", self.scratch(13));
        } else {
            self.memory_instruction("adda", self.scratch(13));
        }
        self.staa(self.scratch(15));
        let set = self.next_label("intrinsic_carry_set");
        let no_input = self.next_label("intrinsic_carry_no_input");
        let done = self.next_label("intrinsic_carry_done");
        self.line(&format!("    bcs {set}"));
        self.ldaa(self.scratch(14));
        self.line(&format!("    beq {no_input}"));
        self.ldaa(self.scratch(15));
        if subtract {
            self.line("    suba #01h");
        } else {
            self.line("    adda #01h");
        }
        self.staa(self.scratch(15));
        self.line(&format!("    bcs {set}"));
        self.line(&format!("{no_input}:"));
        self.ldab_imm(0);
        self.line(&format!("    bra {done}"));
        self.line(&format!("{set}:"));
        self.ldab_imm(1);
        self.line(&format!("{done}:"));
        self.ldaa(self.scratch(15));
        Ok(())
    }

    fn emit_mem_intrinsic(
        &mut self,
        operation: MemIntrinsic,
        args: &[Expr],
    ) -> Result<(), Diagnostic> {
        match operation {
            MemIntrinsic::Peek8 => {
                self.require_arg_count(args, 1, "mem.peek8")?;
                self.emit_pointer_to_x(&args[0])?;
                self.line("    ldaa 0,x");
                Ok(())
            }
            MemIntrinsic::Poke8 => {
                self.require_arg_count(args, 2, "mem.poke8")?;
                self.emit_pointer_to_x(&args[0])?;
                self.emit_expr(&args[1], &u8_type())?;
                self.line("    staa 0,x");
                Ok(())
            }
            MemIntrinsic::CopyNonoverlapping => self.emit_memory_copy(args, false),
            MemIntrinsic::Move => self.emit_memory_copy(args, true),
            MemIntrinsic::Fill => self.emit_memory_fill(args),
            MemIntrinsic::Compare => self.emit_memory_compare(args),
            MemIntrinsic::FindByte => Err(Diagnostic::new(
                "M6800/M6809 find_byte cannot return a pointer in this ABI",
            )),
            MemIntrinsic::LoadLe16
            | MemIntrinsic::LoadLe24
            | MemIntrinsic::LoadBe16
            | MemIntrinsic::LoadBe24
            | MemIntrinsic::StoreLe16
            | MemIntrinsic::StoreLe24
            | MemIntrinsic::StoreBe16
            | MemIntrinsic::StoreBe24 => Err(Diagnostic::new(
                "M6800/M6809 endian memory intrinsics require an unsupported wider scalar",
            )),
        }
    }

    fn emit_memory_copy(&mut self, args: &[Expr], overlap_safe: bool) -> Result<(), Diagnostic> {
        self.require_arg_count(args, 3, "memory copy")?;
        if self.cpu == CpuFamily::M6800 {
            let source = self.static_pointer_address(&args[0])?;
            let target = self.static_pointer_address(&args[1])?;
            let count = self.constant_memory_length(&args[2])?;
            let backward = overlap_safe && target > source && target < source.saturating_add(count);
            let range = if backward {
                (0..count).rev().collect::<Vec<_>>()
            } else {
                (0..count).collect::<Vec<_>>()
            };
            for offset in range {
                self.memory_instruction("ldaa", source + offset);
                self.memory_instruction("staa", target + offset);
            }
            self.restore_m6800_frame_pointer();
            self.clra();
            return Ok(());
        }
        self.emit_pointer_to_x(&args[0])?;
        self.stx(self.scratch(2));
        self.emit_pointer_to_x(&args[1])?;
        self.stx(self.scratch(0));
        self.emit_memory_count(&args[2], 4)?;
        let forward = self.next_label("intrinsic_copy_forward");
        let backward = self.next_label("intrinsic_copy_backward");
        let done = self.next_label("intrinsic_copy_done");
        self.ldx(self.scratch(2));
        if overlap_safe {
            self.memory_instruction("cpx", self.scratch(0));
            self.branch_long("blo", &forward);
            self.branch_long("beq", &forward);
            self.line(&format!("    bra {backward}"));
        } else {
            self.line(&format!("    bra {forward}"));
        }
        self.line(&format!("{forward}:"));
        let loop_label = self.next_label("intrinsic_copy_forward_loop");
        self.line(&format!("{loop_label}:"));
        self.counter_nonzero(4, &done);
        self.ldx(self.scratch(0));
        self.line("    ldaa 0,x");
        self.increment_x();
        self.stx(self.scratch(0));
        self.ldx(self.scratch(2));
        self.line("    staa 0,x");
        self.increment_x();
        self.stx(self.scratch(2));
        self.counter_dec(4);
        self.line(&format!("    bra {loop_label}"));
        if overlap_safe {
            self.line(&format!("{backward}:"));
            self.copy_counter(4, 7);
            self.advance_pointer(0, 7);
            self.copy_counter(4, 7);
            self.advance_pointer(2, 7);
            let backward_loop = self.next_label("intrinsic_copy_backward_loop");
            self.line(&format!("{backward_loop}:"));
            self.counter_nonzero(4, &done);
            self.ldx(self.scratch(0));
            self.decrement_x();
            self.line("    ldaa 0,x");
            self.stx(self.scratch(0));
            self.ldx(self.scratch(2));
            self.decrement_x();
            self.line("    staa 0,x");
            self.stx(self.scratch(2));
            self.counter_dec(4);
            self.line(&format!("    bra {backward_loop}"));
        }
        self.line(&format!("{done}:"));
        self.clra();
        Ok(())
    }

    fn emit_memory_fill(&mut self, args: &[Expr]) -> Result<(), Diagnostic> {
        self.require_arg_count(args, 3, "mem.fill")?;
        if self.cpu == CpuFamily::M6800 {
            let target = self.static_pointer_address(&args[0])?;
            let count = self.constant_memory_length(&args[2])?;
            self.emit_expr(&args[1], &u8_type())?;
            for offset in 0..count {
                self.line(&format!("    ldx #{:04X}h", target + offset));
                self.line("    staa 0,x");
            }
            self.restore_m6800_frame_pointer();
            self.clra();
            return Ok(());
        }
        self.emit_pointer_to_x(&args[0])?;
        self.stx(self.scratch(2));
        self.emit_expr(&args[1], &u8_type())?;
        self.staa(self.scratch(10));
        self.emit_memory_count(&args[2], 4)?;
        let loop_label = self.next_label("intrinsic_fill_loop");
        let done = self.next_label("intrinsic_fill_done");
        self.line(&format!("{loop_label}:"));
        self.counter_nonzero(4, &done);
        self.ldx(self.scratch(2));
        self.ldaa(self.scratch(10));
        self.line("    staa 0,x");
        self.increment_x();
        self.stx(self.scratch(2));
        self.counter_dec(4);
        self.line(&format!("    bra {loop_label}"));
        self.line(&format!("{done}:"));
        self.clra();
        Ok(())
    }

    fn emit_memory_compare(&mut self, args: &[Expr]) -> Result<(), Diagnostic> {
        self.require_arg_count(args, 3, "mem.compare")?;
        if self.cpu == CpuFamily::M6800 {
            let left = self.static_pointer_address(&args[0])?;
            let right = self.static_pointer_address(&args[1])?;
            let count = self.constant_memory_length(&args[2])?;
            let less = self.next_label("intrinsic_compare_less");
            let greater = self.next_label("intrinsic_compare_greater");
            let done = self.next_label("intrinsic_compare_done");
            for offset in 0..count {
                self.memory_instruction("ldaa", left + offset);
                self.memory_instruction("ldab", right + offset);
                self.line("    cba");
                self.branch_long("blo", &less);
                self.branch_long("bhi", &greater);
            }
            self.clra();
            self.line(&format!("    bra {done}"));
            self.line(&format!("{less}:"));
            self.ldaa_imm(0xFF);
            self.line(&format!("    bra {done}"));
            self.line(&format!("{greater}:"));
            self.ldaa_imm(1);
            self.line(&format!("{done}:"));
            self.restore_m6800_frame_pointer();
            return Ok(());
        }
        self.emit_pointer_to_x(&args[0])?;
        self.stx(self.scratch(0));
        self.emit_pointer_to_x(&args[1])?;
        self.stx(self.scratch(2));
        self.emit_memory_count(&args[2], 4)?;
        let loop_label = self.next_label("intrinsic_compare_loop");
        let less = self.next_label("intrinsic_compare_less");
        let greater = self.next_label("intrinsic_compare_greater");
        let done = self.next_label("intrinsic_compare_done");
        self.line(&format!("{loop_label}:"));
        self.counter_nonzero(4, &done);
        self.ldx(self.scratch(0));
        self.line("    ldaa 0,x");
        self.staa(self.scratch(10));
        self.increment_x();
        self.stx(self.scratch(0));
        self.ldx(self.scratch(2));
        self.line("    ldab 0,x");
        self.stab(self.scratch(11));
        self.increment_x();
        self.stx(self.scratch(2));
        self.ldaa(self.scratch(10));
        self.memory_instruction("cmpa", self.scratch(11));
        self.branch_long("blo", &less);
        self.branch_long("bhi", &greater);
        self.counter_dec(4);
        self.line(&format!("    bra {loop_label}"));
        self.line(&format!("{less}:"));
        self.ldaa_imm(0xFF);
        self.line(&format!("    bra {done}"));
        self.line(&format!("{greater}:"));
        self.ldaa_imm(1);
        self.line(&format!("{done}:"));
        Ok(())
    }

    fn emit_pointer_to_x(&mut self, expr: &Expr) -> Result<(), Diagnostic> {
        let address = self.pointer_storage(expr)?;
        if let Some(offset) = frame_offset(address.address) {
            if self.cpu == CpuFamily::M6800 {
                self.restore_m6800_frame_pointer();
                for _ in 0..u32::try_from(offset)
                    .map_err(|_| Diagnostic::new("M6800 frame pointer offset cannot be negative"))?
                {
                    self.line("    inx");
                }
            } else {
                self.line(&format!("    leax {offset},u"));
            }
        } else {
            let address = u16::try_from(address.address).map_err(|_| {
                Diagnostic::new("M6800 pointer is outside the 16-bit address space")
            })?;
            self.line(&format!("    ldx #{address:04X}h"));
        }
        Ok(())
    }

    fn pointer_storage(&self, expr: &Expr) -> Result<Storage, Diagnostic> {
        match expr {
            Expr::AddressOf(name) => self.binding(name).map(|binding| binding.storage),
            Expr::AddressOfIndex { name, index } => {
                let binding = self.binding(name)?;
                let element = m6800_element_type(&self.model.resolved_type(&binding.ty)?)?;
                let index = u32::try_from(self.model.const_value(index)?).map_err(|_| {
                    Diagnostic::new("M6800 pointer index must be a nonnegative constant")
                })?;
                Ok(storage_offset(
                    binding.storage,
                    index * self.model.type_size(&element)?,
                ))
            }
            Expr::AddressOfField { base, field } => {
                let binding = self.binding(base)?;
                Ok(storage_offset(
                    binding.storage,
                    self.model.field(&binding.ty, field)?.offset,
                ))
            }
            Expr::Cast { expr, .. } | Expr::BankedPointer { pointer: expr, .. } => {
                self.pointer_storage(expr)
            }
            _ => Err(Diagnostic::new(
                "M6800 memory intrinsics require a direct pointer expression",
            )),
        }
    }

    fn static_pointer_address(&self, expr: &Expr) -> Result<u32, Diagnostic> {
        let storage = self.pointer_storage(expr)?;
        if frame_offset(storage.address).is_some() {
            return Err(Diagnostic::new(
                "M6800 memory intrinsics require globals or MMIO pointers",
            ));
        }
        Ok(storage.address)
    }

    fn constant_memory_length(&self, expr: &Expr) -> Result<u32, Diagnostic> {
        let value = self
            .model
            .const_value(expr)
            .map_err(|_| Diagnostic::new("M6800 memory intrinsic length must be a constant"))?;
        u32::try_from(value)
            .ok()
            .filter(|value| *value <= 0xFF_FFFF)
            .ok_or_else(|| Diagnostic::new("memory intrinsic length is outside u24"))
    }

    fn emit_memory_count(&mut self, expr: &Expr, offset: u32) -> Result<(), Diagnostic> {
        let value = self
            .model
            .const_value(expr)
            .map_err(|_| Diagnostic::new("M6800 memory intrinsic length must be a constant"))?;
        let value = u32::try_from(value)
            .ok()
            .filter(|value| *value <= 0xFF_FFFF)
            .ok_or_else(|| Diagnostic::new("memory intrinsic length is outside u24"))?;
        for (index, byte) in [value as u8, (value >> 8) as u8, (value >> 16) as u8]
            .into_iter()
            .enumerate()
        {
            self.ldaa_imm(byte);
            self.staa(self.scratch(offset + index as u32));
        }
        Ok(())
    }

    fn counter_nonzero(&mut self, offset: u32, done: &str) {
        self.ldaa(self.scratch(offset));
        self.memory_instruction("oraa", self.scratch(offset + 1));
        self.memory_instruction("oraa", self.scratch(offset + 2));
        self.branch_long("beq", done);
    }

    fn counter_dec(&mut self, offset: u32) {
        let no_borrow = self.next_label("intrinsic_counter_no_borrow");
        let no_middle_borrow = self.next_label("intrinsic_counter_no_middle_borrow");
        self.ldaa(self.scratch(offset));
        self.line("    suba #01h");
        self.staa(self.scratch(offset));
        self.branch_long("bcc", &no_borrow);
        self.ldaa(self.scratch(offset + 1));
        self.line("    suba #01h");
        self.staa(self.scratch(offset + 1));
        self.branch_long("bcc", &no_middle_borrow);
        self.ldaa(self.scratch(offset + 2));
        self.line("    suba #01h");
        self.staa(self.scratch(offset + 2));
        self.line(&format!("{no_middle_borrow}:"));
        self.line(&format!("{no_borrow}:"));
    }

    fn copy_counter(&mut self, source: u32, target: u32) {
        for index in 0..3 {
            self.ldaa(self.scratch(source + index));
            self.staa(self.scratch(target + index));
        }
    }

    fn advance_pointer(&mut self, pointer_offset: u32, counter_offset: u32) {
        let loop_label = self.next_label("intrinsic_advance_pointer");
        let done = self.next_label("intrinsic_advance_done");
        self.line(&format!("{loop_label}:"));
        self.counter_nonzero(counter_offset, &done);
        self.ldx(self.scratch(pointer_offset));
        self.increment_x();
        self.stx(self.scratch(pointer_offset));
        self.counter_dec(counter_offset);
        self.line(&format!("    bra {loop_label}"));
        self.line(&format!("{done}:"));
    }

    fn require_arg_count(
        &self,
        args: &[Expr],
        expected: usize,
        name: &str,
    ) -> Result<(), Diagnostic> {
        if args.len() == expected {
            Ok(())
        } else {
            Err(Diagnostic::new(format!(
                "{name} requires {expected} arguments, got {}",
                args.len()
            )))
        }
    }

    fn intrinsic_constant(
        &self,
        args: &[Expr],
        index: usize,
        requirement: &str,
    ) -> Result<i64, Diagnostic> {
        self.model.const_value(&args[index]).map_err(|_| {
            Diagnostic::new(format!(
                "intrinsic argument {index} requires a constant {requirement}"
            ))
        })
    }

    fn require_u8_integer(&self, expr: &Expr) -> Result<(), Diagnostic> {
        match self.model.resolved_type(&self.expr_type(expr)?)? {
            Type::Named(name) if name == "u8" || name == "i8" => Ok(()),
            _ => Err(Diagnostic::new(
                "M6800/M6809 intrinsic requires an 8-bit integer",
            )),
        }
    }

    fn require_u8_bit_value(&self, expr: &Expr) -> Result<(), Diagnostic> {
        match self.model.resolved_type(&self.expr_type(expr)?)? {
            Type::Named(name) if name == "u8" => Ok(()),
            _ => Err(Diagnostic::new("M6800/M6809 bit intrinsic requires u8")),
        }
    }

    fn validate_bit_range(&self, offset: i64, width: i64) -> Result<(u32, u32), Diagnostic> {
        if offset < 0 || width <= 0 || offset + width > 8 {
            return Err(Diagnostic::new("bit range is outside the u8 input width"));
        }
        Ok((offset as u32, width as u32))
    }

    fn scratch(&self, offset: u32) -> u32 {
        self.intrinsic_scratch.address + offset
    }

    fn emit_bool_from_branch(&mut self, branch: &str) {
        let yes = self.next_label("intrinsic_true");
        let done = self.next_label("intrinsic_bool_done");
        self.branch_long(branch, &yes);
        self.ldaa_imm(0);
        self.line(&format!("    bra {done}"));
        self.line(&format!("{yes}:"));
        self.ldaa_imm(1);
        self.line(&format!("{done}:"));
    }

    fn transfer_b_to_a(&mut self) {
        if self.cpu == CpuFamily::M6809 {
            self.line("    tfr b,a");
        } else {
            self.line("    tba");
        }
    }

    fn ldab(&mut self, address: u32) {
        self.memory_instruction("ldab", address);
    }

    fn ldab_imm(&mut self, value: u8) {
        self.line(&format!("    ldab #{value:02X}h"));
    }

    fn clra(&mut self) {
        self.ldaa_imm(0);
    }

    fn clrb(&mut self) {
        self.ldab_imm(0);
    }

    fn emit_return_two(&mut self, first: &Expr, second: &Expr) -> Result<(), Diagnostic> {
        let first_ty = self.return_types.last().cloned().flatten().ok_or_else(|| {
            Diagnostic::new("function cannot return two values without a first return type")
        })?;
        let second_ty = self
            .second_return_types
            .last()
            .cloned()
            .flatten()
            .ok_or_else(|| Diagnostic::new("function cannot return two values"))?;
        self.require_scalar(&first_ty, "function return")?;
        self.require_scalar(&second_ty, "second function return")?;
        self.emit_expr(first, &first_ty)?;
        self.staa(self.return_scratch.address);
        self.emit_expr(second, &second_ty)?;
        self.transfer_a_to_b();
        self.ldaa(self.return_scratch.address);
        let label = self
            .return_labels
            .last()
            .expect("function return label")
            .clone();
        self.line(&format!("    jmp {label}"));
        Ok(())
    }

    fn emit_two_result_call(
        &mut self,
        path: &[String],
        args: &[Expr],
        first: &Binding,
        second: &Binding,
    ) -> Result<(), Diagnostic> {
        if let Some(resolution) = self.resolve_intrinsic_call(path, args)? {
            return self.emit_intrinsic_pair(resolution, args, first, second);
        }
        let name = path.join(".");
        let signature = self
            .model
            .functions
            .get(&name)
            .or_else(|| path.last().and_then(|n| self.model.functions.get(n)))
            .cloned()
            .ok_or_else(|| Diagnostic::new(format!("unknown function `{name}`")))?;
        let first_return_type = signature.return_type.clone().ok_or_else(|| {
            Diagnostic::new(format!("function `{name}` does not return two values"))
        })?;
        let second_return_type = signature.second_return_type.clone().ok_or_else(|| {
            Diagnostic::new(format!("function `{name}` does not return two values"))
        })?;
        self.require_scalar(&first_return_type, "function return")?;
        self.require_scalar(&second_return_type, "second function return")?;
        self.require_scalar(&first.ty, "local")?;
        self.require_scalar(&second.ty, "local")?;
        if signature.params.len() != args.len() {
            return Err(Diagnostic::new(format!(
                "function `{name}` expects {} arguments, got {}",
                signature.params.len(),
                args.len()
            )));
        }
        let base_pending = self.pending_stack_bytes.last().copied().unwrap_or(0);
        for (arg, ty) in args.iter().zip(&signature.params) {
            if is_function_pointer_type(&self.model, ty)? {
                self.push_function_pointer_argument(arg, ty)?;
            } else {
                self.require_scalar(ty, "function argument")?;
                self.emit_expr(arg, ty)?;
                self.push_argument_a();
            }
        }
        self.line(&format!(
            "    jsr _{}",
            sanitize_label(path.last().unwrap_or(&name))
        ));
        self.restore_m6800_frame_pointer();
        self.stab(second.storage.address);
        self.staa(first.storage.address);
        let argument_bytes = signature
            .params
            .iter()
            .map(|ty| self.model.type_size(ty))
            .try_fold(0u32, |total, size| {
                total
                    .checked_add(size?)
                    .ok_or_else(|| Diagnostic::new("M6800/M6809 argument frame is too large"))
            })?;
        self.pop_call_arguments(argument_bytes);
        if let Some(pending) = self.pending_stack_bytes.last_mut() {
            *pending = base_pending;
        }
        self.restore_m6800_frame_pointer();
        Ok(())
    }

    fn emit_assign_op(&mut self, op: AssignOp) -> Result<(), Diagnostic> {
        self.emit_binary(match op {
            AssignOp::Add => BinaryOp::Add, AssignOp::Sub => BinaryOp::Sub,
            AssignOp::BitAnd => BinaryOp::BitAnd, AssignOp::BitOr => BinaryOp::BitOr, AssignOp::BitXor => BinaryOp::BitXor,
            _ => return Err(Diagnostic::new("M6800 source emitter supports only +=, -=, &=, |=, and ^= compound assignments")),
        })
    }

    fn constant_shift_count(&self, expr: &Expr) -> Option<u32> {
        match expr {
            Expr::Int(value) | Expr::TypedInt(value, _) => u32::try_from(*value).ok(),
            Expr::Ident(name) => self
                .model
                .constants
                .get(name)
                .and_then(|value| u32::try_from(*value).ok()),
            Expr::Cast { expr, .. } => self.constant_shift_count(expr),
            _ => None,
        }
    }

    fn emit_constant_shift(
        &mut self,
        op: BinaryOp,
        count: u32,
        ty: &Type,
    ) -> Result<(), Diagnostic> {
        let instruction = match op {
            BinaryOp::Shl => "    asla",
            BinaryOp::Shr if self.type_is_signed(ty)? => "    asra",
            BinaryOp::Shr => "    lsra",
            _ => unreachable!("constant shift called for a non-shift operation"),
        };
        for _ in 0..count.min(8) {
            self.line(instruction);
        }
        Ok(())
    }

    fn emit_binary(&mut self, op: BinaryOp) -> Result<(), Diagnostic> {
        match op {
            BinaryOp::Add => self.memory_instruction("adda", self.r1.address),
            BinaryOp::Sub => self.memory_instruction("suba", self.r1.address),
            BinaryOp::Mul if self.cpu == CpuFamily::M6809 => {
                self.transfer_a_to_b();
                self.ldab(self.r1.address);
                self.line("    mul");
                self.line("    tfr b,a");
            }
            BinaryOp::Mul => self.multiply(),
            BinaryOp::BitAnd => self.memory_instruction("anda", self.r1.address),
            BinaryOp::BitOr => self.memory_instruction("oraa", self.r1.address),
            BinaryOp::BitXor => self.memory_instruction("eora", self.r1.address),
            op if is_comparison(op) => self.compare(op),
            _ => {
                return Err(Diagnostic::new(
                    "M6800 source emitter does not support this binary operation",
                ));
            }
        }
        Ok(())
    }

    fn multiply(&mut self) {
        self.staa(self.multiply_addend.address);
        self.ldaa_imm(0);
        self.staa(self.multiply_result.address);
        self.ldab(self.r1.address);

        let loop_label = self.next_label("multiply_loop");
        let skip_add = self.next_label("multiply_skip_add");
        self.line(&format!("{loop_label}:"));
        self.line("    bitb #01h");
        self.branch_long("beq", &skip_add);
        self.ldaa(self.multiply_result.address);
        self.memory_instruction("adda", self.multiply_addend.address);
        self.staa(self.multiply_result.address);
        self.line(&format!("{skip_add}:"));
        self.ldaa(self.multiply_addend.address);
        self.line("    asla");
        self.staa(self.multiply_addend.address);
        self.line("    lsrb");
        self.branch_long("bne", &loop_label);
        self.ldaa(self.multiply_result.address);
    }

    fn compare(&mut self, op: BinaryOp) {
        let yes = self.next_label("compare_true");
        let done = self.next_label("compare_done");
        self.memory_instruction("cmpa", self.r1.address);
        let branch = match op {
            BinaryOp::Eq => "beq",
            BinaryOp::Ne => "bne",
            BinaryOp::Lt => "blt",
            BinaryOp::Le => "ble",
            BinaryOp::Gt => "bgt",
            BinaryOp::Ge => "bge",
            _ => unreachable!(),
        };
        self.branch_long(branch, &yes);
        self.ldaa_imm(0);
        self.line(&format!("    jmp {done}"));
        self.line(&format!("{yes}:"));
        self.ldaa_imm(1);
        self.line(&format!("{done}:"));
    }

    fn emit_jump_if_false(
        &mut self,
        condition: &Expr,
        false_label: &str,
    ) -> Result<bool, Diagnostic> {
        let Expr::Binary { left, op, right } = condition else {
            return Ok(false);
        };
        if !matches!(op, BinaryOp::Eq | BinaryOp::Ne) {
            return Ok(false);
        }
        let Expr::Binary {
            left: source,
            op: BinaryOp::BitAnd,
            right: mask_expr,
        } = left.as_ref()
        else {
            return Ok(false);
        };
        let (Ok(mask), Ok(expected)) = (
            self.model.const_value(mask_expr),
            self.model.const_value(right),
        ) else {
            return Ok(false);
        };
        let (Ok(mask), Ok(expected)) = (u8::try_from(mask), u8::try_from(expected)) else {
            return Ok(false);
        };
        if !mask.is_power_of_two() || (expected != 0 && expected != mask) {
            return Ok(false);
        }

        let source_ty = self.expr_type(source)?;
        self.require_scalar(&source_ty, "bit-test operand")?;
        self.emit_expr(source, &source_ty)?;
        self.line(&format!("    bita #{mask:02X}h"));
        let branch = if (*op == BinaryOp::Eq) == (expected == 0) {
            "bne"
        } else {
            "beq"
        };
        self.branch_long(branch, false_label);
        Ok(true)
    }

    fn emit_logical(&mut self, left: &Expr, op: BinaryOp, right: &Expr) -> Result<(), Diagnostic> {
        if matches!(
            (left, op),
            (Expr::Bool(false), BinaryOp::And) | (Expr::Bool(true), BinaryOp::Or)
        ) {
            self.ldaa_imm(u8::from(op == BinaryOp::Or));
            return Ok(());
        }
        let decisive = self.next_label("logical_decisive");
        let done = self.next_label("logical_done");
        self.emit_expr(left, &bool_type())?;
        self.line("    tsta");
        self.branch_long(if op == BinaryOp::And { "beq" } else { "bne" }, &decisive);
        self.emit_expr(right, &bool_type())?;
        self.line(&format!("    jmp {done}"));
        self.line(&format!("{decisive}:"));
        self.ldaa_imm(u8::from(op == BinaryOp::Or));
        self.line(&format!("{done}:"));
        Ok(())
    }

    fn bool_not(&mut self) {
        let yes = self.next_label("not_true");
        let done = self.next_label("not_done");
        self.line("    tsta");
        self.branch_long("beq", &yes);
        self.ldaa_imm(0);
        self.line(&format!("    jmp {done}"));
        self.line(&format!("{yes}:"));
        self.ldaa_imm(1);
        self.line(&format!("{done}:"));
    }

    fn expr_type(&self, expr: &Expr) -> Result<Type, Diagnostic> {
        match expr {
            Expr::Bool(_)
            | Expr::Unary {
                op: UnaryOp::Not, ..
            } => Ok(bool_type()),
            Expr::Int(_) | Expr::Char(_) => Ok(u8_type()),
            Expr::TypedInt(_, ty) => Ok(ty.clone()),
            Expr::Ident(name) => self
                .model
                .constant_types
                .get(name)
                .cloned()
                .or_else(|| self.binding(name).ok().map(|b| b.ty))
                .ok_or_else(|| Diagnostic::new(format!("unknown value `{name}`"))),
            Expr::Binary { op, .. }
                if is_comparison(*op) || matches!(op, BinaryOp::And | BinaryOp::Or) =>
            {
                Ok(bool_type())
            }
            Expr::Binary { left, .. } | Expr::Unary { expr: left, .. } => self.expr_type(left),
            Expr::Cast { ty, .. } => Ok(ty.clone()),
            Expr::BankedPointer { pointer, .. } => self.expr_type(pointer),
            Expr::Call { path, args } => {
                if let Some(resolution) = self.resolve_intrinsic_call(path, args)? {
                    return resolution
                        .result_types
                        .into_iter()
                        .next()
                        .ok_or_else(|| Diagnostic::new("intrinsic has no scalar result"));
                }
                if let Some(signature) = self
                    .model
                    .functions
                    .get(&path.join("."))
                    .or_else(|| path.last().and_then(|name| self.model.functions.get(name)))
                {
                    return signature
                        .return_type
                        .clone()
                        .ok_or_else(|| Diagnostic::new("void function has no value"));
                }
                let name = path
                    .last()
                    .ok_or_else(|| Diagnostic::new("empty function call path"))?;
                let pointer_type = self.model.resolved_type(&self.binding(name)?.ty)?;
                let Type::Ptr(inner) = pointer_type else {
                    return Err(Diagnostic::new(
                        "function pointer call requires a function pointer",
                    ));
                };
                let Type::Function { return_type, .. } = *inner else {
                    return Err(Diagnostic::new(
                        "function pointer call requires a function pointer",
                    ));
                };
                return return_type
                    .map(|ty| *ty)
                    .ok_or_else(|| Diagnostic::new("void function has no value"));
            }
            Expr::AddressOf(name) => {
                if let Some(function_type) = self.function_value_type(name) {
                    return Ok(Type::Ptr(Box::new(function_type)));
                }
                let ty = self.model.resolved_type(&self.binding(name)?.ty)?;
                match ty {
                    Type::Array { element, .. } => Ok(Type::Ptr(element)),
                    ty => Ok(Type::Ptr(Box::new(ty))),
                }
            }
            Expr::AddressOfIndex { name, .. } => {
                let ty = self.model.resolved_type(&self.binding(name)?.ty)?;
                let Type::Array { element, .. } = ty else {
                    return Err(Diagnostic::new("address-of index requires an array"));
                };
                Ok(Type::Ptr(element))
            }
            Expr::AddressOfField { base, field } => Ok(Type::Ptr(Box::new(
                self.model.field(&self.binding(base)?.ty, field)?.ty.clone(),
            ))),
            Expr::Access(path) => self.access_type(path),
            Expr::AddressOfAccess(path) => Ok(Type::Ptr(Box::new(self.access_type(path)?))),
            Expr::Deref(pointer) => match self.model.resolved_type(&self.expr_type(pointer)?)? {
                Type::Ptr(element) => Ok(*element),
                _ => Err(Diagnostic::new("dereference requires a pointer")),
            },
            Expr::Index { name, .. } => {
                let ty = self.model.resolved_type(&self.binding(name)?.ty)?;
                let Type::Array { element, .. } = ty else {
                    return Err(Diagnostic::new("indexing requires an array"));
                };
                Ok(*element)
            }
            Expr::Field { base, field } => {
                Ok(self.model.field(&self.binding(base)?.ty, field)?.ty.clone())
            }
            Expr::String(_) => Ok(Type::Ptr(Box::new(u8_type()))),
            Expr::StructInit { ty, .. } => Ok(Type::Named(ty.clone())),
            Expr::Array(_) | Expr::In(_) => Err(Diagnostic::new(
                "M6800 source emitter supports scalar u8/bool expressions only",
            )),
        }
    }

    fn access_type(&self, path: &AccessPath) -> Result<Type, Diagnostic> {
        let mut ty = self.binding(&path.root)?.ty;
        for segment in &path.segments {
            match segment {
                AccessSegment::Field(field) => {
                    ty = self.model.field(&ty, field)?.ty.clone();
                }
                AccessSegment::Index(_) => {
                    let resolved = self.model.resolved_type(&ty)?;
                    let Type::Array { element, .. } = resolved else {
                        return Err(Diagnostic::new("indexing requires an array"));
                    };
                    ty = *element;
                }
            }
        }
        Ok(ty)
    }

    fn inline_asm(
        &mut self,
        volatile: bool,
        inputs: &[crate::ast::AsmInput],
        outputs: &[crate::ast::AsmOutput],
        clobbers: &[String],
        lines: &[String],
    ) -> Result<(), Diagnostic> {
        let mut operands = HashMap::new();
        self.line(if volatile {
            "    ; asm volatile"
        } else {
            "    ; asm"
        });
        for input in inputs {
            if operands.contains_key(&input.name) {
                return Err(Diagnostic::new(format!(
                    "duplicate inline asm operand `{}`",
                    input.name
                )));
            }
            self.validate_inline_asm_operand(&input.name, &input.ty, &input.class, false)?;
            operands.insert(
                input.name.clone(),
                self.inline_asm_operand(&input.name, &input.class, false)?,
            );
        }
        for output in outputs {
            if operands.contains_key(&output.name) {
                return Err(Diagnostic::new(format!(
                    "duplicate inline asm operand `{}`",
                    output.name
                )));
            }
            self.validate_inline_asm_operand(&output.name, &output.ty, &output.class, true)?;
            operands.insert(
                output.name.clone(),
                self.inline_asm_operand(&output.name, &output.class, true)?,
            );
        }
        if (inputs.iter().any(|input| input.class == "mem")
            || outputs.iter().any(|output| output.class == "mem"))
            && !clobbers.iter().any(|clobber| clobber == "memory")
        {
            return Err(Diagnostic::new(
                "inline asm uses memory operands without declaring clobber `memory`",
            ));
        }
        self.validate_inline_asm_clobbers(clobbers)?;
        for input in inputs {
            match input.class.as_str() {
                "reg8" => {
                    if let Ok(binding) = self.binding(&input.name) {
                        self.ldaa(binding.storage.address);
                    } else {
                        self.ldaa_imm(self.model.constants[&input.name] as u8);
                    }
                }
                "reg16" => {
                    if let Ok(binding) = self.binding(&input.name) {
                        self.ldx(binding.storage.address);
                    } else {
                        self.line(&format!(
                            "    ldx #{:04X}h",
                            self.model.constants[&input.name] as u16
                        ));
                    }
                }
                "mem" | "imm" => {}
                _ => unreachable!(),
            }
        }
        for line in lines {
            self.line(&format!(
                "    {}",
                substitute_inline_asm_operands(line, &operands)?
            ));
        }
        self.restore_m6800_frame_pointer();
        for output in outputs {
            match output.class.as_str() {
                "reg8" => {
                    let binding = self.binding(&output.name)?;
                    self.staa(binding.storage.address);
                }
                "reg16" => {
                    let binding = self.binding(&output.name)?;
                    if self.cpu == CpuFamily::M6800
                        && frame_offset(binding.storage.address).is_some()
                    {
                        return Err(Diagnostic::new(
                            "M6800 inline asm reg16 outputs must target fixed memory",
                        ));
                    }
                    self.stx(binding.storage.address);
                }
                "mem" => {}
                _ => unreachable!(),
            }
        }
        Ok(())
    }

    fn validate_inline_asm_operand(
        &self,
        name: &str,
        ty: &Type,
        class: &str,
        output: bool,
    ) -> Result<(), Diagnostic> {
        let bound = self
            .binding(name)
            .ok()
            .map(|binding| binding.ty)
            .or_else(|| self.model.constant_types.get(name).cloned())
            .ok_or_else(|| Diagnostic::new(format!("unknown inline asm operand `{name}`")))?;
        if self.model.resolved_type(ty)? != self.model.resolved_type(&bound)? {
            return Err(Diagnostic::new(format!(
                "inline asm operand `{name}` declared type does not match its bound type"
            )));
        }
        let width = self.model.type_width(ty).ok();
        let valid = match class {
            "reg8" => width == Some(1),
            "reg16" => width == Some(2),
            "mem" => self.binding(name).is_ok(),
            "imm" => !output && self.model.constants.contains_key(name),
            "reg24" => false,
            _ => false,
        };
        if valid {
            Ok(())
        } else if output && class == "imm" {
            Err(Diagnostic::new(format!(
                "inline asm output `{name}` cannot use imm class"
            )))
        } else if class == "reg24" {
            Err(Diagnostic::new(
                "M6800/M6809 inline asm does not support reg24 operands",
            ))
        } else if class == "imm" {
            Err(Diagnostic::new(format!(
                "inline asm immediate `{name}` must be a compile-time constant"
            )))
        } else {
            Err(Diagnostic::new(format!(
                "inline asm operand `{name}` has incompatible class `{class}`"
            )))
        }
    }

    fn inline_asm_operand(
        &self,
        name: &str,
        class: &str,
        _output: bool,
    ) -> Result<String, Diagnostic> {
        match class {
            "reg8" => Ok("a".to_owned()),
            "reg16" => Ok("x".to_owned()),
            "mem" => Ok(self.address_operand(self.binding(name)?.storage.address)),
            "imm" => {
                let width = self.model.type_width(&self.model.constant_types[name])?;
                Ok(format_inline_immediate(self.model.constants[name], width))
            }
            _ => Err(Diagnostic::new(format!(
                "unsupported inline asm operand class `{class}`"
            ))),
        }
    }

    fn validate_inline_asm_clobbers(&self, clobbers: &[String]) -> Result<(), Diagnostic> {
        let mut seen = HashSet::new();
        for clobber in clobbers {
            let canonical = match clobber.as_str() {
                "flags" => "cc",
                "a" | "b" if self.cpu == CpuFamily::M6809 => "d",
                value => value,
            };
            if !seen.insert(canonical) {
                return Err(Diagnostic::new(format!(
                    "duplicate or overlapping inline asm clobber `{clobber}`"
                )));
            }
            if matches!(clobber.as_str(), "s" | "u") {
                return Err(Diagnostic::new(format!(
                    "inline asm cannot clobber ABI-critical stack register `{clobber}`"
                )));
            }
            let valid = if self.cpu == CpuFamily::M6800 {
                matches!(
                    clobber.as_str(),
                    "a" | "b" | "x" | "cc" | "flags" | "memory" | "ports"
                )
            } else {
                matches!(
                    clobber.as_str(),
                    "a" | "b" | "d" | "x" | "y" | "cc" | "flags" | "memory" | "ports"
                )
            };
            if !valid {
                return Err(Diagnostic::new(format!(
                    "unsupported M6800/M6809 inline asm clobber `{clobber}`"
                )));
            }
        }
        Ok(())
    }

    fn require_scalar(&self, ty: &Type, context: &str) -> Result<(), Diagnostic> {
        match self.model.resolved_type(ty)? {
            Type::Named(name) if name == "u8" || name == "i8" || name == "bool" => Ok(()),
            Type::Ptr(inner) if matches!(inner.as_ref(), Type::Function { .. }) => Ok(()),
            _ => Err(Diagnostic::new(format!(
                "M6800 source emitter supports 8-bit integer and bool {context}s only"
            ))),
        }
    }

    fn type_is_signed(&self, ty: &Type) -> Result<bool, Diagnostic> {
        Ok(matches!(
            self.model.resolved_type(ty)?,
            Type::Named(name) if name == "i8"
        ))
    }
    fn bind(&mut self, name: String, storage: Storage, ty: Type) -> Result<(), Diagnostic> {
        if self
            .scopes
            .iter()
            .rev()
            .any(|scope| scope.contains_key(&name))
        {
            return Err(Diagnostic::new(format!(
                "local `{name}` shadows an existing name"
            )));
        }
        self.scopes
            .last_mut()
            .expect("function scope")
            .insert(name, Binding { storage, ty });
        Ok(())
    }
    fn binding(&self, name: &str) -> Result<Binding, Diagnostic> {
        if let Some(binding) = self.scopes.iter().rev().find_map(|scope| scope.get(name)) {
            return Ok(binding.clone());
        }
        if let Some(storage) = self.model.globals.get(name) {
            return Ok(Binding {
                storage: *storage,
                ty: self.model.global_types[name].clone(),
            });
        }
        self.model
            .mmio
            .get(name)
            .map(|(address, ty, _)| Binding {
                storage: Storage {
                    address: *address,
                    size: self.model.type_size(ty).unwrap_or(1),
                },
                ty: ty.clone(),
            })
            .ok_or_else(|| Diagnostic::new(format!("unknown variable `{name}`")))
    }
    fn push_a(&mut self) {
        if self.cpu == CpuFamily::M6809 {
            self.line("    pshs a");
        } else {
            self.line("    psha");
        }
        if let Some(pending) = self.pending_stack_bytes.last_mut() {
            *pending += 1;
        }
    }

    fn pull_a(&mut self) {
        if self.cpu == CpuFamily::M6809 {
            self.line("    puls a");
        } else {
            self.line("    pula");
        }
        if let Some(pending) = self.pending_stack_bytes.last_mut() {
            *pending = pending.saturating_sub(1);
        }
    }

    fn transfer_a_to_b(&mut self) {
        if self.cpu == CpuFamily::M6809 {
            self.line("    tfr a,b");
        } else {
            self.line("    tab");
        }
    }

    fn decrement_x(&mut self) {
        if self.cpu == CpuFamily::M6809 {
            self.line("    leax -1,x");
        } else {
            self.line("    dex");
        }
    }

    fn increment_x(&mut self) {
        if self.cpu == CpuFamily::M6809 {
            self.line("    leax 1,x");
        } else {
            self.line("    inx");
        }
    }

    fn address_operand(&self, address: u32) -> String {
        if let Some(offset) = frame_offset(address) {
            if self.cpu == CpuFamily::M6800 {
                format!("{offset},x")
            } else {
                format!("{offset},u")
            }
        } else {
            format!(">{address:04X}h")
        }
    }

    fn memory_instruction(&mut self, mnemonic: &str, address: u32) {
        if self.cpu == CpuFamily::M6800 && mnemonic != "stx" && frame_offset(address).is_some() {
            self.restore_m6800_frame_pointer();
        }
        let operand = self.address_operand(address);
        self.line(&format!("    {mnemonic} {operand}"));
    }

    fn ldaa(&mut self, address: u32) {
        self.memory_instruction("ldaa", address);
    }
    fn staa(&mut self, address: u32) {
        self.memory_instruction("staa", address);
    }
    fn ldx(&mut self, address: u32) {
        self.memory_instruction("ldx", address);
    }
    fn stx(&mut self, address: u32) {
        self.memory_instruction("stx", address);
    }

    fn stab(&mut self, address: u32) {
        self.memory_instruction("stab", address);
    }
    fn ldaa_imm(&mut self, value: u8) {
        self.line(&format!("    ldaa #{value:02X}h"));
    }
    fn branch_long(&mut self, branch: &str, target: &str) {
        let skip = self.next_local_label("branch_skip");
        let inverse = match branch {
            "beq" => "bne",
            "bne" => "beq",
            "blt" => "bge",
            "bge" => "blt",
            "ble" => "bgt",
            "bgt" => "ble",
            "blo" => "bhs",
            "bhs" => "blo",
            "bcs" => "bcc",
            "bcc" => "bcs",
            "bvs" => "bvc",
            "bvc" => "bvs",
            "bmi" => "bpl",
            "bpl" => "bmi",
            "bhi" => "bls",
            "bls" => "bhi",
            _ => unreachable!(),
        };
        self.line(&format!("    {inverse} {skip}"));
        self.line(&format!("    jmp {target}"));
        self.line(&format!("{skip}:"));
    }
    fn next_label(&mut self, prefix: &str) -> String {
        let label = format!("__ezra_{prefix}_{}", self.labels);
        self.labels += 1;
        label
    }
    fn next_local_label(&mut self, prefix: &str) -> String {
        let label = format!("m6800_{prefix}_{}", self.labels);
        self.labels += 1;
        label
    }
    fn line(&mut self, line: &str) {
        self.out.push_str(line);
        self.out.push('\n');
    }

    fn restore_m6800_frame_pointer(&mut self) {
        if self.cpu != CpuFamily::M6800 {
            return;
        }
        self.line("    tsx");
        let pending = self.pending_stack_bytes.last().copied().unwrap_or(0);
        for _ in 0..pending {
            self.line("    inx");
        }
    }
}

fn substitute_inline_asm_operands(
    line: &str,
    operands: &HashMap<String, String>,
) -> Result<String, Diagnostic> {
    let mut result = String::new();
    let mut rest = line;
    while let Some(open) = rest.find('{') {
        result.push_str(&rest[..open]);
        let after_open = &rest[open + 1..];
        let close = after_open.find('}').ok_or_else(|| {
            Diagnostic::new(format!(
                "unterminated inline asm operand placeholder in `{line}`"
            ))
        })?;
        let name = &after_open[..close];
        let value = operands.get(name).ok_or_else(|| {
            Diagnostic::new(format!(
                "unknown inline asm operand placeholder `{{{name}}}`"
            ))
        })?;
        result.push_str(value);
        rest = &after_open[close + 1..];
    }
    if rest.contains('}') {
        return Err(Diagnostic::new(format!(
            "unmatched inline asm operand placeholder in `{line}`"
        )));
    }
    result.push_str(rest);
    Ok(result)
}

fn format_inline_immediate(value: i64, width: u8) -> String {
    let bits = u32::from(width) * 8;
    let mask = if bits >= 64 {
        u64::MAX
    } else {
        (1u64 << bits) - 1
    };
    format!(
        "{:0width$X}h",
        (value as u64) & mask,
        width = usize::from(width) * 2
    )
}

fn frame_storage(_cpu: CpuFamily, offset: i32, size: u32) -> Storage {
    Storage {
        address: FRAME_ADDRESS_BASE
            + u32::try_from(offset + FRAME_OFFSET_BIAS)
                .expect("frame displacement must fit the encoded frame address"),
        size,
    }
}

fn frame_offset(address: u32) -> Option<i32> {
    let encoded = address.checked_sub(FRAME_ADDRESS_BASE)?;
    if encoded > 0xFFFF {
        return None;
    }
    Some(i32::try_from(encoded).ok()? - FRAME_OFFSET_BIAS)
}

fn storage_offset(storage: Storage, offset: u32) -> Storage {
    if let Some(base) = frame_offset(storage.address) {
        frame_storage(
            CpuFamily::M6809,
            base + i32::try_from(offset).expect("frame storage offset overflow"),
            storage.size.saturating_sub(offset),
        )
    } else {
        Storage {
            address: storage.address.saturating_add(offset),
            size: storage.size.saturating_sub(offset),
        }
    }
}

fn startup_frame(cpu: CpuFamily) -> FunctionFrame {
    let r1 = frame_storage(cpu, 0, 1);
    let (multiply_addend, multiply_result, pointer_scratch, after_multiply) =
        if cpu == CpuFamily::M6800 {
            (
                frame_storage(cpu, 1, 1),
                frame_storage(cpu, 2, 1),
                frame_storage(cpu, 3, 2),
                5u32,
            )
        } else {
            (r1, r1, r1, 1u32)
        };
    let intrinsic_scratch = frame_storage(
        cpu,
        i32::try_from(after_multiply).expect("startup frame too large"),
        FRAME_SCRATCH_BYTES,
    );
    let return_scratch = frame_storage(
        cpu,
        i32::try_from(after_multiply + FRAME_SCRATCH_BYTES).expect("startup frame too large"),
        1,
    );
    let pointer_scratch = if cpu == CpuFamily::M6800 {
        pointer_scratch
    } else {
        frame_storage(
            cpu,
            i32::try_from(after_multiply + FRAME_SCRATCH_BYTES + 1)
                .expect("startup frame too large"),
            2,
        )
    };
    let frame_size =
        after_multiply + FRAME_SCRATCH_BYTES + if cpu == CpuFamily::M6800 { 1 } else { 3 };
    FunctionFrame {
        locals: HashMap::new(),
        frame_size,
        r1,
        multiply_addend,
        multiply_result,
        intrinsic_scratch,
        return_scratch,
        pointer_scratch,
    }
}

#[cfg(test)]
const M6800_BYTE_CLASS: RegClass = RegClass(0);
const M6800_LOCAL_CLASS: RegClass = RegClass(1);
const M6800_STATIC_SPILL_CLASS: SpillClassId = SpillClassId(0);

fn m6800_local_target(cpu: CpuFamily) -> Target {
    let mut registers = vec![
        PhysicalRegister::new("a", vec![RegUnit(0)]),
        PhysicalRegister::new("b", vec![RegUnit(1)]),
    ];
    if cpu == CpuFamily::M6809 {
        registers.push(PhysicalRegister::new("d", vec![RegUnit(0), RegUnit(1)]));
    }
    Target {
        units: vec![RegisterUnit::new("a"), RegisterUnit::new("b")],
        registers,
        register_classes: vec![
            RegisterClass::new("accumulator-byte", vec![PhysReg(0), PhysReg(1)]),
            RegisterClass::new("memory-local", Vec::new()),
        ],
        spill_classes: vec![
            SpillClass::new("static", None, 0).for_register_classes(vec![M6800_LOCAL_CLASS]),
        ],
    }
}

fn plan_function_locals(
    function: &Function,
    model: &mut SemanticModel,
    cpu: CpuFamily,
) -> Result<FunctionFrame, Diagnostic> {
    let mut source_locals = Vec::new();
    let mut local_types = HashMap::new();
    collect_source_locals(
        function.body.as_slice(),
        model,
        &mut source_locals,
        &mut local_types,
    )?;
    let clobbers = (0..m6800_local_target(cpu).registers.len())
        .map(PhysReg)
        .collect::<Vec<_>>();
    let planned = allocate_source_locals(
        &m6800_local_target(cpu),
        &source_locals,
        &function.body,
        &clobbers,
    )
    .map_err(regalloc_diagnostic)?;
    let spill_bytes = planned
        .allocation
        .spill_slots
        .iter()
        .map(|slot| slot.offset.saturating_add(slot.size))
        .max()
        .unwrap_or(0);
    let local_bytes = spill_bytes;
    let scratch_base = local_bytes;
    let r1 = frame_storage(
        cpu,
        i32::try_from(scratch_base)
            .map_err(|_| Diagnostic::new("M6800/M6809 function frame is too large"))?,
        1,
    );
    let (multiply_addend, multiply_result, pointer_scratch, after_multiply) =
        if cpu == CpuFamily::M6800 {
            let addend = frame_storage(
                cpu,
                i32::try_from(scratch_base + 1)
                    .map_err(|_| Diagnostic::new("M6800 function frame is too large"))?,
                1,
            );
            let result = frame_storage(
                cpu,
                i32::try_from(scratch_base + 2)
                    .map_err(|_| Diagnostic::new("M6800 function frame is too large"))?,
                1,
            );
            let pointer = frame_storage(
                cpu,
                i32::try_from(scratch_base + 3)
                    .map_err(|_| Diagnostic::new("M6800 function frame is too large"))?,
                2,
            );
            (addend, result, pointer, scratch_base + 5)
        } else {
            let same = r1;
            (same, same, same, scratch_base + 1)
        };
    let intrinsic_scratch = frame_storage(
        cpu,
        i32::try_from(after_multiply)
            .map_err(|_| Diagnostic::new("M6800/M6809 function frame is too large"))?,
        FRAME_SCRATCH_BYTES,
    );
    let return_scratch = frame_storage(
        cpu,
        i32::try_from(after_multiply + FRAME_SCRATCH_BYTES)
            .map_err(|_| Diagnostic::new("M6800/M6809 function frame is too large"))?,
        1,
    );
    let pointer_scratch = if cpu == CpuFamily::M6800 {
        pointer_scratch
    } else {
        frame_storage(
            cpu,
            i32::try_from(after_multiply + FRAME_SCRATCH_BYTES + 1)
                .map_err(|_| Diagnostic::new("M6809 function frame is too large"))?,
            2,
        )
    };
    let frame_size =
        after_multiply + FRAME_SCRATCH_BYTES + if cpu == CpuFamily::M6800 { 1 } else { 3 };
    if cpu == CpuFamily::M6800 && frame_size > 255 {
        return Err(Diagnostic::new(
            "M6800 function frame exceeds the 8-bit X indexed displacement",
        ));
    }
    if cpu == CpuFamily::M6809 && frame_size > 32767 {
        return Err(Diagnostic::new(
            "M6809 function frame exceeds the indexed displacement range",
        ));
    }
    let mut bindings = HashMap::new();
    for (name, ty) in local_types {
        let vreg = planned
            .locals
            .vreg(&name)
            .ok_or_else(|| Diagnostic::new(format!("missing allocation for local `{name}`")))?;
        let Location::Spill(slot_index) = planned.allocation.location(vreg).ok_or_else(|| {
            Diagnostic::new(format!("source allocator did not place local `{name}`"))
        })?
        else {
            return Err(Diagnostic::new(format!(
                "M6800 local `{name}` was not allocated to static memory"
            )));
        };
        let slot = planned
            .allocation
            .spill_slots
            .get(slot_index)
            .ok_or_else(|| Diagnostic::new(format!("invalid spill slot for local `{name}`")))?;
        debug_assert_eq!(slot.class, M6800_STATIC_SPILL_CLASS);
        bindings.insert(
            name,
            Binding {
                storage: frame_storage(
                    cpu,
                    if cpu == CpuFamily::M6800 {
                        i32::try_from(slot.offset)
                            .map_err(|_| Diagnostic::new("M6800 function frame is too large"))?
                    } else {
                        i32::try_from(slot.offset.saturating_add(slot.size))
                            .ok()
                            .and_then(|offset| offset.checked_neg())
                            .ok_or_else(|| Diagnostic::new("M6809 function frame is too large"))?
                    },
                    slot.size,
                ),
                ty,
            },
        );
    }
    Ok(FunctionFrame {
        locals: bindings,
        frame_size,
        r1,
        multiply_addend,
        multiply_result,
        intrinsic_scratch,
        return_scratch,
        pointer_scratch,
    })
}

fn collect_source_locals(
    body: &[Stmt],
    model: &SemanticModel,
    locals: &mut Vec<SourceLocal>,
    local_types: &mut HashMap<String, Type>,
) -> Result<(), Diagnostic> {
    for stmt in body {
        match stmt {
            Stmt::Let { name, ty, .. } => {
                if local_types.insert(name.clone(), ty.clone()).is_some() {
                    return Err(Diagnostic::new(format!("duplicate local `{name}`")));
                }
                let function_pointer = is_function_pointer_type(model, ty)?;
                locals.push(
                    SourceLocal::new(
                        name.clone(),
                        if function_pointer { 2 } else { 1 },
                        1,
                        M6800_LOCAL_CLASS,
                    )
                    .with_spill_classes(vec![M6800_STATIC_SPILL_CLASS])
                    .with_force_memory(function_pointer),
                );
            }
            Stmt::LetTwo {
                first_name,
                first_ty,
                second_name,
                second_ty,
                ..
            } => {
                for (name, ty) in [(first_name, first_ty), (second_name, second_ty)] {
                    if local_types.insert(name.clone(), ty.clone()).is_some() {
                        return Err(Diagnostic::new(format!("duplicate local `{name}`")));
                    }
                    let function_pointer = is_function_pointer_type(model, ty)?;
                    locals.push(
                        SourceLocal::new(
                            name.clone(),
                            if function_pointer { 2 } else { 1 },
                            1,
                            M6800_LOCAL_CLASS,
                        )
                        .with_spill_classes(vec![M6800_STATIC_SPILL_CLASS])
                        .with_force_memory(function_pointer),
                    );
                }
            }
            Stmt::If {
                then_body,
                else_body,
                ..
            } => {
                collect_source_locals(then_body, model, locals, local_types)?;
                collect_source_locals(else_body, model, locals, local_types)?
            }
            Stmt::While { body, .. } | Stmt::Loop { body } => {
                collect_source_locals(body, model, locals, local_types)?;
            }
            _ => {}
        }
    }
    Ok(())
}

fn regalloc_diagnostic(diagnostics: Vec<crate::regalloc::Diagnostic>) -> Diagnostic {
    Diagnostic::new(
        diagnostics
            .into_iter()
            .map(|diagnostic| diagnostic.message)
            .collect::<Vec<_>>()
            .join("; "),
    )
}

fn block_guarantees_two_result_return(body: &[Stmt]) -> bool {
    body.iter().any(stmt_guarantees_two_result_return)
}

fn stmt_guarantees_two_result_return(stmt: &Stmt) -> bool {
    match stmt {
        Stmt::ReturnTwo { .. } => true,
        Stmt::If {
            then_body,
            else_body,
            ..
        } if !else_body.is_empty() => {
            block_guarantees_two_result_return(then_body)
                && block_guarantees_two_result_return(else_body)
        }
        _ => false,
    }
}

fn contains_function_pointer_program(program: &Program) -> bool {
    program
        .declarations
        .iter()
        .any(|declaration| match declaration {
            Declaration::Global(global) => type_contains_function_pointer(&global.ty),
            Declaration::Function(function) => {
                function
                    .params
                    .iter()
                    .any(|param| type_contains_function_pointer(&param.ty))
                    || function
                        .return_type
                        .as_ref()
                        .is_some_and(type_contains_function_pointer)
                    || function
                        .second_return_type
                        .as_ref()
                        .is_some_and(type_contains_function_pointer)
                    || statements_contain_function_pointer(&function.body)
            }
            _ => false,
        })
}

fn statements_contain_function_pointer(statements: &[Stmt]) -> bool {
    statements.iter().any(|statement| match statement {
        Stmt::Let { ty, .. } => type_contains_function_pointer(ty),
        Stmt::LetTwo {
            first_ty,
            second_ty,
            ..
        } => type_contains_function_pointer(first_ty) || type_contains_function_pointer(second_ty),
        Stmt::If {
            then_body,
            else_body,
            ..
        } => {
            statements_contain_function_pointer(then_body)
                || statements_contain_function_pointer(else_body)
        }
        Stmt::While { body, .. } | Stmt::Loop { body } => statements_contain_function_pointer(body),
        _ => false,
    })
}

fn type_contains_function_pointer(ty: &Type) -> bool {
    match ty {
        Type::Ptr(inner) => matches!(inner.as_ref(), Type::Function { .. }),
        Type::Array { element, .. } => type_contains_function_pointer(element),
        Type::Function {
            params,
            return_type,
        } => {
            params.iter().any(type_contains_function_pointer)
                || return_type
                    .as_ref()
                    .is_some_and(|ty| type_contains_function_pointer(ty))
        }
        Type::Named(_) => false,
    }
}

fn is_function_pointer_type(model: &SemanticModel, ty: &Type) -> Result<bool, Diagnostic> {
    Ok(matches!(
        model.resolved_type(ty)?,
        Type::Ptr(inner) if matches!(inner.as_ref(), Type::Function { .. })
    ))
}

fn function_pointer_label(name: &str) -> String {
    format!("__ezra_fn_ptr_{}", sanitize_label(name))
}

fn m6800_element_type(ty: &Type) -> Result<Type, Diagnostic> {
    match ty {
        Type::Array { element, .. } | Type::Ptr(element) => Ok((**element).clone()),
        _ => Err(Diagnostic::new(
            "M6800 pointer requires an array or pointer value",
        )),
    }
}

fn is_comparison(op: BinaryOp) -> bool {
    matches!(
        op,
        BinaryOp::Lt | BinaryOp::Le | BinaryOp::Gt | BinaryOp::Ge | BinaryOp::Eq | BinaryOp::Ne
    )
}
fn u8_type() -> Type {
    Type::Named("u8".to_owned())
}
fn bool_type() -> Type {
    Type::Named("bool".to_owned())
}
fn sanitize_label(name: &str) -> String {
    name.chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '_' {
                ch
            } else {
                '_'
            }
        })
        .collect()
}

#[cfg(test)]
mod tests;
