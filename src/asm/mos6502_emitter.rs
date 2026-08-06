use crate::{
    asm::{
        AssemblyOptions,
        comments::{stmt_summary, with_readability_comments},
        reachability::{RoutineProfile, strip_unreachable_generated_routines_with_roots},
    },
    ast::{
        AccessPath, AccessSegment, AssignOp, BinaryOp, Declaration, Expr, Function, Place, Program,
        Stmt, Type, UnaryOp,
    },
    compat::prelude::*,
    declaration::unwrapped_declaration,
    diagnostic::Diagnostic,
    hir::HirProgram,
    intrinsics::{
        BitsIntrinsic, CATALOG, IntIntrinsic, IntrinsicDescriptor, IntrinsicOperation, MemIntrinsic,
    },
    regalloc::{
        Location, PhysReg, PhysicalRegister, RegClass, RegUnit, RegisterClass, RegisterUnit,
        SpillClass, SpillClassId, Target,
        source::{SourceLocal, allocate_source_locals},
    },
    target::CpuFamily,
    tbir::{
        TbirProgram,
        cost::{CostCandidate, CostModel, InstructionCost},
        model::{FunctionSignature, SemanticModel, Storage},
    },
};

const POINTER_ZP: u32 = 0xF0;
const U16_MUL_HELPER: &str = "__ezra_u16_mul";
const U16_DIVMOD_HELPER: &str = "__ezra_u16_divmod";

fn reject_banked_declarations(program: &Program) -> Result<(), Diagnostic> {
    let Some(bank) = program.declarations.iter().find_map(declaration_bank) else {
        return Ok(());
    };
    Err(Diagnostic::new(format!(
        "MOS 6502-family targets do not support banked declaration placement in bank {bank}"
    )))
}

fn declaration_bank(declaration: &Declaration) -> Option<u32> {
    match declaration {
        Declaration::Cfg { declaration, .. } => declaration_bank(declaration),
        Declaration::Bank { bank, .. } => Some(*bank),
        _ => None,
    }
}

pub fn emit_mos6502_assembly_with_options(
    program: &Program,
    options: AssemblyOptions,
) -> Result<String, Diagnostic> {
    reject_banked_declarations(program)?;
    let hir = HirProgram::from_ast(program)?;
    let (lowered_program, source_comments) =
        if contains_two_result_program(program) || contains_function_pointer_program(program) {
            (program.clone(), Vec::new())
        } else {
            let tbir = TbirProgram::lower(&hir, program, &options)?;
            (tbir.lowered_program, tbir.source_comments)
        };
    let model = SemanticModel::from_program(
        &lowered_program,
        options.cpu.capabilities().memory.pointer_width_bits,
        options.ram_base.get(),
        options.rodata_base.get(),
        options.asset_base.get(),
    )?;
    Emitter::new(model, options.clone())
        .emit(&lowered_program)
        .map(|asm| {
            let roots = asm
                .lines()
                .filter_map(|line| line.trim().strip_suffix(':'))
                .filter(|label| label.starts_with("__ezra_fn_ptr_"))
                .collect::<Vec<_>>();
            strip_unreachable_generated_routines_with_roots(&asm, RoutineProfile::Mos6502, &roots)
        })
        .and_then(|asm| cleanup_assembly(&asm, options.cpu))
        .map(|asm| with_readability_comments(asm, program, &options, "mos6502", &source_comments))
}

#[derive(Clone)]
struct Binding {
    storage: Storage,
    ty: Type,
}

#[derive(Clone, Copy)]
enum SecondResultDestination {
    Direct(Storage),
    Pointer(Storage),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ConstantMultiplyPlan {
    Zero,
    Identity,
    Shift { count: u32 },
    ShiftAdd { count: u32, subtract: bool },
    Horner { magnitude: u64 },
    Fallback,
}

impl ConstantMultiplyPlan {
    fn name(self) -> &'static str {
        match self {
            Self::Zero => "zero",
            Self::Identity => "identity",
            Self::Shift { .. } => "shift",
            Self::ShiftAdd {
                subtract: false, ..
            } => "shift-add",
            Self::ShiftAdd { subtract: true, .. } => "shift-sub",
            Self::Horner { .. } => "horner",
            Self::Fallback => "runtime-multiply",
        }
    }
}

#[derive(Clone)]
struct LoopLabels {
    continue_label: String,
    break_label: String,
}

struct Emitter {
    model: SemanticModel,
    out: String,
    labels: usize,
    scopes: Vec<HashMap<String, Binding>>,
    planned_locals: Vec<HashMap<String, Binding>>,
    loops: Vec<LoopLabels>,
    return_labels: Vec<String>,
    return_types: Vec<Option<Type>>,
    second_return_types: Vec<Option<Type>>,
    second_return_pointers: Vec<Option<Storage>>,
    function_ram_bases: Vec<u32>,
    current_functions: Vec<String>,
    recursive_call_edges: HashSet<(String, String)>,
    needs_u16_mul_helper: bool,
    needs_u16_divmod_helper: bool,
    needs_indirect_call_helper: bool,
    function_pointer_slots: Vec<(Type, Vec<Storage>)>,
    functions: HashMap<String, Function>,
    r0: Storage,
    r1: Storage,
    r2: Storage,
    c64_executable: bool,
    cpu: CpuFamily,
}

impl Emitter {
    fn new(mut model: SemanticModel, options: AssemblyOptions) -> Self {
        let r0 = model.allocate(4).expect("6502 result scratch allocation");
        let r1 = model.allocate(4).expect("6502 rhs scratch allocation");
        let r2 = model.allocate(4).expect("6502 work scratch allocation");
        Self {
            model,
            out: String::new(),
            labels: 0,
            scopes: Vec::new(),
            planned_locals: Vec::new(),
            loops: Vec::new(),
            return_labels: Vec::new(),
            return_types: Vec::new(),
            second_return_types: Vec::new(),
            second_return_pointers: Vec::new(),
            function_ram_bases: Vec::new(),
            current_functions: Vec::new(),
            recursive_call_edges: HashSet::new(),
            needs_u16_mul_helper: false,
            needs_u16_divmod_helper: false,
            needs_indirect_call_helper: false,
            function_pointer_slots: Vec::new(),
            functions: HashMap::new(),
            r0,
            r1,
            r2,
            c64_executable: options.c64_executable,
            cpu: options.cpu,
        }
    }

    fn emit(mut self, program: &Program) -> Result<String, Diagnostic> {
        self.validate_function_signatures(program)?;
        self.recursive_call_edges = recursive_call_edges(program, &self.model);
        self.functions = program
            .declarations
            .iter()
            .filter_map(|declaration| match unwrapped_declaration(declaration) {
                Declaration::Function(function) => Some((function.name.clone(), function.clone())),
                _ => None,
            })
            .collect();
        let emitted_functions = reachable_function_names(program, &self.model);
        let function_references = function_pointer_references(program, &self.model)
            .into_iter()
            .filter(|name| emitted_functions.contains(name))
            .collect::<HashSet<_>>();
        self.prepare_function_pointer_slots(program, &function_references)?;
        self.emit_function_pointer_constants(&function_references);
        self.line("; generated by ezrac");
        self.line("; target: MOS 6502");
        self.line("section .text");
        self.line("__ezra_start:");
        self.line("    cld");
        self.line("    ldx #$FF");
        self.line("    txs");
        self.emit_static_initializers(program)?;
        self.line("    jsr _main");
        self.line("__ezra_exit:");
        if self.c64_executable {
            self.line("    jmp $A474"); // BASIC warm start.
        } else {
            self.line("    jmp __ezra_exit");
        }

        for declaration in &program.declarations {
            if let Declaration::Function(function) = unwrapped_declaration(declaration)
                && emitted_functions.contains(&function.name)
                && (function.name == "main"
                    || function_references.contains(&function.name)
                    || !mos6502_small_wrapper_candidate(function)
                    || self.function_is_recursive(&function.name))
            {
                self.emit_function(function)?;
            }
        }
        for declaration in &program.declarations {
            if let Declaration::Function(function) = unwrapped_declaration(declaration)
                && function_references.contains(&function.name)
            {
                self.emit_function_pointer_trampoline(function)?;
            }
        }
        self.emit_runtime_helpers();
        for section in [".header", ".rodata", ".data", ".bss", ".assets", ".scratch"] {
            self.line(&format!("section {section}"));
        }
        Ok(self.out)
    }

    fn emit_static_initializers(&mut self, program: &Program) -> Result<(), Diagnostic> {
        let embeds = self.model.embeds.values().cloned().collect::<Vec<_>>();
        for embed in embeds {
            for (offset, byte) in embed.bytes.iter().copied().enumerate() {
                self.lda_imm(byte);
                self.sta(embed.storage.address + offset as u32);
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
                self.lda_imm(byte);
                self.sta(storage.address + offset as u32);
            }
        }
        for declaration in &program.declarations {
            if let Declaration::Global(global) = unwrapped_declaration(declaration) {
                let storage = self.model.globals[&global.name];
                self.emit_initializer(storage, &global.ty, &global.value)?;
            }
        }
        Ok(())
    }

    fn prepare_function_pointer_slots(
        &mut self,
        program: &Program,
        references: &HashSet<String>,
    ) -> Result<(), Diagnostic> {
        for declaration in &program.declarations {
            let Declaration::Function(function) = unwrapped_declaration(declaration) else {
                continue;
            };
            if !references.contains(&function.name)
                || function.second_return_type.is_some()
                || function.attrs.iter().any(|attr| attr == "interrupt")
            {
                continue;
            }
            let signature = self.model.functions[&function.name].clone();
            let ty = Type::Function {
                params: signature.params,
                return_type: signature.return_type.map(Box::new),
            };
            self.function_pointer_argument_slots(&ty)?;
        }
        Ok(())
    }

    fn function_pointer_argument_slots(&mut self, ty: &Type) -> Result<Vec<Storage>, Diagnostic> {
        let ty = self.model.resolved_type(ty)?;
        if let Some((_, slots)) = self
            .function_pointer_slots
            .iter()
            .find(|(known, _)| *known == ty)
        {
            return Ok(slots.clone());
        }
        let Type::Function { params, .. } = &ty else {
            return Err(Diagnostic::new("expected function pointer type"));
        };
        let slots = params
            .iter()
            .map(|param| self.model.allocate_type(param))
            .collect::<Result<Vec<_>, _>>()?;
        self.function_pointer_slots.push((ty, slots.clone()));
        Ok(slots)
    }

    fn emit_function_pointer_constants(&mut self, references: &HashSet<String>) {
        let mut names = references.iter().collect::<Vec<_>>();
        names.sort();
        for name in names {
            let label = function_pointer_label(name);
            self.line(&format!("{label}_lo equ {label} & $FF"));
            self.line(&format!("{label}_hi equ ({label} >> 8) & $FF"));
        }
    }

    fn emit_function_pointer_trampoline(&mut self, function: &Function) -> Result<(), Diagnostic> {
        if function.attrs.iter().any(|attr| attr == "interrupt") {
            return Err(Diagnostic::new(format!(
                "MOS 6502 function pointer cannot reference interrupt function `{}`",
                function.name
            )));
        }
        if function.second_return_type.is_some() {
            return Err(Diagnostic::new(format!(
                "MOS 6502 function pointer cannot reference two-result function `{}`",
                function.name
            )));
        }
        let signature = self.model.functions[&function.name].clone();
        let ty = Type::Function {
            params: signature.params.clone(),
            return_type: signature.return_type.clone().map(Box::new),
        };
        let slots = self.function_pointer_argument_slots(&ty)?;
        self.line(&format!("{}:", function_pointer_label(&function.name)));
        for (source, target) in slots.iter().zip(&signature.argument_slots) {
            self.copy(*source, *target, source.size);
        }
        self.line(&format!("    jsr {}", function_label(&function.name)));
        self.line("    rts");
        Ok(())
    }

    fn validate_function_signatures(&self, program: &Program) -> Result<(), Diagnostic> {
        for declaration in &program.declarations {
            match unwrapped_declaration(declaration) {
                Declaration::Function(function) => {
                    if let Some(first) = &function.return_type {
                        self.model.type_width(first)?;
                    }
                    if let Some(second) = &function.second_return_type {
                        let first = function.return_type.as_ref().ok_or_else(|| {
                            Diagnostic::new(format!(
                                "MOS 6502 two-result function `{}` must have a first return type",
                                function.name
                            ))
                        })?;
                        if function.name == "main" {
                            return Err(Diagnostic::new(
                                "main cannot return two values because its startup caller has no second-result destination",
                            ));
                        }
                        self.model.type_width(first)?;
                        self.model.type_width(second)?;
                    }
                    if (function.return_type.is_some() || function.second_return_type.is_some())
                        && block_can_complete_normally(&function.body, &self.model)
                    {
                        let message = if function.second_return_type.is_some() {
                            format!(
                                "function `{}` may fall through without returning two values",
                                function.name
                            )
                        } else {
                            format!(
                                "function `{}` may fall through without returning a value",
                                function.name
                            )
                        };
                        return Err(Diagnostic::new(message));
                    }
                }
                Declaration::ExternAsmFunction(function)
                    if function.second_return_type.is_some() =>
                {
                    return Err(Diagnostic::new(format!(
                        "MOS 6502 extern asm function `{}` cannot use two-result returns",
                        function.name
                    )));
                }
                _ => {}
            }
        }
        Ok(())
    }

