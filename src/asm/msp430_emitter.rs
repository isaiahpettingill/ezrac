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
    declaration::unwrapped_declaration,
    diagnostic::Diagnostic,
    hir::HirProgram,
    intrinsics::{self, BitsIntrinsic, IntIntrinsic, IntrinsicOperation, MemIntrinsic},
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

/// Emit the MSP430 source backend.
///
/// The source backend uses a 16-bit scalar ABI. Values are evaluated in R4;
/// R5 through R9 are arithmetic scratch registers, R10 through R12 hold
/// allocated scalar locals, R13 is the frame pointer, and R1 is the stack
/// pointer. A two-result function returns its values in R4 and R5.
pub fn emit_msp430_assembly_with_options(
    program: &Program,
    options: AssemblyOptions,
) -> Result<String, Diagnostic> {
    if !matches!(
        options.cpu,
        CpuFamily::Msp430 | CpuFamily::Msp430X | CpuFamily::Msp430X2
    ) {
        return Err(Diagnostic::new(
            "MSP430 emitter requires an MSP430, MSP430X, or MSP430X2 target",
        ));
    }
    let hir = HirProgram::from_ast(program)?;
    let (lowered_program, source_comments) = if contains_function_pointer_program(program) {
        (program.clone(), Vec::new())
    } else {
        let tbir = TbirProgram::lower(&hir, program, &options)?;
        (tbir.lowered_program, tbir.source_comments)
    };
    let model = SemanticModel::from_program_with_native_int_widths(
        &lowered_program,
        options.cpu.capabilities().memory.pointer_width_bits,
        options.ram_base.get(),
        options.rodata_base.get(),
        options.asset_base.get(),
        options.cpu.capabilities().native_int_widths,
    )?;
    Emitter::new(model, options.clone())
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
            {
                strip_unreachable_generated_routines(&asm, RoutineProfile::Msp430)
            } else {
                asm
            };
            with_readability_comments(asm, program, &options, "msp430", &source_comments)
        })
}

#[derive(Clone, Copy)]
enum BindingLocation {
    Frame(i16),
    Register(u8),
}

#[derive(Clone)]
struct Binding {
    location: BindingLocation,
    ty: Type,
}

struct FunctionFrame {
    locals: HashMap<String, Binding>,
    local_bytes: u16,
}

#[derive(Clone)]
struct LoopLabels {
    continue_label: String,
    break_label: String,
}

struct Emitter {
    model: SemanticModel,
    options: AssemblyOptions,
    out: String,
    labels: usize,
    scopes: Vec<HashMap<String, Binding>>,
    loops: Vec<LoopLabels>,
    return_labels: Vec<String>,
    return_types: Vec<Option<Type>>,
    second_return_types: Vec<Option<Type>>,
    local_plans: Vec<HashMap<String, Binding>>,
    runtime_strings: HashMap<String, Storage>,
    functions: HashMap<String, Function>,
    inline_stack: Vec<String>,
    recursive_functions: HashSet<String>,
}

impl Emitter {
    fn new(model: SemanticModel, options: AssemblyOptions) -> Self {
        Self {
            model,
            options,
            out: String::new(),
            labels: 0,
            scopes: Vec::new(),
            loops: Vec::new(),
            return_labels: Vec::new(),
            return_types: Vec::new(),
            second_return_types: Vec::new(),
            local_plans: Vec::new(),
            runtime_strings: HashMap::new(),
            functions: HashMap::new(),
            inline_stack: Vec::new(),
            recursive_functions: HashSet::new(),
        }
    }

    fn emit(mut self, program: &Program) -> Result<String, Diagnostic> {
        self.validate_two_result_abi(program)?;
        self.functions = program
            .declarations
            .iter()
            .filter_map(|declaration| match unwrapped_declaration(declaration) {
                Declaration::Function(function) if !function.name.contains('.') => {
                    Some((function.name.clone(), function.clone()))
                }
                _ => None,
            })
            .collect();
        let mut reachable = if self
            .options
            .optimization
            .is_enabled(crate::optimization::OptimizationPass::DeadCodeElimination)
        {
            reachable_function_names(program, &self.model)
        } else {
            self.functions.keys().cloned().collect()
        };
        let referenced_functions = referenced_function_names(program, &self.model);
        reachable.extend(referenced_functions.iter().cloned());
        self.recursive_functions = recursive_function_names(program, &self.model);
        self.prepare_runtime_strings(program, &reachable)?;
        self.line("; generated by ezrac");
        self.line("; target: MSP430");
        if false {
            self.line("; TI-99/4A standard cartridge header and one menu entry.");
            self.line("section .header");
            self.line("    db 0xAA, 1, 1, 0");
            self.line("    dw 0, 0x6010, 0, 0, 0, 0");
            self.line("    dw 0, __ezra_start");
            self.line("    db 4, \"EZRA\", 0");
        }
        self.line("section .text");
        self.line("__ezra_start:");
        let stack_top = self.configured_stack_top()?;
        if self.native_20() {
            self.line(&format!("    mov.a #0x{stack_top:05X},r10"));
        } else {
            self.line(&format!("    mov #0x{stack_top:04X}, r10"));
        }
        self.emit_static_initializers(program)?;
        self.line("    call #_main");
        self.line("__ezra_exit:");
        self.line("    jmp __ezra_exit");
        for declaration in &program.declarations {
            if let Declaration::Function(function) = unwrapped_declaration(declaration)
                && !function.name.contains('.')
                && reachable.contains(&function.name)
                && (function.name == "main"
                    || !msp430_compact_wrapper_candidate(function)
                    || self.recursive_functions.contains(&function.name)
                    || referenced_functions.contains(&function.name))
            {
                // Imported qualified names are semantic aliases of the same
                // function. Calls lower to the short assembly label, so
                // emitting dotted aliases would duplicate inline-asm labels.
                self.emit_function(function)?;
            }
        }
        for section in [".header", ".rodata", ".data", ".bss"] {
            self.line(&format!("section {section}"));
        }
        if self.is_ti_cartridge() {
            self.emit_rom_embeds()?;
        } else {
            self.line("section .assets");
        }
        self.line("section .scratch");
        Ok(self.out)
    }

    fn validate_two_result_abi(&self, program: &Program) -> Result<(), Diagnostic> {
        for declaration in &program.declarations {
            match unwrapped_declaration(declaration) {
                Declaration::Function(function) if function.second_return_type.is_some() => {
                    let Some(first) = function.return_type.as_ref() else {
                        return Err(Diagnostic::new(format!(
                            "MSP430 two-result function `{}` must have a first return type",
                            function.name
                        )));
                    };
                    if scalar_width(&self.model, first).is_err()
                        || scalar_width(&self.model, function.second_return_type.as_ref().unwrap())
                            .is_err()
                    {
                        return Err(Diagnostic::new(format!(
                            "MSP430 two-result function `{}` must return scalar values that fit in the target width",
                            function.name
                        )));
                    }
                }
                Declaration::ExternAsmFunction(function)
                    if function.second_return_type.is_some() =>
                {
                    return Err(Diagnostic::new(format!(
                        "MSP430 external two-result function `{}` is unsupported; define a source function with the R4/R5 ABI",
                        function.name
                    )));
                }
                _ => {}
            }
        }
        Ok(())
    }

    fn is_ti_cartridge(&self) -> bool {
        false
    }

    fn emit_rom_embeds(&mut self) -> Result<(), Diagnostic> {
        self.line("section .assets");
        let mut embeds = self
            .model
            .embeds
            .iter()
            .map(|(name, embed)| (name.clone(), embed.clone()))
            .collect::<Vec<_>>();
        embeds.sort_by_key(|(_, embed)| embed.storage.address);
        let mut offset = 0u32;
        for (name, embed) in embeds {
            let embed_offset = embed
                .storage
                .address
                .checked_sub(self.options.asset_base.get())
                .ok_or_else(|| Diagnostic::new("MSP430 embed is below the asset base"))?;
            if embed_offset > offset {
                self.line(&format!("    ds {}", embed_offset - offset));
            }
            self.line(&format!("{}:", embed_label(&name)));
            for bytes in embed.bytes.chunks(16) {
                let values = bytes
                    .iter()
                    .map(|byte| format!(">{byte:02X}"))
                    .collect::<Vec<_>>()
                    .join(", ");
                self.line(&format!("    db {values}"));
            }
            self.line(&format!("{}:", embed_end_label(&name)));
            offset = embed_offset
                .checked_add(embed.storage.size)
                .ok_or_else(|| Diagnostic::new("MSP430 embed section is too large"))?;
        }
        Ok(())
    }

    fn prepare_runtime_strings(
        &mut self,
        program: &Program,
        reachable: &HashSet<String>,
    ) -> Result<(), Diagnostic> {
        let mut values = HashSet::new();
        collect_program_strings(program, reachable, &mut values);
        let mut values = values.into_iter().collect::<Vec<_>>();
        values.sort();
        for value in values {
            let size = u32::try_from(value.len() + 1)
                .map_err(|_| Diagnostic::new("string literal is too large"))?;
            let storage = self.model.allocate(size)?;
            self.runtime_strings.insert(value, storage);
        }
        Ok(())
    }

    fn emit_static_initializers(&mut self, program: &Program) -> Result<(), Diagnostic> {
        if !self.is_ti_cartridge() {
            let embeds = self.model.embeds.values().cloned().collect::<Vec<_>>();
            for embed in embeds {
                for (offset, byte) in embed.bytes.iter().copied().enumerate() {
                    self.load_immediate(i64::from(byte))?;
                    self.store_r0_address(
                        embed.storage.address + offset as u32,
                        &Type::Named("u8".to_owned()),
                    )?;
                }
            }
        }
        let strings = self
            .runtime_strings
            .iter()
            .map(|(value, storage)| (value.clone(), *storage))
            .collect::<Vec<_>>();
        for (value, storage) in strings {
            for (offset, byte) in value.bytes().chain(core::iter::once(0)).enumerate() {
                self.load_immediate(i64::from(byte))?;
                self.store_r0_address(
                    storage.address + offset as u32,
                    &Type::Named("u8".to_owned()),
                )?;
            }
        }
        for declaration in &program.declarations {
            match unwrapped_declaration(declaration) {
                Declaration::Const(constant) if matches!(constant.ty, Type::Array { .. }) => {
                    let storage = self.model.globals[&constant.name];
                    self.emit_const_array_initializer(storage, &constant.ty, &constant.value)?;
                }
                Declaration::Global(global) => {
                    let storage = self.model.globals[&global.name];
                    let ty = self.model.resolved_type(&global.ty)?;
                    self.emit_expr(&global.value, &ty)?;
                    self.store_r0(storage, &ty)?;
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
                    self.emit_expr(value, &element)?;
                    self.store_r0(element_storage, &element)?;
                }
            } else {
                self.load_immediate(0)?;
                self.store_r0(element_storage, &element)?;
            }
        }
        Ok(())
    }