    fn emit_function(&mut self, function: &Function) -> Result<(), Diagnostic> {
        let return_label = self.next_label(&format!("{}_return", function.name));
        let naked = function.attrs.iter().any(|attr| attr == "naked");
        let interrupt = function.attrs.iter().any(|attr| attr == "interrupt");
        let function_ram_base = self.model.next_ram_address();
        let second_return_pointer = function
            .second_return_type
            .as_ref()
            .map(|_| self.model.allocate(u32::from(self.model.pointer_bytes())))
            .transpose()?;
        if interrupt
            && (!function.params.is_empty()
                || function.return_type.is_some()
                || function.second_return_type.is_some())
        {
            return Err(Diagnostic::new(format!(
                "interrupt function `{}` cannot have parameters or a return value",
                function.name
            )));
        }
        if naked
            && function.body.iter().any(|stmt| {
                !matches!(
                    stmt,
                    Stmt::Asm {
                        inputs,
                        outputs,
                        ..
                    } if inputs.is_empty() && outputs.is_empty()
                )
            })
        {
            return Err(Diagnostic::new(format!(
                "naked function `{}` may contain only asm blocks without operands",
                function.name
            )));
        }
        self.line(&format!("{}:", function_label(&function.name)));
        self.scopes.push(HashMap::new());
        self.return_labels.push(return_label.clone());
        self.return_types.push(function.return_type.clone());
        self.second_return_types
            .push(function.second_return_type.clone());
        self.second_return_pointers.push(second_return_pointer);
        self.function_ram_bases.push(function_ram_base);
        self.current_functions.push(function.name.clone());

        if let Some(pointer) = second_return_pointer {
            self.copy_zp_to_storage(pointer, self.model.pointer_bytes());
        }
        if interrupt && !naked {
            self.line("    pha");
            self.line("    txa");
            self.line("    pha");
            self.line("    tya");
            self.line("    pha");
        }
        let signature = self
            .model
            .functions
            .get(&function.name)
            .cloned()
            .ok_or_else(|| Diagnostic::new(format!("unknown function `{}`", function.name)))?;
        for (index, param) in function.params.iter().enumerate() {
            let storage = self.model.allocate_type(&param.ty)?;
            self.bind(param.name.clone(), storage, param.ty.clone())?;
            self.copy(
                signature.argument_slots[index],
                storage,
                self.model.type_size(&param.ty)?,
            );
        }
        let planned_locals = plan_static_locals(function, &mut self.model)?;
        self.planned_locals.push(planned_locals);
        self.emit_block(&function.body)?;
        self.line(&format!("{return_label}:"));
        if interrupt {
            if !naked {
                self.line("    pla");
                self.line("    tay");
                self.line("    pla");
                self.line("    tax");
                self.line("    pla");
            }
            self.line("    rti");
        } else if !naked {
            self.line("    rts");
        }

        self.second_return_pointers.pop();
        self.second_return_types.pop();
        self.return_types.pop();
        self.return_labels.pop();
        self.function_ram_bases.pop();
        self.current_functions.pop();
        self.planned_locals.pop();
        self.scopes.pop();
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
                let binding = self
                    .planned_locals
                    .last()
                    .and_then(|locals| locals.get(name))
                    .cloned()
                    .ok_or_else(|| {
                        Diagnostic::new(format!("missing planned storage for local `{name}`"))
                    })?;
                self.bind(name.clone(), binding.storage, binding.ty)?;
                self.emit_initializer(binding.storage, ty, value)?;
            }
            Stmt::LetTwo {
                first_name,
                first_ty,
                second_name,
                second_ty,
                value,
            } => {
                let first = self
                    .planned_locals
                    .last()
                    .and_then(|locals| locals.get(first_name))
                    .cloned()
                    .ok_or_else(|| {
                        Diagnostic::new(format!("missing planned storage for local `{first_name}`"))
                    })?;
                let second = self
                    .planned_locals
                    .last()
                    .and_then(|locals| locals.get(second_name))
                    .cloned()
                    .ok_or_else(|| {
                        Diagnostic::new(format!(
                            "missing planned storage for local `{second_name}`"
                        ))
                    })?;
                self.emit_two_result_call(
                    value,
                    first_ty,
                    second_ty,
                    SecondResultDestination::Direct(second.storage),
                )?;
                self.copy(self.r0, first.storage, first.storage.size);
                self.bind(first_name.clone(), first.storage, first.ty)?;
                self.bind(second_name.clone(), second.storage, second.ty)?;
            }
            Stmt::Assign { target, op, value } => {
                let ty = self.place_type(target)?;
                let Ok(width) = self.model.type_width(&ty) else {
                    if *op != AssignOp::Set {
                        return Err(Diagnostic::new(
                            "compound assignment requires a scalar value",
                        ));
                    }
                    let size = self.model.type_size(&ty)?;
                    let temporary = self.model.allocate(size)?;
                    self.emit_initializer(temporary, &ty, value)?;
                    self.emit_store_aggregate_place(target, temporary, size)?;
                    return Ok(());
                };
                if *op == AssignOp::Set {
                    if self.emit_65c02_store_zero(target, width, value)? {
                        return Ok(());
                    }
                    if width == 1
                        && let Some(address) = self.direct_place_address(target)?
                        && self.emit_byte_expr_to(value, &ty, address)?
                    {
                        return Ok(());
                    }
                    self.emit_expr(value, &ty)?;
                } else if self.emit_65c02_bit_modify(target, width, *op, value)? {
                    return Ok(());
                } else {
                    self.emit_load_place(target, width)?;
                    if matches!(op, AssignOp::Shl | AssignOp::Shr)
                        && let Ok(count) = self.model.const_value(value)
                        && let Ok(count) = u32::try_from(count)
                    {
                        self.shift_constant(
                            width,
                            *op == AssignOp::Shr,
                            type_is_signed(&ty),
                            count,
                        );
                    } else if *op == AssignOp::Mul
                        && let Ok(factor) = self.model.const_value(value)
                        && self.multiply_constant(width, factor, type_is_signed(&ty), false)
                    {
                    } else {
                        let left = self.model.allocate(u32::from(width))?;
                        self.copy(self.r0, left, u32::from(width));
                        self.emit_expr(value, &ty)?;
                        self.copy(self.r0, self.r1, u32::from(width));
                        self.copy(left, self.r0, u32::from(width));
                        self.emit_binary_op(assign_binary(*op), width, type_is_signed(&ty))?;
                    }
                }
                self.emit_store_place(target, width)?;
            }
            Stmt::If {
                condition,
                then_body,
                else_body,
            } => {
                let else_label = self.next_label("if_else");
                let end_label = self.next_label("if_end");
                if !self.emit_masked_bit_false_branch(condition, &else_label)?
                    && !self.emit_condition_false_branch(condition, &else_label)?
                {
                    self.emit_expr(condition, &Type::Named("bool".to_owned()))?;
                    self.jump_if_zero(self.r0.address, &else_label);
                }
                self.emit_block(then_body)?;
                if !block_terminates(then_body) {
                    self.line(&format!("    jmp {end_label}"));
                }
                self.line(&format!("{else_label}:"));
                self.emit_block(else_body)?;
                self.line(&format!("{end_label}:"));
            }
            Stmt::While { condition, body } => {
                let condition_label = self.next_label("while_condition");
                let break_label = self.next_label("while_end");
                self.loops.push(LoopLabels {
                    continue_label: condition_label.clone(),
                    break_label: break_label.clone(),
                });
                self.line(&format!("{condition_label}:"));
                if !self.emit_masked_bit_false_branch(condition, &break_label)?
                    && !self.emit_condition_false_branch(condition, &break_label)?
                {
                    self.emit_expr(condition, &Type::Named("bool".to_owned()))?;
                    self.jump_if_zero(self.r0.address, &break_label);
                }
                self.emit_block(body)?;
                self.line(&format!("    jmp {condition_label}"));
                self.line(&format!("{break_label}:"));
                self.loops.pop();
            }
            Stmt::Loop { body } => {
                let continue_label = self.next_label("loop_body");
                let break_label = self.next_label("loop_end");
                self.loops.push(LoopLabels {
                    continue_label: continue_label.clone(),
                    break_label: break_label.clone(),
                });
                self.line(&format!("{continue_label}:"));
                self.emit_block(body)?;
                self.line(&format!("    jmp {continue_label}"));
                self.line(&format!("{break_label}:"));
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
                if let Some(second_ty) = self.second_return_types.last().and_then(Clone::clone) {
                    let first_ty = self.return_types.last().and_then(Clone::clone).ok_or_else(
                        || {
                            Diagnostic::new(
                                "function cannot forward two values without a first return type",
                            )
                        },
                    )?;
                    let pointer = self
                        .second_return_pointers
                        .last()
                        .copied()
                        .flatten()
                        .ok_or_else(|| {
                            Diagnostic::new(
                                "two-result function has no caller-provided return slot",
                            )
                        })?;
                    let Some(Expr::Call { .. }) = value.as_ref() else {
                        return Err(Diagnostic::new(
                            "two-result function must use `return first, second` or forward a pair call",
                        ));
                    };
                    self.emit_two_result_call(
                        value.as_ref().expect("pair forwarding value"),
                        &first_ty,
                        &second_ty,
                        SecondResultDestination::Pointer(pointer),
                    )?;
                } else {
                    match (value, self.return_types.last().and_then(Clone::clone)) {
                        (Some(value), Some(ty)) => self.emit_expr(value, &ty)?,
                        (Some(_), None) => {
                            return Err(Diagnostic::new("value return in void function"));
                        }
                        (None, Some(_)) => {
                            return Err(Diagnostic::new(
                                "value-returning function must return a value",
                            ));
                        }
                        (None, None) => {}
                    }
                }
                let label = self
                    .return_labels
                    .last()
                    .expect("function return label")
                    .clone();
                self.line(&format!("    jmp {label}"));
            }
            Stmt::ReturnTwo { first, second } => self.emit_return_two(first, second)?,
            Stmt::Asm {
                inputs,
                outputs,
                lines,
                ..
            } => self.emit_inline_asm(inputs, outputs, lines)?,
            Stmt::Out { port, .. } => {
                return Err(Diagnostic::new(format!(
                    "MOS 6502 does not support separate port I/O `{port}`; use mmio instead"
                )));
            }
            Stmt::Expr(expr) => {
                let ty = self.expr_type(expr).unwrap_or(Type::Named("u8".to_owned()));
                self.emit_expr(expr, &ty)?;
            }
        }
        Ok(())
    }

    fn emit_initializer(
        &mut self,
        storage: Storage,
        ty: &Type,
        value: &Expr,
    ) -> Result<(), Diagnostic> {
        match (self.model.resolved_type(ty)?, value) {
            (Type::Array { .. }, Expr::Ident(name)) => {
                let source = self.binding(name)?;
                self.copy(source.storage, storage, storage.size);
            }
            (Type::Array { element, len }, Expr::Array(values)) => {
                let element_size = self.model.type_size(&element)?;
                let len = u32::try_from(self.model.const_value(&len)?)
                    .map_err(|_| Diagnostic::new("invalid array length"))?;
                for index in 0..len {
                    let target = Storage {
                        address: storage.address + index * element_size,
                        size: element_size,
                    };
                    if let Some(value) = values.get(index as usize) {
                        self.emit_initializer(target, &element, value)?;
                    } else {
                        self.zero(target);
                    }
                }
            }
            (Type::Named(name), Expr::StructInit { fields, .. })
                if self.model.structs.contains_key(&name) =>
            {
                self.zero(storage);
                let layout = self.model.structs[&name].clone();
                for (field_name, value) in fields {
                    let field = layout
                        .fields
                        .get(field_name)
                        .ok_or_else(|| Diagnostic::new(format!("unknown field `{field_name}`")))?;
                    self.emit_initializer(
                        Storage {
                            address: storage.address + field.offset,
                            size: field.size,
                        },
                        &field.ty,
                        value,
                    )?;
                }
            }
            (Type::Named(name), Expr::Ident(source)) if self.model.structs.contains_key(&name) => {
                let source = self.binding(source)?;
                self.copy(source.storage, storage, storage.size);
            }
            (resolved @ Type::Array { .. }, Expr::Deref(pointer)) => {
                self.emit_expr(pointer, &Type::Ptr(Box::new(resolved)))?;
                self.copy_result_to_zp();
                self.copy_indirect_to_storage(storage, storage.size);
            }
            (Type::Named(name), Expr::Deref(pointer)) if self.model.structs.contains_key(&name) => {
                self.emit_expr(pointer, &Type::Ptr(Box::new(Type::Named(name))))?;
                self.copy_result_to_zp();
                self.copy_indirect_to_storage(storage, storage.size);
            }
            (resolved, _) => {
                let width = self.model.type_width(&resolved)?;
                if width == 1 && self.emit_byte_expr_to(value, &resolved, storage.address)? {
                    return Ok(());
                }
                if self.supports_65c02() && self.model.const_value(value).ok() == Some(0) {
                    self.zero(Storage {
                        address: storage.address,
                        size: u32::from(width),
                    });
                } else {
                    self.emit_expr(value, &resolved)?;
                    self.copy(self.r0, storage, u32::from(width));
                }
            }
        }
        Ok(())
    }

    fn can_emit_byte_expr(&self, expr: &Expr, expected: &Type) -> Result<bool, Diagnostic> {
        Ok(match expr {
            Expr::Int(_) | Expr::TypedInt(_, _) | Expr::Bool(_) | Expr::Char(_) => true,
            Expr::Ident(name) => {
                self.model.constants.contains_key(name)
                    || self.model.type_width(&self.binding(name)?.ty)? == 1
            }
            Expr::Deref(pointer) => {
                self.constant_pointer_address(pointer).is_some()
                    || self
                        .absolute_index_parts(pointer)
                        .is_some_and(|(_, index)| {
                            self.model
                                .type_width(
                                    &self
                                        .expr_type(index)
                                        .unwrap_or_else(|_| Type::Named("u16".to_owned())),
                                )
                                .ok()
                                == Some(1)
                        })
            }
            Expr::Binary {
                left,
                op: BinaryOp::Shr,
                right,
            } => {
                !type_is_signed(expected)
                    && self.model.type_width(&self.expr_type(left)?)? == 1
                    && self
                        .model
                        .const_value(right)
                        .ok()
                        .and_then(|value| u32::try_from(value).ok())
                        .is_some()
                    && self.can_emit_byte_expr(left, expected)?
            }
            Expr::Binary { left, op, right } if matches!(op, BinaryOp::Add | BinaryOp::Sub) => {
                self.can_emit_byte_expr(left, expected)?
                    && self.can_emit_byte_expr(right, expected)?
            }
            Expr::Cast { ty, expr } => {
                self.model.type_width(ty)? == 1 && self.can_emit_byte_expr(expr, ty)?
            }
            _ => false,
        })
    }

    fn emit_byte_expr_to(
        &mut self,
        expr: &Expr,
        expected: &Type,
        destination: u32,
    ) -> Result<bool, Diagnostic> {
        if !self.can_emit_byte_expr(expr, expected)? {
            return Ok(false);
        }
        match expr {
            Expr::Int(value) | Expr::TypedInt(value, _) => {
                self.lda_imm(*value as u8);
                self.sta(destination);
            }
            Expr::Bool(value) => {
                self.lda_imm(u8::from(*value));
                self.sta(destination);
            }
            Expr::Char(value) => {
                self.lda_imm(*value as u8);
                self.sta(destination);
            }
            Expr::Ident(name) => {
                if let Some(value) = self.model.constants.get(name).copied() {
                    self.lda_imm(value as u8);
                    self.sta(destination);
                } else {
                    let binding = self.binding(name)?;
                    if self.model.type_width(&binding.ty)? != 1 {
                        return Ok(false);
                    }
                    if binding.storage.address != destination {
                        self.lda(binding.storage.address);
                        self.sta(destination);
                    }
                }
            }
            Expr::Deref(pointer) => {
                if let Some(address) = self.constant_pointer_address(pointer) {
                    self.lda(address);
                    self.sta(destination);
                } else if !self.emit_absolute_indexed_load(pointer, destination)? {
                    return Ok(false);
                }
            }
            Expr::Binary {
                left,
                op: BinaryOp::Shr,
                right,
            } if !type_is_signed(expected) => {
                let Ok(count) = self.model.const_value(right) else {
                    return Ok(false);
                };
                let Ok(count) = u32::try_from(count) else {
                    return Ok(false);
                };
                if count >= 8 {
                    self.lda_imm(0);
                } else {
                    if !self.emit_byte_load_flags(left)? {
                        self.emit_byte_expr_to(left, expected, destination)?;
                        self.lda(destination);
                    }
                    for _ in 0..count {
                        self.line("    lsr a");
                    }
                }
                self.sta(destination);
            }
            Expr::Binary { left, op, right } if matches!(op, BinaryOp::Add | BinaryOp::Sub) => {
                let direct_operand = match right.as_ref() {
                    Expr::Int(value) | Expr::TypedInt(value, _) => {
                        Some(format!("#${:02X}", *value as u8))
                    }
                    Expr::Char(value) => Some(format!("#${:02X}", *value as u8)),
                    Expr::Ident(name) => {
                        if let Some(value) = self.model.constants.get(name) {
                            Some(format!("#${:02X}", *value as u8))
                        } else {
                            let binding = self.binding(name)?;
                            (self.model.type_width(&binding.ty)? == 1)
                                .then(|| format!("${:04X}", binding.storage.address))
                        }
                    }
                    _ => None,
                };
                let operand = if let Some(operand) = direct_operand {
                    operand
                } else {
                    let right_storage = self.model.allocate(1)?;
                    self.emit_byte_expr_to(right, expected, right_storage.address)?;
                    format!("${:04X}", right_storage.address)
                };
                let left_is_destination = matches!(left.as_ref(), Expr::Ident(name)
                    if !self.model.constants.contains_key(name)
                        && self.binding(name)?.storage.address == destination);
                self.emit_byte_expr_to(left, expected, destination)?;
                self.line(if *op == BinaryOp::Add {
                    "    clc"
                } else {
                    "    sec"
                });
                if left_is_destination {
                    self.lda(destination);
                }
                self.line(&format!(
                    "    {} {operand}",
                    if *op == BinaryOp::Add { "adc" } else { "sbc" }
                ));
                self.sta(destination);
            }
            Expr::Cast { ty, expr } if self.model.type_width(ty)? == 1 => {
                return self.emit_byte_expr_to(expr, ty, destination);
            }
            _ => return Ok(false),
        }
        Ok(true)
    }

    fn constant_pointer_address(&self, pointer: &Expr) -> Option<u32> {
        match pointer {
            Expr::BankedPointer { pointer, .. } | Expr::Cast { expr: pointer, .. } => {
                self.constant_pointer_address(pointer)
            }
            Expr::Ident(name) => self
                .model
                .mmio
                .get(name)
                .map(|(address, _, _)| *address)
                .or_else(|| u32::try_from(self.model.constants.get(name).copied()?).ok()),
            _ => u32::try_from(self.model.const_value(pointer).ok()?).ok(),
        }
    }

    fn absolute_index_parts<'a>(&self, pointer: &'a Expr) -> Option<(u32, &'a Expr)> {
        let mut pointer = pointer;
        loop {
            match pointer {
                Expr::BankedPointer { pointer: inner, .. } | Expr::Cast { expr: inner, .. } => {
                    pointer = inner;
                }
                _ => break,
            }
        }
        let Expr::Binary {
            left,
            op: BinaryOp::Add,
            right,
        } = pointer
        else {
            return None;
        };
        Some((self.constant_pointer_address(left)?, right))
    }

    fn emit_absolute_indexed_load(
        &mut self,
        pointer: &Expr,
        destination: u32,
    ) -> Result<bool, Diagnostic> {
        let Some((base, right)) = self.absolute_index_parts(pointer) else {
            return Ok(false);
        };
        if self.model.type_width(&self.expr_type(right)?)? != 1 {
            return Ok(false);
        }
        match right {
            Expr::Ident(name) if !self.model.constants.contains_key(name) => {
                self.line(&format!(
                    "    ldx ${:04X}",
                    self.binding(name)?.storage.address
                ));
            }
            _ => {
                let index = self.model.allocate(1)?;
                self.emit_byte_expr_to(right, &Type::Named("u8".to_owned()), index.address)?;
                self.line(&format!("    ldx ${:04X}", index.address));
            }
        }
        self.line(&format!("    lda ${base:04X},x"));
        self.sta(destination);
        Ok(true)
    }

    fn emit_expr(&mut self, expr: &Expr, expected: &Type) -> Result<(), Diagnostic> {
        let width = self.model.type_width(expected)?;
        match expr {
            Expr::Int(value) | Expr::TypedInt(value, _) => self.load_constant(*value, width),
            Expr::Bool(value) => self.load_constant(i64::from(*value), width),
            Expr::Char(value) => self.load_constant(i64::from(*value), width),
            Expr::String(value) => {
                let storage = self.model.intern_string(value)?;
                self.load_constant(i64::from(storage.address), width);
            }
            Expr::Ident(name) => {
                if let Some(value) = self.model.constants.get(name).copied() {
                    self.load_constant(value, width);
                } else {
                    let binding = self.binding(name)?;
                    let source_width = self.model.type_width(&binding.ty)?;
                    self.copy(binding.storage, self.r0, u32::from(source_width));
                    self.extend_result(source_width, width, type_is_signed(&binding.ty));
                }
            }
            Expr::In(port) => {
                return Err(Diagnostic::new(format!(
                    "MOS 6502 does not support separate port I/O `{port}`; use mmio instead"
                )));
            }
            Expr::AddressOf(name) => {
                if let Some(function_ty) = self.function_value_type(name) {
                    let actual = Type::Ptr(Box::new(function_ty));
                    if self.model.resolved_type(&actual)? != self.model.resolved_type(expected)? {
                        return Err(Diagnostic::new(format!(
                            "function `{name}` reference type does not match its expected pointer type"
                        )));
                    }
                    self.load_function_pointer(name, width);
                } else if self.model.functions.contains_key(name) {
                    return Err(Diagnostic::new(format!(
                        "MOS 6502 function pointer cannot reference two-result or unsupported function `{name}`"
                    )));
                } else {
                    let binding = self.binding(name)?;
                    self.load_constant(i64::from(binding.storage.address), width);
                }
            }
            Expr::AddressOfIndex { name, index } => {
                self.emit_named_index_address(name, index)?;
                self.copy_zp_to_result(width);
            }
            Expr::AddressOfField { base, field } => {
                let binding = self.binding(base)?;
                let field = self.model.field(&binding.ty, field)?;
                self.load_constant(i64::from(binding.storage.address + field.offset), width);
            }
            Expr::AddressOfAccess(path) => {
                let (_, _) = self.emit_access_address(path)?;
                self.copy_zp_to_result(width);
            }
            Expr::Index { name, index } => {
                let element = self.emit_named_index_address(name, index)?;
                let element_width = self.model.type_width(&element)?;
                self.load_indirect(element_width);
                self.extend_result(element_width, width, false);
            }
            Expr::Field { base, field } => {
                let constant_name = format!("{base}.{field}");
                if let Some(value) = self.model.constants.get(&constant_name).copied() {
                    self.load_constant(value, width);
                } else {
                    let binding = self.binding(base)?;
                    let field = self.model.field(&binding.ty, field)?.clone();
                    let source_width = self.model.type_width(&field.ty)?;
                    self.copy(
                        Storage {
                            address: binding.storage.address + field.offset,
                            size: field.size,
                        },
                        self.r0,
                        u32::from(source_width),
                    );
                    self.extend_result(source_width, width, type_is_signed(&field.ty));
                }
            }
            Expr::Access(path) => {
                let (ty, _) = self.emit_access_address(path)?;
                let source_width = self.model.type_width(&ty)?;
                self.load_indirect(source_width);
                self.extend_result(source_width, width, false);
            }
            Expr::Deref(pointer) => {
                if let Some(address) = self.constant_pointer_address(pointer) {
                    for offset in 0..u32::from(width) {
                        self.lda(address + offset);
                        self.sta(self.r0.address + offset);
                    }
                } else if width == 1 && self.emit_absolute_indexed_load(pointer, self.r0.address)? {
                } else {
                    self.emit_expr(pointer, &Type::Ptr(Box::new(expected.clone())))?;
                    self.copy_result_to_zp();
                    self.load_indirect(width);
                }
            }
            Expr::BankedPointer { pointer, .. } => self.emit_expr(pointer, expected)?,
            Expr::Call { path, args } => {
                if intrinsic_descriptor(path).is_some() {
                    self.emit_intrinsic_call(path, args, expected)?;
                } else {
                    self.emit_call(path, args, expected)?;
                }
            }
            Expr::Unary { op, expr } => {
                self.emit_expr(expr, expected)?;
                self.emit_unary(*op, width);
            }
            Expr::Binary { left, op, right } => {
                if matches!(op, BinaryOp::And | BinaryOp::Or) {
                    self.emit_short_circuit(left, *op, right)?;
                    self.extend_result(1, width, false);
                    return Ok(());
                }
                let operand_ty = if is_comparison(*op) || matches!(op, BinaryOp::And | BinaryOp::Or)
                {
                    self.expr_type(left)
                        .or_else(|_| self.expr_type(right))
                        .unwrap_or(expected.clone())
                } else {
                    expected.clone()
                };
                let operand_width = self.model.type_width(&operand_ty)?;
                self.emit_expr(left, &operand_ty)?;
                if matches!(op, BinaryOp::Shl | BinaryOp::Shr)
                    && let Ok(count) = self.model.const_value(right)
                    && let Ok(count) = u32::try_from(count)
                {
                    self.shift_constant(
                        operand_width,
                        *op == BinaryOp::Shr,
                        type_is_signed(&operand_ty),
                        count,
                    );
                    return Ok(());
                }
                if *op == BinaryOp::Mul
                    && let Ok(factor) = self.model.const_value(right)
                    && self.multiply_constant(
                        operand_width,
                        factor,
                        type_is_signed(&operand_ty),
                        true,
                    )
                {
                    return Ok(());
                }
                if matches!(op, BinaryOp::BitAnd | BinaryOp::BitOr | BinaryOp::BitXor)
                    && let Ok(mask) = self.model.const_value(right)
                    && self.emit_immediate_bitwise(*op, operand_width, mask)
                {
                    return Ok(());
                }
                let left_storage = self.model.allocate(u32::from(operand_width))?;
                self.copy(self.r0, left_storage, u32::from(operand_width));
                self.emit_expr(right, &operand_ty)?;
                self.copy(self.r0, self.r1, u32::from(operand_width));
                self.copy(left_storage, self.r0, u32::from(operand_width));
                if matches!(op, BinaryOp::Add | BinaryOp::Sub)
                    && let Type::Ptr(inner) = self.model.resolved_type(&operand_ty)?
                {
                    self.scale_storage(self.r1, operand_width, self.model.type_size(&inner)?);
                }
                self.emit_binary_op(*op, operand_width, type_is_signed(&operand_ty))?;
                if is_comparison(*op) || matches!(op, BinaryOp::And | BinaryOp::Or) {
                    self.extend_result(1, width, false);
                }
            }
            Expr::Cast { ty, expr } => {
                let source_ty = self.expr_type(expr).unwrap_or_else(|_| ty.clone());
                let source_width = self.model.type_width(&source_ty)?;
                self.emit_expr(expr, &source_ty)?;
                self.extend_result(source_width, width, type_is_signed(&source_ty));
            }
            Expr::Array(_) | Expr::StructInit { .. } => {
                return Err(Diagnostic::new("aggregate value requires storage context"));
            }
        }
        Ok(())
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

    fn load_function_pointer(&mut self, name: &str, width: u8) {
        let label = function_pointer_label(name);
        self.lda_imm_symbol(&format!("{label}_lo"));
        self.sta(self.r0.address);
        self.lda_imm_symbol(&format!("{label}_hi"));
        self.sta(self.r0.address + 1);
        for offset in 2..u32::from(width) {
            self.lda_imm(0);
            self.sta(self.r0.address + offset);
        }
    }

    fn addressed_storage(&self, expr: &Expr) -> Option<Storage> {
        match expr {
            Expr::AddressOf(name) | Expr::AddressOfIndex { name, .. } => {
                self.binding(name).ok().map(|binding| binding.storage)
            }
            Expr::AddressOfField { base, .. } => {
                self.binding(base).ok().map(|binding| binding.storage)
            }
            Expr::AddressOfAccess(path) => {
                self.binding(&path.root).ok().map(|binding| binding.storage)
            }
            Expr::BankedPointer { pointer, .. } | Expr::Cast { expr: pointer, .. } => {
                self.addressed_storage(pointer)
            }
            _ => None,
        }
    }

    fn emit_two_result_call(
        &mut self,
        value: &Expr,
        first_ty: &Type,
        second_ty: &Type,
        destination: SecondResultDestination,
    ) -> Result<(), Diagnostic> {
        let Expr::Call { path, args } = value else {
            return Err(Diagnostic::new(
                "two-result bindings require a direct two-result call",
            ));
        };
        if intrinsic_descriptor(path).is_some() {
            match destination {
                SecondResultDestination::Direct(second) => {
                    return self.emit_two_result_intrinsic_call(
                        path, args, self.r0, first_ty, second, second_ty,
                    );
                }
                SecondResultDestination::Pointer(_) => {
                    let first = self.model.allocate(self.model.type_size(first_ty)?)?;
                    let second = self.model.allocate(self.model.type_size(second_ty)?)?;
                    self.emit_two_result_intrinsic_call(
                        path, args, first, first_ty, second, second_ty,
                    )?;
                    self.set_second_result_pointer(destination);
                    self.copy_storage_to_indirect(second, second.size);
                    self.copy(first, self.r0, first.size);
                    return Ok(());
                }
            }
        }

        let name = path.join(".");
        let resolved_name = resolve_called_function(path, &self.model)
            .ok_or_else(|| Diagnostic::new(format!("unknown function `{name}`")))?;
        let signature = self.model.functions[&resolved_name].clone();
        let Some(signature_first) = signature.return_type.as_ref() else {
            return Err(Diagnostic::new(format!(
                "two-result function `{name}` has no first return type"
            )));
        };
        let Some(signature_second) = signature.second_return_type.as_ref() else {
            return Err(Diagnostic::new(format!(
                "function `{name}` does not return two values"
            )));
        };
        if self.model.resolved_type(signature_first)? != self.model.resolved_type(first_ty)? {
            return Err(Diagnostic::new(format!(
                "first result of `{name}` does not match binding type"
            )));
        }
        if self.model.resolved_type(signature_second)? != self.model.resolved_type(second_ty)? {
            return Err(Diagnostic::new(format!(
                "second result of `{name}` does not match binding type"
            )));
        }
        if signature.params.len() != args.len() {
            return Err(Diagnostic::new(format!(
                "function `{name}` expects {} arguments, got {}",
                signature.params.len(),
                args.len()
            )));
        }
        if let SecondResultDestination::Direct(second) = destination {
            let second_size = self.model.type_size(signature_second)?;
            if second.size < second_size {
                return Err(Diagnostic::new(format!(
                    "second result destination for `{name}` is too small"
                )));
            }
        }
        let mut evaluated_args = Vec::with_capacity(args.len());
        for (index, arg) in args.iter().enumerate() {
            let ty = &signature.params[index];
            self.emit_expr(arg, ty)?;
            let storage = self.model.allocate(self.model.type_size(ty)?)?;
            self.copy(self.r0, storage, storage.size);
            evaluated_args.push(storage);
        }
        for (storage, argument_slot) in evaluated_args.into_iter().zip(&signature.argument_slots) {
            self.copy(storage, *argument_slot, storage.size);
        }
        let recursive_edge = self.current_functions.last().is_some_and(|caller| {
            self.recursive_call_edges
                .contains(&(caller.clone(), resolved_name.clone()))
        });
        let additional_exclusion = match destination {
            SecondResultDestination::Direct(storage) => Some(storage),
            SecondResultDestination::Pointer(_) => None,
        };
        let saved = recursive_edge
            .then(|| {
                self.function_ram_bases.last().map(|base| Storage {
                    address: *base,
                    size: self.model.next_ram_address() - *base,
                })
            })
            .flatten()
            .map(|live| self.live_storage_segments(live, args, additional_exclusion))
            .unwrap_or_default();
        for storage in &saved {
            for offset in 0..storage.size {
                self.lda(storage.address + offset);
                self.line("    pha");
            }
        }
        self.set_second_result_pointer(destination);
        self.line(&format!("    jsr {}", function_label(&resolved_name)));
        let returned = self
            .model
            .allocate(self.model.type_size(signature_first)?)?;
        self.copy(self.r0, returned, returned.size);
        for storage in saved.iter().rev() {
            for offset in (0..storage.size).rev() {
                self.line("    pla");
                self.sta(storage.address + offset);
            }
        }
        self.copy(returned, self.r0, returned.size);
        self.extend_result(
            self.model.type_width(signature_first)?,
            self.model.type_width(first_ty)?,
            type_is_signed(signature_first),
        );
        Ok(())
    }

    fn emit_return_two(&mut self, first: &Expr, second: &Expr) -> Result<(), Diagnostic> {
        let first_ty = self
            .return_types
            .last()
            .and_then(Clone::clone)
            .ok_or_else(|| {
                Diagnostic::new("function cannot return two values without a first return type")
            })?;
        let second_ty = self
            .second_return_types
            .last()
            .and_then(Clone::clone)
            .ok_or_else(|| Diagnostic::new("function cannot return two values"))?;
        let pointer = self
            .second_return_pointers
            .last()
            .copied()
            .flatten()
            .ok_or_else(|| {
                Diagnostic::new("two-result function has no caller-provided return slot")
            })?;
        let first_value = self.model.allocate(self.model.type_size(&first_ty)?)?;
        self.emit_expr(first, &first_ty)?;
        self.copy(self.r0, first_value, first_value.size);
        self.emit_expr(second, &second_ty)?;
        self.set_second_result_pointer(SecondResultDestination::Pointer(pointer));
        self.copy_storage_to_indirect(self.r0, self.model.type_size(&second_ty)?);
        self.copy(first_value, self.r0, first_value.size);
        let label = self
            .return_labels
            .last()
            .expect("function return label")
            .clone();
        self.line(&format!("    jmp {label}"));
        Ok(())
    }

    fn emit_call(
        &mut self,
        path: &[String],
        args: &[Expr],
        expected: &Type,
    ) -> Result<(), Diagnostic> {
        let name = path.join(".");
        if intrinsic_descriptor(path).is_some() {
            return self.emit_intrinsic_call(path, args, expected);
        }
        match name.as_str() {
            "mem.peek8" | "ezra.mem.peek8" => {
                self.emit_expr(&args[0], &Type::Ptr(Box::new(Type::Named("u8".to_owned()))))?;
                self.copy_result_to_zp();
                self.load_indirect(1);
                return Ok(());
            }
            "mem.poke8" | "ezra.mem.poke8" => {
                self.emit_expr(&args[0], &Type::Ptr(Box::new(Type::Named("u8".to_owned()))))?;
                let destination = self.model.allocate(2)?;
                self.copy(self.r0, destination, 2);
                self.emit_expr(&args[1], &Type::Named("u8".to_owned()))?;
                self.set_zp_from_storage(POINTER_ZP, destination);
                self.lda(self.r0.address);
                self.line("    ldy #$00");
                self.line(&format!("    sta (${:02X}),y", POINTER_ZP));
                return Ok(());
            }
            "mem.memcpy" | "ezra.mem.memcpy" => {
                self.emit_memcpy(args)?;
                return Ok(());
            }
            "mem.memset" | "ezra.mem.memset" => {
                self.emit_memset(args)?;
                return Ok(());
            }
            _ => {}
        }
        if let Some(resolved_name) = resolve_called_function(path, &self.model)
            && args.is_empty()
            && self.try_emit_small_wrapper(&resolved_name)?
        {
            self.zero(self.r0);
            return Ok(());
        }
        let (resolved_name, signature, indirect_target) = if let Some(resolved_name) =
            resolve_called_function(path, &self.model)
        {
            let signature = self.model.functions[&resolved_name].clone();
            (Some(resolved_name), signature, None)
        } else {
            if path.len() != 1 {
                return Err(Diagnostic::new(format!("unknown function `{name}`")));
            }
            let binding = self.binding(&path[0])?;
            let resolved_binding_type = self.model.resolved_type(&binding.ty)?;
            let Type::Ptr(inner) = resolved_binding_type.clone() else {
                return Err(Diagnostic::new(format!(
                    "function pointer call requires `ptr<fn(...)>`, got `{resolved_binding_type:?}`"
                )));
            };
            let Type::Function {
                params,
                return_type,
            } = *inner
            else {
                return Err(Diagnostic::new(format!(
                    "function pointer call requires `ptr<fn(...)>`, got `{resolved_binding_type:?}`"
                )));
            };
            let function_ty = Type::Function {
                params: params.clone(),
                return_type: return_type.clone(),
            };
            let argument_slots = self.function_pointer_argument_slots(&function_ty)?;
            (
                None,
                FunctionSignature {
                    params,
                    return_type: return_type.map(|ty| *ty),
                    second_return_type: None,
                    argument_slots,
                },
                Some(binding.storage),
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
        let mut evaluated_args = Vec::with_capacity(args.len());
        for (index, arg) in args.iter().enumerate() {
            let ty = &signature.params[index];
            self.emit_expr(arg, ty)?;
            let storage = self.model.allocate(self.model.type_size(ty)?)?;
            self.copy(self.r0, storage, storage.size);
            evaluated_args.push(storage);
        }
        for (storage, argument_slot) in evaluated_args.into_iter().zip(&signature.argument_slots) {
            self.copy(storage, *argument_slot, storage.size);
        }
        let recursive_edge = resolved_name.as_ref().is_some_and(|resolved_name| {
            self.current_functions.last().is_some_and(|caller| {
                self.recursive_call_edges
                    .contains(&(caller.clone(), resolved_name.clone()))
            })
        });
        let saved = if indirect_target.is_some() {
            self.function_ram_bases
                .last()
                .map(|base| Storage {
                    address: *base,
                    size: self.model.next_ram_address() - *base,
                })
                .map(|live| self.live_storage_segments(live, args, None))
                .unwrap_or_default()
        } else {
            recursive_edge
                .then(|| {
                    self.function_ram_bases.last().map(|base| Storage {
                        address: *base,
                        size: self.model.next_ram_address() - *base,
                    })
                })
                .flatten()
                .map(|live| self.live_storage_segments(live, args, None))
                .unwrap_or_default()
        };
        for storage in &saved {
            for offset in 0..storage.size {
                self.lda(storage.address + offset);
                self.line("    pha");
            }
        }
        if let Some(pointer) = indirect_target {
            self.set_zp_from_storage(POINTER_ZP, pointer);
            self.needs_indirect_call_helper = true;
            self.line("    jsr __ezra_indirect_call");
        } else {
            self.line(&format!(
                "    jsr {}",
                function_label(resolved_name.as_ref().expect("direct function name"))
            ));
        }
        let return_storage = signature
            .return_type
            .as_ref()
            .map(|ty| self.model.type_size(ty))
            .transpose()?
            .map(|size| self.model.allocate(size))
            .transpose()?;
        if let Some(return_storage) = return_storage {
            self.copy(self.r0, return_storage, return_storage.size);
        }
        for storage in saved.iter().rev() {
            for offset in (0..storage.size).rev() {
                self.line("    pla");
                self.sta(storage.address + offset);
            }
        }
        if let Some(return_storage) = return_storage {
            self.copy(return_storage, self.r0, return_storage.size);
        }
        if let Some(return_type) = &signature.return_type {
            let return_width = self.model.type_width(return_type)?;
            self.extend_result(return_width, self.model.type_width(expected)?, false);
        } else {
            self.zero(self.r0);
        }
        Ok(())
    }

    fn try_emit_small_wrapper(&mut self, name: &str) -> Result<bool, Diagnostic> {
        let Some(function) = self.functions.get(name).cloned() else {
            return Ok(false);
        };
        let Some(body) = inline_void_body(&function) else {
            return Ok(false);
        };
        if !mos6502_small_wrapper_candidate(&function) || self.function_is_recursive(name) {
            return Ok(false);
        }

        self.emit_block(body)?;
        Ok(true)
    }

    fn function_is_recursive(&self, name: &str) -> bool {
        self.recursive_call_edges
            .iter()
            .any(|(caller, callee)| caller == name || callee == name)
    }

    fn resolve_intrinsic(
        &self,
        path: &[String],
        args: &[Expr],
    ) -> Result<crate::intrinsics::IntrinsicResolution, Diagnostic> {
        let name = path.join(".");
        let argument_types = args
            .iter()
            .map(|arg| self.expr_type(arg))
            .collect::<Result<Vec<_>, _>>()?;
        let constants = args
            .iter()
            .map(|arg| self.model.const_value(arg).ok())
            .collect::<Vec<_>>();
        match CATALOG.validate_types_with_constants(&name, &argument_types, &constants) {
            Ok(resolution) => Ok(resolution),
            Err(error)
                if matches!(
                    name.as_str(),
                    "mem.memcpy" | "ezra.mem.memcpy" | "mem.memset" | "ezra.mem.memset"
                ) && args.len() == 3 =>
            {
                let mut legacy_types = argument_types;
                legacy_types[2] = Type::Named("u24".to_owned());
                CATALOG
                    .validate_types_with_constants(&name, &legacy_types, &constants)
                    .map_err(|_| Diagnostic::new(error.to_string()))
            }
            Err(error) => Err(Diagnostic::new(error.to_string())),
        }
    }

    fn eval_intrinsic_args(
        &mut self,
        args: &[Expr],
        types: &[Type],
    ) -> Result<Vec<Storage>, Diagnostic> {
        let mut values = Vec::with_capacity(args.len());
        for (arg, ty) in args.iter().zip(types) {
            self.emit_expr(arg, ty)?;
            let storage = self.model.allocate(self.model.type_size(ty)?)?;
            self.copy(self.r0, storage, storage.size);
            values.push(storage);
        }
        Ok(values)
    }

    fn check_intrinsic_result(
        &self,
        resolution: &crate::intrinsics::IntrinsicResolution,
        expected: &Type,
    ) -> Result<(), Diagnostic> {
        let result = resolution
            .result_types
            .first()
            .ok_or_else(|| Diagnostic::new("intrinsic has no scalar result"))?;
        if self.model.resolved_type(result)? != self.model.resolved_type(expected)? {
            return Err(Diagnostic::new(format!(
                "intrinsic `{}` returns `{result:?}`, not `{expected:?}`",
                resolution.canonical_name()
            )));
        }
        Ok(())
    }

    fn emit_intrinsic_call(
        &mut self,
        path: &[String],
        args: &[Expr],
        expected: &Type,
    ) -> Result<(), Diagnostic> {
        let resolution = self.resolve_intrinsic(path, args)?;
        if resolution.result_types.len() == 2 {
            return Err(Diagnostic::new(format!(
                "two-result intrinsic `{}` must be consumed by a two-place binding",
                resolution.canonical_name()
            )));
        }
        if resolution.result_types.len() == 1 {
            self.check_intrinsic_result(&resolution, expected)?;
        }
        match resolution.descriptor.operation {
            IntrinsicOperation::Bits(operation) => {
                let values = self.eval_intrinsic_args(args, &resolution.argument_types)?;
                self.emit_bits_intrinsic(operation, &values, args)?;
            }
            IntrinsicOperation::Int(operation) => {
                let values = self.eval_intrinsic_args(args, &resolution.argument_types)?;
                self.emit_int_intrinsic(operation, &values, args, &resolution.result_types)?;
            }
            IntrinsicOperation::Mem(operation) => {
                self.emit_mem_intrinsic(operation, args, &resolution)?;
            }
        }
        if resolution.result_types.is_empty() {
            self.zero(self.r0);
        }
        Ok(())
    }

    fn emit_two_result_intrinsic_call(
        &mut self,
        path: &[String],
        args: &[Expr],
        first: Storage,
        first_ty: &Type,
        second: Storage,
        second_ty: &Type,
    ) -> Result<(), Diagnostic> {
        let resolution = self.resolve_intrinsic(path, args)?;
        if resolution.result_types.len() != 2 {
            return Err(Diagnostic::new(format!(
                "intrinsic `{}` does not produce two results",
                resolution.canonical_name()
            )));
        }
        if self.model.resolved_type(&resolution.result_types[0])?
            != self.model.resolved_type(first_ty)?
            || self.model.resolved_type(&resolution.result_types[1])?
                != self.model.resolved_type(second_ty)?
        {
            return Err(Diagnostic::new(format!(
                "intrinsic `{}` result types do not match the two-place binding",
                resolution.canonical_name()
            )));
        }
        let values = self.eval_intrinsic_args(args, &resolution.argument_types)?;
        match resolution.descriptor.operation {
            IntrinsicOperation::Int(IntIntrinsic::Divmod) => {
                self.emit_divmod_values(&values, &resolution.argument_types)?;
                self.copy(self.r0, first, first.size);
                self.copy(self.r2, second, second.size);
            }
            IntrinsicOperation::Int(IntIntrinsic::AddCarry) => {
                self.emit_carry_values(&values, &resolution.argument_types, false)?;
                self.copy(self.r0, first, first.size);
                self.copy(self.r2, second, second.size);
            }
            IntrinsicOperation::Int(IntIntrinsic::SubBorrow) => {
                self.emit_carry_values(&values, &resolution.argument_types, true)?;
                self.copy(self.r0, first, first.size);
                self.copy(self.r2, second, second.size);
            }
            IntrinsicOperation::Int(IntIntrinsic::FullMul) => {
                self.emit_full_product_values(&values, &resolution.argument_types)?;
                self.copy(self.r0, first, first.size);
                self.copy(self.r1, second, second.size);
            }
            IntrinsicOperation::Mem(MemIntrinsic::FindByte) => {
                self.emit_find_byte_values(&values, &resolution.argument_types)?;
                self.copy(self.r0, first, first.size);
                self.copy(self.r1, second, second.size);
            }
            _ => {
                return Err(Diagnostic::new(format!(
                    "intrinsic `{}` is not a supported two-result operation",
                    resolution.canonical_name()
                )));
            }
        }
        Ok(())
    }

    fn emit_bits_intrinsic(
        &mut self,
        operation: BitsIntrinsic,
        values: &[Storage],
        args: &[Expr],
    ) -> Result<(), Diagnostic> {
        let width = self.model.type_width(&self.expr_type(&args[0])?)?;
        match operation {
            BitsIntrinsic::RotateLeft | BitsIntrinsic::RotateRight => {
                self.copy(values[0], self.r0, u32::from(width));
                self.emit_rotate(
                    width,
                    operation == BitsIntrinsic::RotateRight,
                    values[1],
                    self.model.type_width(&self.expr_type(&args[1])?)?,
                    self.model.const_value(&args[1]).ok(),
                )?;
            }
            BitsIntrinsic::Test => {
                let bit = u32::try_from(self.model.const_value(&args[1]).map_err(|_| {
                    Diagnostic::new("bit test index must be a compile-time constant")
                })?)
                .map_err(|_| Diagnostic::new("bit test index must be non-negative"))?;
                self.emit_test_value(values[0], bit);
            }
            BitsIntrinsic::Set | BitsIntrinsic::Clear | BitsIntrinsic::Toggle => {
                let bit = u32::try_from(self.model.const_value(&args[1]).map_err(|_| {
                    Diagnostic::new("bit update index must be a compile-time constant")
                })?)
                .map_err(|_| Diagnostic::new("bit update index must be non-negative"))?;
                self.copy(values[0], self.r0, u32::from(width));
                self.emit_bit_update(operation, width, bit)?;
            }
            BitsIntrinsic::Extract => {
                let offset = u32::try_from(self.model.const_value(&args[1])?)
                    .map_err(|_| Diagnostic::new("bitfield offset must be non-negative"))?;
                let field_width = u32::try_from(self.model.const_value(&args[2])?)
                    .map_err(|_| Diagnostic::new("bitfield width must be non-negative"))?;
                self.copy(values[0], self.r0, u32::from(width));
                self.shift_constant(width, true, false, offset);
                self.mask_result(width, bit_mask(field_width));
            }
            BitsIntrinsic::Insert => {
                let offset = u32::try_from(self.model.const_value(&args[2])?)
                    .map_err(|_| Diagnostic::new("bitfield offset must be non-negative"))?;
                let field_width = u32::try_from(self.model.const_value(&args[3])?)
                    .map_err(|_| Diagnostic::new("bitfield width must be non-negative"))?;
                self.emit_insert(values[0], values[1], width, offset, field_width);
            }
            BitsIntrinsic::ByteSwap => {
                if self.cpu == CpuFamily::Wdc65C816 && width == 2 {
                    self.line("    rep #$20");
                    self.lda(self.r0.address);
                    self.line("    xba");
                    self.sta(self.r0.address);
                    self.line("    sep #$20");
                } else {
                    self.copy(values[0], self.r2, u32::from(width));
                    for offset in 0..u32::from(width) {
                        self.lda(self.r2.address + u32::from(width) - 1 - offset);
                        self.sta(self.r0.address + offset);
                    }
                }
            }
            BitsIntrinsic::Reverse => self.emit_reverse(values[0], width)?,
            BitsIntrinsic::CountOnes => self.emit_bit_count(values[0], width, false, true),
            BitsIntrinsic::LeadingZeros => self.emit_bit_count(values[0], width, true, false),
            BitsIntrinsic::TrailingZeros => self.emit_bit_count(values[0], width, false, false),
        }
        Ok(())
    }

    fn emit_int_intrinsic(
        &mut self,
        operation: IntIntrinsic,
        values: &[Storage],
        args: &[Expr],
        result_types: &[Type],
    ) -> Result<(), Diagnostic> {
        let width = self.model.type_width(&self.expr_type(&args[0])?)?;
        let signed = type_is_signed(&self.expr_type(&args[0])?);
        match operation {
            IntIntrinsic::WideningMul => {
                let result_width = self.model.type_width(&result_types[0])?;
                let right_width = self.model.type_width(&self.expr_type(&args[1])?)?;
                let product = self.emit_full_product(
                    values[0],
                    values[1],
                    width,
                    right_width,
                    signed,
                    result_width,
                );
                self.copy(product, self.r0, u32::from(result_width));
            }
            IntIntrinsic::MulHigh => {
                let product =
                    self.emit_full_product(values[0], values[1], width, width, signed, width * 2);
                self.copy(
                    Storage {
                        address: product.address + u32::from(width),
                        size: u32::from(width),
                    },
                    self.r0,
                    u32::from(width),
                );
            }
            IntIntrinsic::SaturatingAdd | IntIntrinsic::SaturatingSub => {
                self.emit_saturating(
                    operation == IntIntrinsic::SaturatingSub,
                    values[0],
                    values[1],
                    width,
                    signed,
                );
            }
            IntIntrinsic::Divmod
            | IntIntrinsic::AddCarry
            | IntIntrinsic::SubBorrow
            | IntIntrinsic::FullMul => {
                return Err(Diagnostic::new(
                    "two-result intrinsic requires a two-place binding",
                ));
            }
        }
        Ok(())
    }

    fn emit_full_product(
        &mut self,
        left: Storage,
        right: Storage,
        left_width: u8,
        right_width: u8,
        signed: bool,
        result_width: u8,
    ) -> Storage {
        let multiplicand = self
            .model
            .allocate(u32::from(result_width))
            .expect("multiplicand scratch");
        let multiplier = self
            .model
            .allocate(u32::from(right_width))
            .expect("multiplier scratch");
        let result = self
            .model
            .allocate(u32::from(result_width))
            .expect("product scratch");
        let negative = self.model.allocate(1).expect("product sign scratch");
        self.zero(negative);
        if signed {
            self.normalize_signed_operand(left, left_width, negative, false);
            self.normalize_signed_operand(right, right_width, negative, true);
        }
        self.zero(multiplicand);
        self.copy(left, multiplicand, u32::from(left_width));
        self.copy(right, multiplier, u32::from(right_width));
        self.zero(result);
        for bit in 0..u32::from(right_width) * 8 {
            let skip = self.next_label("product_skip_add");
            self.emit_bit_test_branch(multiplier, bit, &skip, true);
            self.add_storages(result, multiplicand, result, result_width);
            self.line(&format!("{skip}:"));
            self.shift_storage_once(multiplicand, result_width, false);
            self.shift_storage_once(multiplier, right_width, true);
        }
        if signed {
            self.negate_if_flag(result, result_width, negative);
        }
        result
    }

    fn emit_full_product_values(
        &mut self,
        values: &[Storage],
        args: &[Type],
    ) -> Result<(), Diagnostic> {
        let width = self.model.type_width(&args[0])?;
        let product = self.emit_full_product(
            values[0],
            values[1],
            width,
            width,
            type_is_signed(&args[0]),
            width * 2,
        );
        self.copy(product, self.r0, u32::from(width));
        self.copy(
            Storage {
                address: product.address + u32::from(width),
                size: u32::from(width),
            },
            self.r1,
            u32::from(width),
        );
        Ok(())
    }

    fn emit_divmod_values(&mut self, values: &[Storage], args: &[Type]) -> Result<(), Diagnostic> {
        let width = self.model.type_width(&args[0])?;
        self.copy(values[0], self.r0, u32::from(width));
        self.copy(values[1], self.r1, u32::from(width));
        self.divide(width, false, type_is_signed(&args[0]));
        Ok(())
    }

    fn emit_carry_values(
        &mut self,
        values: &[Storage],
        args: &[Type],
        borrow: bool,
    ) -> Result<(), Diagnostic> {
        let width = self.model.type_width(&args[0])?;
        self.copy(values[0], self.r0, u32::from(width));
        self.copy(values[1], self.r1, u32::from(width));
        let no_carry = self.next_label("carry_clear");
        let operation = self.next_label("carry_operation");
        let done = self.next_label("carry_done");
        self.jump_if_zero(values[2].address, &no_carry);
        self.line(if borrow { "    clc" } else { "    sec" });
        self.line(&format!("    jmp {operation}"));
        self.line(&format!("{no_carry}:"));
        self.line(if borrow { "    sec" } else { "    clc" });
        self.line(&format!("{operation}:"));
        if borrow {
            self.sub_without_initial_carry(width);
        } else {
            self.add_without_initial_carry(width);
        }
        let saved = self.model.allocate(u32::from(width))?;
        self.copy(self.r0, saved, u32::from(width));
        let flag = self.model.allocate(1)?;
        let set = self.next_label("carry_set");
        self.branch_long(if borrow { "bcc" } else { "bcs" }, &set);
        self.zero(flag);
        self.line(&format!("    jmp {done}"));
        self.line(&format!("{set}:"));
        self.load_constant(1, 1);
        self.copy(self.r0, flag, 1);
        self.line(&format!("{done}:"));
        self.copy(saved, self.r0, u32::from(width));
        self.copy(flag, self.r2, 1);
        Ok(())
    }

    fn add_without_initial_carry(&mut self, width: u8) {
        for offset in 0..u32::from(width) {
            self.lda(self.r0.address + offset);
            self.line(&format!("    adc ${:04X}", self.r1.address + offset));
            self.sta(self.r0.address + offset);
        }
    }

    fn sub_without_initial_carry(&mut self, width: u8) {
        for offset in 0..u32::from(width) {
            self.lda(self.r0.address + offset);
            self.line(&format!("    sbc ${:04X}", self.r1.address + offset));
            self.sta(self.r0.address + offset);
        }
    }

    fn emit_rotate(
        &mut self,
        width: u8,
        right: bool,
        count: Storage,
        count_width: u8,
        constant: Option<i64>,
    ) -> Result<(), Diagnostic> {
        let bits = u32::from(width) * 8;
        if let Some(count) = constant {
            let count = u32::try_from(count)
                .map_err(|_| Diagnostic::new("rotate count must be non-negative"))?
                % bits;
            for _ in 0..count {
                self.rotate_once(width, right);
            }
            return Ok(());
        }
        let count_value = self.model.allocate(u32::from(count_width))?;
        self.copy(count, count_value, u32::from(count_width));
        let modulus = self.model.allocate(u32::from(count_width))?;
        self.zero(modulus);
        self.load_constant(i64::from(bits), count_width);
        self.copy(self.r0, modulus, u32::from(count_width));
        let modulo = self.next_label("rotate_modulo");
        let modulo_done = self.next_label("rotate_modulo_done");
        self.line(&format!("{modulo}:"));
        self.jump_if_less(count_value, modulus, count_width, &modulo_done);
        self.sub_storages(count_value, modulus, count_value, count_width);
        self.line(&format!("    jmp {modulo}"));
        self.line(&format!("{modulo_done}:"));
        let loop_label = self.next_label("rotate_loop");
        let done = self.next_label("rotate_done");
        self.line(&format!("{loop_label}:"));
        self.jump_storage_zero(count_value, count_width, &done);
        self.rotate_once(width, right);
        self.decrement(count_value, count_width);
        self.line(&format!("    jmp {loop_label}"));
        self.line(&format!("{done}:"));
        Ok(())
    }

    fn rotate_once(&mut self, width: u8, right: bool) {
        self.line("    clc");
        if right {
            for offset in (0..u32::from(width)).rev() {
                self.line(&format!("    ror ${:04X}", self.r0.address + offset));
            }
            let no_carry = self.next_label("rotate_no_carry");
            self.branch_long("bcc", &no_carry);
            self.lda(self.r0.address + u32::from(width) - 1);
            self.line("    ora #$80");
            self.sta(self.r0.address + u32::from(width) - 1);
            self.line(&format!("{no_carry}:"));
        } else {
            for offset in 0..u32::from(width) {
                self.line(&format!("    rol ${:04X}", self.r0.address + offset));
            }
            let no_carry = self.next_label("rotate_no_carry");
            self.branch_long("bcc", &no_carry);
            self.lda(self.r0.address);
            self.line("    ora #$01");
            self.sta(self.r0.address);
            self.line(&format!("{no_carry}:"));
        }
    }

    fn emit_test_value(&mut self, source: Storage, bit: u32) {
        let set = self.next_label("bit_test_set");
        let done = self.next_label("bit_test_done");
        self.zero(self.r0);
        self.emit_bit_test_branch(source, bit, &set, true);
        self.line(&format!("    jmp {done}"));
        self.line(&format!("{set}:"));
        self.load_constant(1, 1);
        self.line(&format!("{done}:"));
    }

    fn emit_bit_test_branch(&mut self, source: Storage, bit: u32, target: &str, set: bool) {
        self.lda_imm(1_u8 << (bit % 8));
        self.line(&format!("    bit ${:04X}", source.address + bit / 8));
        self.branch_long(if set { "bne" } else { "beq" }, target);
    }

    fn emit_bit_update(
        &mut self,
        operation: BitsIntrinsic,
        width: u8,
        bit: u32,
    ) -> Result<(), Diagnostic> {
        if bit >= u32::from(width) * 8 {
            return Err(Diagnostic::new("bit index is outside the value width"));
        }
        let address = self.r0.address + bit / 8;
        let mask = 1_u8 << (bit % 8);
        self.lda(address);
        match operation {
            BitsIntrinsic::Set => self.line(&format!("    ora #${mask:02X}")),
            BitsIntrinsic::Clear => self.line(&format!("    and #${:02X}", !mask)),
            BitsIntrinsic::Toggle => self.line(&format!("    eor #${mask:02X}")),
            _ => unreachable!(),
        }
        self.sta(address);
        Ok(())
    }

    fn mask_result(&mut self, width: u8, mask: u64) {
        let source = self
            .model
            .allocate(u32::from(width))
            .expect("bit mask source scratch");
        let mask_storage = self
            .model
            .allocate(u32::from(width))
            .expect("bit mask scratch");
        self.copy(self.r0, source, u32::from(width));
        self.load_constant(mask as i64, width);
        self.copy(self.r0, mask_storage, u32::from(width));
        self.copy(source, self.r0, u32::from(width));
        self.copy(mask_storage, self.r1, u32::from(width));
        self.emit_binary_op(BinaryOp::BitAnd, width, false)
            .expect("bit mask operation");
    }

    fn emit_insert(
        &mut self,
        base: Storage,
        value: Storage,
        width: u8,
        offset: u32,
        field_width: u32,
    ) {
        let field_mask = bit_mask(field_width);
        let shifted_mask = field_mask << offset;
        let inverse = self
            .model
            .allocate(u32::from(width))
            .expect("insert mask scratch");
        let cleared = self
            .model
            .allocate(u32::from(width))
            .expect("insert base scratch");
        let inserted = self
            .model
            .allocate(u32::from(width))
            .expect("insert value scratch");
        self.load_constant((!shifted_mask) as i64, width);
        self.copy(self.r0, inverse, u32::from(width));
        self.copy(base, self.r0, u32::from(width));
        self.copy(inverse, self.r1, u32::from(width));
        self.emit_binary_op(BinaryOp::BitAnd, width, false)
            .expect("insert clear operation");
        self.copy(self.r0, cleared, u32::from(width));
        self.load_constant(field_mask as i64, width);
        self.copy(self.r0, self.r1, u32::from(width));
        self.copy(value, self.r0, u32::from(width));
        self.emit_binary_op(BinaryOp::BitAnd, width, false)
            .expect("insert value mask operation");
        self.shift_constant(width, false, false, offset);
        self.copy(self.r0, inserted, u32::from(width));
        self.copy(cleared, self.r0, u32::from(width));
        self.copy(inserted, self.r1, u32::from(width));
        self.emit_binary_op(BinaryOp::BitOr, width, false)
            .expect("insert combine operation");
    }

    fn emit_reverse(&mut self, value: Storage, width: u8) -> Result<(), Diagnostic> {
        let source = self.model.allocate(u32::from(width))?;
        let result = self.model.allocate(u32::from(width))?;
        let bit = self.model.allocate(u32::from(width))?;
        self.copy(value, source, u32::from(width));
        self.zero(result);
        for _ in 0..u32::from(width) * 8 {
            self.copy(source, bit, u32::from(width));
            self.copy(bit, self.r0, u32::from(width));
            self.load_constant(1, width);
            self.copy(self.r0, self.r1, u32::from(width));
            self.copy(bit, self.r0, u32::from(width));
            self.emit_binary_op(BinaryOp::BitAnd, width, false)?;
            let source_bit = self.model.allocate(u32::from(width))?;
            self.copy(self.r0, source_bit, u32::from(width));
            self.copy(result, self.r0, u32::from(width));
            self.shift_constant(width, false, false, 1);
            self.copy(source_bit, self.r1, u32::from(width));
            self.emit_binary_op(BinaryOp::BitOr, width, false)?;
            self.copy(self.r0, result, u32::from(width));
            self.copy(source, self.r0, u32::from(width));
            self.shift_constant(width, true, false, 1);
            self.copy(self.r0, source, u32::from(width));
        }
        self.copy(result, self.r0, u32::from(width));
        Ok(())
    }

    fn emit_bit_count(&mut self, source: Storage, width: u8, leading: bool, ones: bool) {
        let count = self.model.allocate(1).expect("bit count scratch");
        self.zero(count);
        let total = u32::from(width) * 8;
        let order = if leading {
            (0..total).rev().collect::<Vec<_>>()
        } else {
            (0..total).collect::<Vec<_>>()
        };
        for bit in order {
            if ones {
                let add = self.next_label("bit_count_add");
                let next = self.next_label("bit_count_next");
                self.emit_bit_test_branch(source, bit, &add, true);
                self.line(&format!("    jmp {next}"));
                self.line(&format!("{add}:"));
                self.increment(count, 1);
                self.line(&format!("{next}:"));
            } else {
                let stop = self.next_label("bit_count_stop");
                self.emit_bit_test_branch(source, bit, &stop, true);
                self.increment(count, 1);
                self.line(&format!("{stop}:"));
            }
        }
        self.zero(self.r0);
        self.copy(count, self.r0, 1);
    }

    fn emit_saturating(
        &mut self,
        subtract: bool,
        left: Storage,
        right: Storage,
        width: u8,
        signed: bool,
    ) {
        if !signed {
            if subtract {
                self.sub_storages(left, right, self.r0, width);
                let no_borrow = self.next_label("saturating_no_borrow");
                let done = self.next_label("saturating_done");
                self.branch_long("bcs", &no_borrow);
                self.zero(self.r0);
                self.line(&format!("    jmp {done}"));
                self.line(&format!("{no_borrow}:"));
                self.line(&format!("{done}:"));
            } else {
                self.add_storages(left, right, self.r0, width);
                let no_carry = self.next_label("saturating_no_carry");
                let done = self.next_label("saturating_done");
                self.branch_long("bcc", &no_carry);
                self.load_constant(bit_mask(u32::from(width) * 8) as i64, width);
                self.line(&format!("    jmp {done}"));
                self.line(&format!("{no_carry}:"));
                self.line(&format!("{done}:"));
            }
            return;
        }
        if subtract {
            self.sub_storages(left, right, self.r0, width);
        } else {
            self.add_storages(left, right, self.r0, width);
        }
        let left_negative = self.next_label("saturating_left_negative");
        let right_negative = self.next_label("saturating_right_negative");
        let overflow_max = self.next_label("saturating_max");
        let overflow_min = self.next_label("saturating_min");
        let done = self.next_label("saturating_done");
        self.jump_if_negative(left, &left_negative);
        if subtract {
            self.jump_if_negative(right, &right_negative);
            self.line(&format!("    jmp {done}"));
            self.line(&format!("{right_negative}:"));
            self.jump_if_negative(self.r0, &done);
            self.line(&format!("    jmp {overflow_max}"));
        } else {
            self.jump_if_negative(right, &right_negative);
            self.jump_if_negative(self.r0, &overflow_max);
            self.line(&format!("    jmp {done}"));
            self.line(&format!("{right_negative}:"));
            self.line(&format!("    jmp {done}"));
        }
        self.line(&format!("{left_negative}:"));
        if subtract {
            let left_neg_right_neg = self.next_label("saturating_sub_no_overflow");
            self.jump_if_negative(right, &left_neg_right_neg);
            self.jump_if_negative(self.r0, &done);
            self.line(&format!("    jmp {overflow_min}"));
            self.line(&format!("{left_neg_right_neg}:"));
            self.line(&format!("    jmp {done}"));
        } else {
            self.jump_if_negative(right, &right_negative);
            self.line(&format!("    jmp {done}"));
            self.line(&format!("{right_negative}:"));
            self.jump_if_negative(self.r0, &done);
            self.line(&format!("    jmp {overflow_min}"));
        }
        self.line(&format!("{overflow_max}:"));
        self.load_constant((1_i64 << (u32::from(width) * 8 - 1)) - 1, width);
        self.line(&format!("    jmp {done}"));
        self.line(&format!("{overflow_min}:"));
        self.load_constant(1_i64 << (u32::from(width) * 8 - 1), width);
        self.line(&format!("{done}:"));
    }

    fn jump_if_negative(&mut self, storage: Storage, label: &str) {
        self.lda(storage.address + storage.size - 1);
        self.branch_long("bmi", label);
    }

    fn shift_storage_once(&mut self, storage: Storage, width: u8, right: bool) {
        self.line("    clc");
        if right {
            for offset in (0..u32::from(width)).rev() {
                self.line(&format!("    ror ${:04X}", storage.address + offset));
            }
        } else {
            for offset in 0..u32::from(width) {
                self.line(&format!("    rol ${:04X}", storage.address + offset));
            }
        }
    }

    fn emit_mem_intrinsic(
        &mut self,
        operation: MemIntrinsic,
        args: &[Expr],
        resolution: &crate::intrinsics::IntrinsicResolution,
    ) -> Result<(), Diagnostic> {
        if matches!(
            resolution.descriptor.effects.volatile,
            crate::intrinsics::VolatilePolicy::NonVolatileOnly
        ) && args.iter().any(|arg| self.is_volatile_pointer(arg))
        {
            return Err(Diagnostic::new(format!(
                "intrinsic `{}` cannot access volatile memory",
                resolution.canonical_name()
            )));
        }
        match operation {
            MemIntrinsic::CopyNonoverlapping | MemIntrinsic::Move => self.emit_memory_transfer(
                args,
                &resolution.argument_types,
                operation == MemIntrinsic::Move,
            )?,
            MemIntrinsic::Fill => self.emit_memory_fill(args, &resolution.argument_types)?,
            MemIntrinsic::FindByte => {
                return Err(Diagnostic::new(
                    "mem.find_byte requires a two-place binding",
                ));
            }
            MemIntrinsic::Compare => self.emit_memory_compare(args, &resolution.argument_types)?,
            MemIntrinsic::LoadLe16
            | MemIntrinsic::LoadLe24
            | MemIntrinsic::LoadBe16
            | MemIntrinsic::LoadBe24 => {
                self.emit_endian_load(operation, args, &resolution.argument_types)?
            }
            MemIntrinsic::StoreLe16
            | MemIntrinsic::StoreLe24
            | MemIntrinsic::StoreBe16
            | MemIntrinsic::StoreBe24 => {
                self.emit_endian_store(operation, args, &resolution.argument_types)?
            }
            MemIntrinsic::Peek8 => self.emit_peek8(args, &resolution.argument_types)?,
            MemIntrinsic::Poke8 => self.emit_poke8(args, &resolution.argument_types)?,
        }
        Ok(())
    }

    fn is_volatile_pointer(&self, expr: &Expr) -> bool {
        match expr {
            Expr::Ident(name) => self
                .model
                .mmio
                .get(name)
                .is_some_and(|(_, _, volatile)| *volatile),
            Expr::Access(path) | Expr::AddressOfAccess(path) => self
                .model
                .mmio
                .get(&path.root)
                .is_some_and(|(_, _, volatile)| *volatile),
            Expr::Cast { expr, .. } | Expr::BankedPointer { pointer: expr, .. } => {
                self.is_volatile_pointer(expr)
            }
            Expr::Binary { left, right, .. } => {
                self.is_volatile_pointer(left) || self.is_volatile_pointer(right)
            }
            _ => false,
        }
    }

    fn zero_extend_memory_length(
        &mut self,
        value: Storage,
        ty: &Type,
    ) -> Result<Storage, Diagnostic> {
        let width = self.model.type_width(ty)?;
        let count = self.model.allocate(3)?;
        self.zero(count);
        self.copy(value, count, u32::from(width.min(3)));
        Ok(count)
    }

    fn emit_memory_transfer(
        &mut self,
        args: &[Expr],
        types: &[Type],
        moving: bool,
    ) -> Result<(), Diagnostic> {
        let values = self.eval_intrinsic_args(args, types)?;
        let pointer_width = self.model.pointer_bytes();
        if self
            .model
            .const_value(&args[2])
            .ok()
            .is_some_and(|length| length == 0)
        {
            return Ok(());
        }
        if self.cpu == CpuFamily::Wdc65C816 && self.emit_65c816_block_transfer(args, moving)? {
            return Ok(());
        }
        let source = values[1];
        let destination = values[0];
        let length = self.zero_extend_memory_length(values[2], &types[2])?;
        let forward = self.next_label("mem_forward");
        let backward = self.next_label("mem_backward");
        let done = self.next_label("mem_done");
        let source_end = self.pointer_plus_storage(source, length, pointer_width)?;
        let maybe_backward = self.next_label("mem_maybe_backward");
        if moving {
            self.jump_if_equal(source, destination, pointer_width, &forward);
            self.jump_if_less(source, destination, pointer_width, &maybe_backward);
        }
        self.line(&format!("    jmp {forward}"));
        self.line(&format!("{maybe_backward}:"));
        if moving {
            self.jump_if_less(destination, source_end, pointer_width, &backward);
        }
        self.line(&format!("    jmp {forward}"));
        self.line(&format!("{backward}:"));
        if moving {
            let offset = self.pointer_minus_one(length, pointer_width)?;
            let source_end = self.pointer_plus_storage(source, offset, pointer_width)?;
            let destination_end = self.pointer_plus_storage(destination, offset, pointer_width)?;
            self.set_pointer_from_storage(POINTER_ZP, source_end, pointer_width);
            self.set_pointer_from_storage(
                POINTER_ZP + u32::from(pointer_width),
                destination_end,
                pointer_width,
            );
            self.emit_memory_loop(length, pointer_width, false, &done);
        }
        self.line(&format!("{forward}:"));
        self.set_pointer_from_storage(POINTER_ZP, source, pointer_width);
        self.set_pointer_from_storage(
            POINTER_ZP + u32::from(pointer_width),
            destination,
            pointer_width,
        );
        self.emit_memory_loop(length, pointer_width, true, &done);
        self.line(&format!("{done}:"));
        self.zero(self.r0);
        Ok(())
    }

    fn emit_65c816_block_transfer(
        &mut self,
        args: &[Expr],
        moving: bool,
    ) -> Result<bool, Diagnostic> {
        let Some(destination) = self.constant_pointer_address(&args[0]) else {
            return Ok(false);
        };
        let Some(source) = self.constant_pointer_address(&args[1]) else {
            return Ok(false);
        };
        let Ok(length) = self.model.const_value(&args[2]) else {
            return Ok(false);
        };
        let Ok(length) = u32::try_from(length) else {
            return Ok(false);
        };
        if length == 0 || length > 0x1_0000 {
            return Ok(false);
        }
        let overlap = (source < destination && destination < source.saturating_add(length))
            || (destination < source && source < destination.saturating_add(length));
        if overlap && !moving {
            return Err(Diagnostic::new(
                "mem.copy_nonoverlapping source and destination ranges overlap",
            ));
        }
        let backwards = moving && overlap;
        let source_address = if backwards {
            source + length - 1
        } else {
            source
        };
        let destination_address = if backwards {
            destination + length - 1
        } else {
            destination
        };
        self.line("    rep #$30");
        self.line(&format!("    ldx #${:04X}", source_address & 0xFFFF));
        self.line(&format!("    ldy #${:04X}", destination_address & 0xFFFF));
        self.line(&format!("    lda #${:04X}", (length - 1) & 0xFFFF));
        let mnemonic = if backwards { "mvp" } else { "mvn" };
        self.line(&format!(
            "    {mnemonic} ${:02X},${:02X}",
            source_address >> 16,
            destination_address >> 16
        ));
        self.line("    sep #$30");
        self.zero(self.r0);
        Ok(true)
    }

    fn emit_memory_loop(&mut self, length: Storage, pointer_width: u8, forward: bool, done: &str) {
        let loop_label = self.next_label(if forward {
            "mem_copy_forward"
        } else {
            "mem_copy_backward"
        });
        self.line(&format!("{loop_label}:"));
        self.jump_storage_zero(length, 3, done);
        self.load_pointer_byte(POINTER_ZP, pointer_width, self.r0.address);
        self.store_pointer_byte(
            POINTER_ZP + u32::from(pointer_width),
            pointer_width,
            self.r0.address,
        );
        if forward {
            self.increment_pointer_zp(POINTER_ZP, pointer_width);
            self.increment_pointer_zp(POINTER_ZP + u32::from(pointer_width), pointer_width);
        } else {
            self.decrement_pointer_zp(POINTER_ZP, pointer_width);
            self.decrement_pointer_zp(POINTER_ZP + u32::from(pointer_width), pointer_width);
        }
        self.decrement(length, 3);
        self.line(&format!("    jmp {loop_label}"));
    }

    fn emit_memory_fill(&mut self, args: &[Expr], types: &[Type]) -> Result<(), Diagnostic> {
        let values = self.eval_intrinsic_args(args, types)?;
        let length = self.zero_extend_memory_length(values[2], &types[2])?;
        let pointer_width = self.model.pointer_bytes();
        let loop_label = self.next_label("mem_fill");
        let done = self.next_label("mem_fill_done");
        self.set_pointer_from_storage(POINTER_ZP, values[0], pointer_width);
        self.line(&format!("{loop_label}:"));
        self.jump_storage_zero(length, 3, &done);
        self.store_pointer_byte(POINTER_ZP, pointer_width, values[1].address);
        self.increment_pointer_zp(POINTER_ZP, pointer_width);
        self.decrement(length, 3);
        self.line(&format!("    jmp {loop_label}"));
        self.line(&format!("{done}:"));
        Ok(())
    }

    fn emit_memory_compare(&mut self, args: &[Expr], types: &[Type]) -> Result<(), Diagnostic> {
        let values = self.eval_intrinsic_args(args, types)?;
        let length = self.zero_extend_memory_length(values[2], &types[2])?;
        let pointer_width = self.model.pointer_bytes();
        let left_byte = self.model.allocate(1)?;
        let less = self.next_label("mem_compare_less");
        let greater = self.next_label("mem_compare_greater");
        let equal = self.next_label("mem_compare_equal");
        let done = self.next_label("mem_compare_done");
        self.set_pointer_from_storage(POINTER_ZP, values[0], pointer_width);
        self.set_pointer_from_storage(
            POINTER_ZP + u32::from(pointer_width),
            values[1],
            pointer_width,
        );
        let loop_label = self.next_label("mem_compare_loop");
        self.line(&format!("{loop_label}:"));
        self.jump_storage_zero(length, 3, &equal);
        self.load_pointer_byte(POINTER_ZP, pointer_width, left_byte.address);
        self.load_pointer_byte(
            POINTER_ZP + u32::from(pointer_width),
            pointer_width,
            self.r0.address,
        );
        self.lda(left_byte.address);
        self.line(&format!("    cmp ${:04X}", self.r0.address));
        self.branch_long("bcc", &less);
        self.branch_long("bne", &greater);
        self.increment_pointer_zp(POINTER_ZP, pointer_width);
        self.increment_pointer_zp(POINTER_ZP + u32::from(pointer_width), pointer_width);
        self.decrement(length, 3);
        self.line(&format!("    jmp {loop_label}"));
        self.line(&format!("{less}:"));
        self.load_constant(-1, 1);
        self.line(&format!("    jmp {done}"));
        self.line(&format!("{greater}:"));
        self.load_constant(1, 1);
        self.line(&format!("    jmp {done}"));
        self.line(&format!("{equal}:"));
        self.load_constant(0, 1);
        self.line(&format!("{done}:"));
        Ok(())
    }

    fn emit_endian_load(
        &mut self,
        operation: MemIntrinsic,
        args: &[Expr],
        types: &[Type],
    ) -> Result<(), Diagnostic> {
        let values = self.eval_intrinsic_args(args, types)?;
        let pointer_width = self.model.pointer_bytes();
        let width = if matches!(operation, MemIntrinsic::LoadLe16 | MemIntrinsic::LoadBe16) {
            2
        } else {
            3
        };
        let big = matches!(operation, MemIntrinsic::LoadBe16 | MemIntrinsic::LoadBe24);
        self.set_pointer_from_storage(POINTER_ZP, values[0], pointer_width);
        self.zero(self.r0);
        for offset in 0..width {
            self.load_pointer_byte(
                POINTER_ZP,
                pointer_width,
                self.r0.address + if big { width - 1 - offset } else { offset },
            );
            if offset + 1 < width {
                self.increment_pointer_zp(POINTER_ZP, pointer_width);
            }
        }
        Ok(())
    }

    fn emit_endian_store(
        &mut self,
        operation: MemIntrinsic,
        args: &[Expr],
        types: &[Type],
    ) -> Result<(), Diagnostic> {
        let values = self.eval_intrinsic_args(args, types)?;
        let pointer_width = self.model.pointer_bytes();
        let width = if matches!(operation, MemIntrinsic::StoreLe16 | MemIntrinsic::StoreBe16) {
            2
        } else {
            3
        };
        let big = matches!(operation, MemIntrinsic::StoreBe16 | MemIntrinsic::StoreBe24);
        self.set_pointer_from_storage(POINTER_ZP, values[0], pointer_width);
        for offset in 0..width {
            self.store_pointer_byte(
                POINTER_ZP,
                pointer_width,
                values[1].address + if big { width - 1 - offset } else { offset },
            );
            if offset + 1 < width {
                self.increment_pointer_zp(POINTER_ZP, pointer_width);
            }
        }
        self.zero(self.r0);
        Ok(())
    }

    fn emit_peek8(&mut self, args: &[Expr], types: &[Type]) -> Result<(), Diagnostic> {
        let values = self.eval_intrinsic_args(args, types)?;
        self.zero(self.r0);
        self.set_pointer_from_storage(POINTER_ZP, values[0], self.model.pointer_bytes());
        self.load_pointer_byte(POINTER_ZP, self.model.pointer_bytes(), self.r0.address);
        Ok(())
    }

    fn emit_poke8(&mut self, args: &[Expr], types: &[Type]) -> Result<(), Diagnostic> {
        let values = self.eval_intrinsic_args(args, types)?;
        self.set_pointer_from_storage(POINTER_ZP, values[0], self.model.pointer_bytes());
        self.store_pointer_byte(POINTER_ZP, self.model.pointer_bytes(), values[1].address);
        self.zero(self.r0);
        Ok(())
    }

    fn emit_find_byte_values(
        &mut self,
        values: &[Storage],
        types: &[Type],
    ) -> Result<(), Diagnostic> {
        let length = self.zero_extend_memory_length(values[1], &types[1])?;
        let pointer_width = self.model.pointer_bytes();
        self.set_pointer_from_storage(POINTER_ZP, values[0], pointer_width);
        let loop_label = self.next_label("find_byte_loop");
        let found = self.next_label("find_byte_found");
        let not_found = self.next_label("find_byte_not_found");
        let done = self.next_label("find_byte_done");
        self.line(&format!("{loop_label}:"));
        self.jump_storage_zero(length, 3, &not_found);
        self.load_pointer_byte(POINTER_ZP, pointer_width, self.r0.address);
        self.line(&format!("    cmp ${:04X}", values[2].address));
        self.branch_long("beq", &found);
        self.increment_pointer_zp(POINTER_ZP, pointer_width);
        self.decrement(length, 3);
        self.line(&format!("    jmp {loop_label}"));
        self.line(&format!("{found}:"));
        self.copy_zp_width_to_result(pointer_width);
        let found_pointer = self.model.allocate(u32::from(pointer_width))?;
        self.copy(self.r0, found_pointer, u32::from(pointer_width));
        self.load_constant(1, 1);
        self.copy(self.r0, self.r1, 1);
        self.copy(found_pointer, self.r0, u32::from(pointer_width));
        self.line(&format!("    jmp {done}"));
        self.line(&format!("{not_found}:"));
        self.copy_zp_width_to_result(pointer_width);
        self.zero(self.r1);
        self.line(&format!("{done}:"));
        Ok(())
    }

    fn pointer_plus_storage(
        &mut self,
        pointer: Storage,
        offset: Storage,
        width: u8,
    ) -> Result<Storage, Diagnostic> {
        let result = self.model.allocate(u32::from(width))?;
        let rhs = self.model.allocate(u32::from(width))?;
        self.zero(rhs);
        self.copy(offset, rhs, u32::from(width).min(offset.size));
        self.add_storages(pointer, rhs, result, width);
        Ok(result)
    }

    fn pointer_minus_one(&mut self, value: Storage, width: u8) -> Result<Storage, Diagnostic> {
        let result = self.model.allocate(u32::from(width))?;
        self.zero(result);
        self.load_constant(1, 1);
        self.copy(self.r0, result, 1);
        self.sub_storages(value, result, result, width);
        Ok(result)
    }

    fn set_pointer_from_storage(&mut self, zero_page: u32, storage: Storage, width: u8) {
        for offset in 0..u32::from(width) {
            self.lda(storage.address + offset);
            self.sta(zero_page + offset);
        }
    }

    fn copy_zp_to_storage(&mut self, storage: Storage, width: u8) {
        for offset in 0..u32::from(width) {
            self.lda(POINTER_ZP + offset);
            self.sta(storage.address + offset);
        }
    }

    fn copy_result_to_zp_width(&mut self, width: u8) {
        for offset in 0..u32::from(width) {
            self.lda(self.r0.address + offset);
            self.sta(POINTER_ZP + offset);
        }
    }

    fn set_second_result_pointer(&mut self, destination: SecondResultDestination) {
        let width = self.model.pointer_bytes();
        match destination {
            SecondResultDestination::Direct(storage) => {
                self.load_constant(i64::from(storage.address), width);
                self.copy_result_to_zp_width(width);
            }
            SecondResultDestination::Pointer(storage) => {
                self.set_pointer_from_storage(POINTER_ZP, storage, width);
            }
        }
    }

    fn copy_storage_to_indirect(&mut self, source: Storage, size: u32) {
        let pointer_width = self.model.pointer_bytes();
        for offset in 0..size {
            self.store_pointer_byte(POINTER_ZP, pointer_width, source.address + offset);
        }
    }

    fn increment_pointer_zp(&mut self, zero_page: u32, width: u8) {
        let done = self.next_label("pointer_incremented");
        for offset in 0..u32::from(width) {
            self.line(&format!("    inc ${:02X}", zero_page + offset));
            self.branch_long("bne", &done);
        }
        self.line(&format!("{done}:"));
    }

    fn decrement_pointer_zp(&mut self, zero_page: u32, width: u8) {
        let done = self.next_label("pointer_decremented");
        for offset in 0..u32::from(width) {
            self.line(&format!("    dec ${:02X}", zero_page + offset));
            self.branch_long("bne", &done);
        }
        self.line(&format!("{done}:"));
    }

    fn load_pointer_byte(&mut self, zero_page: u32, pointer_width: u8, destination: u32) {
        if pointer_width == 3 {
            self.line("    phb");
            self.lda(zero_page + 2);
            self.line("    pha");
            self.line("    plb");
        }
        self.line("    ldy #$00");
        self.line(&format!("    lda (${:02X}),y", zero_page));
        self.sta(destination);
        if pointer_width == 3 {
            self.line("    plb");
        }
    }

    fn store_pointer_byte(&mut self, zero_page: u32, pointer_width: u8, source: u32) {
        if pointer_width == 3 {
            self.line("    phb");
            self.lda(zero_page + 2);
            self.line("    pha");
            self.line("    plb");
        }
        self.lda(source);
        self.line("    ldy #$00");
        self.line(&format!("    sta (${:02X}),y", zero_page));
        if pointer_width == 3 {
            self.line("    plb");
        }
    }

    fn copy_zp_width_to_result(&mut self, width: u8) {
        for offset in 0..u32::from(width) {
            self.lda(POINTER_ZP + offset);
            self.sta(self.r0.address + offset);
        }
    }

    fn add_storages(&mut self, left: Storage, right: Storage, result: Storage, width: u8) {
        self.line("    clc");
        for offset in 0..u32::from(width) {
            self.lda(left.address + offset);
            self.line(&format!("    adc ${:04X}", right.address + offset));
            self.sta(result.address + offset);
        }
    }

    fn sub_storages(&mut self, left: Storage, right: Storage, result: Storage, width: u8) {
        self.line("    sec");
        for offset in 0..u32::from(width) {
            self.lda(left.address + offset);
            self.line(&format!("    sbc ${:04X}", right.address + offset));
            self.sta(result.address + offset);
        }
    }

    fn emit_memcpy(&mut self, args: &[Expr]) -> Result<(), Diagnostic> {
        if args.len() != 3 {
            return Err(Diagnostic::new("mem.memcpy requires three arguments"));
        }
        let pointer = Type::Ptr(Box::new(Type::Named("u8".to_owned())));
        self.emit_expr(&args[0], &pointer)?;
        let destination = self.model.allocate(2)?;
        self.copy(self.r0, destination, 2);
        self.emit_expr(&args[1], &pointer)?;
        let source = self.model.allocate(2)?;
        self.copy(self.r0, source, 2);
        self.emit_expr(&args[2], &Type::Named("u16".to_owned()))?;
        let length = self.model.allocate(2)?;
        self.copy(self.r0, length, 2);
        let loop_label = self.next_label("memcpy_loop");
        let done = self.next_label("memcpy_done");
        self.set_zp_from_storage(POINTER_ZP, source);
        self.set_zp_from_storage(POINTER_ZP + 2, destination);
        self.line(&format!("{loop_label}:"));
        self.jump_storage_zero(length, 2, &done);
        self.line("    ldy #$00");
        self.line(&format!("    lda (${:02X}),y", POINTER_ZP));
        self.line(&format!("    sta (${:02X}),y", POINTER_ZP + 2));
        self.increment_zp(POINTER_ZP);
        self.increment_zp(POINTER_ZP + 2);
        self.decrement(length, 2);
        self.line(&format!("    jmp {loop_label}"));
        self.line(&format!("{done}:"));
        self.zero(self.r0);
        Ok(())
    }

    fn emit_memset(&mut self, args: &[Expr]) -> Result<(), Diagnostic> {
        if args.len() != 3 {
            return Err(Diagnostic::new("mem.memset requires three arguments"));
        }
        let pointer = Type::Ptr(Box::new(Type::Named("u8".to_owned())));
        self.emit_expr(&args[0], &pointer)?;
        let destination = self.model.allocate(2)?;
        self.copy(self.r0, destination, 2);
        self.emit_expr(&args[1], &Type::Named("u8".to_owned()))?;
        let value = self.model.allocate(1)?;
        self.copy(self.r0, value, 1);
        self.emit_expr(&args[2], &Type::Named("u16".to_owned()))?;
        let length = self.model.allocate(2)?;
        self.copy(self.r0, length, 2);
        let loop_label = self.next_label("memset_loop");
        let done = self.next_label("memset_done");
        self.set_zp_from_storage(POINTER_ZP, destination);
        self.line(&format!("{loop_label}:"));
        self.jump_storage_zero(length, 2, &done);
        self.lda(value.address);
        self.line("    ldy #$00");
        self.line(&format!("    sta (${:02X}),y", POINTER_ZP));
        self.increment_zp(POINTER_ZP);
        self.decrement(length, 2);
        self.line(&format!("    jmp {loop_label}"));
        self.line(&format!("{done}:"));
        self.zero(self.r0);
        Ok(())
    }

    fn emit_immediate_bitwise(&mut self, op: BinaryOp, width: u8, mask: i64) -> bool {
        if width <= 1 {
            return false;
        }

        for offset in 0..u32::from(width) {
            let byte = ((mask as u64 >> (offset * 8)) & 0xFF) as u8;
            let address = self.r0.address + offset;
            match (op, byte) {
                (BinaryOp::BitAnd, 0xFF) | (BinaryOp::BitOr, 0x00) | (BinaryOp::BitXor, 0x00) => {}
                (BinaryOp::BitAnd, 0x00) => self.zero(Storage { address, size: 1 }),
                (BinaryOp::BitOr, 0xFF) => {
                    self.lda_imm(0xFF);
                    self.sta(address);
                }
                (BinaryOp::BitAnd, byte) | (BinaryOp::BitOr, byte) | (BinaryOp::BitXor, byte) => {
                    self.lda(address);
                    let mnemonic = match op {
                        BinaryOp::BitAnd => "and",
                        BinaryOp::BitOr => "ora",
                        BinaryOp::BitXor => "eor",
                        _ => unreachable!(),
                    };
                    self.line(&format!("    {mnemonic} #${byte:02X}"));
                    self.sta(address);
                }
                _ => return false,
            }
        }
        true
    }

    fn emit_binary_op(&mut self, op: BinaryOp, width: u8, signed: bool) -> Result<(), Diagnostic> {
        match op {
            BinaryOp::Add => self.add(width),
            BinaryOp::Sub => self.sub(width),
            BinaryOp::BitAnd | BinaryOp::BitOr | BinaryOp::BitXor => {
                for offset in 0..u32::from(width) {
                    self.lda(self.r0.address + offset);
                    let mnemonic = match op {
                        BinaryOp::BitAnd => "and",
                        BinaryOp::BitOr => "ora",
                        BinaryOp::BitXor => "eor",
                        _ => unreachable!(),
                    };
                    self.line(&format!("    {mnemonic} ${:04X}", self.r1.address + offset));
                    self.sta(self.r0.address + offset);
                }
            }
            BinaryOp::Mul if width == 2 && !signed => {
                self.needs_u16_mul_helper = true;
                self.line(&format!("    jsr {U16_MUL_HELPER}"));
            }
            BinaryOp::Div | BinaryOp::Mod if width == 2 && !signed => {
                self.needs_u16_divmod_helper = true;
                self.line(&format!("    jsr {U16_DIVMOD_HELPER}"));
                if op == BinaryOp::Mod {
                    self.copy(self.r2, self.r0, 2);
                }
            }
            BinaryOp::Mul => self.multiply(width, signed),
            BinaryOp::Div | BinaryOp::Mod => self.divide(width, op == BinaryOp::Mod, signed),
            BinaryOp::Shl | BinaryOp::Shr => self.shift(width, op == BinaryOp::Shr, signed),
            BinaryOp::And | BinaryOp::Or => self.logical(op),
            op if is_comparison(op) => self.compare(op, width, signed),
            _ => return Err(Diagnostic::new("unsupported 6502 binary operation")),
        }
        Ok(())
    }

    fn emit_runtime_helpers(&mut self) {
        if self.needs_indirect_call_helper {
            self.line("__ezra_indirect_call:");
            self.line(&format!("    jmp (${:04X})", POINTER_ZP));
        }
        if self.needs_u16_mul_helper {
            self.emit_u16_mul_helper();
        }
        if self.needs_u16_divmod_helper {
            self.emit_u16_divmod_helper();
        }
    }

    fn emit_u16_mul_helper(&mut self) {
        let r0 = self.r0.address;
        let r1 = self.r1.address;
        let r2 = self.r2.address;
        self.line(&format!("{U16_MUL_HELPER}:"));
        self.line("    lda #$00");
        self.line(&format!("    sta ${r2:04X}"));
        self.line(&format!("    sta ${:04X}", r2 + 1));
        self.line("    ldx #$10");
        self.line("__ezra_u16_mul_loop:");
        self.line(&format!("    lda ${r1:04X}"));
        self.line("    and #$01");
        self.line("    beq __ezra_u16_mul_shift");
        self.line("    clc");
        self.line(&format!("    lda ${r2:04X}"));
        self.line(&format!("    adc ${r0:04X}"));
        self.line(&format!("    sta ${r2:04X}"));
        self.line(&format!("    lda ${:04X}", r2 + 1));
        self.line(&format!("    adc ${:04X}", r0 + 1));
        self.line(&format!("    sta ${:04X}", r2 + 1));
        self.line("__ezra_u16_mul_shift:");
        self.line(&format!("    asl ${r0:04X}"));
        self.line(&format!("    rol ${:04X}", r0 + 1));
        self.line(&format!("    lsr ${:04X}", r1 + 1));
        self.line(&format!("    ror ${r1:04X}"));
        self.line("    dex");
        self.line("    bne __ezra_u16_mul_loop");
        self.line(&format!("    lda ${r2:04X}"));
        self.line(&format!("    sta ${r0:04X}"));
        self.line(&format!("    lda ${:04X}", r2 + 1));
        self.line(&format!("    sta ${:04X}", r0 + 1));
        self.line("    rts");
    }

    fn emit_u16_divmod_helper(&mut self) {
        let r0 = self.r0.address;
        let r1 = self.r1.address;
        let r2 = self.r2.address;
        self.line(&format!("{U16_DIVMOD_HELPER}:"));
        self.line("    lda #$00");
        self.line(&format!("    sta ${r2:04X}"));
        self.line(&format!("    sta ${:04X}", r2 + 1));
        self.line(&format!("    lda ${r1:04X}"));
        self.line(&format!("    ora ${:04X}", r1 + 1));
        self.line("    bne __ezra_u16_divmod_nonzero");
        self.line("    lda #$00");
        self.line(&format!("    sta ${r0:04X}"));
        self.line(&format!("    sta ${:04X}", r0 + 1));
        self.line("    rts");
        self.line("__ezra_u16_divmod_nonzero:");
        self.line("    ldx #$10");
        self.line("__ezra_u16_divmod_loop:");
        self.line(&format!("    asl ${r0:04X}"));
        self.line(&format!("    rol ${:04X}", r0 + 1));
        self.line(&format!("    rol ${r2:04X}"));
        self.line(&format!("    rol ${:04X}", r2 + 1));
        self.line(&format!("    lda ${:04X}", r2 + 1));
        self.line(&format!("    cmp ${:04X}", r1 + 1));
        self.line("    bcc __ezra_u16_divmod_next");
        self.line("    bne __ezra_u16_divmod_subtract");
        self.line(&format!("    lda ${r2:04X}"));
        self.line(&format!("    cmp ${r1:04X}"));
        self.line("    bcc __ezra_u16_divmod_next");
        self.line("__ezra_u16_divmod_subtract:");
        self.line("    sec");
        self.line(&format!("    lda ${r2:04X}"));
        self.line(&format!("    sbc ${r1:04X}"));
        self.line(&format!("    sta ${r2:04X}"));
        self.line(&format!("    lda ${:04X}", r2 + 1));
        self.line(&format!("    sbc ${:04X}", r1 + 1));
        self.line(&format!("    sta ${:04X}", r2 + 1));
        self.line(&format!("    inc ${r0:04X}"));
        self.line("    bne __ezra_u16_divmod_next");
        self.line(&format!("    inc ${:04X}", r0 + 1));
        self.line("__ezra_u16_divmod_next:");
        self.line("    dex");
        self.line("    bne __ezra_u16_divmod_loop");
        self.line("    rts");
    }

    fn emit_unary(&mut self, op: UnaryOp, width: u8) {
        match op {
            UnaryOp::BitNot => {
                for offset in 0..u32::from(width) {
                    self.lda(self.r0.address + offset);
                    self.line("    eor #$FF");
                    self.sta(self.r0.address + offset);
                }
            }
            UnaryOp::Neg => {
                for offset in 0..u32::from(width) {
                    self.lda(self.r0.address + offset);
                    self.line("    eor #$FF");
                    self.sta(self.r0.address + offset);
                }
                self.line("    clc");
                for offset in 0..u32::from(width) {
                    self.lda(self.r0.address + offset);
                    self.line(&format!(
                        "    adc #${:02X}",
                        if offset == 0 { 1 } else { 0 }
                    ));
                    self.sta(self.r0.address + offset);
                }
            }
            UnaryOp::Not => {
                let true_label = self.next_label("not_true");
                let done = self.next_label("not_done");
                self.jump_if_zero(self.r0.address, &true_label);
                self.load_constant(0, 1);
                self.line(&format!("    jmp {done}"));
                self.line(&format!("{true_label}:"));
                self.load_constant(1, 1);
                self.line(&format!("{done}:"));
            }
        }
    }

    fn add(&mut self, width: u8) {
        self.line("    clc");
        for offset in 0..u32::from(width) {
            self.lda(self.r0.address + offset);
            self.line(&format!("    adc ${:04X}", self.r1.address + offset));
            self.sta(self.r0.address + offset);
        }
    }

    fn sub(&mut self, width: u8) {
        self.line("    sec");
        for offset in 0..u32::from(width) {
            self.lda(self.r0.address + offset);
            self.line(&format!("    sbc ${:04X}", self.r1.address + offset));
            self.sta(self.r0.address + offset);
        }
    }

    fn multiply_constant(
        &mut self,
        width: u8,
        factor: i64,
        signed: bool,
        preserve_left: bool,
    ) -> bool {
        let magnitude = truncated_magnitude(width, factor);
        let plans = constant_multiply_plans(width, magnitude);
        let candidates = plans
            .iter()
            .map(|plan| {
                CostCandidate::new(
                    plan.name(),
                    self.constant_multiply_plan_cost(
                        *plan,
                        width,
                        magnitude,
                        factor < 0,
                        signed,
                        preserve_left,
                    ),
                )
            })
            .collect::<Vec<_>>();
        let selected = self
            .constant_multiply_cost_model()
            .choose_index(&candidates)
            .expect("constant multiply candidate");
        let plan = plans[selected];
        if plan == ConstantMultiplyPlan::Fallback {
            return false;
        }

        match plan {
            ConstantMultiplyPlan::Zero => self.zero(self.r0),
            ConstantMultiplyPlan::Identity => {}
            ConstantMultiplyPlan::Shift { count } => {
                self.shift_constant(width, false, false, count);
            }
            ConstantMultiplyPlan::ShiftAdd { count, subtract } => {
                let original = self
                    .model
                    .allocate(u32::from(width))
                    .expect("constant multiply scratch");
                self.copy(self.r0, original, u32::from(width));
                self.shift_constant(width, false, false, count);
                self.copy(original, self.r1, u32::from(width));
                if subtract {
                    self.sub(width);
                } else {
                    self.add(width);
                }
            }
            ConstantMultiplyPlan::Horner { magnitude } => {
                let original = self
                    .model
                    .allocate(u32::from(width))
                    .expect("constant multiply scratch");
                self.copy(self.r0, original, u32::from(width));
                let highest_bit = magnitude.ilog2();
                for bit in (0..highest_bit).rev() {
                    self.shift_constant(width, false, false, 1);
                    if magnitude & (1u64 << bit) != 0 {
                        self.copy(original, self.r1, u32::from(width));
                        self.add(width);
                    }
                }
            }
            ConstantMultiplyPlan::Fallback => unreachable!(),
        }
        if factor < 0 {
            self.emit_unary(UnaryOp::Neg, width);
        }
        true
    }

    fn constant_multiply_cost_model(&self) -> CostModel {
        // Multiplication does not promise any particular flags to its caller.
        // CPU-specific instruction costs still distinguish NMOS/2A03 from the
        // CMOS variants, notably for STZ in byte moves and zeroing.
        CostModel::balanced().with_live_flags(crate::tbir::cost::FlagSet::NONE)
    }

    fn constant_multiply_plan_cost(
        &self,
        plan: ConstantMultiplyPlan,
        width: u8,
        magnitude: u64,
        negative: bool,
        signed: bool,
        preserve_left: bool,
    ) -> InstructionCost {
        constant_multiply_plan_cost(
            plan,
            width,
            magnitude,
            negative,
            signed,
            preserve_left,
            self.supports_65c02(),
        )
    }

    fn multiply(&mut self, width: u8, signed: bool) {
        let loop_label = self.next_label("mul_loop");
        let done = self.next_label("mul_done");
        let multiplicand = self
            .model
            .allocate(u32::from(width))
            .expect("multiply scratch");
        let multiplier = self
            .model
            .allocate(u32::from(width))
            .expect("multiply scratch");
        let negative = self.model.allocate(1).expect("multiply sign");
        self.zero(negative);
        if signed {
            self.normalize_signed_operand(self.r0, width, negative, false);
            self.normalize_signed_operand(self.r1, width, negative, true);
        }
        self.copy(self.r0, multiplicand, u32::from(width));
        self.copy(self.r1, multiplier, u32::from(width));
        self.zero(self.r0);
        self.line(&format!("{loop_label}:"));
        self.jump_storage_zero(multiplier, width, &done);
        self.copy(multiplicand, self.r1, u32::from(width));
        self.add(width);
        self.decrement(multiplier, width);
        self.line(&format!("    jmp {loop_label}"));
        self.line(&format!("{done}:"));
        if signed {
            self.negate_if_flag(self.r0, width, negative);
        }
    }

    fn divide(&mut self, width: u8, remainder: bool, signed: bool) {
        let loop_label = self.next_label("div_loop");
        let done = self.next_label("div_done");
        let zero = self.next_label("div_zero");
        let quotient_negative = self.model.allocate(1).expect("division sign");
        let remainder_negative = self.model.allocate(1).expect("division sign");
        self.zero(quotient_negative);
        self.zero(remainder_negative);
        if signed {
            self.lda(self.r0.address + u32::from(width - 1));
            let dividend_positive = self.next_label("dividend_positive");
            self.branch_long("bpl", &dividend_positive);
            self.toggle(quotient_negative);
            self.toggle(remainder_negative);
            self.negate_storage(self.r0, width);
            self.line(&format!("{dividend_positive}:"));
            self.normalize_signed_operand(self.r1, width, quotient_negative, true);
        }
        self.zero(self.r2);
        self.jump_storage_zero(self.r1, width, &zero);
        self.line(&format!("{loop_label}:"));
        self.jump_if_less(self.r0, self.r1, width, &done);
        self.sub(width);
        self.increment(self.r2, width);
        self.line(&format!("    jmp {loop_label}"));
        self.line(&format!("{zero}:"));
        self.zero(self.r0);
        self.line(&format!("    jmp {done}"));
        self.line(&format!("{done}:"));
        if !remainder {
            self.copy(self.r2, self.r0, u32::from(width));
        }
        if signed {
            self.negate_if_flag(
                self.r0,
                width,
                if remainder {
                    remainder_negative
                } else {
                    quotient_negative
                },
            );
        }
    }

    fn shift_constant(&mut self, width: u8, right: bool, signed: bool, count: u32) {
        let bits = u32::from(width) * 8;
        let count = count.min(bits);
        let byte_count = count / 8;
        let bit_count = count % 8;

        // A byte-boundary move and its remaining carry chain are equivalent
        // to the full carry chain, but are not always cheaper for narrow
        // values. Keep the selection costed instead of assuming that every
        // constant shift should be split at a byte boundary.
        let bytewise_cost = self.shift_cost(width, byte_count, bit_count, right, signed);
        let carry_cost = self.shift_cost(width, 0, count, right, signed);
        if byte_count != 0 && bytewise_cost < carry_cost {
            self.shift_bytes(width, right, byte_count, signed);
            self.shift_bits(width, right, bit_count, signed);
        } else {
            self.shift_bits(width, right, count, signed);
        }
    }

    fn shift_cost(
        &self,
        width: u8,
        byte_count: u32,
        bit_count: u32,
        right: bool,
        signed: bool,
    ) -> u32 {
        shift_cost(
            width,
            byte_count,
            bit_count,
            right,
            signed,
            self.supports_65c02(),
        )
    }

    fn shift_bytes(&mut self, width: u8, right: bool, byte_count: u32, signed: bool) {
        if byte_count == 0 {
            return;
        }
        let width = u32::from(width);
        if right {
            if signed && byte_count == width {
                self.lda(self.r0.address + width - 1);
            }
            for offset in 0..width - byte_count {
                self.lda(self.r0.address + offset + byte_count);
                self.sta(self.r0.address + offset);
            }
            if signed {
                if byte_count != width {
                    self.lda(self.r0.address + width - byte_count - 1);
                }
                self.line("    asl a");
                self.line("    lda #$00");
                self.line("    sbc #$00");
                self.line("    eor #$FF");
            } else {
                self.zero(Storage {
                    address: self.r0.address + width - byte_count,
                    size: byte_count,
                });
            }
            if signed {
                for offset in width - byte_count..width {
                    self.sta(self.r0.address + offset);
                }
            }
        } else {
            for offset in (byte_count..width).rev() {
                self.lda(self.r0.address + offset - byte_count);
                self.sta(self.r0.address + offset);
            }
            self.zero(Storage {
                address: self.r0.address,
                size: byte_count,
            });
        }
    }

    fn shift_bits(&mut self, width: u8, right: bool, count: u32, signed: bool) {
        for _ in 0..count {
            self.shift_once(width, right, signed);
        }
    }

    fn shift_once(&mut self, width: u8, right: bool, signed: bool) {
        if right {
            if signed {
                self.lda(self.r0.address + u32::from(width - 1));
                self.line("    asl a");
            } else {
                self.line(&format!(
                    "    lsr ${:04X}",
                    self.r0.address + u32::from(width - 1)
                ));
            }
            let lower_bytes = if signed { width } else { width - 1 };
            for offset in (0..u32::from(lower_bytes)).rev() {
                self.line(&format!("    ror ${:04X}", self.r0.address + offset));
            }
        } else {
            self.line("    clc");
            for offset in 0..u32::from(width) {
                self.line(&format!("    rol ${:04X}", self.r0.address + offset));
            }
        }
    }

    fn shift(&mut self, width: u8, right: bool, signed: bool) {
        let loop_label = self.next_label("shift_loop");
        let done = self.next_label("shift_done");
        self.line(&format!("{loop_label}:"));
        self.jump_if_zero(self.r1.address, &done);
        self.shift_once(width, right, signed);
        self.line(&format!("    dec ${:04X}", self.r1.address));
        self.line(&format!("    jmp {loop_label}"));
        self.line(&format!("{done}:"));
    }

    fn logical(&mut self, op: BinaryOp) {
        let true_label = self.next_label("logic_true");
        let false_label = self.next_label("logic_false");
        let done = self.next_label("logic_done");
        match op {
            BinaryOp::And => {
                self.jump_if_zero(self.r0.address, &false_label);
                self.jump_if_zero(self.r1.address, &false_label);
                self.line(&format!("    jmp {true_label}"));
            }
            BinaryOp::Or => {
                self.jump_if_nonzero(self.r0.address, &true_label);
                self.jump_if_nonzero(self.r1.address, &true_label);
                self.line(&format!("    jmp {false_label}"));
            }
            _ => unreachable!(),
        }
        self.line(&format!("{true_label}:"));
        self.load_constant(1, 1);
        self.line(&format!("    jmp {done}"));
        self.line(&format!("{false_label}:"));
        self.load_constant(0, 1);
        self.line(&format!("{done}:"));
    }

    fn emit_short_circuit(
        &mut self,
        left: &Expr,
        op: BinaryOp,
        right: &Expr,
    ) -> Result<(), Diagnostic> {
        let decisive = self.next_label("logical_decisive");
        let done = self.next_label("logical_done");
        let bool_type = Type::Named("bool".to_owned());
        self.emit_expr(left, &bool_type)?;
        match op {
            BinaryOp::And => self.jump_if_zero(self.r0.address, &decisive),
            BinaryOp::Or => self.jump_if_nonzero(self.r0.address, &decisive),
            _ => unreachable!(),
        }
        self.emit_expr(right, &bool_type)?;
        self.line(&format!("    jmp {done}"));
        self.line(&format!("{decisive}:"));
        self.load_constant(i64::from(op == BinaryOp::Or), 1);
        self.line(&format!("{done}:"));
        Ok(())
    }

    fn compare(&mut self, op: BinaryOp, width: u8, signed: bool) {
        let true_label = self.next_label("compare_true");
        let false_label = self.next_label("compare_false");
        let done = self.next_label("compare_done");
        if signed && !matches!(op, BinaryOp::Eq | BinaryOp::Ne) {
            let top = u32::from(width - 1);
            self.lda(self.r0.address + top);
            self.line("    eor #$80");
            self.sta(self.r0.address + top);
            self.lda(self.r1.address + top);
            self.line("    eor #$80");
            self.sta(self.r1.address + top);
        }
        match op {
            BinaryOp::Eq | BinaryOp::Ne => {
                for offset in 0..u32::from(width) {
                    self.lda(self.r0.address + offset);
                    self.line(&format!("    cmp ${:04X}", self.r1.address + offset));
                    self.branch_long(
                        "bne",
                        if op == BinaryOp::Ne {
                            &true_label
                        } else {
                            &false_label
                        },
                    );
                }
                self.line(&format!(
                    "    jmp {}",
                    if op == BinaryOp::Eq {
                        &true_label
                    } else {
                        &false_label
                    }
                ));
            }
            BinaryOp::Lt | BinaryOp::Le => {
                self.jump_if_less(self.r0, self.r1, width, &true_label);
                if op == BinaryOp::Le {
                    self.jump_if_equal(self.r0, self.r1, width, &true_label);
                }
                self.line(&format!("    jmp {false_label}"));
            }
            BinaryOp::Gt | BinaryOp::Ge => {
                self.jump_if_less(self.r0, self.r1, width, &false_label);
                if op == BinaryOp::Gt {
                    self.jump_if_equal(self.r0, self.r1, width, &false_label);
                }
                self.line(&format!("    jmp {true_label}"));
            }
            _ => unreachable!(),
        }
        self.line(&format!("{true_label}:"));
        self.load_constant(1, 1);
        self.line(&format!("    jmp {done}"));
        self.line(&format!("{false_label}:"));
        self.load_constant(0, 1);
        self.line(&format!("{done}:"));
    }

    fn emit_load_place(&mut self, place: &Place, width: u8) -> Result<(), Diagnostic> {
        match self.place_address(place)? {
            Address::Direct(address) => self.copy(
                Storage {
                    address,
                    size: u32::from(width),
                },
                self.r0,
                u32::from(width),
            ),
            Address::Indirect => self.load_indirect(width),
        }
        Ok(())
    }

    fn emit_store_place(&mut self, place: &Place, width: u8) -> Result<(), Diagnostic> {
        let saved = self.model.allocate(u32::from(width))?;
        self.copy(self.r0, saved, u32::from(width));
        match self.place_address(place)? {
            Address::Direct(address) => self.copy(
                saved,
                Storage {
                    address,
                    size: u32::from(width),
                },
                u32::from(width),
            ),
            Address::Indirect => {
                for offset in 0..u32::from(width) {
                    self.lda(saved.address + offset);
                    self.line(&format!("    ldy #${offset:02X}"));
                    self.line(&format!("    sta (${:02X}),y", POINTER_ZP));
                }
            }
        }
        Ok(())
    }

    fn emit_65c02_store_zero(
        &mut self,
        place: &Place,
        width: u8,
        value: &Expr,
    ) -> Result<bool, Diagnostic> {
        if !self.supports_65c02()
            || self.model.const_value(value).ok() != Some(0)
            || !matches!(place, Place::Ident(_) | Place::Field { .. })
        {
            return Ok(false);
        }
        let Some(address) = self.direct_place_address(place)? else {
            return Ok(false);
        };
        for offset in 0..u32::from(width) {
            self.line(&format!("    stz ${:04X}", address + offset));
        }
        Ok(true)
    }

    fn emit_65c02_bit_modify(
        &mut self,
        place: &Place,
        width: u8,
        op: AssignOp,
        value: &Expr,
    ) -> Result<bool, Diagnostic> {
        if !self.supports_65c02()
            || !matches!(op, AssignOp::BitAnd | AssignOp::BitOr)
            || !matches!(place, Place::Ident(_) | Place::Field { .. })
        {
            return Ok(false);
        }
        let Ok(raw_value) = self.model.const_value(value) else {
            return Ok(false);
        };
        let Some(address) = self.direct_place_address(place)? else {
            return Ok(false);
        };
        for offset in 0..u32::from(width) {
            let byte = (raw_value as u64 >> (offset * 8)) as u8;
            self.lda_imm(if op == AssignOp::BitAnd { !byte } else { byte });
            let mnemonic = if op == AssignOp::BitAnd { "trb" } else { "tsb" };
            self.line(&format!("    {mnemonic} ${:04X}", address + offset));
        }
        Ok(true)
    }

    fn direct_place_address(&self, place: &Place) -> Result<Option<u32>, Diagnostic> {
        match place {
            Place::Ident(name) => Ok(Some(self.binding(name)?.storage.address)),
            Place::Field { base, field } => {
                let binding = self.binding(base)?;
                let field = self.model.field(&binding.ty, field)?;
                Ok(Some(binding.storage.address + field.offset))
            }
            Place::Deref(pointer) => Ok(self.constant_pointer_address(pointer)),
            Place::Index { .. } | Place::Access(_) => Ok(None),
        }
    }

    fn supports_65c02(&self) -> bool {
        matches!(self.cpu, CpuFamily::Cmos65C02 | CpuFamily::Wdc65C816)
    }

    fn emit_store_aggregate_place(
        &mut self,
        place: &Place,
        source: Storage,
        size: u32,
    ) -> Result<(), Diagnostic> {
        match self.place_address(place)? {
            Address::Direct(address) => self.copy(source, Storage { address, size }, size),
            Address::Indirect => {
                for offset in 0..size {
                    self.lda(source.address + offset);
                    self.line("    ldy #$00");
                    self.line(&format!("    sta (${:02X}),y", POINTER_ZP));
                    if offset + 1 < size {
                        self.increment_zp(POINTER_ZP);
                    }
                }
            }
        }
        Ok(())
    }

    fn copy_indirect_to_storage(&mut self, storage: Storage, size: u32) {
        for offset in 0..size {
            self.line("    ldy #$00");
            self.line(&format!("    lda (${:02X}),y", POINTER_ZP));
            self.sta(storage.address + offset);
            if offset + 1 < size {
                self.increment_zp(POINTER_ZP);
            }
        }
    }

    fn place_address(&mut self, place: &Place) -> Result<Address, Diagnostic> {
        match place {
            Place::Ident(name) => {
                let binding = self.binding(name)?;
                Ok(Address::Direct(binding.storage.address))
            }
            Place::Index { name, index } => {
                self.emit_named_index_address(name, index)?;
                Ok(Address::Indirect)
            }
            Place::Field { base, field } => {
                let binding = self.binding(base)?;
                let layout = self.model.field(&binding.ty, field)?;
                Ok(Address::Direct(binding.storage.address + layout.offset))
            }
            Place::Access(path) => {
                self.emit_access_address(path)?;
                Ok(Address::Indirect)
            }
            Place::Deref(expr) => {
                let Type::Ptr(inner) = self.model.resolved_type(&self.expr_type(expr)?)? else {
                    return Err(Diagnostic::new("dereference requires pointer"));
                };
                if let Some(address) = self.constant_pointer_address(expr) {
                    Ok(Address::Direct(address))
                } else {
                    self.emit_expr(expr, &Type::Ptr(inner.clone()))?;
                    self.copy_result_to_zp();
                    Ok(Address::Indirect)
                }
            }
        }
    }

    fn place_type(&self, place: &Place) -> Result<Type, Diagnostic> {
        match place {
            Place::Ident(name) => Ok(self.binding(name)?.ty),
            Place::Index { name, .. } => {
                element_type(&self.model.resolved_type(&self.binding(name)?.ty)?)
            }
            Place::Field { base, field } => {
                Ok(self.model.field(&self.binding(base)?.ty, field)?.ty.clone())
            }
            Place::Access(path) => self.access_type(path),
            Place::Deref(expr) => match self.model.resolved_type(&self.expr_type(expr)?)? {
                Type::Ptr(inner) => Ok(*inner),
                _ => Err(Diagnostic::new("dereference requires pointer")),
            },
        }
    }

    fn emit_named_index_address(&mut self, name: &str, index: &Expr) -> Result<Type, Diagnostic> {
        let binding = self.binding(name)?;
        let resolved = self.model.resolved_type(&binding.ty)?;
        let element = element_type(&resolved)?;
        let element_size = self.model.type_size(&element)?;
        match resolved {
            Type::Array { .. } => self.set_pointer(binding.storage.address),
            Type::Ptr(_) => {
                self.copy(binding.storage, self.r0, 2);
                self.copy_result_to_zp();
            }
            _ => return Err(Diagnostic::new("indexing requires array or pointer")),
        }
        self.add_index_to_pointer(index, element_size)?;
        Ok(element)
    }

    fn emit_access_address(&mut self, path: &AccessPath) -> Result<(Type, bool), Diagnostic> {
        let binding = self.binding(&path.root)?;
        let mut ty = self.model.resolved_type(&binding.ty)?;
        match &ty {
            Type::Ptr(_) => {
                self.copy(binding.storage, self.r0, 2);
                self.copy_result_to_zp();
                if let Type::Ptr(inner) = ty {
                    ty = *inner;
                }
            }
            _ => self.set_pointer(binding.storage.address),
        }
        for segment in &path.segments {
            match segment {
                AccessSegment::Field(name) => {
                    let field = self.model.field(&ty, name)?.clone();
                    self.add_pointer_constant(field.offset);
                    ty = field.ty;
                }
                AccessSegment::Index(index) => {
                    let element = element_type(&self.model.resolved_type(&ty)?)?;
                    let size = self.model.type_size(&element)?;
                    self.add_index_to_pointer(index, size)?;
                    ty = element;
                }
            }
        }
        Ok((ty, true))
    }

    fn add_index_to_pointer(&mut self, index: &Expr, element_size: u32) -> Result<(), Diagnostic> {
        if let Ok(index) = self.model.const_value(index) {
            let offset = u32::try_from(index)
                .ok()
                .and_then(|index| index.checked_mul(element_size))
                .ok_or_else(|| Diagnostic::new("array index offset overflow"))?;
            self.add_pointer_constant(offset);
            return Ok(());
        }
        let saved_lo = self.model.allocate(2)?;
        self.lda(POINTER_ZP);
        self.sta(saved_lo.address);
        self.lda(POINTER_ZP + 1);
        self.sta(saved_lo.address + 1);
        self.emit_expr(index, &Type::Named("u16".to_owned()))?;
        self.lda(saved_lo.address);
        self.sta(POINTER_ZP);
        self.lda(saved_lo.address + 1);
        self.sta(POINTER_ZP + 1);
        self.scale_pointer_index(element_size)?;
        self.line("    clc");
        self.lda(saved_lo.address);
        self.line(&format!("    adc ${:04X}", self.r0.address));
        self.sta(POINTER_ZP);
        self.lda(saved_lo.address + 1);
        self.line(&format!("    adc ${:04X}", self.r0.address + 1));
        self.sta(POINTER_ZP + 1);
        Ok(())
    }

    fn scale_pointer_index(&mut self, element_size: u32) -> Result<(), Diagnostic> {
        let scale = element_size & 0xFFFF;
        if scale == 0 {
            self.zero(self.r0);
            return Ok(());
        }
        if scale == 1 {
            return Ok(());
        }
        if scale.is_power_of_two() {
            self.shift_constant(2, false, false, scale.trailing_zeros());
            return Ok(());
        }
        if self.multiply_constant(2, i64::from(scale), false, false) {
            return Ok(());
        }

        let index = self.model.allocate(2)?;
        self.copy(self.r0, index, 2);
        self.load_constant(i64::from(scale), 2);
        self.copy(self.r0, self.r1, 2);
        self.copy(index, self.r0, 2);
        self.emit_binary_op(BinaryOp::Mul, 2, false)
    }

    fn expr_type(&self, expr: &Expr) -> Result<Type, Diagnostic> {
        match expr {
            Expr::Int(value) => Ok(if (0..=0xFF).contains(value) {
                Type::Named("u8".to_owned())
            } else if (0..=0xFFFF).contains(value) {
                Type::Named("u16".to_owned())
            } else {
                Type::Named("u24".to_owned())
            }),
            Expr::TypedInt(_, ty) | Expr::Cast { ty, .. } => Ok(ty.clone()),
            Expr::Bool(_) => Ok(Type::Named("bool".to_owned())),
            Expr::Char(_) | Expr::In(_) => Ok(Type::Named("u8".to_owned())),
            Expr::String(_) => Ok(Type::Ptr(Box::new(Type::Named("u8".to_owned())))),
            Expr::Ident(name) => self
                .model
                .constant_types
                .get(name)
                .cloned()
                .or_else(|| self.binding(name).ok().map(|binding| binding.ty))
                .ok_or_else(|| Diagnostic::new(format!("unknown value `{name}`"))),
            Expr::Index { name, .. } => {
                element_type(&self.model.resolved_type(&self.binding(name)?.ty)?)
            }
            Expr::Field { base, field } => {
                let constant_name = format!("{base}.{field}");
                if let Some(ty) = self.model.constant_types.get(&constant_name) {
                    Ok(ty.clone())
                } else {
                    Ok(self.model.field(&self.binding(base)?.ty, field)?.ty.clone())
                }
            }
            Expr::AddressOfIndex { name, .. } => Ok(Type::Ptr(Box::new(element_type(
                &self.model.resolved_type(&self.binding(name)?.ty)?,
            )?))),
            Expr::AddressOfField { base, field } => Ok(Type::Ptr(Box::new(
                self.model.field(&self.binding(base)?.ty, field)?.ty.clone(),
            ))),
            Expr::Access(path) => self.access_type(path),
            Expr::AddressOfAccess(path) => Ok(Type::Ptr(Box::new(self.access_type(path)?))),
            Expr::AddressOf(name) => self
                .function_value_type(name)
                .map(|ty| Type::Ptr(Box::new(ty)))
                .or_else(|| {
                    self.binding(name)
                        .ok()
                        .map(|binding| Type::Ptr(Box::new(binding.ty)))
                })
                .ok_or_else(|| Diagnostic::new(format!("unknown value `{name}`"))),
            Expr::StructInit { ty, .. } => Ok(Type::Named(ty.clone())),
            Expr::Deref(expr) => match self.model.resolved_type(&self.expr_type(expr)?)? {
                Type::Ptr(inner) => Ok(*inner),
                _ => Err(Diagnostic::new("dereference requires pointer")),
            },
            Expr::BankedPointer { pointer, .. } => self.expr_type(pointer),
            Expr::Call { path, args } => {
                if let Some(descriptor) = intrinsic_descriptor(path) {
                    let resolution = self.resolve_intrinsic(path, args)?;
                    return resolution.result_types.first().cloned().ok_or_else(|| {
                        Diagnostic::new(format!(
                            "intrinsic `{}` has no scalar result",
                            descriptor.canonical_name
                        ))
                    });
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
                if path.len() == 1 {
                    let binding = self.binding(&path[0])?;
                    if let Type::Ptr(inner) = self.model.resolved_type(&binding.ty)?
                        && let Type::Function { return_type, .. } = *inner
                    {
                        return return_type
                            .map(|ty| *ty)
                            .ok_or_else(|| Diagnostic::new("void function has no value"));
                    }
                }
                Err(Diagnostic::new(format!(
                    "unknown function `{}`",
                    path.join(".")
                )))
            }
            Expr::Unary {
                op: UnaryOp::Not, ..
            } => Ok(Type::Named("bool".to_owned())),
            Expr::Unary { expr, .. } => self.expr_type(expr),
            Expr::Binary { left, op, .. }
                if is_comparison(*op) || matches!(op, BinaryOp::And | BinaryOp::Or) =>
            {
                let _ = left;
                Ok(Type::Named("bool".to_owned()))
            }
            Expr::Binary { left, .. } => self.expr_type(left),
            Expr::Array(_) => Err(Diagnostic::new("array type requires context")),
        }
    }

    fn access_type(&self, path: &AccessPath) -> Result<Type, Diagnostic> {
        let mut ty = self.model.resolved_type(&self.binding(&path.root)?.ty)?;
        if let Type::Ptr(inner) = ty {
            ty = *inner;
        }
        for segment in &path.segments {
            ty = match segment {
                AccessSegment::Field(name) => self.model.field(&ty, name)?.ty.clone(),
                AccessSegment::Index(_) => element_type(&self.model.resolved_type(&ty)?)?,
            };
        }
        Ok(ty)
    }

    fn emit_inline_asm(
        &mut self,
        inputs: &[crate::ast::AsmInput],
        outputs: &[crate::ast::AsmOutput],
        lines: &[String],
    ) -> Result<(), Diagnostic> {
        let mut operands = HashMap::new();
        for input in inputs {
            let binding = self.binding(&input.name)?;
            operands.insert(
                input.name.clone(),
                format!("${:04X}", binding.storage.address),
            );
        }
        for output in outputs {
            let binding = self.binding(&output.name)?;
            operands.insert(
                output.name.clone(),
                format!("${:04X}", binding.storage.address),
            );
        }
        for line in lines {
            let mut emitted = line.clone();
            for (name, value) in &operands {
                emitted = emitted.replace(&format!("{{{name}}}"), value);
            }
            if emitted.contains(['{', '}']) {
                return Err(Diagnostic::new(format!(
                    "unknown inline asm operand placeholder in `{line}`"
                )));
            }
            self.line(&format!("    {emitted}"));
        }
        Ok(())
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
        Err(Diagnostic::new(format!("unknown variable `{name}`")))
    }

    fn live_storage_segments(
        &self,
        live: Storage,
        args: &[Expr],
        additional_exclusion: Option<Storage>,
    ) -> Vec<Storage> {
        let mut excluded = args
            .iter()
            .filter_map(|arg| {
                // The callee can mutate this storage through its pointer parameter.
                // Restoring a pre-call snapshot would discard that mutation.
                self.addressed_storage(arg)
            })
            .chain(additional_exclusion)
            .filter_map(|storage| {
                let start = storage.address.max(live.address);
                let end = storage
                    .address
                    .saturating_add(storage.size)
                    .min(live.address.saturating_add(live.size));
                (start < end).then_some((start, end))
            })
            .collect::<Vec<_>>();
        excluded.sort_unstable();

        let mut saved = Vec::new();
        let mut cursor = live.address;
        for (start, end) in excluded {
            if start > cursor {
                saved.push(Storage {
                    address: cursor,
                    size: start - cursor,
                });
            }
            cursor = cursor.max(end);
        }
        let live_end = live.address.saturating_add(live.size);
        if cursor < live_end {
            saved.push(Storage {
                address: cursor,
                size: live_end - cursor,
            });
        }
        saved
    }

    fn copy(&mut self, source: Storage, target: Storage, size: u32) {
        for offset in 0..size {
            self.lda(source.address + offset);
            self.sta(target.address + offset);
        }
    }

    fn zero(&mut self, storage: Storage) {
        if self.supports_65c02() {
            for offset in 0..storage.size {
                self.line(&format!("    stz ${:04X}", storage.address + offset));
            }
        } else {
            self.lda_imm(0);
            for offset in 0..storage.size {
                self.sta(storage.address + offset);
            }
        }
    }

    fn load_constant(&mut self, value: i64, width: u8) {
        for offset in 0..u32::from(width) {
            self.lda_imm(((value as u64 >> (offset * 8)) & 0xFF) as u8);
            self.sta(self.r0.address + offset);
        }
    }

    fn extend_result(&mut self, source_width: u8, target_width: u8, signed: bool) {
        if target_width <= source_width {
            return;
        }
        if signed {
            let positive = self.next_label("extend_positive");
            let done = self.next_label("extend_done");
            self.lda(self.r0.address + u32::from(source_width - 1));
            self.branch_long("bpl", &positive);
            self.lda_imm(0xFF);
            self.line(&format!("    jmp {done}"));
            self.line(&format!("{positive}:"));
            self.lda_imm(0);
            self.line(&format!("{done}:"));
        } else {
            self.lda_imm(0);
        }
        for offset in u32::from(source_width)..u32::from(target_width) {
            self.sta(self.r0.address + offset);
        }
    }

    fn scale_storage(&mut self, storage: Storage, width: u8, scale: u32) {
        if scale <= 1 {
            return;
        }
        let source = self
            .model
            .allocate(u32::from(width))
            .expect("pointer scale source");
        let result = self
            .model
            .allocate(u32::from(width))
            .expect("pointer scale result");
        self.copy(storage, source, u32::from(width));
        self.zero(result);
        for _ in 0..scale {
            self.copy(result, self.r0, u32::from(width));
            self.copy(source, self.r1, u32::from(width));
            self.add(width);
            self.copy(self.r0, result, u32::from(width));
        }
        self.copy(result, storage, u32::from(width));
    }

    fn normalize_signed_operand(
        &mut self,
        storage: Storage,
        width: u8,
        negative: Storage,
        toggle_sign: bool,
    ) {
        let positive = self.next_label("signed_positive");
        self.lda(storage.address + u32::from(width - 1));
        self.branch_long("bpl", &positive);
        if toggle_sign {
            self.toggle(negative);
        } else {
            self.lda_imm(1);
            self.sta(negative.address);
        }
        self.negate_storage(storage, width);
        self.line(&format!("{positive}:"));
    }

    fn negate_if_flag(&mut self, storage: Storage, width: u8, flag: Storage) {
        let done = self.next_label("sign_done");
        self.jump_if_zero(flag.address, &done);
        self.negate_storage(storage, width);
        self.line(&format!("{done}:"));
    }

    fn negate_storage(&mut self, storage: Storage, width: u8) {
        for offset in 0..u32::from(width) {
            self.lda(storage.address + offset);
            self.line("    eor #$FF");
            self.sta(storage.address + offset);
        }
        self.line("    clc");
        for offset in 0..u32::from(width) {
            self.lda(storage.address + offset);
            self.line(&format!(
                "    adc #${:02X}",
                if offset == 0 { 1 } else { 0 }
            ));
            self.sta(storage.address + offset);
        }
    }

    fn toggle(&mut self, storage: Storage) {
        self.lda(storage.address);
        self.line("    eor #$01");
        self.sta(storage.address);
    }

    fn set_zp_from_storage(&mut self, zero_page: u32, storage: Storage) {
        self.lda(storage.address);
        self.sta(zero_page);
        self.lda(storage.address + 1);
        self.sta(zero_page + 1);
    }

    fn increment_zp(&mut self, zero_page: u32) {
        let done = self.next_label("pointer_incremented");
        self.line(&format!("    inc ${zero_page:02X}"));
        self.branch_long("bne", &done);
        self.line(&format!("    inc ${:02X}", zero_page + 1));
        self.line(&format!("{done}:"));
    }

    fn set_pointer(&mut self, address: u32) {
        self.lda_imm(address as u8);
        self.sta(POINTER_ZP);
        self.lda_imm((address >> 8) as u8);
        self.sta(POINTER_ZP + 1);
    }

    fn add_pointer_constant(&mut self, value: u32) {
        self.line("    clc");
        self.lda(POINTER_ZP);
        self.line(&format!("    adc #${:02X}", value as u8));
        self.sta(POINTER_ZP);
        self.lda(POINTER_ZP + 1);
        self.line(&format!("    adc #${:02X}", (value >> 8) as u8));
        self.sta(POINTER_ZP + 1);
    }

    fn copy_result_to_zp(&mut self) {
        self.lda(self.r0.address);
        self.sta(POINTER_ZP);
        self.lda(self.r0.address + 1);
        self.sta(POINTER_ZP + 1);
    }

    fn copy_zp_to_result(&mut self, width: u8) {
        self.lda(POINTER_ZP);
        self.sta(self.r0.address);
        self.lda(POINTER_ZP + 1);
        self.sta(self.r0.address + 1);
        for offset in 2..u32::from(width) {
            self.lda_imm(0);
            self.sta(self.r0.address + offset);
        }
    }

    fn load_indirect(&mut self, width: u8) {
        for offset in 0..u32::from(width) {
            self.line(&format!("    ldy #${offset:02X}"));
            self.line(&format!("    lda (${:02X}),y", POINTER_ZP));
            self.sta(self.r0.address + offset);
        }
    }

    fn increment(&mut self, storage: Storage, width: u8) {
        let done = self.next_label("increment_done");
        for offset in 0..u32::from(width) {
            self.line(&format!("    inc ${:04X}", storage.address + offset));
            self.branch_long("bne", &done);
        }
        self.line(&format!("{done}:"));
    }

    fn decrement(&mut self, storage: Storage, width: u8) {
        self.line("    sec");
        for offset in 0..u32::from(width) {
            self.lda(storage.address + offset);
            self.line(&format!(
                "    sbc #${:02X}",
                if offset == 0 { 1 } else { 0 }
            ));
            self.sta(storage.address + offset);
        }
    }

    /// Emits a false branch for `(value & bit) == 0|bit` and `!= 0|bit`.
    ///
    /// `value` is evaluated into the normal result storage before `BIT` inspects
    /// one byte, so this preserves side effects and volatile access widths.
    fn emit_masked_bit_false_branch(
        &mut self,
        condition: &Expr,
        false_label: &str,
    ) -> Result<bool, Diagnostic> {
        let Expr::Binary { left, op, right } = condition else {
            return Ok(false);
        };
        let Expr::Binary {
            left: value,
            op: BinaryOp::BitAnd,
            right: mask_expr,
        } = left.as_ref()
        else {
            return Ok(false);
        };
        if !matches!(op, BinaryOp::Eq | BinaryOp::Ne) {
            return Ok(false);
        }

        let Ok(raw_mask) = self.model.const_value(mask_expr) else {
            return Ok(false);
        };
        let Ok(raw_expected) = self.model.const_value(right) else {
            return Ok(false);
        };
        let value_ty = self.expr_type(value)?;
        let width = self.model.type_width(&value_ty)?;
        let width_mask = (1_i64 << (u32::from(width) * 8)) - 1;
        let mask = raw_mask & width_mask;
        let expected = raw_expected & width_mask;
        if !(mask as u64).is_power_of_two() || !matches!(expected, 0) && expected != mask {
            return Ok(false);
        }

        let bit_offset = mask.trailing_zeros();
        let byte_offset = bit_offset / 8;
        let byte_mask = (mask >> (byte_offset * 8)) as u8;
        self.emit_expr(value, &value_ty)?;
        if self.supports_65c02() {
            self.lda(self.r0.address + byte_offset);
            self.line(&format!("    bit #${byte_mask:02X}"));
        } else {
            self.lda_imm(byte_mask);
            self.line(&format!("    bit ${:04X}", self.r0.address + byte_offset));
        }
        let branch = if (*op == BinaryOp::Eq) == (expected == mask) {
            "beq"
        } else {
            "bne"
        };
        self.branch_long(branch, false_label);
        Ok(true)
    }

    fn emit_condition_false_branch(
        &mut self,
        condition: &Expr,
        false_label: &str,
    ) -> Result<bool, Diagnostic> {
        let Expr::Binary { left, op, right } = condition else {
            return Ok(false);
        };
        if !is_comparison(*op) {
            return Ok(false);
        }
        let operand_ty = self.expr_type(left).or_else(|_| self.expr_type(right))?;
        if self.model.type_width(&operand_ty)? != 1 || type_is_signed(&operand_ty) {
            return Ok(false);
        }

        if self.model.const_value(right).ok() == Some(0) && self.emit_byte_load_flags(left)? {
            match op {
                BinaryOp::Eq => self.branch_long("bne", false_label),
                BinaryOp::Ne => self.branch_long("beq", false_label),
                BinaryOp::Lt => self.line(&format!("    jmp {false_label}")),
                BinaryOp::Le => self.branch_long("bne", false_label),
                BinaryOp::Gt => self.branch_long("beq", false_label),
                BinaryOp::Ge => {}
                _ => unreachable!(),
            }
            return Ok(true);
        }

        let right_supported = match right.as_ref() {
            Expr::Int(_) | Expr::TypedInt(_, _) | Expr::Char(_) => true,
            Expr::Ident(name) => {
                self.model.constants.contains_key(name)
                    || self.model.type_width(&self.binding(name)?.ty)? == 1
            }
            _ => false,
        };
        if !right_supported || !self.emit_byte_load_flags(left)? {
            return Ok(false);
        }
        match right.as_ref() {
            Expr::Int(value) | Expr::TypedInt(value, _) => {
                self.line(&format!("    cmp #${:02X}", *value as u8));
            }
            Expr::Char(value) => self.line(&format!("    cmp #${:02X}", *value as u8)),
            Expr::Ident(name) => {
                if let Some(value) = self.model.constants.get(name) {
                    self.line(&format!("    cmp #${:02X}", *value as u8));
                } else {
                    let binding = self.binding(name)?;
                    if self.model.type_width(&binding.ty)? != 1 {
                        return Ok(false);
                    }
                    self.line(&format!("    cmp ${:04X}", binding.storage.address));
                }
            }
            _ => return Ok(false),
        }
        match op {
            BinaryOp::Eq => self.branch_long("bne", false_label),
            BinaryOp::Ne => self.branch_long("beq", false_label),
            BinaryOp::Lt => self.branch_long("bcs", false_label),
            BinaryOp::Ge => self.branch_long("bcc", false_label),
            BinaryOp::Le => {
                let keep = self.next_label("compare_le");
                self.branch_long("bcc", &keep);
                self.branch_long("beq", &keep);
                self.line(&format!("    jmp {false_label}"));
                self.line(&format!("{keep}:"));
            }
            BinaryOp::Gt => {
                self.branch_long("bcc", false_label);
                self.branch_long("beq", false_label);
            }
            _ => unreachable!(),
        }
        Ok(true)
    }

    fn emit_byte_load_flags(&mut self, expr: &Expr) -> Result<bool, Diagnostic> {
        match expr {
            Expr::Int(value) | Expr::TypedInt(value, _) => self.lda_imm(*value as u8),
            Expr::Bool(value) => self.lda_imm(u8::from(*value)),
            Expr::Char(value) => self.lda_imm(*value as u8),
            Expr::Ident(name) => {
                if let Some(value) = self.model.constants.get(name) {
                    self.lda_imm(*value as u8);
                } else {
                    let binding = self.binding(name)?;
                    if self.model.type_width(&binding.ty)? != 1 {
                        return Ok(false);
                    }
                    self.lda(binding.storage.address);
                }
            }
            Expr::Deref(pointer) => {
                if let Some(address) = self.constant_pointer_address(pointer) {
                    self.lda(address);
                } else {
                    let temporary = self.model.allocate(1)?;
                    if !self.emit_absolute_indexed_load(pointer, temporary.address)? {
                        return Ok(false);
                    }
                }
            }
            Expr::Cast { ty, expr } if self.model.type_width(ty)? == 1 => {
                return self.emit_byte_load_flags(expr);
            }
            _ => return Ok(false),
        }
        Ok(true)
    }

    fn jump_storage_zero(&mut self, storage: Storage, width: u8, label: &str) {
        let nonzero = self.next_label("nonzero");
        for offset in 0..u32::from(width) {
            self.jump_if_nonzero(storage.address + offset, &nonzero);
        }
        self.line(&format!("    jmp {label}"));
        self.line(&format!("{nonzero}:"));
    }

    fn jump_if_equal(&mut self, left: Storage, right: Storage, width: u8, label: &str) {
        let different = self.next_label("different");
        for offset in 0..u32::from(width) {
            self.lda(left.address + offset);
            self.line(&format!("    cmp ${:04X}", right.address + offset));
            self.branch_long("bne", &different);
        }
        self.line(&format!("    jmp {label}"));
        self.line(&format!("{different}:"));
    }

    fn jump_if_less(&mut self, left: Storage, right: Storage, width: u8, label: &str) {
        let done = self.next_label("compare_ordered");
        for offset in (0..u32::from(width)).rev() {
            self.lda(left.address + offset);
            self.line(&format!("    cmp ${:04X}", right.address + offset));
            self.branch_long("bcc", label);
            if offset != 0 {
                self.branch_long("bne", &done);
            }
        }
        self.line(&format!("{done}:"));
    }

    fn jump_if_zero(&mut self, address: u32, label: &str) {
        self.lda(address);
        self.branch_long("beq", label);
    }

    fn jump_if_nonzero(&mut self, address: u32, label: &str) {
        self.lda(address);
        self.branch_long("bne", label);
    }

    fn branch_long(&mut self, branch: &str, target: &str) {
        let skip = self.next_label("branch_skip");
        let inverse = match branch {
            "beq" => "bne",
            "bne" => "beq",
            "bcc" => "bcs",
            "bcs" => "bcc",
            "bpl" => "bmi",
            "bmi" => "bpl",
            _ => unreachable!("unsupported branch"),
        };
        self.line(&format!("    {inverse} {skip}"));
        self.line(&format!("    jmp {target}"));
        self.line(&format!("{skip}:"));
    }

    fn lda(&mut self, address: u32) {
        self.line(&format!("    lda ${address:04X}"));
    }

    fn sta(&mut self, address: u32) {
        self.line(&format!("    sta ${address:04X}"));
    }

    fn lda_imm(&mut self, value: u8) {
        self.line(&format!("    lda #${value:02X}"));
    }

    fn lda_imm_symbol(&mut self, symbol: &str) {
        self.line(&format!("    lda #{symbol}"));
    }

    fn next_label(&mut self, name: &str) -> String {
        let label = format!(".L_{}_{}", sanitize(name), self.labels);
        self.labels += 1;
        label
    }

    fn line(&mut self, line: &str) {
        self.out.push_str(line);
        self.out.push('\n');
    }
}

enum Address {
    Direct(u32),
    Indirect,
}

fn block_terminates(body: &[Stmt]) -> bool {
    body.last().is_some_and(stmt_terminates)
}

fn stmt_terminates(stmt: &Stmt) -> bool {
    match stmt {
        Stmt::Return(_) | Stmt::ReturnTwo { .. } | Stmt::Break | Stmt::Continue => true,
        Stmt::If {
            then_body,
            else_body,
            ..
        } => !else_body.is_empty() && block_terminates(then_body) && block_terminates(else_body),
        _ => false,
    }
}

fn cleanup_assembly(assembly: &str, cpu: CpuFamily) -> Result<String, Diagnostic> {
    let variant = match cpu {
        CpuFamily::Mos6502 => crate::asm::mos6502::Mos6502Variant::Nmos6502,
        CpuFamily::Cmos65C02 => crate::asm::mos6502::Mos6502Variant::Cmos65C02,
        CpuFamily::Wdc65C816 => crate::asm::mos6502::Mos6502Variant::Wdc65C816,
        CpuFamily::Ricoh2A03 => crate::asm::mos6502::Mos6502Variant::Ricoh2A03,
        _ => crate::asm::mos6502::Mos6502Variant::Nmos6502,
    };
    let mut lines = assembly.lines().map(str::to_owned).collect::<Vec<_>>();
    loop {
        let mut remove = HashSet::new();
        for (index, line) in lines.iter().enumerate() {
            let Some((mnemonic, operands)) = instruction_parts(line) else {
                continue;
            };
            if !mnemonic.eq_ignore_ascii_case("jmp") {
                continue;
            }
            let target = operands.trim();
            let next_label = lines[index + 1..]
                .iter()
                .find_map(|line| {
                    let text = line.trim();
                    if text.is_empty() || text.starts_with(';') {
                        None
                    } else {
                        Some(line_label(line))
                    }
                })
                .flatten();
            if next_label == Some(target) {
                remove.insert(index);
            }
        }
        if !remove.is_empty() {
            lines = lines
                .into_iter()
                .enumerate()
                .filter_map(|(index, line)| (!remove.contains(&index)).then_some(line))
                .collect();
            continue;
        }

        let (offsets, labels) = assembly_offsets(&lines, variant)?;
        for index in 0..lines.len().saturating_sub(2) {
            let Some((inverse, skip_operand)) = instruction_parts(&lines[index]) else {
                continue;
            };
            let Some(branch) = inverse_branch(inverse) else {
                continue;
            };
            let Some((jmp, target)) = instruction_parts(&lines[index + 1]) else {
                continue;
            };
            if !jmp.eq_ignore_ascii_case("jmp")
                || !skip_operand.trim().starts_with(".L_branch_skip_")
                || line_label(&lines[index + 2]) != Some(skip_operand.trim())
            {
                continue;
            }
            let Some(&target_offset) = labels.get(target.trim()) else {
                continue;
            };
            let branch_offset = offsets[index];
            let adjusted_target = if target_offset > branch_offset {
                target_offset.saturating_sub(3)
            } else {
                target_offset
            };
            let displacement = adjusted_target as i64 - (branch_offset + 2) as i64;
            if (-128..=127).contains(&displacement) {
                lines[index] = format!("    {branch} {}", target.trim());
                lines.remove(index + 2);
                lines.remove(index + 1);
                remove.insert(index);
                break;
            }
        }
        if remove.is_empty() {
            break;
        }
    }

    reuse_compare_at_bne_target(&mut lines);
    let mut output = lines.join("\n");
    if assembly.ends_with('\n') {
        output.push('\n');
    }
    Ok(output)
}

fn assembly_offsets(
    lines: &[String],
    variant: crate::asm::mos6502::Mos6502Variant,
) -> Result<(Vec<usize>, HashMap<String, usize>), Diagnostic> {
    let mut offsets = Vec::with_capacity(lines.len());
    let mut labels = HashMap::new();
    let mut offset = 0;
    for line in lines {
        offsets.push(offset);
        if let Some(label) = line_label(line) {
            labels.insert(label.to_owned(), offset);
        }
        if let Some((mnemonic, _)) = instruction_parts(line)
            && !mnemonic.eq_ignore_ascii_case("section")
        {
            offset +=
                crate::asm::mos6502::instruction_len_for_variant(line.trim(), variant).unwrap_or(0);
        }
    }
    Ok((offsets, labels))
}

fn inverse_branch(branch: &str) -> Option<&'static str> {
    Some(match branch.to_ascii_lowercase().as_str() {
        "beq" => "bne",
        "bne" => "beq",
        "bcc" => "bcs",
        "bcs" => "bcc",
        "bpl" => "bmi",
        "bmi" => "bpl",
        _ => return None,
    })
}

fn reuse_compare_at_bne_target(lines: &mut [String]) {
    let references = lines
        .iter()
        .filter_map(|line| instruction_parts(line))
        .filter_map(|(_, operands)| {
            let operand = operands.trim();
            operand.starts_with(".L_").then_some(operand.to_owned())
        })
        .fold(HashMap::<String, usize>::new(), |mut counts, label| {
            *counts.entry(label).or_default() += 1;
            counts
        });
    for branch_index in 2..lines.len() {
        let Some((branch, target)) = instruction_parts(&lines[branch_index]) else {
            continue;
        };
        if !branch.eq_ignore_ascii_case("bne") || references.get(target.trim()).copied() != Some(1)
        {
            continue;
        }
        let Some(label_index) = lines
            .iter()
            .position(|line| line_label(line) == Some(target.trim()))
        else {
            continue;
        };
        let Some(previous) = (0..label_index)
            .rev()
            .find(|index| instruction_parts(&lines[*index]).is_some())
        else {
            continue;
        };
        let Some((terminator, _)) = instruction_parts(&lines[previous]) else {
            continue;
        };
        if !matches!(
            terminator.to_ascii_lowercase().as_str(),
            "jmp" | "rts" | "rti"
        ) {
            continue;
        }
        let following = (label_index + 1..lines.len())
            .filter(|index| instruction_parts(&lines[*index]).is_some())
            .take(2)
            .collect::<Vec<_>>();
        if following.len() == 2
            && lines[branch_index - 2].trim() == lines[following[0]].trim()
            && lines[branch_index - 1].trim() == lines[following[1]].trim()
        {
            lines[following[0]].clear();
            lines[following[1]].clear();
        }
    }
}

fn line_label(line: &str) -> Option<&str> {
    let text = line.trim();
    let label = text.strip_suffix(':')?;
    (!label.is_empty() && !label.chars().any(char::is_whitespace)).then_some(label)
}

fn instruction_parts(line: &str) -> Option<(&str, &str)> {
    let text = line.split(';').next()?.trim();
    if text.is_empty() || line_label(text).is_some() {
        return None;
    }
    let end = text.find(char::is_whitespace).unwrap_or(text.len());
    Some((&text[..end], text[end..].trim()))
}

fn intrinsic_descriptor(path: &[String]) -> Option<&'static IntrinsicDescriptor> {
    CATALOG.lookup(&path.join("."))
}

fn contains_function_pointer_program(program: &Program) -> bool {
    let function_names = program
        .declarations
        .iter()
        .filter_map(|declaration| match unwrapped_declaration(declaration) {
            Declaration::Function(function) => Some(function.name.clone()),
            Declaration::ExternAsmFunction(function) => Some(function.name.clone()),
            _ => None,
        })
        .collect::<HashSet<_>>();
    program
        .declarations
        .iter()
        .any(|declaration| match unwrapped_declaration(declaration) {
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
                    || function_body_contains_function_pointer(&function.body, &function_names)
            }
            Declaration::Global(global) => {
                type_contains_function_pointer(&global.ty)
                    || expr_contains_function_pointer(&global.value, &function_names)
            }
            _ => false,
        })
}

fn type_contains_function_pointer(ty: &Type) -> bool {
    match ty {
        Type::Ptr(inner) | Type::Array { element: inner, .. } => {
            type_contains_function_pointer(inner)
        }
        Type::Function { .. } => true,
        Type::Named(_) => false,
    }
}

fn function_body_contains_function_pointer(
    body: &[Stmt],
    function_names: &HashSet<String>,
) -> bool {
    body.iter().any(|stmt| match stmt {
        Stmt::Let { ty, value, .. } => {
            type_contains_function_pointer(ty)
                || expr_contains_function_pointer(value, function_names)
        }
        Stmt::LetTwo {
            first_ty,
            second_ty,
            value,
            ..
        } => {
            type_contains_function_pointer(first_ty)
                || type_contains_function_pointer(second_ty)
                || expr_contains_function_pointer(value, function_names)
        }
        Stmt::Return(Some(value)) | Stmt::Expr(value) | Stmt::Out { value, .. } => {
            expr_contains_function_pointer(value, function_names)
        }
        Stmt::ReturnTwo { first, second } => {
            expr_contains_function_pointer(first, function_names)
                || expr_contains_function_pointer(second, function_names)
        }
        Stmt::Assign { value, .. } => expr_contains_function_pointer(value, function_names),
        Stmt::If {
            condition,
            then_body,
            else_body,
        } => {
            expr_contains_function_pointer(condition, function_names)
                || function_body_contains_function_pointer(then_body, function_names)
                || function_body_contains_function_pointer(else_body, function_names)
        }
        Stmt::While { condition, body } => {
            expr_contains_function_pointer(condition, function_names)
                || function_body_contains_function_pointer(body, function_names)
        }
        Stmt::Loop { body } => function_body_contains_function_pointer(body, function_names),
        Stmt::Return(None) | Stmt::Break | Stmt::Continue | Stmt::Asm { .. } => false,
    })
}