    fn emit_function(&mut self, function: &Function) -> Result<(), Diagnostic> {
        let naked = function.attrs.iter().any(|attr| attr == "naked");
        if naked
            && function.body.iter().any(|stmt| {
                !matches!(stmt, Stmt::Asm { inputs, outputs, .. } if inputs.is_empty() && outputs.is_empty())
            })
        {
            return Err(Diagnostic::new(format!(
                "naked function `{}` may contain only asm blocks without operands",
                function.name
            )));
        }
        if function.attrs.iter().any(|attr| attr == "interrupt") {
            return Err(Diagnostic::new(
                "MSP430 interrupt functions are not implemented; use a naked assembly wrapper",
            ));
        }

        self.line(&format!("{}:", function_label(&function.name)));
        if naked {
            for stmt in &function.body {
                let Stmt::Asm { lines, .. } = stmt else {
                    unreachable!("naked body was validated above");
                };
                for line in lines {
                    self.line(line);
                }
            }
            return Ok(());
        }

        let frame = plan_function_frame(function, &self.model)?;
        let return_label = self.next_label(&format!("{}_return", function.name));
        self.scopes.push(HashMap::new());
        self.local_plans.push(frame.locals);
        self.return_labels.push(return_label.clone());
        self.return_types.push(
            function
                .return_type
                .as_ref()
                .map(|ty| self.model.resolved_type(ty))
                .transpose()?,
        );
        self.second_return_types.push(
            function
                .second_return_type
                .as_ref()
                .map(|ty| self.model.resolved_type(ty))
                .transpose()?,
        );

        self.line("    push r9");
        self.line("    mov r10, r9");
        self.adjust_stack(-i32::from(frame.local_bytes));

        let mut offset = 4i16;
        for param in &function.params {
            let ty = self.model.resolved_type(&param.ty)?;
            self.bind(
                param.name.clone(),
                Binding {
                    location: BindingLocation::Frame(offset),
                    ty: ty.clone(),
                },
            )?;
            offset = offset
                .checked_add(
                    i16::try_from(abi_slot_bytes(&self.model, &ty)?).map_err(|_| {
                        Diagnostic::new("MSP430 function parameter frame is too large")
                    })?,
                )
                .ok_or_else(|| Diagnostic::new("MSP430 function parameter frame is too large"))?;
        }
        self.emit_block(&function.body)?;
        self.line(&format!("{return_label}:"));
        self.adjust_stack(i32::from(frame.local_bytes));
        self.line("    mov @r10+, r9");
        self.line("    ret");
        self.second_return_types.pop();
        self.return_types.pop();
        self.return_labels.pop();
        self.local_plans.pop();
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
                let ty = self.model.resolved_type(ty)?;
                let binding = self
                    .local_plans
                    .last()
                    .and_then(|locals| locals.get(name))
                    .cloned()
                    .ok_or_else(|| Diagnostic::new(format!("missing frame slot for `{name}`")))?;
                self.bind(name.clone(), binding.clone())?;
                self.emit_expr(value, &ty)?;
                self.store_binding_r0(&binding)?;
            }
            Stmt::LetTwo {
                first_name,
                second_name,
                value,
                ..
            } => {
                let first_binding = self
                    .local_plans
                    .last()
                    .and_then(|locals| locals.get(first_name))
                    .cloned()
                    .ok_or_else(|| {
                        Diagnostic::new(format!("missing frame slot for `{first_name}`"))
                    })?;
                let second_binding = self
                    .local_plans
                    .last()
                    .and_then(|locals| locals.get(second_name))
                    .cloned()
                    .ok_or_else(|| {
                        Diagnostic::new(format!("missing frame slot for `{second_name}`"))
                    })?;
                self.bind(first_name.clone(), first_binding.clone())?;
                self.bind(second_name.clone(), second_binding.clone())?;
                let Expr::Call { path, args } = value else {
                    return Err(Diagnostic::new(
                        "MSP430 two-place binding requires a direct two-result call",
                    ));
                };
                self.emit_call(path, args, true)?;
                self.store_binding_r0(&first_binding)?;
                self.line("    mov r2, r0");
                self.store_binding_r0(&second_binding)?;
            }
            Stmt::Assign { target, op, value } => {
                let ty = self.place_type(target)?;
                if *op == AssignOp::Set {
                    self.emit_expr(value, &ty)?;
                } else {
                    self.load_place(target, &ty)?;
                    self.push_value(&ty);
                    self.emit_expr(value, &ty)?;
                    self.pop_value_to(1, &ty);
                    self.emit_binary(assign_binary(*op), &ty)?;
                }
                self.store_place(target, &ty)?;
            }
            Stmt::If {
                condition,
                then_body,
                else_body,
            } => {
                let then = self.next_label("if_then");
                let otherwise = self.next_label("if_else");
                let done = self.next_label("if_end");
                self.emit_expr(condition, &Type::Named("bool".to_owned()))?;
                self.line("    ci r0, 0");
                self.line(&format!("    jne {then}"));
                self.line(&format!("    b @{otherwise}"));
                self.line(&format!("{then}:"));
                self.emit_block(then_body)?;
                self.line(&format!("    b @{done}"));
                self.line(&format!("{otherwise}:"));
                self.emit_block(else_body)?;
                self.line(&format!("{done}:"));
            }
            Stmt::While { condition, body } => {
                let check = self.next_label("while_check");
                let body_label = self.next_label("while_body");
                let done = self.next_label("while_end");
                self.loops.push(LoopLabels {
                    continue_label: check.clone(),
                    break_label: done.clone(),
                });
                self.line(&format!("{check}:"));
                self.emit_expr(condition, &Type::Named("bool".to_owned()))?;
                self.line("    ci r0, 0");
                self.line(&format!("    jne {body_label}"));
                self.line(&format!("    b @{done}"));
                self.line(&format!("{body_label}:"));
                self.emit_block(body)?;
                self.line(&format!("    b @{check}"));
                self.line(&format!("{done}:"));
                self.loops.pop();
            }
            Stmt::Loop { body } => {
                let again = self.next_label("loop");
                let done = self.next_label("loop_end");
                self.loops.push(LoopLabels {
                    continue_label: again.clone(),
                    break_label: done.clone(),
                });
                self.line(&format!("{again}:"));
                self.emit_block(body)?;
                self.line(&format!("    b @{again}"));
                self.line(&format!("{done}:"));
                self.loops.pop();
            }
            Stmt::Break => self.jump_loop(false)?,
            Stmt::Continue => self.jump_loop(true)?,
            Stmt::Return(value) => {
                if self
                    .second_return_types
                    .last()
                    .and_then(Clone::clone)
                    .is_some()
                {
                    let Some(Expr::Call { path, args }) = value else {
                        return Err(Diagnostic::new(
                            "MSP430 two-result function must return a direct two-result call or `return first, second`",
                        ));
                    };
                    self.emit_call(path, args, true)?;
                    self.line("    mov r2, r1");
                } else if let Some(value) = value {
                    let ty = self
                        .return_types
                        .last()
                        .and_then(Clone::clone)
                        .ok_or_else(|| Diagnostic::new("value return in void function"))?;
                    self.emit_expr(value, &ty)?;
                }
                let label = self
                    .return_labels
                    .last()
                    .expect("function return label")
                    .clone();
                self.line(&format!("    b @{label}"));
            }
            Stmt::ReturnTwo { first, second } => {
                let first_ty =
                    self.return_types
                        .last()
                        .and_then(Clone::clone)
                        .ok_or_else(|| {
                            Diagnostic::new("MSP430 two-value return without two result types")
                        })?;
                let second_ty = self
                    .second_return_types
                    .last()
                    .and_then(Clone::clone)
                    .ok_or_else(|| {
                        Diagnostic::new("MSP430 two-value return without two result types")
                    })?;
                self.emit_expr(first, &first_ty)?;
                self.line("    dect r10");
                self.line("    mov r0, *r10");
                self.emit_expr(second, &second_ty)?;
                self.line("    mov r0, r2");
                self.line("    mov *r10+, r0");
                self.line("    mov r2, r1");
                let label = self
                    .return_labels
                    .last()
                    .expect("function return label")
                    .clone();
                self.line(&format!("    b @{label}"));
            }
            Stmt::Asm {
                inputs,
                outputs,
                lines,
                ..
            } => {
                if !inputs.is_empty() || !outputs.is_empty() {
                    return Err(Diagnostic::new(
                        "MSP430 inline asm operands are not implemented; use compiler-owned RAM or a naked wrapper",
                    ));
                }
                for line in lines {
                    self.line(line);
                }
            }
            Stmt::Out { .. } => {
                return Err(Diagnostic::new(
                    "MSP430 does not support separate port I/O; use TI-99/4A MMIO or CRU assembly",
                ));
            }
            Stmt::Expr(expr) => self.emit_expr(expr, &Type::Named("u16".to_owned()))?,
        }
        Ok(())
    }

    fn emit_expr(&mut self, expr: &Expr, ty: &Type) -> Result<(), Diagnostic> {
        match expr {
            Expr::Int(value) | Expr::TypedInt(value, _) => self.load_immediate_typed(*value, ty)?,
            Expr::Bool(value) => self.load_immediate(i64::from(*value))?,
            Expr::Char(value) => self.load_immediate(i64::from(*value))?,
            Expr::String(value) => {
                let storage = self
                    .runtime_strings
                    .get(value)
                    .copied()
                    .ok_or_else(|| Diagnostic::new("missing MSP430 string storage"))?;
                self.load_immediate_typed(i64::from(storage.address), ty)?;
            }
            Expr::Ident(name) => self.load_ident(name, ty)?,
            Expr::AddressOf(name) => {
                if let Some(function) = self.functions.get(name) {
                    if function.second_return_type.is_some() {
                        return Err(Diagnostic::new(format!(
                            "MSP430 function pointer cannot reference two-result function `{name}`"
                        )));
                    }
                    self.load_address_label(&function_label(name), ty);
                } else if let Some(binding) = self.binding(name) {
                    let BindingLocation::Frame(offset) = binding.location else {
                        return Err(Diagnostic::new(format!(
                            "address-taken local `{name}` was allocated to a register"
                        )));
                    };
                    self.move_value(9, 0, ty);
                    self.line_typed(&format!("    ai r0, >{:04X}", offset as u16), ty);
                } else if self.is_ti_cartridge() && self.model.embeds.contains_key(name) {
                    self.load_address_label(&embed_label(name), ty);
                } else {
                    let storage = self
                        .model
                        .globals
                        .get(name)
                        .copied()
                        .or_else(|| self.model.embeds.get(name).map(|embed| embed.storage))
                        .ok_or_else(|| {
                            Diagnostic::new(format!(
                                "MSP430 backend can only take the address of a global or embed, not `{name}`"
                            ))
                        })?;
                    self.load_immediate_typed(i64::from(storage.address), ty)?;
                }
            }
            Expr::Deref(pointer) => {
                self.emit_expr(pointer, &Type::Ptr(Box::new(ty.clone())))?;
                self.load_indirect_r0(ty)?;
            }
            Expr::Index { name, index } => self.emit_array_index(name, index)?,
            Expr::BankedPointer { pointer, .. } => self.emit_expr(pointer, ty)?,
            Expr::Call { path, args } => self.emit_call(path, args, false)?,
            Expr::Unary { op, expr } => {
                self.emit_expr(expr, ty)?;
                match op {
                    UnaryOp::Neg => self.line_typed("    neg r0", ty),
                    UnaryOp::BitNot => self.line_typed("    inv r0", ty),
                    UnaryOp::Not => {
                        let yes = self.next_label("not_true");
                        let done = self.next_label("not_done");
                        self.line("    ci r0, 0");
                        self.line(&format!("    jeq {yes}"));
                        self.line("    clr r0");
                        self.line(&format!("    b @{done}"));
                        self.line(&format!("{yes}:"));
                        self.line("    li r0, 1");
                        self.line(&format!("{done}:"));
                    }
                }
            }
            Expr::Binary { left, op, right } => {
                if matches!(op, BinaryOp::And | BinaryOp::Or) {
                    self.emit_logical_expr(left, *op, right)?;
                } else {
                    let operand_ty = if matches!(
                        op,
                        BinaryOp::Eq
                            | BinaryOp::Ne
                            | BinaryOp::Lt
                            | BinaryOp::Le
                            | BinaryOp::Gt
                            | BinaryOp::Ge
                    ) {
                        self.model.resolved_type(&self.expr_type(left)?)?
                    } else {
                        ty.clone()
                    };
                    self.emit_expr(left, &operand_ty)?;
                    self.move_value(0, 1, &operand_ty);
                    if matches!(op, BinaryOp::Shl | BinaryOp::Shr)
                        && let Some(count) = constant_shift_count(right)?
                    {
                        self.emit_shift(*op, &operand_ty, Some(count))?;
                    } else {
                        self.push_value(&operand_ty);
                        self.emit_expr(right, &operand_ty)?;
                        self.pop_value_to(1, &operand_ty);
                        self.emit_binary(*op, &operand_ty)?;
                    }
                }
            }
            Expr::Cast { expr, .. } => self.emit_expr(expr, ty)?,
            Expr::Field { base, field } => self.load_ident(&format!("{base}.{field}"), ty)?,
            Expr::Access(path)
                if path
                    .segments
                    .iter()
                    .all(|segment| matches!(segment, AccessSegment::Field(_))) =>
            {
                let mut name = path.root.clone();
                for segment in &path.segments {
                    let AccessSegment::Field(field) = segment else {
                        unreachable!()
                    };
                    name.push('.');
                    name.push_str(field);
                }
                self.load_ident(&name, ty)?;
            }
            Expr::Access(_)
            | Expr::AddressOfAccess(_)
            | Expr::AddressOfIndex { .. }
            | Expr::AddressOfField { .. }
            | Expr::Array(_)
            | Expr::StructInit { .. }
            | Expr::In(_) => {
                return Err(Diagnostic::new(format!(
                    "MSP430 expression `{expr:?}` is not implemented by the initial source backend"
                )));
            }
        }
        Ok(())
    }

    fn resolve_intrinsic(
        &self,
        path: &[String],
        args: &[Expr],
    ) -> Result<Option<intrinsics::IntrinsicResolution>, Diagnostic> {
        let name = path.join(".");
        if intrinsics::lookup(&name).is_none() {
            return Ok(None);
        }
        let types = args
            .iter()
            .map(|arg| self.expr_type(arg))
            .collect::<Result<Vec<_>, _>>()?;
        let constants = args
            .iter()
            .map(|arg| self.model.const_value(arg).ok())
            .collect::<Vec<_>>();
        let resolution = intrinsics::CATALOG
            .validate_types_with_constants(&name, &types, &constants)
            .map_err(|error| Diagnostic::new(error.to_string()))?;
        self.validate_msp430_intrinsic(&resolution, args)?;
        Ok(Some(resolution))
    }

    fn validate_msp430_intrinsic(
        &self,
        resolution: &intrinsics::IntrinsicResolution,
        args: &[Expr],
    ) -> Result<(), Diagnostic> {
        let length_argument = match resolution.descriptor.operation {
            IntrinsicOperation::Mem(MemIntrinsic::CopyNonoverlapping)
            | IntrinsicOperation::Mem(MemIntrinsic::Move)
            | IntrinsicOperation::Mem(MemIntrinsic::Fill) => Some(2),
            IntrinsicOperation::Mem(MemIntrinsic::FindByte)
            | IntrinsicOperation::Mem(MemIntrinsic::Compare) => Some(1),
            _ => None,
        };
        for (index, ty) in resolution.argument_types.iter().enumerate() {
            if Some(index) == length_argument {
                continue;
            }
            if let Some(info) = intrinsics::integer_info(ty)
                && info.bits > 16
            {
                return Err(Diagnostic::new(format!(
                    "MSP430 intrinsic `{}` does not support `{ty:?}`; scalar values are at most 16 bits",
                    resolution.canonical_name()
                )));
            }
        }
        for ty in &resolution.result_types {
            if let Some(info) = intrinsics::integer_info(ty)
                && info.bits > 16
            {
                return Err(Diagnostic::new(format!(
                    "MSP430 intrinsic `{}` does not support `{ty:?}` results",
                    resolution.canonical_name()
                )));
            }
        }
        if let Some(index) = length_argument {
            let length = self.model.const_value(&args[index]).map_err(|_| {
                Diagnostic::new(format!(
                    "MSP430 intrinsic `{}` requires a constant length that fits in 16 bits",
                    resolution.canonical_name()
                ))
            })?;
            if !(0..=u16::MAX as i64).contains(&length) {
                return Err(Diagnostic::new(format!(
                    "MSP430 intrinsic `{}` length {length} is outside the 16-bit address space",
                    resolution.canonical_name()
                )));
            }
        }
        if !matches!(
            resolution.descriptor.effects.volatile,
            intrinsics::VolatilePolicy::PreservesAccess
        ) && self.intrinsic_accesses_volatile_memory(resolution.descriptor.operation, args)
        {
            return Err(Diagnostic::new(format!(
                "MSP430 intrinsic `{}` cannot access volatile memory",
                resolution.canonical_name()
            )));
        }
        Ok(())
    }

    fn intrinsic_accesses_volatile_memory(
        &self,
        operation: IntrinsicOperation,
        args: &[Expr],
    ) -> bool {
        let indexes: &[usize] = match operation {
            IntrinsicOperation::Mem(MemIntrinsic::CopyNonoverlapping)
            | IntrinsicOperation::Mem(MemIntrinsic::Move) => &[0, 1],
            IntrinsicOperation::Mem(MemIntrinsic::Fill)
            | IntrinsicOperation::Mem(MemIntrinsic::FindByte)
            | IntrinsicOperation::Mem(MemIntrinsic::LoadLe16)
            | IntrinsicOperation::Mem(MemIntrinsic::LoadLe24)
            | IntrinsicOperation::Mem(MemIntrinsic::LoadBe16)
            | IntrinsicOperation::Mem(MemIntrinsic::LoadBe24)
            | IntrinsicOperation::Mem(MemIntrinsic::StoreLe16)
            | IntrinsicOperation::Mem(MemIntrinsic::StoreLe24)
            | IntrinsicOperation::Mem(MemIntrinsic::StoreBe16)
            | IntrinsicOperation::Mem(MemIntrinsic::StoreBe24) => &[0],
            IntrinsicOperation::Mem(MemIntrinsic::Compare) => &[0, 1],
            _ => &[],
        };
        indexes
            .iter()
            .copied()
            .any(|index| self.is_volatile_msp430_expr(&args[index]))
    }

    fn is_volatile_msp430_expr(&self, expr: &Expr) -> bool {
        match expr {
            Expr::Ident(name) => self
                .model
                .mmio
                .get(name)
                .is_some_and(|(_, _, volatile)| *volatile),
            Expr::Cast { expr, .. } | Expr::BankedPointer { pointer: expr, .. } => {
                self.is_volatile_msp430_expr(expr)
            }
            _ => false,
        }
    }

    fn emit_intrinsic(
        &mut self,
        _path: &[String],
        args: &[Expr],
        resolution: &intrinsics::IntrinsicResolution,
    ) -> Result<(), Diagnostic> {
        match resolution.descriptor.operation {
            IntrinsicOperation::Bits(operation) => {
                self.emit_msp430_bits(operation, args, resolution)
            }
            IntrinsicOperation::Int(operation) => self.emit_msp430_int(operation, args, resolution),
            IntrinsicOperation::Mem(operation) => self.emit_msp430_mem(operation, args, resolution),
        }
        .map_err(|error| {
            Diagnostic::new(format!(
                "MSP430 intrinsic `{}`: {}",
                resolution.canonical_name(),
                error.message
            ))
        })
    }

    fn emit_msp430_argument(&mut self, expr: &Expr, ty: &Type) -> Result<(), Diagnostic> {
        if matches!(intrinsics::integer_info(ty), Some(info) if info.bits > 16) {
            return self.load_immediate(self.model.const_value(expr)?);
        }
        self.emit_expr(expr, ty)
    }

    fn emit_msp430_push_argument(&mut self, expr: &Expr, ty: &Type) -> Result<(), Diagnostic> {
        self.emit_msp430_argument(expr, ty)?;
        self.line("    dect r10");
        self.line("    mov r0, *r10");
        Ok(())
    }

    fn emit_msp430_two_arguments(
        &mut self,
        args: &[Expr],
        resolution: &intrinsics::IntrinsicResolution,
    ) -> Result<(), Diagnostic> {
        self.emit_msp430_push_argument(&args[0], &resolution.argument_types[0])?;
        self.emit_msp430_argument(&args[1], &resolution.argument_types[1])?;
        self.line("    mov r0, r1");
        self.line("    mov *r10+, r0");
        Ok(())
    }

    fn emit_msp430_three_arguments(
        &mut self,
        args: &[Expr],
        resolution: &intrinsics::IntrinsicResolution,
    ) -> Result<(), Diagnostic> {
        for (arg, ty) in args.iter().zip(&resolution.argument_types) {
            self.emit_msp430_push_argument(arg, ty)?;
        }
        self.line("    mov *r10+, r2");
        self.line("    mov *r10+, r1");
        self.line("    mov *r10+, r0");
        Ok(())
    }

    fn emit_msp430_bits(
        &mut self,
        operation: BitsIntrinsic,
        args: &[Expr],
        resolution: &intrinsics::IntrinsicResolution,
    ) -> Result<(), Diagnostic> {
        let ty = &resolution.argument_types[0];
        let bits = msp430_intrinsic_integer_bits(ty)?;
        let mask = msp430_intrinsic_integer_mask(bits);
        match operation {
            BitsIntrinsic::RotateLeft | BitsIntrinsic::RotateRight => {
                self.emit_msp430_two_arguments(args, resolution)?;
                self.line(&format!("    andi r1, >{:04X}", bits - 1));
                self.line("    mov r1, r2");
                self.line(&format!("    li r3, >{bits:04X}"));
                let loop_label = self.next_label("intrinsic_rotate_loop");
                let done = self.next_label("intrinsic_rotate_done");
                self.line(&format!("{loop_label}:"));
                self.line("    ci r2, 0");
                self.line(&format!("    jeq {done}"));
                if operation == BitsIntrinsic::RotateLeft {
                    self.line("    mov r0, r5");
                    self.line(&format!("    andi r5, >{:04X}", 1u16 << (bits - 1)));
                    self.line("    sla r0, 1");
                    self.line(&format!("    andi r0, >{mask:04X}"));
                    self.line("    ci r5, 0");
                    let no_bit = self.next_label("intrinsic_rotate_no_bit");
                    self.line(&format!("    jeq {no_bit}"));
                    self.line("    ori r0, >0001");
                    self.line(&format!("{no_bit}:"));
                } else {
                    self.line("    mov r0, r5");
                    self.line("    andi r5, >0001");
                    self.line("    srl r0, 1");
                    self.line("    ci r5, 0");
                    let no_bit = self.next_label("intrinsic_rotate_no_bit");
                    self.line(&format!("    jeq {no_bit}"));
                    self.line(&format!("    ori r0, >{:04X}", 1u16 << (bits - 1)));
                    self.line(&format!("{no_bit}:"));
                }
                self.line("    dec r2");
                self.line(&format!("    b @{loop_label}"));
                self.line(&format!("{done}:"));
            }
            BitsIntrinsic::Test
            | BitsIntrinsic::Set
            | BitsIntrinsic::Clear
            | BitsIntrinsic::Toggle => {
                self.emit_msp430_argument(&args[0], ty)?;
                let bit = 1u16 << self.model.const_value(&args[1])? as u16;
                match operation {
                    BitsIntrinsic::Test => {
                        self.line(&format!("    andi r0, >{bit:04X}"));
                        self.emit_msp430_boolean_from_r0();
                    }
                    BitsIntrinsic::Set => self.line(&format!("    ori r0, >{bit:04X}")),
                    BitsIntrinsic::Clear => self.line(&format!("    andi r0, >{:04X}", !bit)),
                    BitsIntrinsic::Toggle => {
                        self.line(&format!("    li r1, >{bit:04X}"));
                        self.line("    xor r1, r0");
                    }
                    _ => unreachable!(),
                }
                if operation == BitsIntrinsic::Toggle {
                    self.line(&format!("    andi r0, >{mask:04X}"));
                } else {
                    self.line(&format!("    andi r0, >{mask:04X}"));
                }
            }
            BitsIntrinsic::Extract => {
                self.emit_msp430_argument(&args[0], ty)?;
                let offset = self.model.const_value(&args[1])? as u16;
                let width = self.model.const_value(&args[2])? as u16;
                self.line(&format!("    srl r0, {offset}"));
                self.line(&format!(
                    "    andi r0, >{:04X}",
                    msp430_intrinsic_integer_mask(width)
                ));
            }
            BitsIntrinsic::Insert => {
                self.emit_msp430_two_arguments(args, resolution)?;
                let offset = self.model.const_value(&args[2])? as u16;
                let width = self.model.const_value(&args[3])? as u16;
                let field = msp430_intrinsic_integer_mask(width);
                let shifted = field << offset;
                self.line(&format!("    andi r1, >{field:04X}"));
                self.line(&format!("    sla r1, {offset}"));
                self.line(&format!("    andi r0, >{:04X}", !shifted));
                self.line("    soc r1, r0");
                self.line(&format!("    andi r0, >{mask:04X}"));
            }
            BitsIntrinsic::ByteSwap => {
                self.emit_msp430_argument(&args[0], ty)?;
                self.line("    swpb r0");
            }
            BitsIntrinsic::Reverse => self.emit_msp430_bit_loop(&args[0], ty, false)?,
            BitsIntrinsic::CountOnes => self.emit_msp430_bit_loop(&args[0], ty, true)?,
            BitsIntrinsic::LeadingZeros => self.emit_msp430_zero_count(&args[0], ty, true)?,
            BitsIntrinsic::TrailingZeros => self.emit_msp430_zero_count(&args[0], ty, false)?,
        }
        Ok(())
    }

    fn emit_msp430_bit_loop(
        &mut self,
        expr: &Expr,
        ty: &Type,
        count_ones: bool,
    ) -> Result<(), Diagnostic> {
        let bits = msp430_intrinsic_integer_bits(ty)?;
        self.emit_msp430_argument(expr, ty)?;
        self.line("    mov r0, r1");
        self.line("    clr r2");
        self.line(&format!("    li r3, >{bits:04X}"));
        let loop_label = self.next_label("intrinsic_bit_loop");
        let done = self.next_label("intrinsic_bit_done");
        let no_bit = self.next_label("intrinsic_bit_no_bit");
        self.line(&format!("{loop_label}:"));
        self.line("    ci r3, 0");
        self.line(&format!("    jeq {done}"));
        if !count_ones {
            self.line("    sla r2, 1");
        }
        self.line("    mov r1, r4");
        self.line("    andi r4, >0001");
        self.line("    ci r4, 0");
        self.line(&format!("    jeq {no_bit}"));
        self.line("    inc r2");
        self.line(&format!("{no_bit}:"));
        self.line("    srl r1, 1");
        self.line("    dec r3");
        self.line(&format!("    b @{loop_label}"));
        self.line(&format!("{done}:"));
        self.line("    mov r2, r0");
        if count_ones {
            self.line(&format!(
                "    andi r0, >{:04X}",
                msp430_intrinsic_integer_mask(bits)
            ));
        }
        Ok(())
    }

    fn emit_msp430_zero_count(
        &mut self,
        expr: &Expr,
        ty: &Type,
        leading: bool,
    ) -> Result<(), Diagnostic> {
        let bits = msp430_intrinsic_integer_bits(ty)?;
        self.emit_msp430_argument(expr, ty)?;
        self.line("    mov r0, r1");
        self.line("    clr r2");
        self.line(&format!("    li r3, >{bits:04X}"));
        let loop_label = self.next_label("intrinsic_zero_loop");
        let done = self.next_label("intrinsic_zero_done");
        self.line(&format!("{loop_label}:"));
        self.line("    ci r3, 0");
        self.line(&format!("    jeq {done}"));
        self.line("    mov r1, r4");
        self.line(&format!(
            "    andi r4, >{:04X}",
            if leading { 1u16 << (bits - 1) } else { 1 }
        ));
        self.line("    ci r4, 0");
        self.line(&format!("    jne {done}"));
        if leading {
            self.line("    sla r1, 1");
        } else {
            self.line("    srl r1, 1");
        }
        self.line("    inc r2");
        self.line("    dec r3");
        self.line(&format!("    b @{loop_label}"));
        self.line(&format!("{done}:"));
        self.line("    mov r2, r0");
        Ok(())
    }

    fn emit_msp430_int(
        &mut self,
        operation: IntIntrinsic,
        args: &[Expr],
        resolution: &intrinsics::IntrinsicResolution,
    ) -> Result<(), Diagnostic> {
        let ty = &resolution.argument_types[0];
        match operation {
            IntIntrinsic::WideningMul | IntIntrinsic::MulHigh | IntIntrinsic::FullMul => {
                self.emit_msp430_product(operation, ty, args, resolution)?
            }
            IntIntrinsic::SaturatingAdd | IntIntrinsic::SaturatingSub => self
                .emit_msp430_saturating(
                    operation == IntIntrinsic::SaturatingSub,
                    ty,
                    args,
                    resolution,
                )?,
            IntIntrinsic::Divmod => self.emit_msp430_divmod(ty, args, resolution)?,
            IntIntrinsic::AddCarry | IntIntrinsic::SubBorrow => {
                self.emit_msp430_carry(operation == IntIntrinsic::SubBorrow, ty, args, resolution)?
            }
        }
        Ok(())
    }

    fn emit_msp430_product(
        &mut self,
        operation: IntIntrinsic,
        ty: &Type,
        args: &[Expr],
        resolution: &intrinsics::IntrinsicResolution,
    ) -> Result<(), Diagnostic> {
        let bits = msp430_intrinsic_integer_bits(ty)?;
        let signed = msp430_intrinsic_integer_signed(ty);
        self.emit_msp430_two_arguments(args, resolution)?;
        if bits == 8 {
            if signed {
                self.line("    sla r0, 8");
                self.line("    sra r0, 8");
                self.line("    sla r1, 8");
                self.line("    sra r1, 8");
            } else {
                self.line("    andi r0, >00FF");
                self.line("    andi r1, >00FF");
            }
        }
        if signed {
            self.line("    clr r3");
            self.line("    ci r0, 0");
            let left_done = self.next_label("intrinsic_mul_left_done");
            self.line(&format!("    jgt {left_done}"));
            self.line(&format!("    jeq {left_done}"));
            self.line("    neg r0");
            self.line("    inc r3");
            self.line(&format!("{left_done}:"));
            self.line("    ci r1, 0");
            let right_done = self.next_label("intrinsic_mul_right_done");
            self.line(&format!("    jgt {right_done}"));
            self.line(&format!("    jeq {right_done}"));
            self.line("    neg r1");
            self.line("    inc r3");
            self.line(&format!("{right_done}:"));
        }
        self.line("    mpy r1, r0");
        if signed {
            let positive = self.next_label("intrinsic_mul_positive");
            let no_carry = self.next_label("intrinsic_mul_no_carry");
            self.line("    andi r3, >0001");
            self.line("    ci r3, 0");
            self.line(&format!("    jeq {positive}"));
            self.line("    inv r0");
            self.line("    inv r1");
            self.line("    ai r1, 1");
            self.line(&format!("    jne {no_carry}"));
            self.line("    ai r0, 1");
            self.line(&format!("{no_carry}:"));
            self.line(&format!("{positive}:"));
        }
        if operation == IntIntrinsic::WideningMul {
            self.line("    mov r1, r0");
        } else if operation == IntIntrinsic::MulHigh {
            if bits == 8 {
                self.line("    srl r1, 8");
                self.line("    mov r1, r0");
            }
            // For 16-bit values MPY already leaves the high half in R0.
        } else if bits == 8 {
            self.line("    mov r1, r3");
            self.line("    srl r3, 8");
            self.line("    andi r3, >00FF");
            self.line("    mov r3, r2");
            self.line("    andi r1, >00FF");
            self.line("    mov r1, r0");
        } else {
            self.line("    mov r0, r2");
            self.line("    mov r1, r0");
        }
        if bits == 8 && operation == IntIntrinsic::FullMul {
            self.line("    andi r0, >00FF");
            self.line("    andi r2, >00FF");
        }
        Ok(())
    }

    fn emit_msp430_saturating(
        &mut self,
        subtract: bool,
        ty: &Type,
        args: &[Expr],
        resolution: &intrinsics::IntrinsicResolution,
    ) -> Result<(), Diagnostic> {
        let bits = msp430_intrinsic_integer_bits(ty)?;
        let mask = msp430_intrinsic_integer_mask(bits);
        self.emit_msp430_two_arguments(args, resolution)?;
        if !msp430_intrinsic_integer_signed(ty) {
            if subtract {
                let zero = self.next_label("intrinsic_sat_zero");
                let done = self.next_label("intrinsic_sat_done");
                self.line("    c r0, r1");
                self.line(&format!("    jl {zero}"));
                self.line("    s r1, r0");
                self.line(&format!("    b @{done}"));
                self.line(&format!("{zero}:"));
                self.line("    clr r0");
                self.line(&format!("{done}:"));
            } else {
                let clamp = self.next_label("intrinsic_sat_clamp");
                let done = self.next_label("intrinsic_sat_done");
                self.line("    a r1, r0");
                if bits == 8 {
                    self.line("    ci r0, >00FF");
                    self.line(&format!("    jh {clamp}"));
                } else {
                    self.line(&format!("    joc {clamp}"));
                }
                self.line(&format!("    b @{done}"));
                self.line(&format!("{clamp}:"));
                self.line(&format!("    li r0, >{mask:04X}"));
                self.line(&format!("{done}:"));
            }
            self.line(&format!("    andi r0, >{mask:04X}"));
            return Ok(());
        }
        if bits == 8 {
            self.line("    sla r0, 8");
            self.line("    sra r0, 8");
            self.line("    sla r1, 8");
            self.line("    sra r1, 8");
        }
        self.line("    mov r0, r3");
        self.line("    mov r1, r4");
        if subtract {
            self.line("    s r1, r0");
        } else {
            self.line("    a r1, r0");
        }
        let left_positive = self.next_label("intrinsic_sat_left_positive");
        let left_negative = self.next_label("intrinsic_sat_left_negative");
        let clamp_max = self.next_label("intrinsic_sat_max");
        let clamp_min = self.next_label("intrinsic_sat_min");
        let done = self.next_label("intrinsic_sat_done");
        self.line("    ci r3, 0");
        self.line(&format!("    jgt {left_positive}"));
        self.line(&format!("    jlt {left_negative}"));
        self.line(&format!("    b @{done}"));
        self.line(&format!("{left_positive}:"));
        if subtract {
            self.line("    ci r4, 0");
            self.line(&format!("    jlt {clamp_max}"));
        } else {
            self.line("    ci r4, 0");
            self.line(&format!("    jle {done}"));
            self.line("    ci r0, 0");
            self.line(&format!("    jlt {clamp_max}"));
        }
        self.line(&format!("    b @{done}"));
        self.line(&format!("{left_negative}:"));
        if subtract {
            self.line("    ci r4, 0");
            self.line(&format!("    jgt {done}"));
            self.line("    ci r0, 0");
            self.line(&format!("    jhe {clamp_min}"));
        } else {
            self.line("    ci r4, 0");
            self.line(&format!("    jhe {done}"));
            self.line("    ci r0, 0");
            self.line(&format!("    jhe {clamp_min}"));
        }
        self.line(&format!("    b @{done}"));
        self.line(&format!("{clamp_max}:"));
        self.line(&format!(
            "    li r0, >{:04X}",
            msp430_intrinsic_integer_mask(bits - 1)
        ));
        self.line(&format!("    b @{done}"));
        self.line(&format!("{clamp_min}:"));
        self.line(&format!("    li r0, >{:04X}", 1u16 << (bits - 1)));
        self.line(&format!("{done}:"));
        self.line(&format!("    andi r0, >{mask:04X}"));
        Ok(())
    }

    fn emit_msp430_divmod(
        &mut self,
        ty: &Type,
        args: &[Expr],
        resolution: &intrinsics::IntrinsicResolution,
    ) -> Result<(), Diagnostic> {
        let bits = msp430_intrinsic_integer_bits(ty)?;
        let signed = msp430_intrinsic_integer_signed(ty);
        self.emit_msp430_two_arguments(args, resolution)?;
        if bits == 8 {
            if signed {
                self.line("    sla r0, 8");
                self.line("    sra r0, 8");
                self.line("    sla r1, 8");
                self.line("    sra r1, 8");
            } else {
                self.line("    andi r0, >00FF");
                self.line("    andi r1, >00FF");
            }
        }
        let zero = self.next_label("intrinsic_div_zero");
        let done = self.next_label("intrinsic_div_done");
        self.line("    ci r1, 0");
        self.line(&format!("    jeq {zero}"));
        if signed {
            self.line("    clr r4");
            self.line("    clr r5");
            let dividend_done = self.next_label("intrinsic_dividend_done");
            self.line("    ci r0, 0");
            self.line(&format!("    jgt {dividend_done}"));
            self.line(&format!("    jeq {dividend_done}"));
            self.line("    neg r0");
            self.line("    li r5, 1");
            self.line("    ai r4, 1");
            self.line(&format!("{dividend_done}:"));
            let divisor_done = self.next_label("intrinsic_divisor_done");
            self.line("    ci r1, 0");
            self.line(&format!("    jgt {divisor_done}"));
            self.line(&format!("    jeq {divisor_done}"));
            self.line("    neg r1");
            self.line("    ai r4, 1");
            self.line(&format!("{divisor_done}:"));
        }
        // The intrinsic evaluator leaves the dividend in R0 and divisor in
        // R1, while the shared software routine uses R1/R0 respectively.
        self.line("    mov r0, r3");
        self.line("    mov r1, r0");
        self.line("    mov r3, r1");
        self.emit_software_divide(bits, ty);
        self.line("    mov r2, r0");
        if signed {
            self.line("    andi r4, >0001");
            let quotient_done = self.next_label("intrinsic_div_quotient_done");
            self.line(&format!("    jeq {quotient_done}"));
            self.line("    neg r0");
            self.line(&format!("{quotient_done}:"));
            self.line("    ci r5, 0");
            let remainder_done = self.next_label("intrinsic_div_remainder_done");
            self.line(&format!("    jeq {remainder_done}"));
            self.line("    neg r3");
            self.line(&format!("{remainder_done}:"));
        }
        self.line("    mov r3, r2");
        self.line(&format!("    b @{done}"));
        self.line(&format!("{zero}:"));
        self.line("    clr r0");
        self.line("    clr r2");
        self.line(&format!("{done}:"));
        if bits == 8 {
            self.line("    andi r0, >00FF");
            self.line("    andi r2, >00FF");
        }
        Ok(())
    }

    fn emit_msp430_carry(
        &mut self,
        subtract: bool,
        ty: &Type,
        args: &[Expr],
        resolution: &intrinsics::IntrinsicResolution,
    ) -> Result<(), Diagnostic> {
        let bits = msp430_intrinsic_integer_bits(ty)?;
        let mask = msp430_intrinsic_integer_mask(bits);
        self.emit_msp430_three_arguments(args, resolution)?;
        self.line("    andi r2, >0001");
        let set = self.next_label("intrinsic_carry_set");
        let done = self.next_label("intrinsic_carry_done");
        if subtract {
            self.line("    mov r1, r3");
            self.line("    a r2, r3");
            self.line(&format!("    joc {set}"));
            self.line("    c r0, r3");
            self.line(&format!("    jl {set}"));
            self.line("    s r3, r0");
            self.line("    clr r2");
            self.line(&format!("    b @{done}"));
            self.line(&format!("{set}:"));
            self.line("    s r3, r0");
            self.line("    li r2, 1");
        } else {
            self.line("    a r1, r0");
            self.line(&format!("    joc {set}"));
            self.line("    a r2, r0");
            self.line(&format!("    joc {set}"));
            self.line("    clr r3");
            self.line(&format!("    b @{done}"));
            self.line(&format!("{set}:"));
            self.line("    li r3, 1");
            self.line("    a r2, r0");
        }
        self.line(&format!("{done}:"));
        if subtract {
            self.line(&format!("    andi r0, >{mask:04X}"));
            self.line("    mov r2, r3");
        } else {
            self.line(&format!("    andi r0, >{mask:04X}"));
            self.line("    mov r3, r2");
        }
        if bits == 8 {
            self.line("    andi r0, >00FF");
        }
        Ok(())
    }

    fn emit_msp430_mem(
        &mut self,
        operation: MemIntrinsic,
        args: &[Expr],
        resolution: &intrinsics::IntrinsicResolution,
    ) -> Result<(), Diagnostic> {
        match operation {
            MemIntrinsic::CopyNonoverlapping | MemIntrinsic::Move => {
                self.emit_msp430_copy_or_move(operation == MemIntrinsic::Move, args, resolution)
            }
            MemIntrinsic::Fill => self.emit_msp430_fill(args, resolution),
            MemIntrinsic::FindByte => self.emit_msp430_find_byte(args, resolution),
            MemIntrinsic::Compare => self.emit_msp430_compare(args, resolution),
            MemIntrinsic::LoadLe16
            | MemIntrinsic::LoadBe16
            | MemIntrinsic::LoadLe24
            | MemIntrinsic::LoadBe24 => {
                if matches!(operation, MemIntrinsic::LoadLe24 | MemIntrinsic::LoadBe24) {
                    return Err(Diagnostic::new(
                        "24-bit memory loads need a 24-bit target value",
                    ));
                }
                self.emit_msp430_load16(operation == MemIntrinsic::LoadBe16, args, resolution)
            }
            MemIntrinsic::StoreLe16
            | MemIntrinsic::StoreBe16
            | MemIntrinsic::StoreLe24
            | MemIntrinsic::StoreBe24 => {
                if matches!(operation, MemIntrinsic::StoreLe24 | MemIntrinsic::StoreBe24) {
                    return Err(Diagnostic::new(
                        "24-bit memory stores need a 24-bit target value",
                    ));
                }
                self.emit_msp430_store16(operation == MemIntrinsic::StoreBe16, args, resolution)
            }
            MemIntrinsic::Peek8 => self.emit_msp430_peek8(args, resolution),
            MemIntrinsic::Poke8 => self.emit_msp430_poke8(args, resolution),
        }
    }

    fn emit_msp430_copy_or_move(
        &mut self,
        move_semantics: bool,
        args: &[Expr],
        resolution: &intrinsics::IntrinsicResolution,
    ) -> Result<(), Diagnostic> {
        self.emit_msp430_push_argument(&args[0], &resolution.argument_types[0])?;
        self.emit_msp430_push_argument(&args[1], &resolution.argument_types[1])?;
        self.emit_msp430_push_argument(&args[2], &resolution.argument_types[2])?;
        self.line("    mov *r10+, r2");
        self.line("    mov *r10+, r4");
        self.line("    mov *r10+, r3");
        let done = self.next_label("intrinsic_copy_done");
        self.line("    ci r2, 0");
        self.line(&format!("    jeq {done}"));
        if move_semantics {
            let backward = self.next_label("intrinsic_move_backward");
            let forward = self.next_label("intrinsic_move_forward");
            self.line("    c r3, r4");
            self.line(&format!("    jh {backward}"));
            self.line(&format!("    b @{forward}"));
            self.line(&format!("{backward}:"));
            self.line("    mov r2, r0");
            self.line("    dec r0");
            self.line("    a r0, r3");
            self.line("    a r0, r4");
            self.emit_msp430_copy_loop(true, &done);
            self.line(&format!("{forward}:"));
        }
        self.emit_msp430_copy_loop(false, &done);
        self.line(&format!("{done}:"));
        Ok(())
    }

    fn emit_msp430_copy_loop(&mut self, backward: bool, done: &str) {
        let loop_label = self.next_label("intrinsic_copy_loop");
        self.line(&format!("{loop_label}:"));
        self.line("    clr r1");
        self.line("    movb *r4, r1");
        self.line("    movb r1, *r3");
        if backward {
            self.line("    dec r3");
            self.line("    dec r4");
        } else {
            self.line("    inc r3");
            self.line("    inc r4");
        }
        self.line("    dec r2");
        self.line(&format!("    jne {loop_label}"));
        self.line(&format!("    b @{done}"));
    }

    fn emit_msp430_fill(
        &mut self,
        args: &[Expr],
        resolution: &intrinsics::IntrinsicResolution,
    ) -> Result<(), Diagnostic> {
        self.emit_msp430_push_argument(&args[0], &resolution.argument_types[0])?;
        self.emit_msp430_push_argument(&args[1], &resolution.argument_types[1])?;
        self.emit_msp430_push_argument(&args[2], &resolution.argument_types[2])?;
        self.line("    mov *r10+, r2");
        self.line("    mov *r10+, r4");
        self.line("    mov *r10+, r3");
        let done = self.next_label("intrinsic_fill_done");
        let loop_label = self.next_label("intrinsic_fill_loop");
        self.line("    ci r2, 0");
        self.line(&format!("    jeq {done}"));
        self.line(&format!("{loop_label}:"));
        self.line("    mov r4, r1");
        self.line("    movb r1, *r3");
        self.line("    inc r3");
        self.line("    dec r2");
        self.line(&format!("    jne {loop_label}"));
        self.line(&format!("{done}:"));
        Ok(())
    }

    fn emit_msp430_find_byte(
        &mut self,
        args: &[Expr],
        resolution: &intrinsics::IntrinsicResolution,
    ) -> Result<(), Diagnostic> {
        self.emit_msp430_push_argument(&args[0], &resolution.argument_types[0])?;
        self.emit_msp430_push_argument(&args[1], &resolution.argument_types[1])?;
        self.emit_msp430_argument(&args[2], &resolution.argument_types[2])?;
        self.line("    mov r0, r5");
        self.line("    mov *r10+, r4");
        self.line("    mov *r10+, r3");
        self.line("    andi r5, >00FF");
        let loop_label = self.next_label("intrinsic_find_loop");
        let found = self.next_label("intrinsic_find_found");
        let not_found = self.next_label("intrinsic_find_not_found");
        self.line(&format!("{loop_label}:"));
        self.line("    ci r4, 0");
        self.line(&format!("    jeq {not_found}"));
        self.line("    clr r1");
        self.line("    movb *r3, r1");
        self.line("    c r1, r5");
        self.line(&format!("    jeq {found}"));
        self.line("    inc r3");
        self.line("    dec r4");
        self.line(&format!("    b @{loop_label}"));
        self.line(&format!("{found}:"));
        self.line("    mov r3, r0");
        self.line("    li r2, 1");
        let done = self.next_label("intrinsic_find_done");
        self.line(&format!("    b @{done}"));
        self.line(&format!("{not_found}:"));
        self.line("    mov r3, r0");
        self.line("    clr r2");
        self.line(&format!("{done}:"));
        Ok(())
    }

    fn emit_msp430_compare(
        &mut self,
        args: &[Expr],
        resolution: &intrinsics::IntrinsicResolution,
    ) -> Result<(), Diagnostic> {
        self.emit_msp430_push_argument(&args[0], &resolution.argument_types[0])?;
        self.emit_msp430_push_argument(&args[1], &resolution.argument_types[1])?;
        self.emit_msp430_push_argument(&args[2], &resolution.argument_types[2])?;
        self.line("    mov *r10+, r2");
        self.line("    mov *r10+, r4");
        self.line("    mov *r10+, r3");
        let loop_label = self.next_label("intrinsic_compare_loop");
        let less = self.next_label("intrinsic_compare_less");
        let greater = self.next_label("intrinsic_compare_greater");
        let done = self.next_label("intrinsic_compare_done");
        self.line(&format!("{loop_label}:"));
        self.line("    ci r2, 0");
        self.line(&format!("    jeq {done}"));
        self.line("    clr r0");
        self.line("    movb *r3, r0");
        self.line("    clr r1");
        self.line("    movb *r4, r1");
        self.line("    c r0, r1");
        self.line(&format!("    jl {less}"));
        self.line(&format!("    jh {greater}"));
        self.line("    inc r3");
        self.line("    inc r4");
        self.line("    dec r2");
        self.line(&format!("    b @{loop_label}"));
        self.line(&format!("{less}:"));
        self.line("    li r0, >FFFF");
        self.line(&format!("    b @{done}"));
        self.line(&format!("{greater}:"));
        self.line("    li r0, 1");
        self.line(&format!("    b @{done}"));
        self.line(&format!("{done}:"));
        Ok(())
    }

    fn emit_msp430_load16(
        &mut self,
        big_endian: bool,
        args: &[Expr],
        resolution: &intrinsics::IntrinsicResolution,
    ) -> Result<(), Diagnostic> {
        self.emit_msp430_argument(&args[0], &resolution.argument_types[0])?;
        self.line("    mov r0, r3");
        self.line("    clr r0");
        self.line("    movb *r3, r0");
        self.line("    mov r0, r4");
        self.line("    inc r3");
        self.line("    clr r0");
        self.line("    movb *r3, r0");
        if big_endian {
            self.line("    sla r4, 8");
            self.line("    soc r0, r4");
            self.line("    mov r4, r0");
        } else {
            self.line("    sla r0, 8");
            self.line("    soc r4, r0");
        }
        Ok(())
    }

    fn emit_msp430_store16(
        &mut self,
        big_endian: bool,
        args: &[Expr],
        resolution: &intrinsics::IntrinsicResolution,
    ) -> Result<(), Diagnostic> {
        self.emit_msp430_push_argument(&args[0], &resolution.argument_types[0])?;
        self.emit_msp430_argument(&args[1], &resolution.argument_types[1])?;
        self.line("    mov *r10+, r3");
        if big_endian {
            self.line("    mov r0, r4");
            self.line("    srl r4, 8");
            self.line("    movb r4, *r3");
            self.line("    inc r3");
            self.line("    movb r0, *r3");
        } else {
            self.line("    movb r0, *r3");
            self.line("    inc r3");
            self.line("    mov r0, r4");
            self.line("    srl r4, 8");
            self.line("    movb r4, *r3");
        }
        Ok(())
    }

    fn emit_msp430_peek8(
        &mut self,
        args: &[Expr],
        resolution: &intrinsics::IntrinsicResolution,
    ) -> Result<(), Diagnostic> {
        self.emit_msp430_argument(&args[0], &resolution.argument_types[0])?;
        self.load_indirect_r0(&Type::Named("u8".to_owned()))
    }

    fn emit_msp430_poke8(
        &mut self,
        args: &[Expr],
        resolution: &intrinsics::IntrinsicResolution,
    ) -> Result<(), Diagnostic> {
        self.emit_msp430_push_argument(&args[0], &resolution.argument_types[0])?;
        self.emit_msp430_argument(&args[1], &resolution.argument_types[1])?;
        self.line("    mov *r10+, r1");
        self.line("    movb r0, *r1");
        Ok(())
    }

    fn emit_msp430_boolean_from_r0(&mut self) {
        let yes = self.next_label("intrinsic_true");
        let done = self.next_label("intrinsic_bool_done");
        self.line("    ci r0, 0");
        self.line(&format!("    jne {yes}"));
        self.line("    clr r0");
        self.line(&format!("    b @{done}"));
        self.line(&format!("{yes}:"));
        self.line("    li r0, 1");
        self.line(&format!("{done}:"));
    }

    fn emit_call(
        &mut self,
        path: &[String],
        args: &[Expr],
        two_results: bool,
    ) -> Result<(), Diagnostic> {
        let name = path
            .last()
            .ok_or_else(|| Diagnostic::new("empty function call path"))?;
        if let Some(resolution) = self.resolve_intrinsic(path, args)? {
            if resolution.result_count().as_usize() == 2 && !two_results {
                return Err(Diagnostic::new(format!(
                    "MSP430 two-result intrinsic `{}` requires a two-result binding",
                    resolution.canonical_name()
                )));
            }
            if resolution.result_count().as_usize() != 2 && two_results {
                return Err(Diagnostic::new(format!(
                    "MSP430 intrinsic `{}` does not return two values",
                    resolution.canonical_name()
                )));
            }
            return self.emit_intrinsic(path, args, &resolution);
        }
        if !two_results && args.is_empty() && self.try_emit_inline_call(name)? {
            return Ok(());
        }
        let direct_signature = self
            .model
            .functions
            .get(name)
            .or_else(|| self.model.functions.get(&path.join(".")))
            .cloned();
        let (signature, indirect) = if let Some(signature) = direct_signature {
            (signature, false)
        } else {
            if path.len() != 1 {
                return Err(Diagnostic::new(format!(
                    "unknown MSP430 function `{}`",
                    path.join(".")
                )));
            }
            let pointer_type = self.named_type(name)?;
            let pointer_type = self.model.resolved_type(&pointer_type)?;
            let Type::Ptr(inner) = pointer_type.clone() else {
                return Err(Diagnostic::new(format!(
                    "MSP430 function pointer call requires `ptr<fn(...)>`, got `{pointer_type:?}`"
                )));
            };
            let Type::Function {
                params,
                return_type,
            } = *inner
            else {
                return Err(Diagnostic::new(format!(
                    "MSP430 function pointer call requires `ptr<fn(...)>`, got `{pointer_type:?}`"
                )));
            };
            (
                crate::tbir::model::FunctionSignature {
                    params,
                    return_type: return_type.map(|ty| *ty),
                    second_return_type: None,
                    argument_slots: Vec::new(),
                },
                true,
            )
        };
        if signature.second_return_type.is_some() != two_results {
            return Err(Diagnostic::new(if signature.second_return_type.is_some() {
                format!(
                    "MSP430 two-result call `{name}` cannot be used where one result is expected"
                )
            } else {
                format!("MSP430 call `{name}` does not return two values")
            }));
        }
        if args.len() != signature.params.len() {
            return Err(Diagnostic::new(format!(
                "function `{name}` expects {} arguments, got {}",
                signature.params.len(),
                args.len()
            )));
        }
        if args.len() > 9 {
            return Err(Diagnostic::new(format!(
                "MSP430 function `{name}` has {} arguments; the target ABI supports at most 9",
                args.len()
            )));
        }
        let argument_bytes = signature
            .params
            .iter()
            .map(|ty| abi_slot_bytes(&self.model, ty))
            .try_fold(0i32, |total, bytes| {
                let bytes = i32::from(bytes?);
                total
                    .checked_add(bytes)
                    .ok_or_else(|| Diagnostic::new("MSP430 argument frame is too large"))
            })?;
        self.line("    dect r10");
        self.line("    mov r1, *r10");
        self.adjust_stack(-argument_bytes);
        let mut offset = 0u16;
        for (arg, ty) in args.iter().zip(&signature.params) {
            self.emit_expr(arg, ty)?;
            self.store_argument_r0(offset, ty)?;
            offset = offset
                .checked_add(abi_slot_bytes(&self.model, ty)?)
                .ok_or_else(|| Diagnostic::new("MSP430 argument frame is too large"))?;
        }
        // Compiled functions read arguments from their stack frame. Mirroring
        // the first nine values in R0..R8 preserves the naked SDK wrapper ABI.
        let mut offset = 0u16;
        for (index, ty) in signature.params.iter().enumerate() {
            self.load_argument_r0(offset, ty)?;
            self.move_value(0, index as u8, ty);
            offset = offset
                .checked_add(abi_slot_bytes(&self.model, ty)?)
                .ok_or_else(|| Diagnostic::new("MSP430 argument frame is too large"))?;
        }
        if indirect {
            let pointer_type = self.named_type(name)?;
            self.emit_expr(&Expr::Ident(name.to_owned()), &pointer_type)?;
            self.line("    call r0");
        } else {
            self.line(&format!("    bl @{}", function_label(name)));
        }
        if two_results {
            // The caller restores its saved R1 after the call, so preserve the
            // second result in the caller-saved scratch register R2 first.
            self.line("    mov r1, r2");
        }
        self.adjust_stack(argument_bytes);
        self.line("    mov *r10+, r1");
        Ok(())
    }

    fn try_emit_inline_call(&mut self, name: &str) -> Result<bool, Diagnostic> {
        let Some(function) = self.functions.get(name).cloned() else {
            return Ok(false);
        };
        let Some(body) = inline_void_body(&function) else {
            return Ok(false);
        };
        if !msp430_compact_wrapper_candidate(&function)
            || self.recursive_functions.contains(name)
            || self.inline_stack.iter().any(|active| active == name)
        {
            return Ok(false);
        }

        self.inline_stack.push(name.to_owned());
        let result = self.emit_block(body);
        self.inline_stack.pop();
        result?;
        Ok(true)
    }

    fn emit_logical_expr(
        &mut self,
        left: &Expr,
        op: BinaryOp,
        right: &Expr,
    ) -> Result<(), Diagnostic> {
        if matches!((left, op), (Expr::Bool(false), BinaryOp::And))
            || matches!((left, op), (Expr::Bool(true), BinaryOp::Or))
        {
            self.load_immediate(i64::from(op == BinaryOp::Or))?;
            return Ok(());
        }
        let short = self.next_label("logical_short");
        let done = self.next_label("logical_done");
        self.emit_expr(left, &Type::Named("bool".to_owned()))?;
        self.line("    ci r0, 0");
        let jump = if op == BinaryOp::And { "jeq" } else { "jne" };
        self.line(&format!("    {jump} {short}"));
        self.emit_expr(right, &Type::Named("bool".to_owned()))?;
        self.line("    ci r0, 0");
        self.emit_boolean_from_jump("jne");
        self.line(&format!("    b @{done}"));
        self.line(&format!("{short}:"));
        self.load_immediate(i64::from(op == BinaryOp::Or))?;
        self.line(&format!("{done}:"));
        Ok(())
    }

    fn emit_binary(&mut self, op: BinaryOp, ty: &Type) -> Result<(), Diagnostic> {
        match op {
            BinaryOp::Add => {
                self.line_typed("    a r0, r1", ty);
                self.move_value(1, 0, ty);
            }
            BinaryOp::Sub => {
                self.line_typed("    s r0, r1", ty);
                self.move_value(1, 0, ty);
            }
            BinaryOp::BitAnd => {
                self.line_typed("    inv r0", ty);
                self.line_typed("    szc r0, r1", ty);
                self.move_value(1, 0, ty);
            }
            BinaryOp::BitOr => {
                self.line_typed("    soc r0, r1", ty);
                self.move_value(1, 0, ty);
            }
            BinaryOp::BitXor => {
                self.line_typed("    xor r0, r1", ty);
                self.move_value(1, 0, ty);
            }
            BinaryOp::Shl | BinaryOp::Shr => self.emit_shift(op, ty, None)?,
            BinaryOp::Eq
            | BinaryOp::Ne
            | BinaryOp::Lt
            | BinaryOp::Le
            | BinaryOp::Gt
            | BinaryOp::Ge => {
                self.line_typed("    c r1, r0", ty);
                let signed = type_is_signed(ty);
                match (op, signed) {
                    (BinaryOp::Eq, _) => self.emit_boolean_from_jump("jeq"),
                    (BinaryOp::Ne, _) => self.emit_boolean_from_jump("jne"),
                    (BinaryOp::Lt, true) => self.emit_boolean_from_jump("jlt"),
                    (BinaryOp::Le, true) => self.emit_boolean_from_jumps(&["jlt", "jeq"]),
                    (BinaryOp::Gt, true) => self.emit_boolean_from_jump("jgt"),
                    (BinaryOp::Ge, true) => self.emit_boolean_from_jumps(&["jgt", "jeq"]),
                    (BinaryOp::Lt, false) => self.emit_boolean_from_jump("jl"),
                    (BinaryOp::Le, false) => self.emit_boolean_from_jump("jle"),
                    (BinaryOp::Gt, false) => self.emit_boolean_from_jump("jh"),
                    (BinaryOp::Ge, false) => self.emit_boolean_from_jump("jhe"),
                    _ => unreachable!(),
                }
            }
            BinaryOp::And | BinaryOp::Or => {
                unreachable!("logical expressions are emitted before evaluating the right operand")
            }
            BinaryOp::Mul => self.multiply(type_is_signed(ty), ty),
            BinaryOp::Div | BinaryOp::Mod => self.divide(
                op == BinaryOp::Mod,
                type_is_signed(ty),
                scalar_width(&self.model, ty)? == 1,
                ty,
            ),
        }
        self.mask_value(ty);
        Ok(())
    }

    fn emit_shift(
        &mut self,
        op: BinaryOp,
        ty: &Type,
        constant_count: Option<u16>,
    ) -> Result<(), Diagnostic> {
        let right = op == BinaryOp::Shr;
        let signed = type_is_signed(ty);
        let byte = scalar_width(&self.model, ty)? == 1;
        let bits = if self.uses_native_20(ty) { 20 } else { 16 };

        if right && signed && byte {
            self.line("    sla r1, 8");
            self.line("    sra r1, 8");
        }

        if let Some(count) = constant_count {
            if count == 0 {
                self.move_value(1, 0, ty);
            } else if count < bits {
                let mnemonic = if right && signed {
                    "sra"
                } else if right {
                    "srl"
                } else {
                    "sla"
                };
                self.line_typed(&format!("    {mnemonic} r1, {count}"), ty);
                self.move_value(1, 0, ty);
            } else if right && signed {
                self.line_typed(&format!("    sra r1, {}", bits - 1), ty);
                self.move_value(1, 0, ty);
            } else {
                self.line("    clr r0");
            }
            return Ok(());
        }

        let zero = self.next_label("shift_zero");
        let overflow = self.next_label("shift_overflow");
        let done = self.next_label("shift_done");
        self.line("    ci r0, 0");
        self.line(&format!("    jeq {zero}"));
        self.line(&format!("    ci r0, {}", bits - 1));
        self.line(&format!("    jh {overflow}"));
        // Per the TI MSP430 Programmer's Guide, a zero instruction count takes
        // the count from the low four bits of R0. Expression evaluation already
        // places the value in R1 and the runtime count in R0.
        let mnemonic = if right && signed {
            "sra"
        } else if right {
            "srl"
        } else {
            "sla"
        };
        self.line_typed(&format!("    {mnemonic} r1, 0"), ty);
        self.move_value(1, 0, ty);
        self.line(&format!("    b @{done}"));
        self.line(&format!("{zero}:"));
        self.move_value(1, 0, ty);
        self.line(&format!("    b @{done}"));
        self.line(&format!("{overflow}:"));
        if right && signed {
            self.line_typed(&format!("    sra r1, {}", bits - 1), ty);
            self.move_value(1, 0, ty);
        } else {
            self.line("    clr r0");
        }
        self.line(&format!("{done}:"));
        self.mask_value(ty);
        Ok(())
    }

    fn multiply(&mut self, signed: bool, ty: &Type) {
        // MSP430 has no core multiply instruction. Normalize signed operands,
        // then use a width-sized shift/add loop. The low value is returned in
        // R0; the source-shaped registers R3..R6 map to MSP430 scratch R7..R10.
        let bits = if self.uses_native_20(ty) { 20 } else { 16 };
        let product_nonnegative = self.next_label("mul_product_nonnegative");
        let done = self.next_label("mul_done");
        if signed {
            let left_nonnegative = self.next_label("mul_left_nonnegative");
            let right_nonnegative = self.next_label("mul_right_nonnegative");
            self.line("    clr r3");
            self.line("    ci r1, 0");
            self.line(&format!("    jhe {left_nonnegative}"));
            self.line("    neg r1");
            self.line("    ai r3, 1");
            self.line(&format!("{left_nonnegative}:"));
            self.line("    ci r0, 0");
            self.line(&format!("    jhe {right_nonnegative}"));
            self.line("    neg r0");
            self.line("    ai r3, 1");
            self.line(&format!("{right_nonnegative}:"));
        }
        self.move_value(1, 4, ty);
        self.move_value(0, 5, ty);
        if self.uses_native_20(ty) {
            self.line("    mov.a #0,r2");
        } else {
            self.line("    clr r2");
        }
        self.line(&format!("    li r6, {bits}"));
        let loop_label = self.next_label("mul_loop");
        let no_add = self.next_label("mul_no_add");
        self.line(&format!("{loop_label}:"));
        self.line_typed("    bit #1,r4", ty);
        self.line(&format!("    jeq {no_add}"));
        self.line_typed("    a r5, r2", ty);
        self.line(&format!("{no_add}:"));
        self.line_typed("    sla r5, 1", ty);
        self.line_typed("    srl r4, 1", ty);
        self.line("    dec r6");
        self.line(&format!("    jne {loop_label}"));
        if signed {
            self.line("    andi r3, 1");
            self.line(&format!("    jeq {product_nonnegative}"));
            self.line_typed("    neg r2", ty);
            self.line(&format!("    b @{done}"));
            self.line(&format!("{product_nonnegative}:"));
        }
        self.line(&format!("{done}:"));
        self.move_value(2, 0, ty);
        self.mask_value(ty);
    }

    fn emit_software_divide(&mut self, bits: u16, ty: &Type) {
        // Unsigned restoring division. R1 is the dividend, R0 the divisor,
        // R2 receives the quotient, and R3 receives the remainder.
        let loop_label = self.next_label("div_loop");
        let no_subtract = self.next_label("div_no_subtract");
        self.line("    clr r2");
        self.line("    clr r3");
        self.line(&format!("    li r6, {bits}"));
        self.line(&format!("{loop_label}:"));
        self.line_typed("    sla r1, 1", ty);
        self.line_typed("    addc r3, r3", ty);
        self.line_typed("    c r0, r3", ty);
        self.line(&format!("    jl {no_subtract}"));
        self.line_typed("    s r0, r3", ty);
        self.line_typed("    ori r2, 1", ty);
        self.line(&format!("{no_subtract}:"));
        self.line("    dec r6");
        self.line(&format!("    jne {loop_label}"));
    }

    fn divide(&mut self, remainder: bool, signed: bool, byte: bool, ty: &Type) {
        // DIV divides the unsigned R2:R3 dividend by its source, leaving the
        // quotient in R2 and remainder in R3. Expressions arrive as left in R1
        // and right in R0, so form a zero-extended 32-bit dividend first.
        let bits = if self.uses_native_20(ty) { 20 } else { 16 };
        let zero = self.next_label("div_zero");
        let done = self.next_label("div_done");
        self.line("    ci r0, 0");
        self.line(&format!("    jeq {zero}"));
        if signed {
            if byte {
                self.line("    sla r1, 8");
                self.line("    sra r1, 8");
                self.line("    sla r0, 8");
                self.line("    sra r0, 8");
            }

            let left_nonnegative = self.next_label("div_left_nonnegative");
            let right_nonnegative = self.next_label("div_right_nonnegative");
            let result_nonnegative = self.next_label("div_result_nonnegative");
            self.line("    clr r4");
            self.line("    clr r5");
            self.line("    ci r1, 0");
            self.line(&format!("    jgt {left_nonnegative}"));
            self.line(&format!("    jeq {left_nonnegative}"));
            self.line("    neg r1");
            self.line("    li r5, 1");
            self.line("    ai r4, 1");
            self.line(&format!("{left_nonnegative}:"));
            self.line("    ci r0, 0");
            self.line(&format!("    jgt {right_nonnegative}"));
            self.line(&format!("    jeq {right_nonnegative}"));
            self.line("    neg r0");
            self.line("    ai r4, 1");
            self.line(&format!("{right_nonnegative}:"));
            self.emit_software_divide(bits, ty);

            if remainder {
                self.line("    ci r5, 0");
                self.line(&format!("    jeq {result_nonnegative}"));
                self.line("    neg r3");
                self.line(&format!("{result_nonnegative}:"));
                self.line("    mov r3, r0");
            } else {
                self.line("    andi r4, 1");
                self.line("    ci r4, 0");
                self.line(&format!("    jeq {result_nonnegative}"));
                self.line("    neg r2");
                self.line(&format!("{result_nonnegative}:"));
                self.line("    mov r2, r0");
            }
        } else {
            self.emit_software_divide(bits, ty);
            self.line(if remainder {
                "    mov r3, r0"
            } else {
                "    mov r2, r0"
            });
        }
        self.line(&format!("    b @{done}"));
        self.line(&format!("{zero}:"));
        self.line("    clr r0");
        self.line(&format!("{done}:"));
        self.mask_value(ty);
    }

    fn emit_boolean_from_jump(&mut self, jump: &str) {
        self.emit_boolean_from_jumps(&[jump]);
    }

    fn emit_boolean_from_jumps(&mut self, jumps: &[&str]) {
        let yes = self.next_label("comparison_true");
        let done = self.next_label("comparison_done");
        for jump in jumps {
            self.line(&format!("    {jump} {yes}"));
        }
        self.line("    clr r0");
        self.line(&format!("    b @{done}"));
        self.line(&format!("{yes}:"));
        self.line("    li r0, 1");
        self.line(&format!("{done}:"));
    }

    fn emit_array_index(&mut self, name: &str, index: &Expr) -> Result<(), Diagnostic> {
        let array_ty = self.model.resolved_type(&self.named_type(name)?)?;
        let Type::Array { element, len } = array_ty else {
            return Err(Diagnostic::new("indexing requires an array"));
        };
        let storage = self
            .model
            .globals
            .get(name)
            .copied()
            .ok_or_else(|| Diagnostic::new(format!("unknown const array `{name}`")))?;
        let element = *element;
        let element_size = self.model.type_size(&element)?;
        if let Ok(index_value) = self.model.const_value(index) {
            let length = self.model.const_value(&len)?;
            if index_value < 0 || index_value >= length {
                return Err(Diagnostic::new(format!(
                    "array index {index_value} is out of bounds for `{name}` length {length}"
                )));
            }
            let offset = u32::try_from(index_value)
                .ok()
                .and_then(|index| index.checked_mul(element_size))
                .ok_or_else(|| Diagnostic::new("array index offset overflow"))?;
            return self.load_address_r0(storage.address + offset, &element);
        }

        self.emit_expr(index, &Type::Named("u16".to_owned()))?;
        if element_size == 2 {
            self.line("    sla r0, 1");
        } else if element_size != 1 {
            return Err(Diagnostic::new(
                "MSP430 dynamic array indexing supports one- and two-byte elements only",
            ));
        }
        self.line(&format!("    li r1, >{:04X}", storage.address));
        self.line("    a r1, r0");
        self.load_indirect_r0(&element)?;
        Ok(())
    }

    fn load_ident(&mut self, name: &str, expected: &Type) -> Result<(), Diagnostic> {
        if let Some(binding) = self.binding(name) {
            return self.load_binding_r0(&binding);
        }
        if self.is_ti_cartridge() {
            if let Some(embed_name) = name.strip_suffix(".ptr")
                && self.model.embeds.contains_key(embed_name)
            {
                self.line(&format!("    li r0, {}", embed_label(embed_name)));
                return Ok(());
            }
            if let Some(embed_name) = name.strip_suffix(".end")
                && self.model.embeds.contains_key(embed_name)
            {
                self.line(&format!("    li r0, {}", embed_end_label(embed_name)));
                return Ok(());
            }
        }
        if let Some(value) = self.model.constants.get(name) {
            return self.load_immediate(*value);
        }
        if let Some(storage) = self.model.globals.get(name) {
            let ty = self
                .model
                .global_types
                .get(name)
                .unwrap_or(expected)
                .clone();
            return self.load_r0(*storage, &ty);
        }
        if let Some((address, ty, _)) = self.model.mmio.get(name) {
            return self.load_address_r0(*address, &ty.clone());
        }
        Err(Diagnostic::new(format!("unknown MSP430 value `{name}`")))
    }

    fn load_place(&mut self, place: &Place, ty: &Type) -> Result<(), Diagnostic> {
        match place {
            Place::Ident(name) => self.load_ident(name, ty),
            Place::Deref(pointer) => {
                self.emit_expr(pointer, &Type::Ptr(Box::new(ty.clone())))?;
                self.load_indirect_r0(ty)
            }
            _ => Err(Diagnostic::new(
                "this assignment target is not implemented by the initial MSP430 source backend",
            )),
        }
    }

    fn store_place(&mut self, place: &Place, ty: &Type) -> Result<(), Diagnostic> {
        match place {
            Place::Ident(name) => {
                if let Some(binding) = self.binding(name) {
                    return self.store_binding_r0(&binding);
                }
                if let Some(storage) = self.model.globals.get(name) {
                    let ty = self.model.global_types.get(name).unwrap_or(ty).clone();
                    return self.store_r0(*storage, &ty);
                }
                if let Some((address, target_ty, _)) = self.model.mmio.get(name) {
                    return self.store_r0_address(*address, &target_ty.clone());
                }
                Err(Diagnostic::new(format!(
                    "unknown MSP430 assignment target `{name}`"
                )))
            }
            Place::Deref(pointer) => {
                self.move_value(0, 1, ty);
                self.emit_expr(pointer, &Type::Ptr(Box::new(ty.clone())))?;
                match scalar_width(&self.model, ty)? {
                    1 => self.line("    movb r1, *r0"),
                    2 => self.line("    mov r1, *r0"),
                    4 if self.uses_native_20(ty) => self.line("    mov.a r1,*r0"),
                    _ => unreachable!("unsupported MSP430 scalar width"),
                }
                Ok(())
            }
            _ => Err(Diagnostic::new(
                "this assignment target is not implemented by the initial MSP430 source backend",
            )),
        }
    }

    fn place_type(&self, place: &Place) -> Result<Type, Diagnostic> {
        match place {
            Place::Ident(name) => {
                if let Some(binding) = self.binding(name) {
                    return Ok(binding.ty);
                }
                if let Some(ty) = self.model.global_types.get(name) {
                    return self.model.resolved_type(ty);
                }
                if let Some((_, ty, _)) = self.model.mmio.get(name) {
                    return self.model.resolved_type(ty);
                }
                Err(Diagnostic::new(format!(
                    "unknown MSP430 assignment target `{name}`"
                )))
            }
            Place::Deref(expr) => match self.model.resolved_type(&self.expr_type(expr)?)? {
                Type::Ptr(inner) => Ok(*inner),
                _ => Err(Diagnostic::new("dereference requires pointer")),
            },
            _ => Err(Diagnostic::new(
                "this assignment target is not implemented by the initial MSP430 source backend",
            )),
        }
    }

    fn expr_type(&self, expr: &Expr) -> Result<Type, Diagnostic> {
        match expr {
            Expr::Int(value) => Ok(if (0..=0xFF).contains(value) {
                Type::Named("u8".to_owned())
            } else {
                Type::Named("u16".to_owned())
            }),
            Expr::TypedInt(_, ty) | Expr::Cast { ty, .. } => self.model.resolved_type(ty),
            Expr::Bool(_) => Ok(Type::Named("bool".to_owned())),
            Expr::Char(_) | Expr::In(_) => Ok(Type::Named("u8".to_owned())),
            Expr::String(_) => Ok(Type::Ptr(Box::new(Type::Named("u8".to_owned())))),
            Expr::Ident(name) => self
                .model
                .constant_types
                .get(name)
                .cloned()
                .or_else(|| self.binding(name).map(|binding| binding.ty))
                .or_else(|| self.model.global_types.get(name).cloned())
                .ok_or_else(|| Diagnostic::new(format!("unknown value `{name}`"))),
            Expr::AddressOf(name) => {
                if let Some(function) = self.functions.get(name) {
                    if function.second_return_type.is_some() {
                        return Err(Diagnostic::new(format!(
                            "MSP430 function pointer cannot reference two-result function `{name}`"
                        )));
                    }
                    Ok(Type::Ptr(Box::new(Type::Function {
                        params: function
                            .params
                            .iter()
                            .map(|param| param.ty.clone())
                            .collect(),
                        return_type: function.return_type.clone().map(Box::new),
                    })))
                } else {
                    self.binding(name)
                        .map(|binding| binding.ty)
                        .or_else(|| self.model.global_types.get(name).cloned())
                        .map(|ty| Type::Ptr(Box::new(ty)))
                        .ok_or_else(|| Diagnostic::new(format!("unknown value `{name}`")))
                }
            }
            Expr::Field { base, field } => self
                .model
                .constant_types
                .get(&format!("{base}.{field}"))
                .cloned()
                .ok_or_else(|| Diagnostic::new(format!("unknown field `{field}`"))),
            Expr::Call { path, args } => {
                if let Some(resolution) = self.resolve_intrinsic(path, args)? {
                    return resolution.result_types.first().cloned().ok_or_else(|| {
                        Diagnostic::new(format!(
                            "MSP430 intrinsic `{}` does not return a value",
                            resolution.canonical_name()
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
                let name = path
                    .last()
                    .ok_or_else(|| Diagnostic::new("empty function call path"))?;
                let pointer_type = self.named_type(name)?;
                let pointer_type = self.model.resolved_type(&pointer_type)?;
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
                return_type
                    .map(|ty| *ty)
                    .ok_or_else(|| Diagnostic::new("void function has no value"))
            }
            Expr::Unary { expr, .. } | Expr::Binary { left: expr, .. } => self.expr_type(expr),
            Expr::Deref(expr) => match self.model.resolved_type(&self.expr_type(expr)?)? {
                Type::Ptr(inner) => Ok(*inner),
                _ => Err(Diagnostic::new("dereference requires pointer")),
            },
            Expr::Index { name, .. } => match self.model.resolved_type(&self.named_type(name)?)? {
                Type::Array { element, .. } => Ok(*element),
                _ => Err(Diagnostic::new("indexing requires an array")),
            },
            _ => Err(Diagnostic::new(
                "expression type is not implemented by the initial MSP430 source backend",
            )),
        }
    }

    fn load_indirect_r0(&mut self, ty: &Type) -> Result<(), Diagnostic> {
        match scalar_width(&self.model, ty)? {
            1 => {
                let pointer = Type::Ptr(Box::new(ty.clone()));
                self.move_value(0, 1, &pointer);
                self.line("    clr r0");
                self.line("    movb *r1, r0");
            }
            2 => self.line("    mov *r0, r0"),
            4 if self.uses_native_20(ty) => self.line("    mov.a @r0,r0"),
            _ => unreachable!("unsupported MSP430 scalar width"),
        }
        self.mask_value(ty);
        Ok(())
    }

    fn load_binding_r0(&mut self, binding: &Binding) -> Result<(), Diagnostic> {
        match binding.location {
            BindingLocation::Frame(offset) => self.load_frame_r0(offset, &binding.ty),
            BindingLocation::Register(register) => {
                self.move_value(register, 0, &binding.ty);
                self.mask_value(&binding.ty);
                Ok(())
            }
        }
    }

    fn store_binding_r0(&mut self, binding: &Binding) -> Result<(), Diagnostic> {
        match binding.location {
            BindingLocation::Frame(offset) => self.store_frame_r0(offset, &binding.ty),
            BindingLocation::Register(register) => {
                self.mask_value(&binding.ty);
                self.move_value(0, register, &binding.ty);
                Ok(())
            }
        }
    }

    fn load_frame_r0(&mut self, offset: i16, ty: &Type) -> Result<(), Diagnostic> {
        if self.uses_native_20(ty) {
            self.line(&format!("    mov.a @>{:04X}(r9),r0", offset as u16));
        } else {
            self.line(&format!("    mov @>{:04X}(r9), r0", offset as u16));
        }
        self.mask_value(ty);
        Ok(())
    }

    fn load_argument_r0(&mut self, offset: u16, ty: &Type) -> Result<(), Diagnostic> {
        if self.uses_native_20(ty) {
            self.line(&format!("    mov.a @>{offset:04X}(r10),r0"));
        } else {
            self.line(&format!("    mov @>{offset:04X}(r10),r0"));
        }
        self.mask_value(ty);
        Ok(())
    }

    fn store_argument_r0(&mut self, offset: u16, ty: &Type) -> Result<(), Diagnostic> {
        if self.uses_native_20(ty) {
            self.line(&format!("    mov.a r0,@>{offset:04X}(r10)"));
        } else {
            self.line(&format!("    mov r0,@>{offset:04X}(r10)"));
        }
        Ok(())
    }

    fn store_frame_r0(&mut self, offset: i16, ty: &Type) -> Result<(), Diagnostic> {
        self.mask_value(ty);
        if self.uses_native_20(ty) {
            self.line(&format!("    mov.a r0,@>{:04X}(r9)", offset as u16));
        } else {
            self.line(&format!("    mov r0, @>{:04X}(r9)", offset as u16));
        }
        Ok(())
    }

    fn adjust_stack(&mut self, amount: i32) {
        if amount != 0 {
            self.line(&format!("    ai r10, >{:04X}", amount as i16 as u16));
        }
    }

    fn load_r0(&mut self, storage: Storage, ty: &Type) -> Result<(), Diagnostic> {
        self.load_register(storage, ty, 0)
    }

    fn load_register(
        &mut self,
        storage: Storage,
        ty: &Type,
        register: u8,
    ) -> Result<(), Diagnostic> {
        self.load_address_register(storage.address, ty, register)
    }

    fn load_address_r0(&mut self, address: u32, ty: &Type) -> Result<(), Diagnostic> {
        self.load_address_register(address, ty, 0)
    }

    fn load_address_register(
        &mut self,
        address: u32,
        ty: &Type,
        register: u8,
    ) -> Result<(), Diagnostic> {
        let register = format!("r{register}");
        match scalar_width(&self.model, ty)? {
            1 => {
                self.line(&format!("    clr {register}"));
                self.line(&format!("    movb @>{address:04X}, {register}"));
            }
            2 => self.line(&format!("    mov @>{address:04X}, {register}")),
            4 if self.uses_native_20(ty) => {
                self.line(&format!("    mov.a @>{address:05X},{register}"))
            }
            _ => {
                return Err(Diagnostic::new(
                    "MSP430 source values must fit in the target width",
                ));
            }
        }
        Ok(())
    }

    fn store_r0(&mut self, storage: Storage, ty: &Type) -> Result<(), Diagnostic> {
        self.store_r0_address(storage.address, ty)
    }

    fn store_r0_address(&mut self, address: u32, ty: &Type) -> Result<(), Diagnostic> {
        match scalar_width(&self.model, ty)? {
            1 => self.line(&format!("    movb r0, @>{address:04X}")),
            2 => self.line(&format!("    mov r0, @>{address:04X}")),
            4 if self.uses_native_20(ty) => self.line(&format!("    mov.a r0,@>{address:05X}")),
            _ => {
                return Err(Diagnostic::new(
                    "MSP430 source values must fit in the target width",
                ));
            }
        }
        Ok(())
    }

    fn load_immediate(&mut self, value: i64) -> Result<(), Diagnostic> {
        if !(-32768..=65535).contains(&value) {
            return Err(Diagnostic::new(format!(
                "MSP430 immediate `{value}` is outside 16 bits"
            )));
        }
        self.line(&format!("    li r0, >{:04X}", value as u16));
        Ok(())
    }

    fn bind(&mut self, name: String, binding: Binding) -> Result<(), Diagnostic> {
        let scope = self
            .scopes
            .last_mut()
            .ok_or_else(|| Diagnostic::new("local binding outside function"))?;
        if scope.insert(name.clone(), binding).is_some() {
            return Err(Diagnostic::new(format!("duplicate local `{name}`")));
        }
        Ok(())
    }

    fn binding(&self, name: &str) -> Option<Binding> {
        self.scopes
            .iter()
            .rev()
            .find_map(|scope| scope.get(name).cloned())
    }

    fn named_type(&self, name: &str) -> Result<Type, Diagnostic> {
        self.binding(name)
            .map(|binding| binding.ty)
            .or_else(|| self.model.global_types.get(name).cloned())
            .ok_or_else(|| Diagnostic::new(format!("unknown MSP430 value `{name}`")))
    }

    fn jump_loop(&mut self, continue_loop: bool) -> Result<(), Diagnostic> {
        let labels = self
            .loops
            .last()
            .ok_or_else(|| {
                Diagnostic::new(if continue_loop {
                    "continue outside loop"
                } else {
                    "break outside loop"
                })
            })?
            .clone();
        self.line(&format!(
            "    b @{}",
            if continue_loop {
                labels.continue_label
            } else {
                labels.break_label
            }
        ));
        Ok(())
    }

    fn next_label(&mut self, stem: &str) -> String {
        let label = format!("__ezra_{stem}_{}", self.labels);
        self.labels += 1;
        label
    }

    fn line(&mut self, line: &str) {
        let translated = translate_msp430_line(line);
        self.out.push_str(&translated);
        if !translated.ends_with('\n') {
            self.out.push('\n');
        }
    }

    fn configured_stack_top(&self) -> Result<u32, Diagnostic> {
        let maximum = if self.native_20() { 0xF_FFFF } else { 0xFFFF };
        let stack_top = self.options.stack_top.get();
        if stack_top > maximum {
            return Err(Diagnostic::new(format!(
                "MSP430 stack_top 0x{stack_top:X} exceeds the target address space"
            )));
        }
        Ok(stack_top & !1)
    }

    fn native_20(&self) -> bool {
        self.options
            .cpu
            .capabilities()
            .native_int_widths
            .contains(&20)
    }

    fn is_20_type(&self, ty: &Type) -> bool {
        matches!(
            self.model.resolved_type(ty).ok(),
            Some(Type::Named(name)) if name == "u20" || name == "i20"
        )
    }

    fn is_pointer_type(&self, ty: &Type) -> bool {
        matches!(self.model.resolved_type(ty).ok(), Some(Type::Ptr(_)))
    }

    fn uses_native_20(&self, ty: &Type) -> bool {
        self.native_20() && (self.is_20_type(ty) || self.is_pointer_type(ty))
    }

    fn move_value(&mut self, source: u8, destination: u8, ty: &Type) {
        if self.uses_native_20(ty) {
            self.line(&format!("    mov.a r{source}, r{destination}"));
        } else {
            self.line(&format!("    mov r{source}, r{destination}"));
        }
    }

    fn push_value(&mut self, ty: &Type) {
        if self.uses_native_20(ty) {
            self.line("    add.a #0xFFFFC,r10");
            self.line("    mov.a r0,0(r10)");
        } else {
            self.line("    dect r10");
            self.line("    mov r0, *r10");
        }
    }

    fn pop_value_to(&mut self, register: u8, ty: &Type) {
        if self.uses_native_20(ty) {
            self.line(&format!("    mov.a 0(r10),r{register}"));
            self.line("    add.a #4,r10");
        } else {
            self.line(&format!("    mov *r10+, r{register}"));
        }
    }

    fn mask_value(&mut self, ty: &Type) {
        if self.uses_native_20(ty) {
            self.line("    and.a #0xFFFFF,r0");
        } else if scalar_width(&self.model, ty).ok() == Some(1) {
            self.line("    andi r0, >00FF");
        }
    }

    fn load_immediate_typed(&mut self, value: i64, ty: &Type) -> Result<(), Diagnostic> {
        if self.uses_native_20(ty) {
            let value = (value as i128 & 0x0F_FFFF) as u32;
            self.line(&format!("    mov.a #0x{value:05X},r0"));
            Ok(())
        } else if self.is_20_type(ty) {
            Err(Diagnostic::new(
                "u20 and i20 values require an MSP430X or MSP430X2 target",
            ))
        } else {
            self.load_immediate(value)
        }
    }

    fn load_address_label(&mut self, label: &str, ty: &Type) {
        if self.uses_native_20(ty) {
            self.line(&format!("    mov.a #{label},r0"));
        } else {
            self.line(&format!("    mov #{label},r0"));
        }
    }

    fn line_typed(&mut self, line: &str, ty: &Type) {
        if !self.uses_native_20(ty) {
            self.line(line);
            return;
        }
        let trimmed = line.trim();
        let Some((mnemonic, operands)) = trimmed.split_once(char::is_whitespace) else {
            self.line(line);
            return;
        };
        let mapped = match mnemonic {
            "a" => "add.a",
            "s" => "sub.a",
            "c" | "ci" => "cmp.a",
            "soc" => "bis.a",
            "szc" => "bic.a",
            "mov" => "mov.a",
            "xor" => "xor.a",
            "and" => "and.a",
            "andi" => "and.a",
            "ori" => "bis.a",
            "inc" => "add.a",
            "dec" => "sub.a",
            "sra" => "rra.a",
            "srl" => "rrc.a",
            "sla" => "add.a",
            "inv" => {
                self.line("    xor.a #0xFFFFF,r0");
                return;
            }
            "neg" => {
                self.line("    xor.a #0xFFFFF,r0");
                self.line("    add.a #1,r0");
                return;
            }
            _ => {
                self.line(line);
                return;
            }
        };
        let operands = if matches!(mnemonic, "ci" | "andi" | "ori" | "inc" | "dec") {
            operands
                .split_once(',')
                .map(|(left, right)| format!("{},{}", right.trim(), left.trim()))
                .unwrap_or_else(|| operands.trim().to_owned())
        } else {
            operands.trim().to_owned()
        };
        self.line(&format!("    {mapped} {operands}"));
    }
}

/// Translate the compact internal instruction selection used by this initial
/// 16-bit backend into real MSP430 syntax. Keeping this at the output boundary
/// avoids duplicating the source lowering and register-allocation logic while
/// still making the generated assembly usable by the MSP430 assembler.
fn translate_msp430_line(line: &str) -> String {
    let (code, comment) = line.split_once(';').unwrap_or((line, ""));
    let trimmed = code.trim();
    if trimmed.is_empty() || trimmed.ends_with(':') || trimmed.starts_with("section ") {
        return line.to_owned();
    }

    let text = map_msp430_registers(code.trim());

    let mut parts = text.splitn(2, char::is_whitespace);
    let mnemonic = parts.next().unwrap_or_default().to_ascii_lowercase();
    let operands = parts.next().unwrap_or_default().trim();
    let mut translated = match mnemonic.as_str() {
        "li" => format!("mov #{}", translate_msp430_value(operands, true)),
        "ai" => format!("add #{}", translate_msp430_value(operands, true)),
        "andi" => format!("and #{}", translate_msp430_value(operands, true)),
        "ori" => format!("bis #{}", translate_msp430_value(operands, true)),
        "ci" => format!("cmp #{}", translate_msp430_value(operands, true)),
        "a" => format!("add {}", translate_msp430_operands(operands)),
        "s" => format!("sub {}", translate_msp430_operands(operands)),
        "c" => format!("cmp {}", translate_msp430_operands(operands)),
        "soc" => format!("bis {}", translate_msp430_operands(operands)),
        "szc" => format!("bic {}", translate_msp430_operands(operands)),
        "movb" => format!("mov.b {}", translate_msp430_operands(operands)),
        "dect" => format!("sub #2,{}", translate_msp430_operand(operands)),
        "inv" => format!("xor #0xffff,{}", translate_msp430_operand(operands)),
        "neg" => format!(
            "xor #0xffff,{}\n    inc {}",
            translate_msp430_operand(operands),
            translate_msp430_operand(operands)
        ),
        "b" => format!("jmp {}", translate_msp430_branch_operand(operands)),
        "bl" if operands.trim_start().starts_with('*') => {
            format!("call {}", translate_msp430_branch_operand(operands))
        }
        "bl" => format!("call #{}", translate_msp430_branch_operand(operands)),
        "jhe" => format!("jhs {}", translate_msp430_operand(operands)),
        "joc" => format!("jc {}", translate_msp430_operand(operands)),
        "jno" => format!("jnc {}", translate_msp430_operand(operands)),
        "jgt" => format!("jge {}", translate_msp430_operand(operands)),
        "jle" => format!("jlo {}", translate_msp430_operand(operands)),
        "jh" => format!("jhs {}", translate_msp430_operand(operands)),
        "jlt" => format!("jl {}", translate_msp430_operand(operands)),
        _ => {
            if is_msp430_shift_pseudo(&mnemonic, operands) {
                translate_msp430_shift(&mnemonic, operands)
            } else {
                let operands = translate_msp430_operands(operands);
                if operands.is_empty() {
                    mnemonic.clone()
                } else {
                    format!("{mnemonic} {operands}")
                }
            }
        }
    };
    if !comment.is_empty() {
        translated.push_str(" ;");
        translated.push_str(comment);
    }
    translated
}

fn map_msp430_registers(text: &str) -> String {
    const REGISTER_MAP: [u8; 12] = [4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 1, 14];
    let chars = text.chars().collect::<Vec<_>>();
    let mut output = String::with_capacity(text.len());
    let mut index = 0;
    while index < chars.len() {
        let is_register = chars[index] == 'r'
            && chars
                .get(index + 1)
                .is_some_and(|character| character.is_ascii_digit())
            && (index == 0 || !chars[index - 1].is_ascii_alphanumeric());
        if !is_register {
            output.push(chars[index]);
            index += 1;
            continue;
        }
        let mut end = index + 1;
        while end < chars.len() && chars[end].is_ascii_digit() {
            end += 1;
        }
        let number = chars[index + 1..end]
            .iter()
            .collect::<String>()
            .parse::<usize>()
            .ok();
        if let Some(number) = number.and_then(|number| REGISTER_MAP.get(number).copied()) {
            output.push('r');
            output.push_str(&number.to_string());
            index = end;
        } else {
            for character in &chars[index..end] {
                output.push(*character);
            }
            index = end;
        }
    }
    output
}

fn translate_msp430_value(text: &str, immediate: bool) -> String {
    let text = translate_msp430_operands(text);
    if immediate {
        text.split_once(',')
            .map(|(register, value)| {
                format!(
                    "{},{}",
                    translate_msp430_operand(value),
                    translate_msp430_operand(register)
                )
            })
            .unwrap_or_else(|| translate_msp430_operand(&text))
    } else {
        translate_msp430_operand(&text)
    }
}

fn translate_msp430_operands(text: &str) -> String {
    text.split(',')
        .map(translate_msp430_operand)
        .collect::<Vec<_>>()
        .join(",")
}

fn translate_msp430_operand(text: &str) -> String {
    let text = text.trim();
    if let Some(value) = text.strip_prefix('@') {
        if value.starts_with('r') {
            return format!("@{value}");
        }
        if let Some(value) = value.strip_prefix('>') {
            let value = format!("0x{value}");
            return if value.contains('(') {
                value
            } else {
                format!("&{value}")
            };
        }
    }
    if let Some(value) = text.strip_prefix('*') {
        return value
            .strip_suffix('+')
            .map_or_else(|| format!("0({value})"), |value| format!("@{value}+"));
    }
    text.replace('>', "0x")
}

fn translate_msp430_branch_operand(text: &str) -> String {
    let text = text.trim();
    text.strip_prefix('@')
        .or_else(|| text.strip_prefix('*'))
        .map_or_else(|| text.to_owned(), ToOwned::to_owned)
}

fn is_msp430_shift_pseudo(mnemonic: &str, operands: &str) -> bool {
    matches!(mnemonic, "sra" | "srl" | "sla") && operands.split_once(',').is_some()
}

fn translate_msp430_shift(mnemonic: &str, operands: &str) -> String {
    let (register, count) = operands.split_once(',').unwrap_or((operands, "0"));
    let register = translate_msp430_operand(register);
    let count = count.trim().parse::<usize>().unwrap_or(1);
    let instruction = match mnemonic {
        "sra" => "rra",
        "sla" => "add",
        "srl" => "rrc",
        _ => unreachable!(),
    };
    if count == 0 {
        return format!("    {instruction} {register}");
    }
    (0..count)
        .map(|_| match mnemonic {
            "sra" => format!("    rra {register}"),
            "sla" => format!("    add {register},{register}"),
            "srl" => format!("    bic #1,sr\n    rrc {register}"),
            _ => unreachable!(),
        })
        .collect::<Vec<_>>()
        .join("\n")
}

const MSP430_LOCAL_REGISTERS: [u8; 3] = [6, 7, 8];
const MSP430_SCALAR_WORD_CLASS: RegClass = RegClass(0);
const MSP430_STACK_SPILL_CLASS: SpillClassId = SpillClassId(0);

fn msp430_local_target() -> Target {
    Target {
        units: MSP430_LOCAL_REGISTERS
            .iter()
            .map(|register| RegisterUnit::new(format!("r{register}")))
            .collect(),
        registers: MSP430_LOCAL_REGISTERS
            .iter()
            .enumerate()
            .map(|(unit, register)| {
                PhysicalRegister::new(format!("r{register}"), vec![RegUnit(unit)])
            })
            .collect(),
        register_classes: vec![RegisterClass::new(
            "scalar-word",
            (0..MSP430_LOCAL_REGISTERS.len()).map(PhysReg).collect(),
        )],
        spill_classes: vec![
            SpillClass::new("stack", None, 1)
                .with_base_alignment(2)
                .for_register_classes(vec![MSP430_SCALAR_WORD_CLASS]),
        ],
    }
}

fn plan_function_frame(
    function: &Function,
    model: &SemanticModel,
) -> Result<FunctionFrame, Diagnostic> {
    let mut source_locals = Vec::new();
    let mut local_types = HashMap::new();
    collect_frame_locals(&function.body, model, &mut source_locals, &mut local_types)?;

    let clobbers = (0..MSP430_LOCAL_REGISTERS.len())
        .map(PhysReg)
        .collect::<Vec<_>>();
    let planned = allocate_source_locals(
        &msp430_local_target(),
        &source_locals,
        &function.body,
        &clobbers,
    )
    .map_err(|diagnostics| {
        Diagnostic::new(
            diagnostics
                .into_iter()
                .map(|diagnostic| diagnostic.message)
                .collect::<Vec<_>>()
                .join("; "),
        )
    })?;

    let mut spill_bytes = 0;
    for slot in &planned.allocation.spill_slots {
        let end = slot
            .offset
            .checked_add(slot.size)
            .ok_or_else(|| Diagnostic::new("MSP430 function frame is too large"))?;
        spill_bytes = spill_bytes.max(end);
    }
    if spill_bytes > 1 << 15 {
        return Err(Diagnostic::new(
            "MSP430 function frame exceeds the 16-bit signed frame displacement",
        ));
    }
    let local_bytes = u16::try_from(spill_bytes)
        .map_err(|_| Diagnostic::new("MSP430 function frame is too large"))?;
    let mut locals = HashMap::new();
    for (name, ty) in local_types {
        let vreg = planned
            .locals
            .vreg(&name)
            .ok_or_else(|| Diagnostic::new(format!("missing allocation for local `{name}`")))?;
        let location = match planned.allocation.location(vreg) {
            Some(Location::Register(register)) => {
                let register = *MSP430_LOCAL_REGISTERS.get(register.0).ok_or_else(|| {
                    Diagnostic::new(format!("invalid MSP430 local register for `{name}`"))
                })?;
                BindingLocation::Register(register)
            }
            Some(Location::Spill(slot_index)) => {
                let slot = planned
                    .allocation
                    .spill_slots
                    .get(slot_index)
                    .ok_or_else(|| Diagnostic::new(format!("invalid spill slot for `{name}`")))?;
                debug_assert_eq!(slot.class, MSP430_STACK_SPILL_CLASS);
                let end = slot
                    .offset
                    .checked_add(slot.size)
                    .ok_or_else(|| Diagnostic::new("MSP430 function frame is too large"))?;
                let offset = i32::try_from(end)
                    .ok()
                    .and_then(|end| end.checked_neg())
                    .and_then(|offset| i16::try_from(offset).ok())
                    .ok_or_else(|| {
                        Diagnostic::new(
                            "MSP430 function frame exceeds the 16-bit signed frame displacement",
                        )
                    })?;
                BindingLocation::Frame(offset)
            }
            Some(Location::Unused) | None => {
                return Err(Diagnostic::new(format!(
                    "source allocator did not place local `{name}`"
                )));
            }
        };
        locals.insert(name, Binding { location, ty });
    }

    Ok(FunctionFrame {
        locals,
        local_bytes,
    })
}

fn collect_frame_locals(
    body: &[Stmt],
    model: &SemanticModel,
    locals: &mut Vec<SourceLocal>,
    local_types: &mut HashMap<String, Type>,
) -> Result<(), Diagnostic> {
    for stmt in body {
        match stmt {
            Stmt::Let { name, ty, .. } => {
                let ty = model.resolved_type(ty)?;
                let aggregate = matches!(&ty, Type::Array { .. })
                    || matches!(&ty, Type::Named(name) if model.structs.contains_key(name));
                let size = if aggregate {
                    model
                        .type_size(&ty)?
                        .checked_add(1)
                        .map(|size| size & !1)
                        .ok_or_else(|| Diagnostic::new("MSP430 function frame is too large"))?
                        .max(2)
                } else {
                    let width = scalar_width(model, &ty)?;
                    if width == 4 { 4 } else { 2 }
                };
                if local_types.insert(name.clone(), ty).is_some() {
                    return Err(Diagnostic::new(format!("duplicate local `{name}`")));
                }
                locals.push(
                    SourceLocal::new(name.clone(), size, 2, MSP430_SCALAR_WORD_CLASS)
                        .with_spill_classes(vec![MSP430_STACK_SPILL_CLASS])
                        .with_force_memory(aggregate),
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
                    let ty = model.resolved_type(ty)?;
                    let aggregate = matches!(&ty, Type::Array { .. })
                        || matches!(&ty, Type::Named(name) if model.structs.contains_key(name));
                    let size = if aggregate {
                        model
                            .type_size(&ty)?
                            .checked_add(1)
                            .map(|size| size & !1)
                            .ok_or_else(|| Diagnostic::new("MSP430 function frame is too large"))?
                            .max(2)
                    } else {
                        scalar_width(model, &ty)?;
                        2
                    };
                    if local_types.insert(name.clone(), ty).is_some() {
                        return Err(Diagnostic::new(format!("duplicate local `{name}`")));
                    }
                    locals.push(
                        SourceLocal::new(name.clone(), size, 2, MSP430_SCALAR_WORD_CLASS)
                            .with_spill_classes(vec![MSP430_STACK_SPILL_CLASS])
                            .with_force_memory(aggregate),
                    );
                }
            }
            Stmt::If {
                then_body,
                else_body,
                ..
            } => {
                collect_frame_locals(then_body, model, locals, local_types)?;
                collect_frame_locals(else_body, model, locals, local_types)?;
            }
            Stmt::While { body, .. } | Stmt::Loop { body } => {
                collect_frame_locals(body, model, locals, local_types)?;
            }
            _ => {}
        }
    }
    Ok(())
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

fn referenced_function_names(program: &Program, model: &SemanticModel) -> HashSet<String> {
    let mut references = HashSet::new();
    for declaration in &program.declarations {
        match declaration {
            Declaration::Function(function) => {
                collect_stmt_function_references(&function.body, &mut references)
            }
            Declaration::Global(global) => {
                collect_expr_function_references(&global.value, &mut references)
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
        Expr::Array(values) => values
            .iter()
            .for_each(|value| collect_expr_function_references(value, references)),
        Expr::Index { index, .. } | Expr::AddressOfIndex { index, .. } => {
            collect_expr_function_references(index, references)
        }
        Expr::Access(path) | Expr::AddressOfAccess(path) => {
            for segment in &path.segments {
                if let AccessSegment::Index(index) = segment {
                    collect_expr_function_references(index, references);
                }
            }
        }
        Expr::StructInit { fields, .. } => fields
            .iter()
            .for_each(|(_, value)| collect_expr_function_references(value, references)),
        Expr::Deref(value)
        | Expr::BankedPointer { pointer: value, .. }
        | Expr::Unary { expr: value, .. }
        | Expr::Cast { expr: value, .. } => collect_expr_function_references(value, references),
        Expr::Call { args, .. } => args
            .iter()
            .for_each(|arg| collect_expr_function_references(arg, references)),
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

fn recursive_function_names(program: &Program, model: &SemanticModel) -> HashSet<String> {
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
                recursive.insert(caller.clone());
                recursive.insert(callee.clone());
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

fn resolve_called_function(path: &[String], model: &SemanticModel) -> Option<String> {
    path.last()
        .filter(|name| model.functions.contains_key(*name))
        .cloned()
}

fn collect_stmt_calls(stmts: &[Stmt], calls: &mut Vec<Vec<String>>) {
    for stmt in stmts {
        match stmt {
            Stmt::Let { value, .. } | Stmt::Expr(value) => {
                collect_expr_calls(value, calls);
            }
            Stmt::LetTwo { value, .. } | Stmt::Return(Some(value)) => {
                collect_expr_calls(value, calls);
            }
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
        Expr::Binary { left, op, right } => {
            collect_expr_calls(left, calls);
            if !matches!(
                (left.as_ref(), *op),
                (Expr::Bool(false), BinaryOp::And) | (Expr::Bool(true), BinaryOp::Or)
            ) {
                collect_expr_calls(right, calls);
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
        | Expr::AddressOfField { .. }
        | Expr::AddressOf(_) => {}
    }
}

fn collect_access_path_calls(path: &AccessPath, calls: &mut Vec<Vec<String>>) {
    for segment in &path.segments {
        if let AccessSegment::Index(index) = segment {
            collect_expr_calls(index, calls);
        }
    }
}

fn collect_program_strings(
    program: &Program,
    reachable: &HashSet<String>,
    values: &mut HashSet<String>,
) {
    for declaration in &program.declarations {
        match declaration {
            Declaration::Function(function) if reachable.contains(&function.name) => {
                collect_stmt_strings(&function.body, values);
            }
            Declaration::Global(global) => collect_expr_strings(&global.value, values),
            _ => {}
        }
    }
}

fn collect_stmt_strings(body: &[Stmt], values: &mut HashSet<String>) {
    for stmt in body {
        match stmt {
            Stmt::Let { value, .. } | Stmt::Expr(value) => {
                collect_expr_strings(value, values);
            }
            Stmt::LetTwo { value, .. } | Stmt::Return(Some(value)) => {
                collect_expr_strings(value, values);
            }
            Stmt::ReturnTwo { first, second } => {
                collect_expr_strings(first, values);
                collect_expr_strings(second, values);
            }
            Stmt::Assign { target, value, .. } => {
                match target {
                    Place::Index { index, .. } | Place::Deref(index) => {
                        collect_expr_strings(index, values);
                    }
                    Place::Access(path) => collect_access_path_strings(path, values),
                    Place::Ident(_) | Place::Field { .. } => {}
                }
                collect_expr_strings(value, values);
            }
            Stmt::If {
                condition,
                then_body,
                else_body,
            } => {
                collect_expr_strings(condition, values);
                collect_stmt_strings(then_body, values);
                collect_stmt_strings(else_body, values);
            }
            Stmt::While { condition, body } => {
                collect_expr_strings(condition, values);
                collect_stmt_strings(body, values);
            }
            Stmt::Loop { body } => collect_stmt_strings(body, values),
            Stmt::Out { value, .. } => collect_expr_strings(value, values),
            Stmt::Return(None) | Stmt::Break | Stmt::Continue | Stmt::Asm { .. } => {}
        }
    }
}

fn collect_expr_strings(expr: &Expr, values: &mut HashSet<String>) {
    match expr {
        Expr::String(value) => {
            values.insert(value.clone());
        }
        Expr::Array(items) => {
            for item in items {
                collect_expr_strings(item, values);
            }
        }
        Expr::Index { index, .. } | Expr::AddressOfIndex { index, .. } => {
            collect_expr_strings(index, values);
        }
        Expr::Access(path) | Expr::AddressOfAccess(path) => {
            collect_access_path_strings(path, values);
        }
        Expr::StructInit { fields, .. } => {
            for (_, value) in fields {
                collect_expr_strings(value, values);
            }
        }
        Expr::Deref(value)
        | Expr::BankedPointer { pointer: value, .. }
        | Expr::Unary { expr: value, .. }
        | Expr::Cast { expr: value, .. } => {
            collect_expr_strings(value, values);
        }
        Expr::Call { args, .. } => {
            for arg in args {
                collect_expr_strings(arg, values);
            }
        }
        Expr::Binary { left, right, .. } => {
            collect_expr_strings(left, values);
            collect_expr_strings(right, values);
        }
        Expr::Int(_)
        | Expr::TypedInt(_, _)
        | Expr::Bool(_)
        | Expr::Char(_)
        | Expr::Ident(_)
        | Expr::In(_)
        | Expr::Field { .. }
        | Expr::AddressOfField { .. }
        | Expr::AddressOf(_) => {}
    }
}

fn collect_access_path_strings(path: &AccessPath, values: &mut HashSet<String>) {
    for segment in &path.segments {
        if let AccessSegment::Index(index) = segment {
            collect_expr_strings(index, values);
        }
    }
}

fn msp430_intrinsic_integer_bits(ty: &Type) -> Result<u16, Diagnostic> {
    match ty {
        Type::Named(name) => match name.as_str() {
            "u8" | "i8" => Ok(8),
            "u16" | "i16" => Ok(16),
            "u20" | "i20" => Ok(20),
            _ => Err(Diagnostic::new(format!(
                "intrinsic integer operation does not support type `{name}`"
            ))),
        },
        _ => Err(Diagnostic::new(
            "intrinsic integer operation requires an exact-width integer",
        )),
    }
}

fn msp430_intrinsic_integer_signed(ty: &Type) -> bool {
    matches!(ty, Type::Named(name) if matches!(name.as_str(), "i8" | "i16" | "i20"))
}

fn msp430_intrinsic_integer_mask(bits: u16) -> u16 {
    if bits >= 16 {
        u16::MAX
    } else {
        (1u16 << bits) - 1
    }
}

fn scalar_width(model: &SemanticModel, ty: &Type) -> Result<u8, Diagnostic> {
    match model.type_width(ty)? {
        1..=4 => model.type_width(ty),
        _ => Err(Diagnostic::new("MSP430 source values must fit in 32 bits")),
    }
}

fn abi_slot_bytes(model: &SemanticModel, ty: &Type) -> Result<u16, Diagnostic> {
    Ok(if scalar_width(model, ty)? == 4 { 4 } else { 2 })
}

fn type_is_signed(ty: &Type) -> bool {
    matches!(ty, Type::Named(name) if matches!(name.as_str(), "i8" | "i16" | "i20"))
}

fn constant_shift_count(expr: &Expr) -> Result<Option<u16>, Diagnostic> {
    let value = match expr {
        Expr::Int(value) | Expr::TypedInt(value, _) => *value,
        _ => return Ok(None),
    };
    u16::try_from(value).map(Some).map_err(|_| {
        Diagnostic::new(format!(
            "MSP430 shift count {value} is outside supported range 0..=65535"
        ))
    })
}

fn assign_binary(op: AssignOp) -> BinaryOp {
    match op {
        AssignOp::Set => unreachable!("set assignments bypass binary lowering"),
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

fn msp430_compact_wrapper_candidate(function: &Function) -> bool {
    let Some(body) = inline_void_body(function) else {
        return false;
    };
    body.is_empty()
        || matches!(
            body,
            [Stmt::Expr(Expr::Call { args, .. })] if args.is_empty()
        )
}

fn sanitize_label(name: &str) -> String {
    name.replace(|ch: char| !ch.is_ascii_alphanumeric() && ch != '_', "_")
}

fn function_label(name: &str) -> String {
    format!("_{}", sanitize_label(name))
}

fn embed_label(name: &str) -> String {
    format!("__ezra_embed_{}", sanitize_label(name))
}

fn embed_end_label(name: &str) -> String {
    format!("{}_end", embed_label(name))
}

#[cfg(test)]
mod tests;