fn expr_contains_function_pointer(expr: &Expr, function_names: &HashSet<String>) -> bool {
    match expr {
        Expr::AddressOf(name) => function_names.contains(name),
        Expr::Cast { ty, expr } => {
            type_contains_function_pointer(ty)
                || expr_contains_function_pointer(expr, function_names)
        }
        Expr::Array(values) => values
            .iter()
            .any(|value| expr_contains_function_pointer(value, function_names)),
        Expr::Index { index, .. } | Expr::AddressOfIndex { index, .. } => {
            expr_contains_function_pointer(index, function_names)
        }
        Expr::Access(path) | Expr::AddressOfAccess(path) => path.segments.iter().any(|segment| {
            matches!(segment, AccessSegment::Index(index) if expr_contains_function_pointer(index, function_names))
        }),
        Expr::StructInit { fields, .. } => fields
            .iter()
            .any(|(_, value)| expr_contains_function_pointer(value, function_names)),
        Expr::Deref(value)
        | Expr::BankedPointer { pointer: value, .. }
        | Expr::Unary { expr: value, .. } => {
            expr_contains_function_pointer(value, function_names)
        }
        Expr::Call { args, .. } => args
            .iter()
            .any(|arg| expr_contains_function_pointer(arg, function_names)),
        Expr::Binary { left, right, .. } => {
            expr_contains_function_pointer(left, function_names)
                || expr_contains_function_pointer(right, function_names)
        }
        Expr::Int(_)
        | Expr::TypedInt(_, _)
        | Expr::Bool(_)
        | Expr::Char(_)
        | Expr::String(_)
        | Expr::Ident(_)
        | Expr::In(_)
        | Expr::Field { .. }
        | Expr::AddressOfField { .. } => false,
    }
}

fn contains_two_result_program(program: &Program) -> bool {
    program
        .declarations
        .iter()
        .any(|declaration| match unwrapped_declaration(declaration) {
            Declaration::Function(function) => {
                function.second_return_type.is_some()
                    || contains_two_result_statement(&function.body)
            }
            Declaration::ExternAsmFunction(function) => function.second_return_type.is_some(),
            _ => false,
        })
}

fn block_can_complete_normally(body: &[Stmt], model: &SemanticModel) -> bool {
    let mut reachable = true;
    for stmt in body {
        if !reachable {
            break;
        }
        reachable = stmt_can_complete_normally(stmt, model);
    }
    reachable
}

fn stmt_can_complete_normally(stmt: &Stmt, model: &SemanticModel) -> bool {
    match stmt {
        Stmt::Return(_) | Stmt::ReturnTwo { .. } | Stmt::Break | Stmt::Continue => false,
        Stmt::If {
            condition,
            then_body,
            else_body,
        } => match model.const_value(condition) {
            Ok(0) => block_can_complete_normally(else_body, model),
            Ok(_) => block_can_complete_normally(then_body, model),
            Err(_) => {
                block_can_complete_normally(then_body, model)
                    || block_can_complete_normally(else_body, model)
            }
        },
        Stmt::Loop { body } => block_can_break_current_loop(body, model),
        Stmt::While { condition, body } => {
            !condition_is_const_true(condition, model) || block_can_break_current_loop(body, model)
        }
        _ => true,
    }
}

fn condition_is_const_true(condition: &Expr, model: &SemanticModel) -> bool {
    matches!(condition, Expr::Bool(true))
        || matches!(condition, Expr::Ident(name) if name == "true")
        || model.const_value(condition).is_ok_and(|value| value != 0)
}

fn block_can_break_current_loop(body: &[Stmt], model: &SemanticModel) -> bool {
    let mut reachable = true;
    for stmt in body {
        if !reachable {
            break;
        }
        if stmt_can_break_current_loop(stmt, model) {
            return true;
        }
        reachable = stmt_can_complete_normally(stmt, model);
    }
    false
}

fn stmt_can_break_current_loop(stmt: &Stmt, model: &SemanticModel) -> bool {
    match stmt {
        Stmt::Break => true,
        Stmt::If {
            condition,
            then_body,
            else_body,
        } => match model.const_value(condition) {
            Ok(0) => block_can_break_current_loop(else_body, model),
            Ok(_) => block_can_break_current_loop(then_body, model),
            Err(_) => {
                block_can_break_current_loop(then_body, model)
                    || block_can_break_current_loop(else_body, model)
            }
        },
        Stmt::While { .. } | Stmt::Loop { .. } => false,
        _ => false,
    }
}

fn contains_two_result_statement(body: &[Stmt]) -> bool {
    body.iter().any(|stmt| match stmt {
        Stmt::LetTwo { .. } | Stmt::ReturnTwo { .. } => true,
        Stmt::If {
            then_body,
            else_body,
            ..
        } => contains_two_result_statement(then_body) || contains_two_result_statement(else_body),
        Stmt::While { body, .. } | Stmt::Loop { body } => contains_two_result_statement(body),
        _ => false,
    })
}

fn bit_mask(bits: u32) -> u64 {
    if bits >= 64 {
        u64::MAX
    } else if bits == 0 {
        0
    } else {
        (1_u64 << bits) - 1
    }
}

fn element_type(ty: &Type) -> Result<Type, Diagnostic> {
    match ty {
        Type::Array { element, .. } | Type::Ptr(element) => Ok((**element).clone()),
        _ => Err(Diagnostic::new("indexing requires array or pointer")),
    }
}

fn type_is_signed(ty: &Type) -> bool {
    matches!(ty, Type::Named(name) if name.starts_with('i'))
}

fn shift_cost(
    width: u8,
    byte_count: u32,
    bit_count: u32,
    right: bool,
    signed: bool,
    cmos: bool,
) -> u32 {
    let width = u32::from(width);
    let byte_moves = if byte_count == 0 {
        0
    } else {
        let copied_bytes = 2 * (width - byte_count);
        let fill_bytes = if right && signed {
            5 + byte_count
        } else if cmos {
            byte_count
        } else {
            1 + byte_count
        };
        copied_bytes + fill_bytes
    };
    let per_bit = if right && signed {
        width + 2
    } else {
        width + 1
    };
    byte_moves + bit_count * per_bit
}

#[derive(Default)]
struct CostAccumulator {
    bytes: u32,
    cycles: u32,
    temporaries: u8,
}

impl CostAccumulator {
    fn add(&mut self, cost: InstructionCost) {
        self.bytes = self.bytes.saturating_add(u32::from(cost.bytes));
        self.cycles = self.cycles.saturating_add(cost.cycles);
        self.temporaries = self.temporaries.max(cost.temporaries);
    }

    fn add_repeated(&mut self, cost: InstructionCost, count: u32) {
        self.bytes = self
            .bytes
            .saturating_add(u32::from(cost.bytes).saturating_mul(count));
        self.cycles = self
            .cycles
            .saturating_add(cost.cycles.saturating_mul(count));
        self.temporaries = self.temporaries.max(cost.temporaries);
    }

    fn finish(self) -> InstructionCost {
        InstructionCost::new(
            self.bytes.min(u32::from(u16::MAX)) as u16,
            self.cycles,
            self.temporaries,
            crate::tbir::cost::FlagEffects::none(),
        )
    }
}

fn instruction_cost(bytes: u32, cycles: u32, temporaries: u8) -> InstructionCost {
    InstructionCost::new(
        bytes.min(u32::from(u16::MAX)) as u16,
        cycles,
        temporaries,
        crate::tbir::cost::FlagEffects::none(),
    )
}

fn truncated_magnitude(width: u8, factor: i64) -> u64 {
    let magnitude = factor.unsigned_abs();
    let bits = u32::from(width) * 8;
    if bits >= 64 {
        magnitude
    } else {
        magnitude & ((1u64 << bits) - 1)
    }
}

fn constant_multiply_plans(width: u8, magnitude: u64) -> Vec<ConstantMultiplyPlan> {
    let mut plans = Vec::new();
    match magnitude {
        0 => plans.push(ConstantMultiplyPlan::Zero),
        1 => plans.push(ConstantMultiplyPlan::Identity),
        value => {
            if value.is_power_of_two() {
                plans.push(ConstantMultiplyPlan::Shift {
                    count: value.trailing_zeros(),
                });
            }
            let bits = (u32::from(width) * 8).min(63);
            for count in 2..=bits {
                let power = 1u64 << count;
                if value == power - 1 {
                    plans.push(ConstantMultiplyPlan::ShiftAdd {
                        count,
                        subtract: true,
                    });
                } else if value == power + 1 {
                    plans.push(ConstantMultiplyPlan::ShiftAdd {
                        count,
                        subtract: false,
                    });
                }
            }
            plans.push(ConstantMultiplyPlan::Horner { magnitude: value });
        }
    }
    plans.push(ConstantMultiplyPlan::Fallback);
    plans
}

fn constant_multiply_plan_cost(
    plan: ConstantMultiplyPlan,
    width: u8,
    magnitude: u64,
    negative: bool,
    signed: bool,
    preserve_left: bool,
    cmos: bool,
) -> InstructionCost {
    let mut cost = CostAccumulator::default();
    match plan {
        ConstantMultiplyPlan::Zero => cost.add(zero_cost(width, cmos)),
        ConstantMultiplyPlan::Identity => {}
        ConstantMultiplyPlan::Shift { count } => {
            cost.add(constant_shift_left_cost(width, count, cmos));
        }
        ConstantMultiplyPlan::ShiftAdd { count, subtract } => {
            cost.add(copy_cost(width));
            cost.add(constant_shift_left_cost(width, count, cmos));
            cost.add(copy_cost(width));
            cost.add(if subtract {
                sub_cost(width)
            } else {
                add_cost(width)
            });
            cost.temporaries = 1;
        }
        ConstantMultiplyPlan::Horner { magnitude } => {
            cost.add(copy_cost(width));
            cost.temporaries = 1;
            for bit in (0..magnitude.ilog2()).rev() {
                cost.add(shift_once_left_cost(width));
                if magnitude & (1u64 << bit) != 0 {
                    cost.add(copy_cost(width));
                    cost.add(add_cost(width));
                }
            }
        }
        ConstantMultiplyPlan::Fallback => {
            cost.add(fallback_multiply_cost(
                width,
                magnitude,
                signed,
                preserve_left,
                cmos,
            ));
        }
    }
    if negative
        && !matches!(
            plan,
            ConstantMultiplyPlan::Zero | ConstantMultiplyPlan::Fallback
        )
    {
        cost.add(negate_cost(width));
    }
    cost.finish()
}

fn fallback_multiply_cost(
    width: u8,
    magnitude: u64,
    signed: bool,
    preserve_left: bool,
    cmos: bool,
) -> InstructionCost {
    let mut cost = CostAccumulator::default();
    if preserve_left {
        cost.add(copy_cost(width));
    }
    cost.add(load_constant_cost(width));
    cost.add(copy_cost(width));
    if preserve_left {
        cost.add(copy_cost(width));
    }

    if width == 2 && !signed {
        // The helper is emitted once when selected. Include its code and its
        // fixed 16-iteration run so a long constant chain does not win only
        // because the helper call itself is short.
        cost.add(instruction_cost(3, 6, 0));
        cost.add(instruction_cost(64, 800, 0));
    } else {
        cost.add(generic_multiply_cost(width, magnitude, signed, cmos));
    }
    cost.finish()
}

fn generic_multiply_cost(width: u8, magnitude: u64, signed: bool, cmos: bool) -> InstructionCost {
    let mut cost = CostAccumulator::default();
    if signed {
        cost.add(zero_cost(1, cmos));
        cost.add(signed_normalize_cost(width, false));
        cost.add(signed_normalize_cost(width, true));
    }
    cost.add(copy_cost(width));
    cost.add(copy_cost(width));
    cost.add(zero_cost(width, cmos));

    let check = instruction_cost(8, 9, 0);
    let body = {
        let mut body = CostAccumulator::default();
        body.add(copy_cost(width));
        body.add(add_cost(width));
        body.add(decrement_cost(width));
        body.add(instruction_cost(3, 3, 0));
        body.finish()
    };
    let zero_check = {
        let mut check_cost = CostAccumulator::default();
        check_cost.add_repeated(check, u32::from(width));
        check_cost.add(instruction_cost(3, 3, 0));
        check_cost.finish()
    };
    let iterations = magnitude.min(u64::from(u32::MAX)) as u32;
    cost.add(zero_check);
    cost.add(body);
    cost.cycles = cost.cycles.saturating_add(
        body.cycles
            .saturating_mul(iterations)
            .saturating_add(zero_check.cycles.saturating_mul(iterations)),
    );
    if signed {
        cost.add(signed_negate_if_flag_cost(width));
        cost.temporaries = 3;
    } else {
        cost.temporaries = 2;
    }
    cost.finish()
}

fn signed_normalize_cost(width: u8, toggle: bool) -> InstructionCost {
    let mut cost = CostAccumulator::default();
    cost.add(instruction_cost(8, 9, 0));
    cost.add(if toggle {
        instruction_cost(8, 10, 0)
    } else {
        instruction_cost(5, 6, 0)
    });
    cost.add(negate_cost(width));
    cost.finish()
}

fn signed_negate_if_flag_cost(width: u8) -> InstructionCost {
    let mut cost = CostAccumulator::default();
    cost.add(instruction_cost(8, 9, 0));
    cost.add(negate_cost(width));
    cost.finish()
}

fn zero_cost(width: u8, cmos: bool) -> InstructionCost {
    let width = u32::from(width);
    if cmos {
        instruction_cost(3 * width, 4 * width, 0)
    } else {
        instruction_cost(2 + 3 * width, 2 + 4 * width, 0)
    }
}

fn load_constant_cost(width: u8) -> InstructionCost {
    let width = u32::from(width);
    instruction_cost(5 * width, 6 * width, 0)
}

fn copy_cost(width: u8) -> InstructionCost {
    let width = u32::from(width);
    instruction_cost(6 * width, 8 * width, 0)
}

fn add_cost(width: u8) -> InstructionCost {
    let width = u32::from(width);
    instruction_cost(1 + 9 * width, 2 + 12 * width, 0)
}

fn sub_cost(width: u8) -> InstructionCost {
    add_cost(width)
}

fn decrement_cost(width: u8) -> InstructionCost {
    let width = u32::from(width);
    instruction_cost(1 + 8 * width, 2 + 10 * width, 0)
}

fn negate_cost(width: u8) -> InstructionCost {
    let width = u32::from(width);
    instruction_cost(1 + 13 * width, 2 + 16 * width, 0)
}

fn shift_once_left_cost(width: u8) -> InstructionCost {
    let width = u32::from(width);
    instruction_cost(1 + 3 * width, 2 + 6 * width, 0)
}

fn constant_shift_left_cost(width: u8, count: u32, cmos: bool) -> InstructionCost {
    let bits = u32::from(width) * 8;
    let count = count.min(bits);
    let byte_count = count / 8;
    let bit_count = count % 8;
    let bytewise_cost = shift_cost(width, byte_count, bit_count, false, false, cmos);
    let carry_cost = shift_cost(width, 0, count, false, false, cmos);
    let mut cost = CostAccumulator::default();
    if byte_count != 0 && bytewise_cost < carry_cost {
        cost.add(copy_cost((u32::from(width) - byte_count) as u8));
        cost.add(zero_cost(byte_count as u8, cmos));
        cost.add_repeated(shift_once_left_cost(width), bit_count);
    } else {
        cost.add_repeated(shift_once_left_cost(width), count);
    }
    cost.finish()
}

fn is_comparison(op: BinaryOp) -> bool {
    matches!(
        op,
        BinaryOp::Eq | BinaryOp::Ne | BinaryOp::Lt | BinaryOp::Le | BinaryOp::Gt | BinaryOp::Ge
    )
}

const MOS6502_MEMORY_LOCAL_CLASS: RegClass = RegClass(0);
const MOS6502_STATIC_SPILL_CLASS: SpillClassId = SpillClassId(0);

fn mos6502_local_target() -> Target {
    Target {
        units: ["A", "X", "Y"].into_iter().map(RegisterUnit::new).collect(),
        registers: vec![
            PhysicalRegister::new("A", vec![RegUnit(0)]),
            PhysicalRegister::new("X", vec![RegUnit(1)]),
            PhysicalRegister::new("Y", vec![RegUnit(2)]),
        ],
        register_classes: vec![
            RegisterClass::new("memory-only", vec![]),
            RegisterClass::new("accumulator", vec![PhysReg(0)]),
            RegisterClass::new("index", vec![PhysReg(1), PhysReg(2)]),
        ],
        spill_classes: vec![
            SpillClass::new("static-bytes", None, 1)
                .with_base_alignment(1)
                .for_register_classes(vec![MOS6502_MEMORY_LOCAL_CLASS]),
        ],
    }
}

fn plan_static_locals(
    function: &Function,
    model: &mut SemanticModel,
) -> Result<HashMap<String, Binding>, Diagnostic> {
    let mut locals = Vec::new();
    let mut local_types = HashMap::new();
    collect_static_locals(&function.body, model, &mut locals, &mut local_types)?;
    let planned = allocate_source_locals(&mos6502_local_target(), &locals, &function.body, &[])
        .map_err(|diagnostics| {
            Diagnostic::new(format!(
                "MOS 6502 local allocation failed: {}",
                diagnostics
                    .iter()
                    .map(ToString::to_string)
                    .collect::<Vec<_>>()
                    .join("; ")
            ))
        })?;
    let backing_size = planned
        .allocation
        .spill_slots
        .iter()
        .map(|slot| slot.offset.saturating_add(slot.size))
        .max()
        .unwrap_or(0);
    let backing = (backing_size != 0)
        .then(|| model.allocate(backing_size))
        .transpose()?;
    let mut bindings = HashMap::new();
    for (name, ty) in local_types {
        let vreg = planned.locals.vreg(&name).ok_or_else(|| {
            Diagnostic::new(format!("missing MOS 6502 local allocation for `{name}`"))
        })?;
        let slot_index = match planned.allocation.location(vreg) {
            Some(Location::Spill(slot_index)) => slot_index,
            Some(Location::Register(_)) => {
                return Err(Diagnostic::new(format!(
                    "MOS 6502 local `{name}` was assigned a register"
                )));
            }
            Some(Location::Unused) | None => {
                return Err(Diagnostic::new(format!(
                    "MOS 6502 local `{name}` has no storage allocation"
                )));
            }
        };
        let slot = planned
            .allocation
            .spill_slots
            .get(slot_index)
            .ok_or_else(|| Diagnostic::new(format!("invalid spill slot for local `{name}`")))?;
        if slot.class != MOS6502_STATIC_SPILL_CLASS {
            return Err(Diagnostic::new(format!(
                "invalid spill class for MOS 6502 local `{name}`"
            )));
        }
        let backing = backing.ok_or_else(|| {
            Diagnostic::new(format!("missing static backing storage for local `{name}`"))
        })?;
        bindings.insert(
            name,
            Binding {
                storage: Storage {
                    address: backing.address + slot.offset,
                    size: model.type_size(&ty)?,
                },
                ty,
            },
        );
    }
    Ok(bindings)
}

fn collect_static_locals(
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
                locals.push(
                    SourceLocal::new(
                        name.clone(),
                        model.type_size(ty)?,
                        1,
                        MOS6502_MEMORY_LOCAL_CLASS,
                    )
                    .with_spill_classes(vec![MOS6502_STATIC_SPILL_CLASS])
                    .with_force_memory(true),
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
                    locals.push(
                        SourceLocal::new(
                            name.clone(),
                            model.type_size(ty)?,
                            1,
                            MOS6502_MEMORY_LOCAL_CLASS,
                        )
                        .with_spill_classes(vec![MOS6502_STATIC_SPILL_CLASS])
                        .with_force_memory(true),
                    );
                }
            }
            Stmt::If {
                then_body,
                else_body,
                ..
            } => {
                collect_static_locals(then_body, model, locals, local_types)?;
                collect_static_locals(else_body, model, locals, local_types)?;
            }
            Stmt::While { body, .. } | Stmt::Loop { body } => {
                collect_static_locals(body, model, locals, local_types)?;
            }
            _ => {}
        }
    }
    Ok(())
}

fn assign_binary(op: AssignOp) -> BinaryOp {
    match op {
        AssignOp::Add => BinaryOp::Add,
        AssignOp::Sub => BinaryOp::Sub,
        AssignOp::Mul => BinaryOp::Mul,
        AssignOp::Div => BinaryOp::Div,
        AssignOp::Mod => BinaryOp::Mod,
        AssignOp::BitAnd => BinaryOp::BitAnd,
        AssignOp::BitOr => BinaryOp::BitOr,
        AssignOp::BitXor => BinaryOp::BitXor,
        AssignOp::Shl => BinaryOp::Shl,
        AssignOp::Shr => BinaryOp::Shr,
        AssignOp::Set => unreachable!(),
    }
}

fn recursive_call_edges(program: &Program, model: &SemanticModel) -> HashSet<(String, String)> {
    let mut graph = HashMap::new();
    for declaration in &program.declarations {
        if let Declaration::Function(function) = declaration {
            let mut calls = Vec::new();
            collect_stmt_calls(&function.body, &mut calls);
            graph.insert(
                function.name.clone(),
                calls
                    .into_iter()
                    .filter_map(|path| resolve_called_function(&path, model))
                    .collect::<Vec<_>>(),
            );
        }
    }

    let mut recursive = HashSet::new();
    for (caller, callees) in &graph {
        for callee in callees {
            if function_reaches(callee, caller, &graph) {
                recursive.insert((caller.clone(), callee.clone()));
            }
        }
    }
    recursive
}

fn function_reaches(start: &str, destination: &str, graph: &HashMap<String, Vec<String>>) -> bool {
    let mut visited = HashSet::new();
    let mut pending = vec![start];
    while let Some(function) = pending.pop() {
        if function == destination {
            return true;
        }
        if !visited.insert(function) {
            continue;
        }
        if let Some(callees) = graph.get(function) {
            pending.extend(callees.iter().map(String::as_str));
        }
    }
    false
}

fn reachable_function_names(program: &Program, model: &SemanticModel) -> HashSet<String> {
    let mut graph = HashMap::new();
    let mut roots = vec!["main".to_owned()];

    for declaration in &program.declarations {
        match declaration {
            Declaration::Function(function) => {
                let mut calls = Vec::new();
                collect_stmt_calls(&function.body, &mut calls);
                graph.insert(
                    function.name.clone(),
                    calls
                        .into_iter()
                        .filter_map(|path| resolve_called_function(&path, model))
                        .collect::<Vec<_>>(),
                );
                if function
                    .attrs
                    .iter()
                    .any(|attr| attr == "naked" || attr == "interrupt")
                {
                    roots.push(function.name.clone());
                }
            }
            Declaration::Global(global) => {
                let mut calls = Vec::new();
                collect_expr_calls(&global.value, &mut calls);
                roots.extend(
                    calls
                        .into_iter()
                        .filter_map(|path| resolve_called_function(&path, model)),
                );
            }
            _ => {}
        }
    }

    let mut reachable = HashSet::new();
    let mut pending = roots;
    while let Some(name) = pending.pop() {
        if !reachable.insert(name.clone()) {
            continue;
        }
        if let Some(calls) = graph.get(&name) {
            pending.extend(calls.iter().cloned());
        }
    }
    reachable
}

fn resolve_called_function(path: &[String], model: &SemanticModel) -> Option<String> {
    let qualified = path.join(".");
    if model.functions.contains_key(&qualified) {
        Some(qualified)
    } else {
        path.last()
            .filter(|name| model.functions.contains_key(*name))
            .cloned()
    }
}

fn function_pointer_references(program: &Program, model: &SemanticModel) -> HashSet<String> {
    let mut references = HashSet::new();
    for declaration in &program.declarations {
        match unwrapped_declaration(declaration) {
            Declaration::Function(function) => {
                collect_stmt_function_references(&function.body, &mut references);
            }
            Declaration::Global(global) => {
                collect_expr_function_references(&global.value, &mut references);
            }
            _ => {}
        }
    }
    references.retain(|name| model.functions.contains_key(name));
    references
}

fn collect_stmt_function_references(stmts: &[Stmt], references: &mut HashSet<String>) {
    for stmt in stmts {
        match stmt {
            Stmt::Let { value, .. }
            | Stmt::LetTwo { value, .. }
            | Stmt::Return(Some(value))
            | Stmt::Expr(value)
            | Stmt::Out { value, .. } => collect_expr_function_references(value, references),
            Stmt::ReturnTwo { first, second } => {
                collect_expr_function_references(first, references);
                collect_expr_function_references(second, references);
            }
            Stmt::Assign { value, .. } => collect_expr_function_references(value, references),
            Stmt::If {
                condition,
                then_body,
                else_body,
            } => {
                collect_expr_function_references(condition, references);
                collect_stmt_function_references(then_body, references);
                collect_stmt_function_references(else_body, references);
            }
            Stmt::While { condition, body } => {
                collect_expr_function_references(condition, references);
                collect_stmt_function_references(body, references);
            }
            Stmt::Loop { body } => collect_stmt_function_references(body, references),
            Stmt::Return(None) | Stmt::Break | Stmt::Continue | Stmt::Asm { .. } => {}
        }
    }
}

fn collect_expr_function_references(expr: &Expr, references: &mut HashSet<String>) {
    match expr {
        Expr::AddressOf(name) => {
            references.insert(name.clone());
        }
        Expr::Array(values) => {
            for value in values {
                collect_expr_function_references(value, references);
            }
        }
        Expr::Index { index, .. } | Expr::AddressOfIndex { index, .. } => {
            collect_expr_function_references(index, references);
        }
        Expr::Access(path) | Expr::AddressOfAccess(path) => {
            for segment in &path.segments {
                if let AccessSegment::Index(index) = segment {
                    collect_expr_function_references(index, references);
                }
            }
        }
        Expr::StructInit { fields, .. } => {
            for (_, value) in fields {
                collect_expr_function_references(value, references);
            }
        }
        Expr::Deref(value)
        | Expr::BankedPointer { pointer: value, .. }
        | Expr::Unary { expr: value, .. }
        | Expr::Cast { expr: value, .. } => collect_expr_function_references(value, references),
        Expr::Call { args, .. } => {
            for arg in args {
                collect_expr_function_references(arg, references);
            }
        }
        Expr::Binary { left, right, .. } => {
            collect_expr_function_references(left, references);
            collect_expr_function_references(right, references);
        }
        Expr::Int(_)
        | Expr::TypedInt(_, _)
        | Expr::Bool(_)
        | Expr::Char(_)
        | Expr::String(_)
        | Expr::Ident(_)
        | Expr::In(_)
        | Expr::Field { .. }
        | Expr::AddressOfField { .. } => {}
    }
}

fn collect_stmt_calls(stmts: &[Stmt], calls: &mut Vec<Vec<String>>) {
    for stmt in stmts {
        match stmt {
            Stmt::Let { value, .. } | Stmt::LetTwo { value, .. } | Stmt::Expr(value) => {
                collect_expr_calls(value, calls);
            }
            Stmt::Return(Some(value)) => collect_expr_calls(value, calls),
            Stmt::ReturnTwo { first, second } => {
                collect_expr_calls(first, calls);
                collect_expr_calls(second, calls);
            }
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
                collect_stmt_calls(then_body, calls);
                collect_stmt_calls(else_body, calls);
            }
            Stmt::While { condition, body } => {
                collect_expr_calls(condition, calls);
                collect_stmt_calls(body, calls);
            }
            Stmt::Loop { body } => collect_stmt_calls(body, calls),
            Stmt::Out { value, .. } => collect_expr_calls(value, calls),
            Stmt::Return(None) | Stmt::Break | Stmt::Continue | Stmt::Asm { .. } => {}
        }
    }
}

fn collect_place_calls(place: &Place, calls: &mut Vec<Vec<String>>) {
    match place {
        Place::Index { index, .. } | Place::Deref(index) => collect_expr_calls(index, calls),
        Place::Access(path) => collect_access_path_calls(path, calls),
        Place::Ident(_) | Place::Field { .. } => {}
    }
}

fn collect_expr_calls(expr: &Expr, calls: &mut Vec<Vec<String>>) {
    match expr {
        Expr::Array(values) => {
            for value in values {
                collect_expr_calls(value, calls);
            }
        }
        Expr::Index { index, .. } | Expr::AddressOfIndex { index, .. } => {
            collect_expr_calls(index, calls);
        }
        Expr::Access(path) | Expr::AddressOfAccess(path) => collect_access_path_calls(path, calls),
        Expr::StructInit { fields, .. } => {
            for (_, value) in fields {
                collect_expr_calls(value, calls);
            }
        }
        Expr::Deref(value)
        | Expr::BankedPointer { pointer: value, .. }
        | Expr::Unary { expr: value, .. }
        | Expr::Cast { expr: value, .. } => {
            collect_expr_calls(value, calls);
        }
        Expr::Call { path, args } => {
            calls.push(path.clone());
            for arg in args {
                collect_expr_calls(arg, calls);
            }
        }
        Expr::Binary { left, right, .. } => {
            collect_expr_calls(left, calls);
            collect_expr_calls(right, calls);
        }
        Expr::AddressOf(name) => calls.push(vec![name.clone()]),
        Expr::Int(_)
        | Expr::TypedInt(_, _)
        | Expr::Bool(_)
        | Expr::Char(_)
        | Expr::String(_)
        | Expr::Ident(_)
        | Expr::In(_)
        | Expr::Field { .. }
        | Expr::AddressOfField { .. } => {}
    }
}

fn collect_access_path_calls(path: &AccessPath, calls: &mut Vec<Vec<String>>) {
    for segment in &path.segments {
        if let AccessSegment::Index(index) = segment {
            collect_expr_calls(index, calls);
        }
    }
}

fn inline_void_body(function: &Function) -> Option<&[Stmt]> {
    if !function.params.is_empty()
        || function.return_type.is_some()
        || function.second_return_type.is_some()
        || function
            .attrs
            .iter()
            .any(|attr| attr == "naked" || attr == "interrupt")
    {
        return None;
    }
    let body = if matches!(function.body.last(), Some(Stmt::Return(None))) {
        &function.body[..function.body.len() - 1]
    } else {
        &function.body
    };
    body.iter()
        .all(|stmt| matches!(stmt, Stmt::Expr(_)))
        .then_some(body)
}

fn mos6502_small_wrapper_candidate(function: &Function) -> bool {
    if !function.attrs.is_empty() {
        return false;
    }
    let Some(body) = inline_void_body(function) else {
        return false;
    };
    body.is_empty()
        || matches!(
            body,
            [Stmt::Expr(Expr::Call { args, .. })] if args.is_empty()
        )
}

fn function_label(name: &str) -> String {
    format!("_{}", sanitize(name))
}

fn function_pointer_label(name: &str) -> String {
    format!("__ezra_fn_ptr_{}", sanitize(&name.replace('.', "__")))
}

fn sanitize(name: &str) -> String {
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
mod structural_tests {
    use std::path::Path;

    use crate::{asm::AssemblyOptions, parser::parse_program, target::CpuFamily};

    use super::*;

    fn emit_for_cpu_result(
        source: &str,
        cpu: CpuFamily,
    ) -> Result<String, crate::diagnostic::Diagnostic> {
        let program = parse_program(Path::new("test.ezra"), source).unwrap();
        emit_mos6502_assembly_with_options(
            &program,
            AssemblyOptions {
                cpu,
                ram_base: crate::target::Address24::new(0xA000),
                rodata_base: crate::target::Address24::new(0x8000),
                asset_base: crate::target::Address24::new(0xC000),
                default_sdk_symbols: false,
                ..AssemblyOptions::default()
            },
        )
    }

    fn emit_for_cpu(source: &str, cpu: CpuFamily) -> String {
        emit_for_cpu_result(source, cpu).unwrap()
    }

    fn emit(source: &str) -> String {
        emit_for_cpu(source, CpuFamily::Mos6502)
    }

    fn planned_main_locals(source: &str) -> (HashMap<String, Binding>, u32) {
        let program = parse_program(Path::new("mos6502-locals.ezra"), source).unwrap();
        let options = AssemblyOptions {
            cpu: CpuFamily::Mos6502,
            ram_base: crate::target::Address24::new(0xA000),
            rodata_base: crate::target::Address24::new(0x8000),
            asset_base: crate::target::Address24::new(0xC000),
            default_sdk_symbols: false,
            ..AssemblyOptions::default()
        };
        let mut model = SemanticModel::from_program(
            &program,
            16,
            options.ram_base.get(),
            options.rodata_base.get(),
            options.asset_base.get(),
        )
        .unwrap();
        let start = model.next_ram_address();
        let bindings = plan_static_locals(program.main_function().unwrap(), &mut model).unwrap();
        (bindings, model.next_ram_address() - start)
    }

    #[test]
    fn rejects_banked_declarations_instead_of_omitting_them() {
        let source = "@cfg(bank(2)) fn banked() {}\nfn main() { banked() }";
        for cpu in [
            CpuFamily::Mos6502,
            CpuFamily::Cmos65C02,
            CpuFamily::Wdc65C816,
            CpuFamily::Ricoh2A03,
        ] {
            let error = emit_for_cpu_result(source, cpu).unwrap_err();
            assert!(
                error
                    .message
                    .contains("do not support banked declaration placement"),
                "{error}"
            );
        }
    }

    #[test]
    fn uses_24_bit_semantic_pointers_for_65c816_addresses() {
        let assembly = emit_for_cpu(
            r#"
                global observed: u24 = 0
                volatile mmio far: ptr<u8> = 0x123456
                fn main() {
                    let pointer: ptr<u8> = 0x123456
                    observed = pointer
                    *far = 0x42
                }
            "#,
            CpuFamily::Wdc65C816,
        );

        assert!(assembly.contains("    lda #$12"), "{assembly}");
        assert!(assembly.contains("    sta $123456"), "{assembly}");
    }

    #[test]
    fn local_target_models_distinct_registers_and_memory_only_source_class() {
        let target = mos6502_local_target();
        assert!(
            target.register_classes[MOS6502_MEMORY_LOCAL_CLASS.0]
                .registers
                .is_empty()
        );
        assert!(!target.registers_alias(PhysReg(0), PhysReg(1)));
        assert!(!target.registers_alias(PhysReg(0), PhysReg(2)));
        assert!(!target.registers_alias(PhysReg(1), PhysReg(2)));
        assert_eq!(
            target.spill_classes[MOS6502_STATIC_SPILL_CLASS.0].base_alignment,
            1
        );
    }

    #[test]
    fn static_local_plan_reuses_only_nonoverlapping_storage() {
        let (reused, reused_bytes) = planned_main_locals(
            "global result: u8 = 0; fn main() { let first: u8 = 1; result = first; let second: u8 = 2; result = second }",
        );
        assert_eq!(
            reused["first"].storage.address,
            reused["second"].storage.address
        );
        assert_eq!(reused_bytes, 1);

        let (overlapping, overlapping_bytes) = planned_main_locals(
            "global result: u8 = 0; fn main() { let first: u8 = 1; let second: u8 = 2; result = first + second }",
        );
        assert_ne!(
            overlapping["first"].storage.address,
            overlapping["second"].storage.address
        );
        assert_eq!(overlapping_bytes, 2);
        emit(
            "global result: u8 = 0; fn main() { let first: u8 = 1; let second: u8 = 2; result = first + second }",
        );
    }

    #[test]
    fn emits_only_demanded_bounded_u16_runtime_helpers() {
        let no_helpers = emit("fn main() { let value: u16 = 1 }");
        assert!(!no_helpers.contains(U16_MUL_HELPER), "{no_helpers}");
        assert!(!no_helpers.contains(U16_DIVMOD_HELPER), "{no_helpers}");

        let multiply = emit(
            r#"
                global product: u16 = 0
                fn multiply(left: u16, right: u16) -> u16 { return left * right }
                fn main() { product = multiply(0xFFFF, 0x1234) }
            "#,
        );
        assert_eq!(multiply.matches(&format!("{U16_MUL_HELPER}:")).count(), 1);
        assert!(
            multiply.contains(&format!("    jsr {U16_MUL_HELPER}")),
            "{multiply}"
        );
        assert!(multiply.contains("    ldx #$10"), "{multiply}");
        assert!(!multiply.contains(U16_DIVMOD_HELPER), "{multiply}");

        let divide_and_remainder = emit(
            r#"
                global quotient: u16 = 0
                global remainder: u16 = 0
                fn divide(dividend: u16, divisor: u16) -> u16 { return dividend / divisor }
                fn remainder_of(dividend: u16, divisor: u16) -> u16 { return dividend % divisor }
                fn main() {
                    quotient = divide(0xFFFF, 251)
                    remainder = remainder_of(0xFFFF, 251)
                }
            "#,
        );
        assert_eq!(
            divide_and_remainder
                .matches(&format!("{U16_DIVMOD_HELPER}:"))
                .count(),
            1
        );
        assert_eq!(
            divide_and_remainder
                .matches(&format!("    jsr {U16_DIVMOD_HELPER}"))
                .count(),
            2
        );
        assert!(
            divide_and_remainder.contains("__ezra_u16_divmod_loop:\n"),
            "{divide_and_remainder}"
        );
        assert!(
            divide_and_remainder.contains("    bne __ezra_u16_divmod_next\n    inc $"),
            "{divide_and_remainder}"
        );
    }

    #[test]
    fn selects_shift_add_instructions_for_small_constant_multiplication() {
        let assembly = emit(
            r#"
                global value: u16 = 0x1234
                global times_three: u16 = 0
                global times_seven: u16 = 0
                global times_eight: u16 = 0
                global times_ten: u16 = 0
                global negative: i16 = 0
                global compound: u16 = 0
                fn main() {
                    times_three = value * 3
                    times_seven = value * 7
                    times_eight = value * 8
                    times_ten = value * 10
                    let signed: i16 = -1234
                    negative = signed * -7
                    compound = value
                    compound *= 5
                }
            "#,
        );

        assert!(!assembly.contains(U16_MUL_HELPER), "{assembly}");
        assert!(!assembly.contains("mul_loop"), "{assembly}");
        assert!(assembly.contains("    adc $"), "{assembly}");
        assert!(assembly.contains("    sbc $"), "{assembly}");
        assert!(assembly.contains("    rol $"), "{assembly}");
        crate::vm::assemble_subset_with_symbols_at(
            crate::target::AssemblerCpu::Mos6502,
            &assembly,
            0x0200,
        )
        .unwrap();
    }

    #[test]
    fn costed_constant_multiplication_accounts_for_cpu_variant() {
        let source = r#"
            global value: u16 = 0x1234
            global times_seven: u16 = 0
            global times_eight: u16 = 0
            global byte_aligned: u16 = 0
            fn main() {
                times_seven = value * 7
                times_eight = value * 8
                byte_aligned = value * 256
            }
        "#;
        let nmos = emit_for_cpu(source, CpuFamily::Mos6502);
        let cmos = emit_for_cpu(source, CpuFamily::Cmos65C02);

        for (assembly, cpu) in [
            (&nmos, crate::target::AssemblerCpu::Mos6502),
            (&cmos, crate::target::AssemblerCpu::Cmos65C02),
        ] {
            assert!(!assembly.contains(U16_MUL_HELPER), "{assembly}");
            assert!(!assembly.contains("mul_loop"), "{assembly}");
            assert!(assembly.contains("    sbc $"), "{assembly}");
            assert!(assembly.contains("    rol $"), "{assembly}");
            crate::vm::assemble_subset_with_symbols_at(cpu, assembly, 0x0200)
                .unwrap_or_else(|error| panic!("{error}\n{assembly}"));
        }
        assert!(!nmos.contains("    stz $"), "{nmos}");
        assert!(cmos.contains("    stz $"), "{cmos}");

        let large = emit(
            r#"
                global value: u16 = 0x1234
                global product: u16 = 0
                fn main() { product = value * 0xFFFE }
            "#,
        );
        assert!(
            large.contains(&format!("    jsr {U16_MUL_HELPER}")),
            "{large}"
        );
        assert_eq!(large.matches(&format!("{U16_MUL_HELPER}:")).count(), 1);
    }

    #[test]
    fn selects_fixed_instructions_for_constant_shifts() {
        let assembly = emit(
            r#"
                global left: u24 = 0
                global right: u24 = 0
                global sign_fill: i16 = 0
                fn main() {
                    let value: u24 = 0x123456
                    left = value << 12
                    right = value >> 16
                    let signed: i16 = -2
                    sign_fill = signed >> 16
                }
            "#,
        );

        assert!(!assembly.contains("shift_loop"), "{assembly}");
        assert!(!assembly.contains("shift_done"), "{assembly}");
        assert!(
            assembly.matches("    rol $").count() + assembly.matches("    ror $").count() >= 3,
            "{assembly}"
        );
        assert!(assembly.contains("    eor #$FF"), "{assembly}");
    }

    #[test]
    fn selects_costed_byte_boundary_moves_and_short_carry_chains() {
        let assembly = emit(
            r#"
                global boundary_left: u24 = 0
                global boundary_right: u24 = 0
                global near_left: u24 = 0
                global near_right: u24 = 0
                fn main() {
                    let value: u24 = 0x123456
                    boundary_left = value << 8
                    boundary_right = value >> 8
                    near_left = value << 15
                    near_right = value >> 15
                }
            "#,
        );

        assert!(assembly.contains("    rol $"), "{assembly}");
        assert!(assembly.contains("    lsr $"), "{assembly}");
        assert!(assembly.contains("    ror $"), "{assembly}");
        assert!(!assembly.contains("    clc\n    ror $"), "{assembly}");
    }

    #[test]
    fn four_byte_scratch_slots_do_not_overlap_u32_i32_operations() {
        let program = parse_program(Path::new("mos6502-scratch.ezra"), "fn main() {}").unwrap();
        let options = AssemblyOptions {
            cpu: CpuFamily::Mos6502,
            stack_top: crate::target::Address24::new(0x01FF),
            ram_base: crate::target::Address24::new(0xA000),
            rodata_base: crate::target::Address24::new(0x8000),
            asset_base: crate::target::Address24::new(0xC000),
            default_sdk_symbols: false,
            ..AssemblyOptions::default()
        };
        let hir = HirProgram::from_ast(&program).unwrap();
        let tbir = TbirProgram::lower(&hir, &program, &options).unwrap();
        let model = SemanticModel::from_program(
            &tbir.lowered_program,
            16,
            options.ram_base.get(),
            options.rodata_base.get(),
            options.asset_base.get(),
        )
        .unwrap();
        let mut emitter = Emitter::new(model, options);

        assert_eq!(emitter.r0.size, 4);
        assert_eq!(emitter.r1.size, 4);
        assert_eq!(emitter.r2.size, 4);
        assert!(emitter.r0.address + emitter.r0.size <= emitter.r1.address);
        assert!(emitter.r1.address + emitter.r1.size <= emitter.r2.address);

        emitter.add(4);
        emitter.sub(4);
        for offset in 0..4 {
            let r0_byte = format!("${:04X}", emitter.r0.address + offset);
            let r1_byte = format!("${:04X}", emitter.r1.address + offset);
            assert!(
                emitter.out.contains(&r0_byte),
                "missing u32/i32 lhs byte {offset}:\n{}",
                emitter.out
            );
            assert!(
                emitter.out.contains(&r1_byte),
                "missing u32/i32 rhs byte {offset}:\n{}",
                emitter.out
            );
        }
    }

    #[test]
    fn emits_cmos_bit_operations_only_for_a_cmos_target() {
        let source = r#"
            fn main() {
                let zero: u8 = 1
                zero = 0
                let bits: u8 = 0
                bits &= 0xFE
                bits |= 1
                if (bits & 0x80) == 0 { bits = 0 }
            }
        "#;
        let nmos = emit(source);
        let cmos = emit_for_cpu(source, CpuFamily::Cmos65C02);

        for mnemonic in ["stz ", "trb ", "tsb ", "bit #"] {
            assert!(!nmos.contains(mnemonic), "{mnemonic} in:\n{nmos}");
        }
        assert!(cmos.contains("    stz $"), "{cmos}");
        assert!(cmos.contains("    trb $"), "{cmos}");
        assert!(cmos.contains("    tsb $"), "{cmos}");
        assert!(cmos.contains("    bit #$80"), "{cmos}");
        crate::vm::assemble_subset_with_symbols_at(
            crate::target::AssemblerCpu::Cmos65C02,
            &cmos,
            0x0200,
        )
        .unwrap_or_else(|error| panic!("{error}\n{cmos}"));
    }

    #[test]
    fn lowers_multibyte_immediate_bitwise_operations_for_nmos_and_cmos() {
        let source = r#"
            fn keep_low(value: u16) -> u16 { return value & 0x00FFu16 }
            fn set_high(value: u16) -> u16 { return value | 0xFF00u16 }
            fn toggle_low(value: u16) -> u16 { return value ^ 0x00FFu16 }
            fn partial_and(value: u16) -> u16 { return value & 0x0FFFu16 }
            fn partial_or(value: u16) -> u16 { return value | 0x1200u16 }
            fn main() {
                let kept: u16 = keep_low(0x1234)
                let set: u16 = set_high(0x1234)
                let toggled: u16 = toggle_low(0x1234)
                let anded: u16 = partial_and(0x1234)
                let ored: u16 = partial_or(0x1234)
            }
        "#;
        let nmos = emit_for_cpu(source, CpuFamily::Mos6502);
        let cmos = emit_for_cpu(source, CpuFamily::Cmos65C02);
        let body = |assembly: &str, name: &str| {
            assembly
                .split(&format!("_{name}:"))
                .nth(1)
                .unwrap()
                .split("    rts")
                .next()
                .unwrap()
                .to_owned()
        };

        assert!(!nmos.contains("    stz $"), "{nmos}");
        assert!(body(&nmos, "keep_low").contains("    lda #$00"), "{nmos}");
        assert!(body(&cmos, "keep_low").contains("    stz $"), "{cmos}");
        for assembly in [&nmos, &cmos] {
            let keep_low = body(assembly, "keep_low");
            assert!(!keep_low.contains("    and #$FF"), "{keep_low}");
            assert!(!keep_low.contains("    and $"), "{keep_low}");

            let set_high = body(assembly, "set_high");
            assert!(set_high.contains("    lda #$FF"), "{set_high}");
            assert!(!set_high.contains("    ora #$00"), "{set_high}");
            assert!(!set_high.contains("    ora $"), "{set_high}");

            let toggle_low = body(assembly, "toggle_low");
            assert!(toggle_low.contains("    eor #$FF"), "{toggle_low}");
            assert!(!toggle_low.contains("    eor #$00"), "{toggle_low}");
            assert!(!toggle_low.contains("    eor $"), "{toggle_low}");

            let partial_and = body(assembly, "partial_and");
            assert!(partial_and.contains("    and #$0F"), "{partial_and}");
            let partial_or = body(assembly, "partial_or");
            assert!(partial_or.contains("    ora #$12"), "{partial_or}");
        }

        for (assembly, cpu) in [
            (&nmos, crate::target::AssemblerCpu::Mos6502),
            (&cmos, crate::target::AssemblerCpu::Cmos65C02),
        ] {
            crate::vm::assemble_subset_with_symbols_at(cpu, assembly, 0x0200)
                .unwrap_or_else(|error| panic!("{error}\n{assembly}"));
        }
    }

    #[test]
    fn keeps_cmos_mmio_access_widths_and_avoids_cmos_memory_forms() {
        let assembly = emit_for_cpu(
            r#"
                volatile mmio status: ptr<u16> = 0xD000
                fn main() {
                    *(status) = 0
                    if (*(status) & 0x0100) == 0 { }
                }
            "#,
            CpuFamily::Cmos65C02,
        );

        assert!(!assembly.contains("    stz $D000"), "{assembly}");
        assert!(!assembly.contains("    trb $D000"), "{assembly}");
        assert!(!assembly.contains("    tsb $D000"), "{assembly}");
        assert!(assembly.contains("    bit #$01"), "{assembly}");
        assert!(assembly.contains("    sta $D000"), "{assembly}");
        assert!(assembly.contains("    sta $D001"), "{assembly}");
        assert!(assembly.contains("    lda $D000"), "{assembly}");
        assert!(assembly.contains("    lda $D001"), "{assembly}");
        assert!(!assembly.contains("($F0),y"), "{assembly}");
    }

    #[test]
    fn cleans_fallthrough_jumps_and_relaxes_only_in_range_long_branches() {
        let mut near = String::from(
            "start:\n    beq .L_branch_skip_0\n    jmp .L_target\n.L_branch_skip_0:\n    nop\n    jmp .L_next\n.L_next:\n    rts\n.L_target:\n    rts\n",
        );
        let cleaned = cleanup_assembly(&near, CpuFamily::Mos6502).unwrap();
        assert!(cleaned.contains("    bne .L_target"), "{cleaned}");
        assert!(!cleaned.contains("jmp .L_target"), "{cleaned}");
        assert!(!cleaned.contains("jmp .L_next"), "{cleaned}");

        near =
            String::from("start:\n    beq .L_branch_skip_0\n    jmp .L_far\n.L_branch_skip_0:\n");
        for _ in 0..130 {
            near.push_str("    nop\n");
        }
        near.push_str(".L_far:\n    rts\n");
        let far = cleanup_assembly(&near, CpuFamily::Cmos65C02).unwrap();
        assert!(far.contains("    beq .L_branch_skip_0"), "{far}");
        assert!(far.contains("    jmp .L_far"), "{far}");
    }

    #[test]
    fn emits_direct_absolute_mmio_and_indexed_pointer_accesses() {
        let assembly = emit(
            r#"
                volatile mmio WORD: ptr<u16> = 0x2200
                volatile mmio LENGTH: ptr<u8> = 0x2300
                volatile mmio INPUT: ptr<u8> = 0x2301
                volatile mmio OUTPUT: ptr<u8> = 0x2401
                fn main() {
                    let index: u8 = *LENGTH
                    let value: u8 = *(INPUT + index)
                    *WORD = 0x1234
                    *OUTPUT = value
                }
            "#,
        );
        for instruction in [
            "lda $2300",
            "lda $2301,x",
            "sta $2200",
            "sta $2201",
            "sta $2401",
        ] {
            assert!(
                assembly.contains(instruction),
                "missing {instruction}\n{assembly}"
            );
        }
        assert!(!assembly.contains("($F0),y"), "{assembly}");
    }

    #[test]
    fn emits_direct_scaled_variable_index_arithmetic() {
        let assembly = emit(
            r#"
                struct Three { byte: u8 word: u16 }
                global byte_result: u8 = 0
                global wide_result: u32 = 0
                global three_result: u16 = 0
                fn read(index: u16) {
                    let bytes: ptr<u8> = 0x3000
                    let wides: ptr<u32> = 0x4000
                    let threes: [Three; 2] = [
                        Three { byte: 1, word: 2 },
                        Three { byte: 3, word: 4 }
                    ]
                    byte_result = bytes[index]
                    wide_result = wides[index]
                    three_result = threes[index].word
                }
                fn main() { read(257) }
            "#,
        );

        assert!(!assembly.contains("index_scale"), "{assembly}");
        assert!(!assembly.contains("index_done"), "{assembly}");
        assert!(!assembly.contains("mul_loop"), "{assembly}");
        assert!(!assembly.contains(U16_MUL_HELPER), "{assembly}");
        assert!(
            assembly.matches("    clc\n    lda $").count() >= 3,
            "{assembly}"
        );
        assert!(assembly.matches("    rol $").count() >= 4, "{assembly}");
        assert!(assembly.contains("    adc $"), "{assembly}");
    }

    #[test]
    fn saves_static_locals_only_on_recursive_call_edges() {
        let nonrecursive = emit(
            r#"
                fn leaf(value: u8) -> u8 { return value + 1 }
                fn main() {
                    let local: u8 = 4
                    let result: u8 = leaf(local)
                }
            "#,
        );
        assert!(!nonrecursive.contains("    pha"), "{nonrecursive}");
        assert!(!nonrecursive.contains("    pla"), "{nonrecursive}");

        let recursive = emit(
            r#"
                fn recurse(value: u8) -> u8 {
                    if value == 0 { return 0 }
                    return value + recurse(value - 1)
                }
                fn main() { let result: u8 = recurse(3) }
            "#,
        );
        assert!(recursive.contains("    pha"), "{recursive}");
        assert!(recursive.contains("    pla"), "{recursive}");

        let mutual = emit(
            r#"
                fn left(value: u8) -> u8 {
                    if value == 0 { return 0 }
                    return value + right(value - 1)
                }
                fn right(value: u8) -> u8 {
                    if value == 0 { return 0 }
                    return value + left(value - 1)
                }
                fn main() { let result: u8 = left(3) }
            "#,
        );
        assert!(mutual.contains("    pha"), "{mutual}");
        assert!(mutual.contains("    pla"), "{mutual}");
    }

    #[test]
    fn void_self_tail_calls_do_not_use_recursive_stack_frames() {
        let assembly = emit(
            r#"
                fn recurse(output: ptr<u8>) {
                    let local: u8 = 0
                    recurse(&local)
                }
                fn main() {
                    let result: u8 = 0
                    recurse(&result)
                }
            "#,
        );

        assert_eq!(
            assembly.matches("    jsr _recurse").count(),
            1,
            "{assembly}"
        );
        assert!(assembly.contains(".L_loop_body_"), "{assembly}");
    }

    #[test]
    fn explicit_inline_calls_preserve_unsafe_conditional_calls_and_omit_inline_labels() {
        let assembly = emit(
            r#"
                global calls: u8 = 0
                global result: u8 = 0
                global guarded: bool = false
                volatile mmio FLAG: ptr<bool> = 0xD000
                inline fn approved(value: u8) -> u8 { return value + 1 }
                inline fn nested(value: u8) -> u8 { return approved(value) }
                inline fn conditional(value: bool) -> bool {
                    calls += 1
                    return value
                }
                fn main() {
                    result = nested(4)
                    guarded = *FLAG && conditional(true)
                }
            "#,
        );

        assert!(!assembly.contains("_approved:"), "{assembly}");
        assert!(!assembly.contains("_nested:"), "{assembly}");
        assert!(assembly.contains("_conditional:"), "{assembly}");
        assert!(assembly.contains("    jsr _conditional"), "{assembly}");
        crate::vm::assemble_subset_with_symbols_at(
            crate::target::AssemblerCpu::Mos6502,
            &assembly,
            0x0200,
        )
        .unwrap_or_else(|error| panic!("{error}\n{assembly}"));
    }

    #[test]
    fn keeps_automatic_small_wrapper_inlining_separate_from_explicit_inlining() {
        let assembly = emit(
            r#"
                fn sink() {}
                fn automatic_wrapper() { sink() }
                @inline fn explicit_wrapper() { sink(); sink() }
                fn retained_wrapper() { sink(); sink() }
                @inline fn recursive_wrapper() { recursive_wrapper() }
                fn main() {
                    automatic_wrapper()
                    explicit_wrapper()
                    retained_wrapper()
                    recursive_wrapper()
                }
            "#,
        );

        assert!(!assembly.contains("_sink:"), "{assembly}");
        assert!(!assembly.contains("_automatic_wrapper:"), "{assembly}");
        assert!(!assembly.contains("_explicit_wrapper:"), "{assembly}");
        assert!(assembly.contains("_retained_wrapper:"), "{assembly}");
        assert!(assembly.contains("    jsr _retained_wrapper"), "{assembly}");
        assert!(assembly.contains("_recursive_wrapper:"), "{assembly}");
        assert!(
            assembly.contains("    jsr _recursive_wrapper"),
            "{assembly}"
        );
    }
}

#[cfg(all(test, feature = "mos6502-emulator"))]
mod tests;
