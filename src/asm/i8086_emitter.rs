//! Generic Intel 8086 EZRA source emitter.
//!
//! The ABI deliberately mirrors the other small generic backends: arguments and
//! locals have compiler-owned static addresses, scalar expression results live
//! in `r0`, and a caller saves its live static frame around a call.  Pointers are
//! 16-bit offsets in the single segment established by the startup stub.

use crate::{
    asm::{
        AssemblyOptions,
        comments::{stmt_summary, with_readability_comments},
        i8086::instruction_len,
        reachability::{RoutineProfile, strip_unreachable_generated_routines},
    },
    ast::{
        AccessPath, AccessSegment, AssignOp, BinaryOp, Declaration, Expr, Function, Place, Program,
        Stmt, Type, UnaryOp,
    },
    compat::prelude::*,
    diagnostic::Diagnostic,
    hir::HirProgram,
    intrinsics::{BitsIntrinsic, CATALOG, IntIntrinsic, IntrinsicOperation, MemIntrinsic},
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

const SCRATCH_BYTES: u32 = 8;

/// Lowers an EZRA AST through HIR and TBIR and emits strict original-8086
/// assembly. The generated image is one flat segment; CS is copied to DS and ES
/// before static initialization and all calls are near calls. Bare targets also initialize SS:SP;
/// DOS `.COM` programs preserve the loader-provided stack.
pub fn emit_i8086_assembly_with_options(
    program: &Program,
    options: AssemblyOptions,
) -> Result<String, Diagnostic> {
    if options.cpu != CpuFamily::I8086 {
        return Err(Diagnostic::new("i8086 emitter requires an i8086 target"));
    }
    let hir = HirProgram::from_ast(program)?;
    let tbir = TbirProgram::lower(&hir, program, &options)?;
    let model = SemanticModel::from_program(
        &tbir.lowered_program,
        16,
        options.ram_base.get(),
        options.rodata_base.get(),
        options.asset_base.get(),
    )?;
    Emitter::new(model, options.clone())?
        .emit(&tbir.lowered_program, program)
        .map(|asm| {
            let asm = strip_unreachable_generated_routines(&asm, RoutineProfile::I8086);
            let asm = relax_i8086_branches(&asm);
            with_readability_comments(asm, program, &options, "i8086", &tbir.source_comments)
        })
}

#[derive(Clone, Copy)]
enum BindingLocation {
    Static(Storage),
    Bp,
}

#[derive(Clone)]
struct Binding {
    location: BindingLocation,
    ty: Type,
}

#[derive(Clone, Copy)]
enum PlannedLocation {
    Bp,
    Spill(usize),
}

struct PlannedLocal {
    location: PlannedLocation,
    ty: Type,
}

struct FunctionLocals {
    bindings: HashMap<String, PlannedLocal>,
    spill_sizes: Vec<u32>,
}

#[derive(Clone)]
struct LoopLabels {
    continue_label: String,
    break_label: String,
}

enum Address {
    Direct(u32),
    Indirect,
}

#[derive(Clone, Copy)]
enum MaterializedPlace {
    Direct(u32),
    Indirect(Storage),
    Bp,
}

#[derive(Clone, Copy, Default)]
struct FunctionState {
    interrupt: bool,
    naked: bool,
}

enum AccessRoot {
    Storage(Binding),
    Constant { address: u32, ty: Type },
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
    second_return_pointers: Vec<Option<Storage>>,
    function_ram_bases: Vec<u32>,
    function_states: Vec<FunctionState>,
    current_live_after: Vec<HashSet<String>>,
    interrupt_functions: HashSet<String>,
    memory_barrier_functions: HashSet<String>,
    ports: HashMap<String, u8>,
    r0: Storage,
    r1: Storage,
    r2: Storage,
    function_pointer_slots: Vec<(Type, Vec<Storage>)>,
}

impl Emitter {
    fn new(mut model: SemanticModel, options: AssemblyOptions) -> Result<Self, Diagnostic> {
        let r0 = model.allocate(SCRATCH_BYTES)?;
        let r1 = model.allocate(SCRATCH_BYTES)?;
        let r2 = model.allocate(SCRATCH_BYTES)?;
        Ok(Self {
            model,
            options,
            out: String::new(),
            labels: 0,
            scopes: Vec::new(),
            loops: Vec::new(),
            return_labels: Vec::new(),
            return_types: Vec::new(),
            second_return_types: Vec::new(),
            second_return_pointers: Vec::new(),
            function_ram_bases: Vec::new(),
            function_states: Vec::new(),
            current_live_after: Vec::new(),
            interrupt_functions: HashSet::new(),
            memory_barrier_functions: HashSet::new(),
            ports: HashMap::new(),
            r0,
            r1,
            r2,
            function_pointer_slots: Vec::new(),
        })
    }

    fn emit(mut self, program: &Program, source_program: &Program) -> Result<String, Diagnostic> {
        self.validate_source_returns(source_program)?;
        // Validate the source declarations before TBIR reachability can remove
        // unused functions with invalid ABI signatures.
        self.validate_function_signatures(source_program)?;
        self.collect_ports(source_program)?;
        self.collect_function_attributes(program);
        self.collect_memory_barrier_functions(program);
        if self.interrupt_functions.contains("main") {
            return Err(Diagnostic::new(
                "entry function `main` cannot be an interrupt function",
            ));
        }
        self.validate_function_signatures(program)?;
        self.collect_ports(program)?;
        self.prepare_function_pointer_slots(program)?;
        self.line("; generated by ezrac");
        self.line("; target: Intel 8086 (single flat segment)");
        self.line("section .text");
        self.line("__ezra_start:");
        if !self.options.dos_executable {
            self.line("    cli");
        }
        self.line("    mov ax,cs");
        self.line("    mov ds,ax");
        self.line("    mov es,ax");
        if self.options.dos_executable {
            // The fixed layout uses offsets through EFFFh and requires room for a
            // practical stack above it. Reject undersized loader allocations before
            // static initialization can touch memory outside the process block.
            self.line("    mov bx,[0002h]");
            self.line("    sub bx,ax");
            self.line("    cmp bx,0F80h");
            self.line("    jae short __ezra_dos_memory_ready");
            self.line("    mov ax,0x4cff");
            self.line("    int 0x21");
            self.line("__ezra_dos_memory_ready:");
        }
        if !self.options.dos_executable {
            self.line("    mov ss,ax");
            self.line(&format!(
                "    mov sp,{}",
                imm(self.options.stack_top.get() & !1)
            ));
        }
        self.line("    cld");
        self.emit_static_initializers(program)?;
        self.line("    call near _main");
        self.line("__ezra_exit:");
        if self.options.dos_executable {
            self.line("    mov ax,0x4c00");
            self.line("    int 0x21");
        } else {
            self.line("    jmp near __ezra_exit");
        }

        let emitted_functions = reachable_function_names(program, &self.model);
        for declaration in &program.declarations {
            if let Declaration::Function(function) = declaration
                && emitted_functions.contains(&function.name)
            {
                self.emit_function(function)?;
                self.emit_function_pointer_trampoline(function)?;
            }
        }
        Ok(self.out)
    }

    fn prepare_function_pointer_slots(&mut self, program: &Program) -> Result<(), Diagnostic> {
        for declaration in &program.declarations {
            let Declaration::Function(function) = declaration else {
                continue;
            };
            if function.second_return_type.is_some() {
                continue;
            }
            if function.attrs.iter().any(|attr| attr == "interrupt") {
                continue;
            }
            let ty = Type::Function {
                params: function
                    .params
                    .iter()
                    .map(|param| param.ty.clone())
                    .collect(),
                return_type: function.return_type.clone().map(Box::new),
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

    fn emit_function_pointer_trampoline(&mut self, function: &Function) -> Result<(), Diagnostic> {
        if function.attrs.iter().any(|attr| attr == "interrupt")
            || function.second_return_type.is_some()
        {
            return Ok(());
        }
        let Some(signature) = self.model.functions.get(&function.name).cloned() else {
            return Ok(());
        };
        let ty = Type::Function {
            params: signature.params.clone(),
            return_type: signature.return_type.clone().map(Box::new),
        };
        let slots = self.function_pointer_argument_slots(&ty)?;
        self.line(&format!("{}:", function_pointer_label(&function.name)));
        for (source, target) in slots.iter().zip(&signature.argument_slots) {
            self.copy(*source, *target, source.size);
        }
        self.line(&format!("    call near {}", function_label(&function.name)));
        self.line("    ret");
        Ok(())
    }

    fn validate_source_returns(&self, program: &Program) -> Result<(), Diagnostic> {
        for declaration in &program.declarations {
            let Declaration::Function(function) = declaration else {
                continue;
            };
            let naked = function.attrs.iter().any(|attr| attr == "naked");
            if (function.return_type.is_some() || function.second_return_type.is_some())
                && block_contains_empty_return(&function.body)
            {
                return Err(Diagnostic::new(
                    "return without a value in value-returning function",
                ));
            }
            if !naked
                && (function.return_type.is_some() || function.second_return_type.is_some())
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
        Ok(())
    }

    fn collect_function_attributes(&mut self, program: &Program) {
        for declaration in &program.declarations {
            if let Declaration::Function(function) = declaration
                && function.attrs.iter().any(|attr| attr == "interrupt")
            {
                self.interrupt_functions.insert(function.name.clone());
            }
        }
    }

    fn collect_memory_barrier_functions(&mut self, program: &Program) {
        let mut calls = HashMap::<String, HashSet<String>>::new();
        let mut opaque_callers = HashSet::new();

        fn visit(
            declarations: &[Declaration],
            model: &SemanticModel,
            barriers: &mut HashSet<String>,
            calls: &mut HashMap<String, HashSet<String>>,
            opaque_callers: &mut HashSet<String>,
        ) {
            for declaration in declarations {
                match declaration {
                    Declaration::Cfg { declaration, .. }
                    | Declaration::Bank { declaration, .. } => {
                        visit(
                            core::slice::from_ref(declaration),
                            model,
                            barriers,
                            calls,
                            opaque_callers,
                        );
                    }
                    Declaration::ExternAsmFunction(function) => {
                        barriers.insert(function.name.clone());
                    }
                    Declaration::Function(function) => {
                        let mut paths = Vec::new();
                        collect_stmt_calls(&function.body, &mut paths);
                        let mut callees = HashSet::new();
                        for path in paths {
                            if let Some(callee) = resolve_called_function(&path, model) {
                                callees.insert(callee);
                            } else if !is_i8086_intrinsic_call(&path) {
                                opaque_callers.insert(function.name.clone());
                            }
                        }
                        calls.insert(function.name.clone(), callees);
                        if block_contains_memory_barrier_asm(&function.body) {
                            barriers.insert(function.name.clone());
                        }
                    }
                    _ => {}
                }
            }
        }

        visit(
            &program.declarations,
            &self.model,
            &mut self.memory_barrier_functions,
            &mut calls,
            &mut opaque_callers,
        );
        self.memory_barrier_functions.extend(opaque_callers);
        loop {
            let mut changed = false;
            for (caller, callees) in &calls {
                if callees
                    .iter()
                    .any(|callee| self.memory_barrier_functions.contains(callee))
                    && self.memory_barrier_functions.insert(caller.clone())
                {
                    changed = true;
                }
            }
            if !changed {
                break;
            }
        }
    }

    fn validate_function_signatures(&self, program: &Program) -> Result<(), Diagnostic> {
        for declaration in &program.declarations {
            let (name, params, return_type, second_return_type, is_extern) = match declaration {
                Declaration::Function(function) => (
                    &function.name,
                    &function.params,
                    &function.return_type,
                    &function.second_return_type,
                    false,
                ),
                Declaration::ExternAsmFunction(function) => (
                    &function.name,
                    &function.params,
                    &function.return_type,
                    &function.second_return_type,
                    true,
                ),
                _ => continue,
            };
            for param in params {
                self.require_scalar_signature_type(name, "parameter", &param.ty)?;
            }
            if let Some(return_type) = return_type {
                self.require_scalar_signature_type(name, "return", return_type)?;
            }
            if let Some(second_return_type) = second_return_type {
                if return_type.is_none() {
                    return Err(Diagnostic::new(format!(
                        "8086 two-result function `{name}` must have a first return type"
                    )));
                }
                if is_extern {
                    return Err(Diagnostic::new(format!(
                        "8086 extern asm function `{name}` cannot use two-result returns"
                    )));
                }
                self.require_scalar_signature_type(name, "second return", second_return_type)?;
            }
        }
        Ok(())
    }

    fn require_scalar_signature_type(
        &self,
        function: &str,
        position: &str,
        ty: &Type,
    ) -> Result<(), Diagnostic> {
        match self.model.resolved_type(ty)? {
            Type::Array { .. } => Err(Diagnostic::new(format!(
                "8086 function `{function}` {position} type `{}` is an array; pass it by pointer",
                type_display(ty)
            ))),
            Type::Named(name) if self.model.structs.contains_key(&name) => {
                Err(Diagnostic::new(format!(
                    "8086 function `{function}` {position} type `{}` is a struct; pass it by pointer",
                    type_display(ty)
                )))
            }
            _ => Ok(()),
        }
    }

    fn collect_ports(&mut self, program: &Program) -> Result<(), Diagnostic> {
        for declaration in &program.declarations {
            if let Declaration::Port(port) = declaration {
                if self.model.resolved_type(&port.ty)? != Type::Named("u8".to_owned()) {
                    return Err(Diagnostic::new(format!(
                        "8086 port `{}` must have type `u8`, got `{}`",
                        port.name,
                        type_display(&port.ty)
                    )));
                }
                self.const_integer_expr_type(&port.value).map_err(|_| {
                    Diagnostic::new(format!(
                        "8086 port `{}` address must be an integer constant",
                        port.name
                    ))
                })?;
                let value = self.model.const_value(&port.value)?;
                let value = u8::try_from(value).map_err(|_| {
                    Diagnostic::new(format!("8086 port `{}` is outside 0..255", port.name))
                })?;
                self.ports.insert(port.name.clone(), value);
            }
        }
        Ok(())
    }

    fn const_integer_expr_type(&self, expr: &Expr) -> Result<Type, Diagnostic> {
        let ty = match expr {
            Expr::Int(value) => integer_value_type(*value),
            Expr::TypedInt(_, ty) | Expr::Cast { ty, .. } => {
                let resolved = self.model.resolved_type(ty)?;
                self.require_integer_type(&resolved)?;
                resolved
            }
            Expr::Char(_) => Type::Named("u8".to_owned()),
            Expr::Ident(name) => {
                let ty = self
                    .model
                    .constant_types
                    .get(name)
                    .ok_or_else(|| Diagnostic::new(format!("unknown constant `{name}`")))?;
                let resolved = self.model.resolved_type(ty)?;
                self.require_integer_type(&resolved)?;
                resolved
            }
            Expr::Unary {
                op: UnaryOp::Neg | UnaryOp::BitNot,
                expr,
            } => self.const_integer_expr_type(expr)?,
            Expr::Binary { left, op, right }
                if !is_comparison(*op) && !matches!(op, BinaryOp::And | BinaryOp::Or) =>
            {
                let left_ty = self.const_integer_expr_type(left)?;
                let right_ty = self.const_integer_expr_type(right)?;
                if matches!(op, BinaryOp::Shl | BinaryOp::Shr) {
                    if self.type_is_signed(&right_ty)? {
                        return Err(Diagnostic::new("shift count must be unsigned"));
                    }
                    let count = self.model.const_value(right)?;
                    if !(0..=u8::MAX as i64).contains(&count) {
                        return Err(Diagnostic::new("shift count is outside u8 range"));
                    }
                    left_ty
                } else if is_untyped_integer_expr(left) && is_untyped_integer_expr(right) {
                    common_literal_type(
                        self.model.const_value(left)?,
                        self.model.const_value(right)?,
                    )
                } else if is_untyped_integer_expr(left) {
                    self.validate_literal_for_type(self.model.const_value(left)?, &right_ty)?;
                    right_ty
                } else if is_untyped_integer_expr(right) {
                    self.validate_literal_for_type(self.model.const_value(right)?, &left_ty)?;
                    left_ty
                } else {
                    if self.type_is_signed(&left_ty)? != self.type_is_signed(&right_ty)? {
                        return Err(Diagnostic::new("signed/unsigned mix without cast"));
                    }
                    if self.model.type_width(&left_ty)? != self.model.type_width(&right_ty)? {
                        return Err(Diagnostic::new("integer operands must have the same width"));
                    }
                    left_ty
                }
            }
            _ => return Err(Diagnostic::new("expression is not an integer constant")),
        };
        self.require_integer_type(&ty)?;
        Ok(ty)
    }

    fn require_integer_type(&self, ty: &Type) -> Result<(), Diagnostic> {
        if matches!(
            self.model.resolved_type(ty)?,
            Type::Named(ref name)
                if matches!(
                    name.as_str(),
                    "u8" | "i8" | "u16" | "i16" | "u24" | "i24" | "u32" | "i32"
                )
        ) {
            Ok(())
        } else {
            Err(Diagnostic::new("type is not an integer"))
        }
    }

    fn emit_static_initializers(&mut self, program: &Program) -> Result<(), Diagnostic> {
        let embeds = self.model.embeds.values().cloned().collect::<Vec<_>>();
        for embed in embeds {
            for (offset, value) in embed.bytes.into_iter().enumerate() {
                self.store_immediate(embed.storage.address + offset as u32, value);
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
                self.store_immediate(storage.address + offset as u32, byte);
            }
        }
        for declaration in &program.declarations {
            match declaration {
                Declaration::Global(global) => {
                    let resolved_type = self.model.resolved_type(&global.ty)?;
                    if matches!(global.value, Expr::Array(_))
                        && !matches!(resolved_type, Type::Array { .. })
                    {
                        return Err(Diagnostic::new(format!(
                            "global `{}` is declared `{}`, but its initializer is an array; use an array type such as `[ptr<u8>; 4]` for these values",
                            global.name,
                            type_display(&global.ty)
                        )));
                    }
                    let storage = self.model.globals[&global.name];
                    self.emit_initializer(storage, &global.ty, &global.value)?;
                }
                Declaration::Const(constant) if matches!(constant.ty, Type::Array { .. }) => {
                    let storage = self.model.globals[&constant.name];
                    self.emit_initializer(storage, &constant.ty, &constant.value)?;
                }
                _ => {}
            }
        }
        Ok(())
    }

    fn emit_function(&mut self, function: &Function) -> Result<(), Diagnostic> {
        let naked = function.attrs.iter().any(|attr| attr == "naked");
        let interrupt = function.attrs.iter().any(|attr| attr == "interrupt");
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
        if function.second_return_type.is_none() && contains_two_result_statement(&function.body) {
            return Err(Diagnostic::new(format!(
                "8086 function `{}` contains a two-result statement without a two-result signature",
                function.name
            )));
        }
        let local_plan = plan_function_locals(function, &self.model)?;
        let return_label = self.next_label(&format!("{}_return", function.name));
        let function_ram_base = self.model.next_ram_address();
        let second_return_pointer = function
            .second_return_type
            .as_ref()
            .map(|_| self.model.allocate(2))
            .transpose()?;
        self.line(&format!("{}:", function_label(&function.name)));
        self.scopes.push(HashMap::new());
        self.return_labels.push(return_label.clone());
        self.return_types.push(function.return_type.clone());
        self.second_return_types
            .push(function.second_return_type.clone());
        self.second_return_pointers.push(second_return_pointer);
        self.function_ram_bases.push(function_ram_base);
        self.function_states
            .push(FunctionState { interrupt, naked });
        if let Some(pointer) = second_return_pointer {
            self.line(&format!("    mov {},bx", mem(pointer.address)));
        }
        if interrupt && !naked {
            self.line("    push ax");
            self.line("    push bx");
            self.line("    push cx");
            self.line("    push dx");
            self.line("    push si");
            self.line("    push di");
            self.line("    push bp");
            self.line("    push ds");
            self.line("    push es");
            self.line("    mov ax,cs");
            self.line("    mov ds,ax");
            self.line("    mov es,ax");
            self.push_bytes(self.r0);
            self.push_bytes(self.r1);
            self.push_bytes(self.r2);
        }

        let signature = self
            .model
            .functions
            .get(&function.name)
            .cloned()
            .ok_or_else(|| Diagnostic::new(format!("unknown function `{}`", function.name)))?;
        for (param, slot) in function.params.iter().zip(signature.argument_slots) {
            let storage = self.model.allocate_type(&param.ty)?;
            self.bind(
                param.name.clone(),
                BindingLocation::Static(storage),
                param.ty.clone(),
            )?;
            self.copy(slot, storage, storage.size);
        }
        let spill_storage = local_plan
            .spill_sizes
            .iter()
            .map(|size| self.model.allocate(*size))
            .collect::<Result<Vec<_>, _>>()?;
        for (name, planned) in local_plan.bindings {
            let location = match planned.location {
                PlannedLocation::Bp => BindingLocation::Bp,
                PlannedLocation::Spill(slot) => BindingLocation::Static(
                    *spill_storage
                        .get(slot)
                        .ok_or_else(|| Diagnostic::new("invalid i8086 static spill slot"))?,
                ),
            };
            self.bind(name, location, planned.ty)?;
        }
        self.emit_block(&function.body)?;
        self.line(&format!("{return_label}:"));
        if interrupt {
            if !naked {
                self.pop_bytes(self.r2);
                self.pop_bytes(self.r1);
                self.pop_bytes(self.r0);
                self.line("    pop es");
                self.line("    pop ds");
                self.line("    pop bp");
                self.line("    pop di");
                self.line("    pop si");
                self.line("    pop dx");
                self.line("    pop cx");
                self.line("    pop bx");
                self.line("    pop ax");
            }
            self.line("    iret");
        } else if !naked {
            self.line("    ret");
        }
        self.function_states.pop();
        self.function_ram_bases.pop();
        self.second_return_pointers.pop();
        self.second_return_types.pop();
        self.return_types.pop();
        self.return_labels.pop();
        self.scopes.pop();
        Ok(())
    }

    fn emit_block(&mut self, body: &[Stmt]) -> Result<(), Diagnostic> {
        self.emit_block_with_inherited_live(body, &HashSet::new())
    }

    fn emit_block_with_inherited_live(
        &mut self,
        body: &[Stmt],
        inherited: &HashSet<String>,
    ) -> Result<(), Diagnostic> {
        let live_after = statement_live_after(body, inherited);
        for (stmt, live) in body.iter().zip(live_after) {
            self.current_live_after.push(live);
            self.emit_stmt(stmt)?;
            self.current_live_after.pop();
            if !stmt_can_complete_normally(stmt, &self.model) {
                break;
            }
        }
        Ok(())
    }

    fn emit_loop_block_with_inherited_live(
        &mut self,
        body: &[Stmt],
        inherited: &HashSet<String>,
    ) -> Result<(), Diagnostic> {
        let live_after = statement_live_after(body, inherited);
        let loop_entry = block_live_entry(body, inherited);
        for (stmt, mut live) in body.iter().zip(live_after) {
            live.extend(loop_entry.iter().cloned());
            self.current_live_after.push(live);
            self.emit_stmt(stmt)?;
            self.current_live_after.pop();
            if !stmt_can_complete_normally(stmt, &self.model) {
                break;
            }
        }
        Ok(())
    }

    fn inherited_live_after(&self) -> HashSet<String> {
        self.current_live_after.last().cloned().unwrap_or_default()
    }

    fn emit_stmt(&mut self, stmt: &Stmt) -> Result<(), Diagnostic> {
        self.line(&format!("    ; source: {}", stmt_summary(stmt)));
        match stmt {
            Stmt::Let { name, ty, value } => {
                let binding = self.binding(name)?;
                self.emit_binding_initializer(&binding, ty, value)?;
            }
            Stmt::LetTwo {
                first_name,
                first_ty,
                second_name,
                second_ty,
                value,
            } => {
                let first = self.binding(first_name)?;
                let second = self.binding(second_name)?;
                let second_storage = self.static_storage(&second)?;
                self.emit_two_result_call(value, first_ty, second_ty, second_storage)?;
                self.store_result_binding(&first)?;
            }
            Stmt::Assign { target, op, value } => {
                let ty = self.place_type(target)?;
                let place = self.materialize_place(target)?;
                if *op == AssignOp::Set
                    && let MaterializedPlace::Direct(address) = place
                    && let Ok(width) = self.scalar_width(&ty)
                {
                    let destination = Storage {
                        address,
                        size: u32::from(width),
                    };
                    match value {
                        Expr::Int(value) | Expr::TypedInt(value, _) => {
                            self.load_constant_into(destination, *value, width);
                            return Ok(());
                        }
                        Expr::Bool(value) => {
                            self.load_constant_into(destination, i64::from(*value), width);
                            return Ok(());
                        }
                        Expr::Char(value) => {
                            self.load_constant_into(destination, i64::from(*value), width);
                            return Ok(());
                        }
                        Expr::Ident(source)
                            if let Ok(binding) = self.binding(source)
                                && self.scalar_width(&binding.ty).ok() == Some(width)
                                && self.model.resolved_type(&binding.ty).ok()
                                    == self.model.resolved_type(&ty).ok() =>
                        {
                            self.copy_binding_to_storage(&binding, destination, u32::from(width));
                            return Ok(());
                        }
                        _ => {}
                    }
                }
                let Ok(width) = self.scalar_width(&ty) else {
                    if *op != AssignOp::Set {
                        return Err(Diagnostic::new(
                            "compound assignment requires a scalar value",
                        ));
                    }
                    let size = self.model.type_size(&ty)?;
                    let temporary = self.model.allocate(size)?;
                    self.emit_initializer(temporary, &ty, value)?;
                    self.store_materialized_place(place, temporary, size);
                    return Ok(());
                };
                if *op == AssignOp::Set {
                    self.emit_expr(value, &ty)?;
                } else {
                    self.load_materialized_place(place, width);
                    let op = assign_binary(*op);
                    let signed = self.type_is_signed(&ty)?;
                    let constant = self.model.const_value(value).ok();
                    let immediate = if width == 4 {
                        if let Some(value) = constant {
                            self.binary_immediate(op, value, signed)?
                        } else {
                            false
                        }
                    } else {
                        false
                    };
                    if !immediate {
                        let left = self.model.allocate(u32::from(width))?;
                        self.copy(self.r0, left, u32::from(width));
                        self.emit_expr(value, &ty)?;
                        self.copy(self.r0, self.r1, u32::from(width));
                        self.copy(left, self.r0, u32::from(width));
                        self.binary(op, width, signed, constant)?;
                    }
                }
                let result = self.model.allocate(u32::from(width))?;
                self.copy(self.r0, result, u32::from(width));
                self.store_materialized_place(place, result, u32::from(width));
            }
            Stmt::If {
                condition,
                then_body,
                else_body,
            } => {
                let then_label = self.next_label("if_then");
                let otherwise = self.next_label("if_else");
                let done = self.next_label("if_done");
                let live_after = self.inherited_live_after();
                let mut condition_live = live_after.clone();
                condition_live.extend(statements_uses(then_body));
                condition_live.extend(statements_uses(else_body));
                self.current_live_after.push(condition_live);
                self.emit_condition(condition, &then_label, &otherwise)?;
                self.current_live_after.pop();
                self.line(&format!("{then_label}:"));
                self.emit_block_with_inherited_live(then_body, &live_after)?;
                if block_can_complete_normally(then_body, &self.model) {
                    self.line(&format!("    jmp near {done}"));
                }
                self.line(&format!("{otherwise}:"));
                self.emit_block_with_inherited_live(else_body, &live_after)?;
                self.line(&format!("{done}:"));
            }
            Stmt::While { condition, body } => {
                let head = self.next_label("while_condition");
                let body_label = self.next_label("while_body");
                let done = self.next_label("while_done");
                let live_after = self.inherited_live_after();
                let mut condition_live = live_after.clone();
                condition_live.extend(statements_uses(body));
                let mut body_live = live_after.clone();
                let mut condition_uses = HashSet::new();
                expr_uses(condition, &mut condition_uses);
                body_live.extend(condition_uses);
                self.loops.push(LoopLabels {
                    continue_label: head.clone(),
                    break_label: done.clone(),
                });
                self.line(&format!("{head}:"));
                self.current_live_after.push(condition_live);
                self.emit_condition(condition, &body_label, &done)?;
                self.current_live_after.pop();
                self.line(&format!("{body_label}:"));
                self.emit_loop_block_with_inherited_live(body, &body_live)?;
                if block_can_complete_normally(body, &self.model) {
                    self.line(&format!("    jmp near {head}"));
                }
                self.line(&format!("{done}:"));
                self.loops.pop();
            }
            Stmt::Loop { body } => {
                let head = self.next_label("loop_body");
                let done = self.next_label("loop_done");
                let body_live = self.inherited_live_after();
                self.loops.push(LoopLabels {
                    continue_label: head.clone(),
                    break_label: done.clone(),
                });
                self.line(&format!("{head}:"));
                self.emit_loop_block_with_inherited_live(body, &body_live)?;
                if block_can_complete_normally(body, &self.model) {
                    self.line(&format!("    jmp near {head}"));
                }
                self.line(&format!("{done}:"));
                self.loops.pop();
            }
            Stmt::Break | Stmt::Continue => {
                let labels = self
                    .loops
                    .last()
                    .ok_or_else(|| Diagnostic::new("loop control outside loop"))?;
                let target = if matches!(stmt, Stmt::Continue) {
                    &labels.continue_label
                } else {
                    &labels.break_label
                }
                .clone();
                self.line(&format!("    jmp near {target}"));
            }
            Stmt::Return(value) => {
                if self
                    .second_return_types
                    .last()
                    .and_then(Option::as_ref)
                    .is_some()
                {
                    return Err(Diagnostic::new(
                        "two-result function must use `return first, second`",
                    ));
                }
                let return_type = self.return_types.last().and_then(Clone::clone);
                match (value, return_type) {
                    (Some(value), Some(ty)) => self.emit_expr(value, &ty)?,
                    (Some(_), None) => {
                        return Err(Diagnostic::new("value return in void function"));
                    }
                    (None, Some(_)) => {
                        return Err(Diagnostic::new(
                            "return without a value in value-returning function",
                        ));
                    }
                    (None, None) => {}
                }
                let target = self.return_labels.last().expect("return label").clone();
                self.line(&format!("    jmp near {target}"));
            }
            Stmt::ReturnTwo { first, second } => self.emit_return_two(first, second)?,
            Stmt::Asm {
                volatile,
                inputs,
                outputs,
                clobbers,
                lines,
            } => self.inline_asm(*volatile, inputs, outputs, clobbers, lines)?,
            Stmt::Out { port, value } => {
                let port = self.port(port)?;
                self.emit_expr(value, &Type::Named("u8".to_owned()))?;
                self.load_al(self.r0.address);
                self.line(&format!("    out {},al", imm(u32::from(port))));
            }
            Stmt::Expr(expr) => {
                let ty = self
                    .expr_type(expr)
                    .unwrap_or_else(|_| Type::Named("u8".to_owned()));
                self.emit_expr(expr, &ty)?;
            }
        }
        Ok(())
    }

    fn emit_binding_initializer(
        &mut self,
        binding: &Binding,
        ty: &Type,
        value: &Expr,
    ) -> Result<(), Diagnostic> {
        let BindingLocation::Bp = binding.location else {
            return self.emit_initializer(self.static_storage(binding)?, ty, value);
        };
        debug_assert_eq!(self.scalar_width(ty)?, 2);
        match value {
            Expr::Int(value) | Expr::TypedInt(value, _) => {
                self.line(&format!("    mov bp,{}", format_immediate(*value, 2)));
            }
            Expr::Bool(value) => self.line(&format!("    mov bp,{}", imm(u32::from(*value)))),
            Expr::Char(value) => self.line(&format!("    mov bp,{}", imm(u32::from(*value)))),
            Expr::Ident(source) => {
                let source = self.binding(source)?;
                match source.location {
                    BindingLocation::Bp => {}
                    BindingLocation::Static(storage) => {
                        self.load_ax(storage.address);
                        self.line("    mov bp,ax");
                    }
                }
            }
            _ => {
                self.emit_expr(value, ty)?;
                self.load_ax(self.r0.address);
                self.line("    mov bp,ax");
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
                self.copy(self.static_storage(&source)?, storage, storage.size);
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
                for (name, value) in fields {
                    let field = layout
                        .fields
                        .get(name)
                        .ok_or_else(|| Diagnostic::new(format!("unknown field `{name}`")))?;
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
                self.copy(self.static_storage(&source)?, storage, storage.size);
            }
            (resolved @ Type::Array { .. }, Expr::Deref(pointer)) => {
                self.emit_expr(pointer, &Type::Ptr(Box::new(resolved)))?;
                self.result_to_bx();
                self.copy_indirect_to_storage(storage, storage.size);
            }
            (Type::Named(name), Expr::Deref(pointer)) if self.model.structs.contains_key(&name) => {
                let resolved = Type::Named(name);
                self.emit_expr(pointer, &Type::Ptr(Box::new(resolved)))?;
                self.result_to_bx();
                self.copy_indirect_to_storage(storage, storage.size);
            }
            (resolved, Expr::Int(value) | Expr::TypedInt(value, _)) => {
                let width = self.scalar_width(&resolved)?;
                self.load_constant_into(storage, *value, width);
            }
            (resolved, Expr::Bool(value)) => {
                let width = self.scalar_width(&resolved)?;
                self.load_constant_into(storage, i64::from(*value), width);
            }
            (resolved, Expr::Char(value)) => {
                let width = self.scalar_width(&resolved)?;
                self.load_constant_into(storage, i64::from(*value), width);
            }
            (resolved, Expr::Ident(source)) => {
                if let Ok(binding) = self.binding(source) {
                    let width = self.scalar_width(&resolved)?;
                    let source_width = self.scalar_width(&binding.ty)?;
                    if source_width == width && self.model.resolved_type(&binding.ty)? == resolved {
                        self.copy_binding_to_storage(&binding, storage, u32::from(width));
                        return Ok(());
                    }
                }
                let width = self.scalar_width(&resolved)?;
                self.emit_expr(value, &resolved)?;
                self.copy(self.r0, storage, u32::from(width));
            }
            (resolved, _) => {
                let width = self.scalar_width(&resolved)?;
                self.emit_expr(value, &resolved)?;
                self.copy(self.r0, storage, u32::from(width));
            }
        }
        Ok(())
    }

    fn emit_condition(
        &mut self,
        condition: &Expr,
        true_label: &str,
        false_label: &str,
    ) -> Result<(), Diagnostic> {
        if let Ok(value) = self.model.const_value(condition) {
            self.line(&format!(
                "    jmp near {}",
                if value != 0 { true_label } else { false_label }
            ));
            return Ok(());
        }
        match condition {
            Expr::Unary {
                op: UnaryOp::Not,
                expr,
            } => self.emit_condition(expr, false_label, true_label),
            Expr::Binary {
                left,
                op: BinaryOp::And,
                right,
            } => {
                let rhs = self.next_label("condition_and_rhs");
                self.emit_condition_preserving(left, &rhs, false_label, &[right])?;
                self.line(&format!("{rhs}:"));
                self.emit_condition(right, true_label, false_label)
            }
            Expr::Binary {
                left,
                op: BinaryOp::Or,
                right,
            } => {
                let rhs = self.next_label("condition_or_rhs");
                self.emit_condition_preserving(left, true_label, &rhs, &[right])?;
                self.line(&format!("{rhs}:"));
                self.emit_condition(right, true_label, false_label)
            }
            Expr::Binary { left, op, right } if is_comparison(*op) => {
                let operand_ty = self.comparison_operand_type(left, *op, right, &bool_ty())?;
                let width = self.scalar_width(&operand_ty)?;
                let signed = self.type_is_signed(&operand_ty)?;
                self.emit_expr_preserving(left, &operand_ty, &[right])?;
                let left_value = self.model.allocate(u32::from(width))?;
                self.copy(self.r0, left_value, u32::from(width));
                self.emit_expr(right, &operand_ty)?;
                self.copy(self.r0, self.r1, u32::from(width));
                self.copy(left_value, self.r0, u32::from(width));
                self.compare_branch(*op, width, signed, true_label, false_label);
                Ok(())
            }
            _ => {
                self.emit_expr(condition, &bool_ty())?;
                self.jump_storage_nonzero(self.r0, 1, true_label);
                self.line(&format!("    jmp near {false_label}"));
                Ok(())
            }
        }
    }

    fn emit_condition_preserving(
        &mut self,
        condition: &Expr,
        true_label: &str,
        false_label: &str,
        continuations: &[&Expr],
    ) -> Result<(), Diagnostic> {
        let mut live_after = self.inherited_live_after();
        for continuation in continuations {
            expr_uses(continuation, &mut live_after);
        }
        self.current_live_after.push(live_after);
        let result = self.emit_condition(condition, true_label, false_label);
        self.current_live_after.pop();
        result
    }

    fn emit_expr_preserving(
        &mut self,
        expr: &Expr,
        expected: &Type,
        continuations: &[&Expr],
    ) -> Result<(), Diagnostic> {
        let mut live_after = self.inherited_live_after();
        for continuation in continuations {
            expr_uses(continuation, &mut live_after);
        }
        self.current_live_after.push(live_after);
        let result = self.emit_expr(expr, expected);
        self.current_live_after.pop();
        result
    }

    fn emit_expr(&mut self, expr: &Expr, expected: &Type) -> Result<(), Diagnostic> {
        let width = self.scalar_width(expected)?;
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
                    let source_width = self.scalar_width(&binding.ty)?;
                    self.copy_binding_to_storage(
                        &binding,
                        self.r0,
                        u32::from(source_width.min(width)),
                    );
                    self.extend_result(source_width, width, self.type_is_signed(&binding.ty)?);
                }
            }
            Expr::In(port) => {
                let port = self.port(port)?;
                self.line(&format!("    in al,{}", imm(u32::from(port))));
                self.store_al(self.r0.address);
                self.extend_result(1, width, false);
            }
            Expr::AddressOf(name) => {
                if let Some(function_ty) = self.function_value_type(name) {
                    let expected = self.model.resolved_type(expected)?;
                    let actual = Type::Ptr(Box::new(self.model.resolved_type(&function_ty)?));
                    if expected != actual {
                        return Err(Diagnostic::new(format!(
                            "function `{name}` has type `{}`, expected `{}`",
                            type_display(&actual),
                            type_display(&expected)
                        )));
                    }
                    self.line(&format!("    mov ax,{}", function_pointer_label(name)));
                    self.line(&format!("    mov {},ax", mem(self.r0.address)));
                } else if self
                    .model
                    .functions
                    .get(name)
                    .is_some_and(|signature| signature.second_return_type.is_some())
                {
                    return Err(Diagnostic::new(format!(
                        "8086 function pointer cannot reference two-result function `{name}`"
                    )));
                } else {
                    let binding = self.binding(name)?;
                    self.load_constant(i64::from(self.static_storage(&binding)?.address), width)
                }
            }
            Expr::AddressOfIndex { name, index } => {
                self.named_index_address(name, index)?;
                self.bx_to_result(width);
            }
            Expr::AddressOfField { base, field } => {
                let (address, _) = self.named_field_address(base, field)?;
                match address {
                    Address::Direct(address) => self.load_constant(i64::from(address), width),
                    Address::Indirect => self.bx_to_result(width),
                }
            }
            Expr::AddressOfAccess(path) => {
                self.access_address(path)?;
                self.bx_to_result(width);
            }
            Expr::Index { name, index } => {
                let element = self.named_index_address(name, index)?;
                let source_width = self.scalar_width(&element)?;
                self.load_indirect(source_width);
                self.extend_result(source_width, width, self.type_is_signed(&element)?);
            }
            Expr::Field { base, field } => {
                let constant = format!("{base}.{field}");
                if let Some(value) = self.model.constants.get(&constant).copied() {
                    self.load_constant(value, width);
                } else {
                    let (address, field) = self.named_field_address(base, field)?;
                    let source_width = self.scalar_width(&field.ty)?;
                    match address {
                        Address::Direct(address) => self.copy(
                            Storage {
                                address,
                                size: field.size,
                            },
                            self.r0,
                            u32::from(source_width.min(width)),
                        ),
                        Address::Indirect => self.load_indirect(source_width),
                    }
                    self.extend_result(source_width, width, self.type_is_signed(&field.ty)?);
                }
            }
            Expr::Access(path) => {
                let ty = self.access_address(path)?;
                let source_width = self.scalar_width(&ty)?;
                self.load_indirect(source_width);
                self.extend_result(source_width, width, self.type_is_signed(&ty)?);
            }
            Expr::Deref(pointer) => {
                if let Some(address) = self.constant_pointer_address(pointer) {
                    self.load_direct_memory(address, width);
                } else {
                    self.emit_expr(pointer, &Type::Ptr(Box::new(expected.clone())))?;
                    self.result_to_bx();
                    self.load_indirect(width);
                }
            }
            Expr::BankedPointer { pointer, .. } => self.emit_expr(pointer, expected)?,
            Expr::Call { path, args } => self.emit_call(path, args, expected)?,
            Expr::Unary { op, expr } => {
                self.emit_expr(expr, expected)?;
                self.unary(*op, width);
            }
            Expr::Binary { left, op, right } => {
                if *op == BinaryOp::Add && self.emit_pointer_add(left, right, expected)? {
                    return Ok(());
                }
                if matches!(op, BinaryOp::And | BinaryOp::Or) {
                    self.short_circuit(left, *op, right)?;
                    self.extend_result(1, width, false);
                    return Ok(());
                }
                let operand_ty = if is_comparison(*op) {
                    self.comparison_operand_type(left, *op, right, expected)?
                } else {
                    expected.clone()
                };
                let operand_width = self.scalar_width(&operand_ty)?;
                let signed = self.type_is_signed(&operand_ty)?;
                let constant = self.model.const_value(right).ok();
                self.emit_expr_preserving(left, &operand_ty, &[right])?;
                let immediate = operand_width == 4
                    && !matches!(self.model.resolved_type(&operand_ty)?, Type::Ptr(_))
                    && if let Some(value) = constant {
                        self.binary_immediate(*op, value, signed)?
                    } else {
                        false
                    };
                if !immediate {
                    let left_value = self.model.allocate(u32::from(operand_width))?;
                    self.copy(self.r0, left_value, u32::from(operand_width));
                    self.emit_expr(right, &operand_ty)?;
                    self.copy(self.r0, self.r1, u32::from(operand_width));
                    self.copy(left_value, self.r0, u32::from(operand_width));
                    if matches!(op, BinaryOp::Add | BinaryOp::Sub)
                        && let Type::Ptr(inner) = self.model.resolved_type(&operand_ty)?
                    {
                        self.scale(self.r1, operand_width, self.model.type_size(&inner)?)?;
                    }
                    self.binary(*op, operand_width, signed, constant)?;
                }
                if is_comparison(*op) {
                    self.extend_result(1, width, false);
                }
            }
            Expr::Cast { ty, expr } => {
                let source_ty = self.expr_type(expr).unwrap_or_else(|_| ty.clone());
                let source_width = self.scalar_width(&source_ty)?;
                self.emit_expr(expr, &source_ty)?;
                self.extend_result(source_width, width, self.type_is_signed(&source_ty)?);
            }
            Expr::Array(_) | Expr::StructInit { .. } => {
                return Err(Diagnostic::new("aggregate value requires storage context"));
            }
        }
        Ok(())
    }

    fn store_result_binding(&mut self, binding: &Binding) -> Result<(), Diagnostic> {
        let width = self.scalar_width(&binding.ty)?;
        match binding.location {
            BindingLocation::Static(storage) => self.copy(self.r0, storage, u32::from(width)),
            BindingLocation::Bp => {
                debug_assert_eq!(width, 2);
                self.load_ax(self.r0.address);
                self.line("    mov bp,ax");
            }
        }
        Ok(())
    }

    fn emit_two_result_call(
        &mut self,
        value: &Expr,
        first_ty: &Type,
        second_ty: &Type,
        second_destination: Storage,
    ) -> Result<(), Diagnostic> {
        let Expr::Call { path, args } = value else {
            return Err(Diagnostic::new(
                "two-result bindings require a direct two-result call",
            ));
        };
        let name = path.join(".");
        if CATALOG.lookup(&name).is_some() {
            return self.emit_intrinsic_two_result_call(
                &name,
                args,
                first_ty,
                second_ty,
                second_destination,
            );
        }
        let resolved = resolve_function(path, &self.model)
            .ok_or_else(|| Diagnostic::new(format!("unknown function `{name}`")))?;
        if self.interrupt_functions.contains(&resolved) {
            return Err(Diagnostic::new(format!(
                "interrupt function `{resolved}` cannot be called with ordinary `call`"
            )));
        }
        let signature = self.model.functions[&resolved].clone();
        let Some(signature_second) = signature.second_return_type.as_ref() else {
            return Err(Diagnostic::new(format!(
                "function `{name}` does not return two values"
            )));
        };
        let Some(signature_first) = signature.return_type.as_ref() else {
            return Err(Diagnostic::new(format!(
                "two-result function `{name}` has no first return type"
            )));
        };
        if self.model.resolved_type(signature_first)? != self.model.resolved_type(first_ty)? {
            return Err(Diagnostic::new(format!(
                "first result of `{name}` does not match binding type `{}`",
                type_display(first_ty)
            )));
        }
        if self.model.resolved_type(signature_second)? != self.model.resolved_type(second_ty)? {
            return Err(Diagnostic::new(format!(
                "second result of `{name}` does not match binding type `{}`",
                type_display(second_ty)
            )));
        }
        if signature.params.len() != args.len() {
            return Err(Diagnostic::new(format!(
                "function `{name}` expects {} arguments, got {}",
                signature.params.len(),
                args.len()
            )));
        }
        let second_width = self.scalar_width(signature_second)?;
        if second_destination.size < u32::from(second_width) {
            return Err(Diagnostic::new(format!(
                "second result destination for `{name}` is too small"
            )));
        }
        let mut values = Vec::with_capacity(args.len());
        for (index, (arg, ty)) in args.iter().zip(&signature.params).enumerate() {
            let value = self.model.allocate(self.model.type_size(ty)?)?;
            let mut argument_live = self.inherited_live_after();
            for continuation in &args[index + 1..] {
                expr_uses(continuation, &mut argument_live);
            }
            self.current_live_after.push(argument_live);
            let result = self.emit_initializer(value, ty, arg);
            self.current_live_after.pop();
            result?;
            values.push(value);
        }
        for (value, slot) in values.into_iter().zip(&signature.argument_slots) {
            self.copy(value, *slot, value.size);
        }

        let live = self.function_ram_bases.last().map(|base| Storage {
            address: *base,
            size: self.model.next_ram_address() - *base,
        });
        let live_after = self.current_live_after.last().cloned().unwrap_or_default();
        let saved_live = live
            .map(|live| {
                self.live_storage_segments(live, args, &live_after, false, &[second_destination])
            })
            .unwrap_or_default();
        for storage in &saved_live {
            self.push_bytes(*storage);
        }
        self.line(&format!("    mov bx,{}", imm(second_destination.address)));
        self.line(&format!("    call near {}", function_label(&resolved)));
        let returned = self
            .model
            .allocate(self.model.type_size(signature_first)?)?;
        self.copy(self.r0, returned, returned.size);
        for storage in saved_live.iter().rev() {
            self.pop_bytes(*storage);
        }
        self.copy(returned, self.r0, returned.size);
        self.extend_result(
            self.scalar_width(signature_first)?,
            self.scalar_width(first_ty)?,
            self.type_is_signed(signature_first)?,
        );
        Ok(())
    }

    fn emit_return_two(&mut self, first: &Expr, second: &Expr) -> Result<(), Diagnostic> {
        let Some(first_type) = self.return_types.last().and_then(Clone::clone) else {
            return Err(Diagnostic::new(
                "function cannot return two values without a first return type",
            ));
        };
        let Some(second_type) = self.second_return_types.last().and_then(Clone::clone) else {
            return Err(Diagnostic::new("function cannot return two values"));
        };
        let Some(pointer) = self.second_return_pointers.last().copied().flatten() else {
            return Err(Diagnostic::new(
                "two-result function has no caller-provided return slot",
            ));
        };
        let first_value = self.model.allocate(self.model.type_size(&first_type)?)?;
        let second_width = self.scalar_width(&second_type)?;
        self.emit_expr(first, &first_type)?;
        self.copy(self.r0, first_value, first_value.size);
        self.emit_expr(second, &second_type)?;
        self.load_bx(pointer.address);
        self.copy_storage_to_indirect(self.r0, u32::from(second_width));
        self.copy(first_value, self.r0, first_value.size);
        let target = self.return_labels.last().expect("return label").clone();
        self.line(&format!("    jmp near {target}"));
        Ok(())
    }

    fn emit_call(
        &mut self,
        path: &[String],
        args: &[Expr],
        expected: &Type,
    ) -> Result<(), Diagnostic> {
        let name = path.join(".");
        if CATALOG.lookup(&name).is_some() {
            return self.emit_intrinsic_call(&name, args, expected);
        }
        let (signature, direct_target, indirect_target) =
            if let Some(resolved) = resolve_function(path, &self.model) {
                if self.interrupt_functions.contains(&resolved) {
                    return Err(Diagnostic::new(format!(
                        "interrupt function `{resolved}` cannot be called with ordinary `call`"
                    )));
                }
                (
                    self.model.functions[&resolved].clone(),
                    Some(resolved),
                    None,
                )
            } else {
                if path.len() != 1 {
                    return Err(Diagnostic::new(format!("unknown function `{name}`")));
                }
                let binding = self.binding(&path[0])?;
                let resolved_binding_type = self.model.resolved_type(&binding.ty)?;
                let Type::Ptr(inner) = &resolved_binding_type else {
                    return Err(Diagnostic::new(format!(
                        "function pointer call requires `ptr<fn(...)>`, got `{}`",
                        type_display(&resolved_binding_type)
                    )));
                };
                let Type::Function {
                    params,
                    return_type,
                } = inner.as_ref()
                else {
                    return Err(Diagnostic::new(format!(
                        "function pointer call requires `ptr<fn(...)>`, got `{}`",
                        type_display(&resolved_binding_type)
                    )));
                };
                let function_ty = Type::Function {
                    params: params.clone(),
                    return_type: return_type.clone(),
                };
                let argument_slots = self.function_pointer_argument_slots(&function_ty)?;
                (
                    crate::tbir::model::FunctionSignature {
                        params: params.clone(),
                        return_type: return_type.clone().map(|ty| *ty),
                        second_return_type: None,
                        argument_slots,
                    },
                    None,
                    Some(self.static_storage(&binding)?),
                )
            };
        if signature.second_return_type.is_some() {
            return Err(Diagnostic::new(format!(
                "two-result function `{name}` requires a two-destination call"
            )));
        }
        if self
            .function_states
            .last()
            .is_some_and(|state| state.interrupt)
        {
            return Err(Diagnostic::new(
                "interrupt handler cannot call user function because the 8086 static-frame ABI is not reentrant",
            ));
        }
        if signature.params.len() != args.len() {
            return Err(Diagnostic::new(format!(
                "function `{name}` expects {} arguments, got {}",
                signature.params.len(),
                args.len()
            )));
        }
        let mut values = Vec::with_capacity(args.len());
        for (index, (arg, ty)) in args.iter().zip(&signature.params).enumerate() {
            let value = self.model.allocate(self.model.type_size(ty)?)?;
            let mut argument_live = self.inherited_live_after();
            for continuation in &args[index + 1..] {
                expr_uses(continuation, &mut argument_live);
            }
            self.current_live_after.push(argument_live);
            let result = self.emit_initializer(value, ty, arg);
            self.current_live_after.pop();
            result?;
            values.push(value);
        }
        for (value, slot) in values.into_iter().zip(&signature.argument_slots) {
            self.copy(value, *slot, value.size);
        }

        let live = self.function_ram_bases.last().map(|base| Storage {
            address: *base,
            size: self.model.next_ram_address() - *base,
        });
        let live_after = self.current_live_after.last().cloned().unwrap_or_default();
        let opaque_callee = indirect_target.is_some()
            || direct_target
                .as_ref()
                .is_some_and(|name| self.memory_barrier_functions.contains(name));
        let saved_live = live
            .map(|live| self.live_storage_segments(live, args, &live_after, opaque_callee, &[]))
            .unwrap_or_default();
        for storage in &saved_live {
            self.push_bytes(*storage);
        }
        if let Some(resolved) = direct_target {
            self.line(&format!("    call near {}", function_label(&resolved)));
        } else if let Some(pointer) = indirect_target {
            self.load_bx(pointer.address);
            self.line("    call bx");
        }
        let returned = signature
            .return_type
            .as_ref()
            .map(|ty| self.model.type_size(ty))
            .transpose()?
            .map(|size| self.model.allocate(size))
            .transpose()?;
        if let Some(returned) = returned {
            self.copy(self.r0, returned, returned.size);
        }
        for storage in saved_live.iter().rev() {
            self.pop_bytes(*storage);
        }
        if let Some(returned) = returned {
            self.copy(returned, self.r0, returned.size);
            let return_width = self.scalar_width(signature.return_type.as_ref().unwrap())?;
            self.extend_result(
                return_width,
                self.scalar_width(expected)?,
                self.type_is_signed(signature.return_type.as_ref().unwrap())?,
            );
        }
        Ok(())
    }

    fn resolve_intrinsic(
        &self,
        name: &str,
        args: &[Expr],
    ) -> Result<crate::intrinsics::IntrinsicResolution, Diagnostic> {
        let argument_types = args
            .iter()
            .map(|arg| self.model.resolved_type(&self.expr_type(arg)?))
            .collect::<Result<Vec<_>, _>>()?;
        let constants = args
            .iter()
            .map(|arg| self.model.const_value(arg).ok())
            .collect::<Vec<_>>();
        CATALOG
            .validate_types_with_constants(name, &argument_types, &constants)
            .map_err(|error| Diagnostic::new(error.to_string()))
    }

    fn intrinsic_arguments(
        &mut self,
        args: &[Expr],
        resolution: &crate::intrinsics::IntrinsicResolution,
    ) -> Result<Vec<Storage>, Diagnostic> {
        resolution
            .argument_types
            .iter()
            .zip(args)
            .map(|(ty, arg)| {
                let storage = self.model.allocate(self.model.type_size(ty)?)?;
                self.emit_initializer(storage, ty, arg)?;
                Ok(storage)
            })
            .collect()
    }

    fn emit_intrinsic_call(
        &mut self,
        name: &str,
        args: &[Expr],
        expected: &Type,
    ) -> Result<(), Diagnostic> {
        let resolution = self.resolve_intrinsic(name, args)?;
        if resolution.result_types.len() == 2 {
            return Err(Diagnostic::new(format!(
                "two-result intrinsic `{name}` requires a two-destination call"
            )));
        }
        let values = self.intrinsic_arguments(args, &resolution)?;
        let constants = args
            .iter()
            .map(|arg| self.model.const_value(arg).ok())
            .collect::<Vec<_>>();
        if let Some(result) = resolution.result_types.first() {
            if self.model.resolved_type(result)? != self.model.resolved_type(expected)? {
                return Err(Diagnostic::new(format!(
                    "result of intrinsic `{name}` does not match `{}`",
                    type_display(expected)
                )));
            }
        }
        self.lower_intrinsic(
            resolution.descriptor.operation,
            &resolution.argument_types,
            &values,
            &constants,
            resolution.result_types.first(),
            None,
        )
    }

    fn emit_intrinsic_two_result_call(
        &mut self,
        name: &str,
        args: &[Expr],
        first_ty: &Type,
        second_ty: &Type,
        second_destination: Storage,
    ) -> Result<(), Diagnostic> {
        let resolution = self.resolve_intrinsic(name, args)?;
        if resolution.result_types.len() != 2
            || self.model.resolved_type(&resolution.result_types[0])?
                != self.model.resolved_type(first_ty)?
            || self.model.resolved_type(&resolution.result_types[1])?
                != self.model.resolved_type(second_ty)?
        {
            return Err(Diagnostic::new(format!(
                "result types of intrinsic `{name}` do not match the two-result bindings"
            )));
        }
        let second_width = self.scalar_width(second_ty)?;
        if second_destination.size < u32::from(second_width) {
            return Err(Diagnostic::new(format!(
                "second result destination for `{name}` is too small"
            )));
        }
        let values = self.intrinsic_arguments(args, &resolution)?;
        let constants = args
            .iter()
            .map(|arg| self.model.const_value(arg).ok())
            .collect::<Vec<_>>();
        self.lower_intrinsic(
            resolution.descriptor.operation,
            &resolution.argument_types,
            &values,
            &constants,
            resolution.result_types.first(),
            Some(second_destination),
        )
    }

    fn lower_intrinsic(
        &mut self,
        operation: IntrinsicOperation,
        argument_types: &[Type],
        values: &[Storage],
        constants: &[Option<i64>],
        result_type: Option<&Type>,
        second_destination: Option<Storage>,
    ) -> Result<(), Diagnostic> {
        match operation {
            IntrinsicOperation::Bits(operation) => {
                self.lower_bits_intrinsic(operation, argument_types, values, constants)
            }
            IntrinsicOperation::Int(operation) => {
                self.lower_int_intrinsic(operation, argument_types, values, second_destination)
            }
            IntrinsicOperation::Mem(operation) => self.lower_mem_intrinsic(
                operation,
                argument_types,
                values,
                result_type,
                second_destination,
            ),
        }
    }

    fn lower_bits_intrinsic(
        &mut self,
        operation: BitsIntrinsic,
        argument_types: &[Type],
        values: &[Storage],
        constants: &[Option<i64>],
    ) -> Result<(), Diagnostic> {
        let width = self.scalar_width(&argument_types[0])?;
        match operation {
            BitsIntrinsic::Test
            | BitsIntrinsic::Set
            | BitsIntrinsic::Clear
            | BitsIntrinsic::Toggle => {
                let bit =
                    usize::try_from(constants[1].ok_or_else(|| {
                        Diagnostic::new("bit index must be a compile-time constant")
                    })?)
                    .map_err(|_| Diagnostic::new("bit index must be non-negative"))?;
                let byte = (bit / 8) as u32;
                let mask = 1u8 << (bit % 8);
                self.copy(values[0], self.r0, u32::from(width));
                self.load_al(self.r0.address + byte);
                match operation {
                    BitsIntrinsic::Test => {
                        self.line(&format!("    test al,{}", imm(u32::from(mask))));
                        let set = self.next_label("intrinsic_bit_set");
                        let done = self.next_label("intrinsic_bit_test_done");
                        self.branch_long("jnz", &set);
                        self.load_constant(0, 1);
                        self.line(&format!("    jmp near {done}"));
                        self.line(&format!("{set}:"));
                        self.load_constant(1, 1);
                        self.line(&format!("{done}:"));
                    }
                    BitsIntrinsic::Set => {
                        self.line(&format!("    or al,{}", imm(u32::from(mask))));
                        self.store_al(self.r0.address + byte);
                    }
                    BitsIntrinsic::Clear => {
                        self.line(&format!("    and al,{}", imm(u32::from(!mask & 0xff))));
                        self.store_al(self.r0.address + byte);
                    }
                    BitsIntrinsic::Toggle => {
                        self.line(&format!("    xor al,{}", imm(u32::from(mask))));
                        self.store_al(self.r0.address + byte);
                    }
                    _ => unreachable!(),
                }
            }
            BitsIntrinsic::Extract => {
                let offset = u32::try_from(constants[1].ok_or_else(|| {
                    Diagnostic::new("bit offset must be a compile-time constant")
                })?)
                .map_err(|_| Diagnostic::new("bit offset must be non-negative"))?;
                let bits =
                    u32::try_from(constants[2].ok_or_else(|| {
                        Diagnostic::new("bit width must be a compile-time constant")
                    })?)
                    .map_err(|_| Diagnostic::new("bit width must be non-negative"))?;
                self.copy(values[0], self.r0, u32::from(width));
                for _ in 0..offset {
                    self.shift_storage_once(self.r0, width, true, false);
                }
                self.intrinsic_mask(self.r0, width, intrinsic_mask(bits));
            }
            BitsIntrinsic::Insert => {
                let offset = u32::try_from(constants[2].ok_or_else(|| {
                    Diagnostic::new("bit offset must be a compile-time constant")
                })?)
                .map_err(|_| Diagnostic::new("bit offset must be non-negative"))?;
                let bits =
                    u32::try_from(constants[3].ok_or_else(|| {
                        Diagnostic::new("bit width must be a compile-time constant")
                    })?)
                    .map_err(|_| Diagnostic::new("bit width must be non-negative"))?;
                let mask = intrinsic_mask(bits).checked_shl(offset).unwrap_or(0);
                self.copy(values[0], self.r0, u32::from(width));
                self.copy(values[1], self.r1, u32::from(width));
                for _ in 0..offset {
                    self.shift_storage_once(self.r1, width, false, false);
                }
                self.intrinsic_mask(self.r1, width, mask);
                self.intrinsic_mask(self.r0, width, !mask);
                self.intrinsic_or(self.r0, self.r1, width);
            }
            BitsIntrinsic::ByteSwap => {
                self.copy(values[0], self.r0, u32::from(width));
                for offset in 0..u32::from(width) / 2 {
                    let other = u32::from(width) - offset - 1;
                    self.load_al(self.r0.address + offset);
                    self.line(&format!("    xchg al,{}", mem(self.r0.address + other)));
                    self.store_al(self.r0.address + offset);
                }
            }
            BitsIntrinsic::Reverse => self.intrinsic_reverse(values[0], width),
            BitsIntrinsic::CountOnes => self.intrinsic_count_bits(values[0], width, false),
            BitsIntrinsic::LeadingZeros => self.intrinsic_count_bits(values[0], width, true),
            BitsIntrinsic::TrailingZeros => self.intrinsic_count_trailing_bits(values[0], width),
            BitsIntrinsic::RotateLeft | BitsIntrinsic::RotateRight => {
                self.intrinsic_rotate(operation, values[0], values[1], width)?
            }
        }
        Ok(())
    }

    fn lower_int_intrinsic(
        &mut self,
        operation: IntIntrinsic,
        argument_types: &[Type],
        values: &[Storage],
        second_destination: Option<Storage>,
    ) -> Result<(), Diagnostic> {
        let width = self.scalar_width(&argument_types[0])?;
        let signed = self.type_is_signed(&argument_types[0])?;
        match operation {
            IntIntrinsic::WideningMul => {
                let product_width = width + self.scalar_width(&argument_types[1])?;
                let product = self.intrinsic_full_product(
                    values[0],
                    values[1],
                    width,
                    self.scalar_width(&argument_types[1])?,
                    signed,
                )?;
                self.copy(product, self.r0, u32::from(product_width));
            }
            IntIntrinsic::MulHigh => {
                let product =
                    self.intrinsic_full_product(values[0], values[1], width, width, signed)?;
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
                self.copy(values[0], self.r0, u32::from(width));
                self.copy(values[1], self.r1, u32::from(width));
                if operation == IntIntrinsic::SaturatingAdd {
                    self.add(width);
                } else {
                    self.sub(width);
                }
                let overflow = self.next_label("intrinsic_saturating_overflow");
                let done = self.next_label("intrinsic_saturating_done");
                if !signed {
                    if operation == IntIntrinsic::SaturatingAdd {
                        self.branch_long("jc", &overflow);
                    } else {
                        self.branch_long("jc", &overflow);
                    }
                    self.line(&format!("    jmp near {done}"));
                    self.line(&format!("{overflow}:"));
                    self.load_constant_into(
                        self.r0,
                        if operation == IntIntrinsic::SaturatingAdd {
                            (1_i64 << (u32::from(width) * 8)) - 1
                        } else {
                            0
                        },
                        width,
                    );
                } else {
                    let lhs_negative = self.next_label("intrinsic_lhs_negative");
                    let no_overflow = self.next_label("intrinsic_no_overflow");
                    self.load_al(values[0].address + u32::from(width - 1));
                    self.line("    test al,80h");
                    self.branch_long("jnz", &lhs_negative);
                    self.load_al(values[1].address + u32::from(width - 1));
                    self.line("    test al,80h");
                    if operation == IntIntrinsic::SaturatingAdd {
                        self.branch_long("jnz", &no_overflow);
                        self.load_al(self.r0.address + u32::from(width - 1));
                        self.line("    test al,80h");
                        self.branch_long("jz", &no_overflow);
                        self.load_constant_into(
                            self.r0,
                            (1_i64 << (u32::from(width) * 8 - 1)) - 1,
                            width,
                        );
                        self.line(&format!("    jmp near {done}"));
                    } else {
                        let lhs_positive_rhs_negative =
                            self.next_label("intrinsic_sub_positive_negative");
                        self.branch_long("jnz", &lhs_positive_rhs_negative);
                        self.line(&format!("    jmp near {no_overflow}"));
                        self.line(&format!("{lhs_positive_rhs_negative}:"));
                        self.load_al(self.r0.address + u32::from(width - 1));
                        self.line("    test al,80h");
                        self.branch_long("jz", &no_overflow);
                        self.load_constant_into(
                            self.r0,
                            (1_i64 << (u32::from(width) * 8 - 1)) - 1,
                            width,
                        );
                        self.line(&format!("    jmp near {done}"));
                    }
                    self.line(&format!("{lhs_negative}:"));
                    self.load_al(values[1].address + u32::from(width - 1));
                    self.line("    test al,80h");
                    if operation == IntIntrinsic::SaturatingAdd {
                        self.branch_long("jz", &no_overflow);
                        self.load_al(self.r0.address + u32::from(width - 1));
                        self.line("    test al,80h");
                        self.branch_long("jnz", &no_overflow);
                    } else {
                        self.branch_long("jnz", &no_overflow);
                        self.load_al(self.r0.address + u32::from(width - 1));
                        self.line("    test al,80h");
                        self.branch_long("jnz", &no_overflow);
                    }
                    self.load_constant_into(self.r0, -(1_i64 << (u32::from(width) * 8 - 1)), width);
                }
                self.line(&format!("{done}:"));
            }
            IntIntrinsic::Divmod => {
                let Some(second_destination) = second_destination else {
                    return Err(Diagnostic::new(
                        "divmod requires a second result destination",
                    ));
                };
                let quotient = self.model.allocate(u32::from(width))?;
                self.copy(values[0], self.r0, u32::from(width));
                self.copy(values[1], self.r1, u32::from(width));
                self.divide(width, false, signed)?;
                self.copy(self.r0, quotient, u32::from(width));
                self.copy(values[0], self.r0, u32::from(width));
                self.copy(values[1], self.r1, u32::from(width));
                self.divide(width, true, signed)?;
                self.copy(self.r0, second_destination, u32::from(width));
                self.copy(quotient, self.r0, u32::from(width));
            }
            IntIntrinsic::AddCarry | IntIntrinsic::SubBorrow => {
                let Some(second_destination) = second_destination else {
                    return Err(Diagnostic::new(
                        "paired integer intrinsic requires a second result destination",
                    ));
                };
                self.copy(values[0], self.r0, u32::from(width));
                self.copy(values[1], self.r1, u32::from(width));
                let with_flag = self.next_label("intrinsic_with_flag");
                let no_flag = self.next_label("intrinsic_without_flag");
                let done = self.next_label("intrinsic_carry_done");
                self.jump_storage_nonzero(values[2], 1, &with_flag);
                self.line(&format!("{no_flag}:"));
                self.line(if operation == IntIntrinsic::AddCarry {
                    "    clc"
                } else {
                    "    clc"
                });
                self.intrinsic_add_sub_with_carry(
                    width,
                    operation == IntIntrinsic::AddCarry,
                    false,
                );
                self.line(&format!("    jmp near {done}"));
                self.line(&format!("{with_flag}:"));
                self.line("    stc");
                self.intrinsic_add_sub_with_carry(width, operation == IntIntrinsic::AddCarry, true);
                self.line(&format!("{done}:"));
                let no_carry = self.next_label("intrinsic_no_carry");
                self.branch_long("jnc", &no_carry);
                self.load_constant_into(second_destination, 1, 1);
                let result_done = self.next_label("intrinsic_carry_result_done");
                self.line(&format!("    jmp near {result_done}"));
                self.line(&format!("{no_carry}:"));
                self.load_constant_into(second_destination, 0, 1);
                self.line(&format!("{result_done}:"));
            }
            IntIntrinsic::FullMul => {
                let Some(second_destination) = second_destination else {
                    return Err(Diagnostic::new(
                        "full_mul requires a second result destination",
                    ));
                };
                let product =
                    self.intrinsic_full_product(values[0], values[1], width, width, signed)?;
                self.copy(product, self.r0, u32::from(width));
                self.copy(
                    Storage {
                        address: product.address + u32::from(width),
                        size: u32::from(width),
                    },
                    second_destination,
                    u32::from(width),
                );
            }
        }
        Ok(())
    }

    fn lower_mem_intrinsic(
        &mut self,
        operation: MemIntrinsic,
        argument_types: &[Type],
        values: &[Storage],
        _result_type: Option<&Type>,
        second_destination: Option<Storage>,
    ) -> Result<(), Diagnostic> {
        match operation {
            MemIntrinsic::Peek8 => {
                self.load_bx(values[0].address);
                self.load_indirect(1);
            }
            MemIntrinsic::Poke8 => {
                self.load_bx(values[0].address);
                self.load_al(values[1].address);
                self.line("    mov [bx],al");
            }
            MemIntrinsic::LoadLe16
            | MemIntrinsic::LoadBe16
            | MemIntrinsic::LoadLe24
            | MemIntrinsic::LoadBe24 => {
                let width = match operation {
                    MemIntrinsic::LoadLe16 | MemIntrinsic::LoadBe16 => 2,
                    _ => 3,
                };
                self.load_bx(values[0].address);
                for offset in 0..width {
                    let source_offset =
                        if matches!(operation, MemIntrinsic::LoadLe16 | MemIntrinsic::LoadLe24) {
                            offset
                        } else {
                            width - 1 - offset
                        };
                    self.line(&format!("    mov al,{}", indexed_bx(source_offset)));
                    self.store_al(self.r0.address + u32::from(offset));
                }
            }
            MemIntrinsic::StoreLe16
            | MemIntrinsic::StoreBe16
            | MemIntrinsic::StoreLe24
            | MemIntrinsic::StoreBe24 => {
                let width = match operation {
                    MemIntrinsic::StoreLe16 | MemIntrinsic::StoreBe16 => 2,
                    _ => 3,
                };
                self.load_bx(values[0].address);
                for offset in 0..width {
                    let source_offset =
                        if matches!(operation, MemIntrinsic::StoreLe16 | MemIntrinsic::StoreLe24) {
                            offset
                        } else {
                            width - 1 - offset
                        };
                    self.load_al(values[1].address + u32::from(source_offset));
                    self.line(&format!("    mov {},al", indexed_bx(offset)));
                }
                self.zero(self.r0);
            }
            MemIntrinsic::CopyNonoverlapping | MemIntrinsic::Move | MemIntrinsic::Fill => {
                self.intrinsic_memory_range(operation, values)?;
                self.zero(self.r0);
            }
            MemIntrinsic::FindByte => {
                let Some(second_destination) = second_destination else {
                    return Err(Diagnostic::new(
                        "find_byte requires a second result destination",
                    ));
                };
                self.intrinsic_find_byte(values, second_destination)?;
            }
            MemIntrinsic::Compare => self.intrinsic_compare(values),
        }
        let _ = argument_types;
        Ok(())
    }

    fn intrinsic_mask(&mut self, storage: Storage, width: u8, mask: u32) {
        for offset in 0..u32::from(width) {
            self.load_al(storage.address + offset);
            self.line(&format!(
                "    and al,{}",
                imm((mask >> (offset * 8)) & 0xff)
            ));
            self.store_al(storage.address + offset);
        }
    }

    fn intrinsic_or(&mut self, target: Storage, source: Storage, width: u8) {
        for offset in 0..u32::from(width) {
            self.load_al(target.address + offset);
            self.line(&format!("    or al,{}", mem(source.address + offset)));
            self.store_al(target.address + offset);
        }
    }

    fn intrinsic_reverse(&mut self, source: Storage, width: u8) {
        let input = self
            .model
            .allocate(u32::from(width))
            .expect("reverse input");
        let output = self
            .model
            .allocate(u32::from(width))
            .expect("reverse output");
        self.copy(source, input, u32::from(width));
        self.zero(output);
        for _ in 0..u32::from(width) * 8 {
            self.shift_storage_once(output, width, false, false);
            self.load_al(input.address);
            self.line("    and al,1");
            let no_bit = self.next_label("reverse_no_bit");
            self.branch_long("jz", &no_bit);
            self.load_al(output.address);
            self.line("    or al,1");
            self.store_al(output.address);
            self.line(&format!("{no_bit}:"));
            self.shift_storage_once(input, width, true, false);
        }
        self.copy(output, self.r0, u32::from(width));
    }

    fn intrinsic_count_bits(&mut self, source: Storage, width: u8, leading: bool) {
        let input = self.model.allocate(u32::from(width)).expect("count input");
        self.copy(source, input, u32::from(width));
        self.zero(self.r0);
        let done = self.next_label("count_bits_done");
        for _ in 0..u32::from(width) * 8 {
            let offset = if leading { u32::from(width - 1) } else { 0 };
            self.load_al(input.address + offset);
            self.line(if leading {
                "    test al,80h"
            } else {
                "    test al,1"
            });
            if !leading {
                let bit = self.next_label("count_bits_one");
                self.branch_long("jnz", &bit);
                self.line(&format!("    jmp near {done}"));
                self.line(&format!("{bit}:"));
            } else {
                self.branch_long("jnz", &done);
            }
            self.load_al(self.r0.address);
            self.line("    inc al");
            self.store_al(self.r0.address);
            self.shift_storage_once(input, width, leading, false);
        }
        self.line(&format!("{done}:"));
    }

    fn intrinsic_count_trailing_bits(&mut self, source: Storage, width: u8) {
        self.intrinsic_count_bits(source, width, false);
    }

    fn intrinsic_rotate(
        &mut self,
        operation: BitsIntrinsic,
        value: Storage,
        count: Storage,
        width: u8,
    ) -> Result<(), Diagnostic> {
        let count_width = count.size as u8;
        let remaining = self.model.allocate(count.size)?;
        self.copy(count, remaining, count.size);
        let one = self.model.allocate(count.size)?;
        self.load_constant_into(one, 1, count_width);
        self.copy(value, self.r0, u32::from(width));
        let loop_label = self.next_label("rotate_loop");
        let done = self.next_label("rotate_done");
        self.line(&format!("{loop_label}:"));
        self.jump_storage_zero(remaining, count_width, &done);
        self.shift_storage_once(
            self.r0,
            width,
            operation == BitsIntrinsic::RotateRight,
            false,
        );
        let no_carry = self.next_label("rotate_no_carry");
        self.branch_long("jnc", &no_carry);
        if operation == BitsIntrinsic::RotateRight {
            self.load_al(self.r0.address + u32::from(width - 1));
            self.line("    or al,80h");
            self.store_al(self.r0.address + u32::from(width - 1));
        } else {
            self.load_al(self.r0.address);
            self.line("    or al,1");
            self.store_al(self.r0.address);
        }
        self.line(&format!("{no_carry}:"));
        self.sub_storage(remaining, one, count_width);
        self.line(&format!("    jmp near {loop_label}"));
        self.line(&format!("{done}:"));
        Ok(())
    }

    fn intrinsic_full_product(
        &mut self,
        left: Storage,
        right: Storage,
        left_width: u8,
        right_width: u8,
        signed: bool,
    ) -> Result<Storage, Diagnostic> {
        let product_width = left_width + right_width;
        if left_width == 1 && right_width == 1 {
            let product = self.model.allocate(2)?;
            self.load_al(left.address);
            self.line(&format!("    mov bl,{}", mem(right.address)));
            self.line(&format!("    {} bl", if signed { "imul" } else { "mul" }));
            self.line(&format!("    mov {},ax", mem(product.address)));
            return Ok(product);
        }
        if left_width == 2 && right_width == 2 {
            let product = self.model.allocate(4)?;
            self.load_ax(left.address);
            self.load_bx(right.address);
            self.line(&format!("    {} bx", if signed { "imul" } else { "mul" }));
            self.line(&format!("    mov {},ax", mem(product.address)));
            self.line(&format!("    mov {},dx", mem(product.address + 2)));
            return Ok(product);
        }
        let product = self.model.allocate(u32::from(product_width))?;
        let multiplicand = self.model.allocate(u32::from(product_width))?;
        let multiplier = self.model.allocate(u32::from(right_width))?;
        let negative = self.model.allocate(1)?;
        self.zero(product);
        self.zero(multiplicand);
        self.copy(left, multiplicand, u32::from(left_width));
        self.copy(right, multiplier, u32::from(right_width));
        self.zero(negative);
        if signed {
            self.normalize_signed(multiplicand, left_width, negative, false);
            self.normalize_signed(multiplier, right_width, negative, true);
        }
        for _ in 0..u32::from(right_width) * 8 {
            self.load_al(multiplier.address);
            self.line("    test al,1");
            let no_add = self.next_label("multiply_no_add");
            self.branch_long("jz", &no_add);
            self.add_storage(product, multiplicand, product_width);
            self.line(&format!("{no_add}:"));
            self.shift_storage_once(multiplicand, product_width, false, false);
            self.shift_storage_once(multiplier, right_width, true, false);
        }
        if signed {
            self.negate_if(negative, product, product_width);
        }
        Ok(product)
    }

    fn intrinsic_add_sub_with_carry(&mut self, width: u8, add: bool, carry_in: bool) {
        for offset in 0..u32::from(width) {
            self.load_al(self.r0.address + offset);
            let mnemonic = if offset == 0 {
                if add { "adc" } else { "sbb" }
            } else if add {
                "adc"
            } else {
                "sbb"
            };
            self.line(&format!(
                "    {mnemonic} al,{}",
                mem(self.r1.address + offset)
            ));
            self.store_al(self.r0.address + offset);
            if offset == 0 && !carry_in {
                // The carry flag was cleared by the caller; the first operation
                // still uses ADC/SBB so later bytes see the same flag chain.
            }
        }
    }

    fn intrinsic_memory_range(
        &mut self,
        operation: MemIntrinsic,
        values: &[Storage],
    ) -> Result<(), Diagnostic> {
        let length = values[2];
        if operation == MemIntrinsic::Fill {
            self.load_di(values[0].address);
            self.load_al(values[1].address);
            let manual = self.next_label("fill_manual");
            let done = self.next_label("fill_done");
            self.load_al(length.address + 2);
            self.line("    or al,al");
            self.branch_long("jnz", &manual);
            self.load_cx(length.address);
            self.load_al(values[1].address);
            self.line("    rep stosb");
            self.line(&format!("    jmp near {done}"));
            self.line(&format!("{manual}:"));
            let loop_label = self.next_label("fill_loop");
            self.line(&format!("{loop_label}:"));
            self.jump_storage_zero(length, 3, &done);
            self.load_al(values[1].address);
            self.line("    mov [di],al");
            self.line("    inc di");
            self.decrement_storage(length, 3);
            self.line(&format!("    jmp near {loop_label}"));
            self.line(&format!("{done}:"));
            return Ok(());
        }
        self.load_si(values[1].address);
        self.load_di(values[0].address);
        if operation == MemIntrinsic::Move {
            let forward = self.next_label("move_forward");
            self.line("    cmp di,si");
            self.branch_long("jbe", &forward);
            self.intrinsic_copy_backward(length);
            let done = self.next_label("move_done");
            self.line(&format!("    jmp near {done}"));
            self.line(&format!("{forward}:"));
            self.intrinsic_copy_forward(length);
            self.line(&format!("{done}:"));
        } else {
            self.intrinsic_copy_forward(length);
        }
        Ok(())
    }

    fn intrinsic_copy_forward(&mut self, length: Storage) {
        let manual = self.next_label("copy_manual");
        let done = self.next_label("copy_done");
        self.load_al(length.address + 2);
        self.line("    or al,al");
        self.branch_long("jnz", &manual);
        self.load_cx(length.address);
        self.line("    rep movsb");
        self.line(&format!("    jmp near {done}"));
        self.line(&format!("{manual}:"));
        let loop_label = self.next_label("copy_loop");
        self.line(&format!("{loop_label}:"));
        self.jump_storage_zero(length, 3, &done);
        self.line("    mov al,[si]");
        self.line("    mov [di],al");
        self.line("    inc si");
        self.line("    inc di");
        self.decrement_storage(length, 3);
        self.line(&format!("    jmp near {loop_label}"));
        self.line(&format!("{done}:"));
    }

    fn intrinsic_copy_backward(&mut self, length: Storage) {
        self.load_bx(length.address);
        self.line("    dec bx");
        self.line("    add si,bx");
        self.line("    add di,bx");
        let loop_label = self.next_label("copy_backward_loop");
        let done = self.next_label("copy_backward_done");
        self.line(&format!("{loop_label}:"));
        self.jump_storage_zero(length, 3, &done);
        self.line("    mov al,[si]");
        self.line("    mov [di],al");
        self.line("    dec si");
        self.line("    dec di");
        self.decrement_storage(length, 3);
        self.line(&format!("    jmp near {loop_label}"));
        self.line(&format!("{done}:"));
    }

    fn intrinsic_find_byte(
        &mut self,
        values: &[Storage],
        second_destination: Storage,
    ) -> Result<(), Diagnostic> {
        self.load_si(values[0].address);
        let length = values[1];
        let found = self.next_label("find_byte_found");
        let next = self.next_label("find_byte_next");
        let not_found = self.next_label("find_byte_not_found");
        let done = self.next_label("find_byte_done");
        let loop_label = self.next_label("find_byte_loop");
        self.line(&format!("{loop_label}:"));
        self.jump_storage_zero(length, 3, &not_found);
        self.line("    mov al,[si]");
        self.line(&format!("    cmp al,{}", mem(values[2].address)));
        self.branch_long("je", &found);
        self.line(&format!("{next}:"));
        self.line("    inc si");
        self.decrement_storage(length, 3);
        self.line(&format!("    jmp near {loop_label}"));
        self.line(&format!("{found}:"));
        self.line("    mov ax,si");
        self.line(&format!("    mov {},ax", mem(self.r0.address)));
        self.load_constant_into(second_destination, 1, 1);
        self.line(&format!("    jmp near {done}"));
        self.line(&format!("{not_found}:"));
        self.line("    mov ax,si");
        self.line(&format!("    mov {},ax", mem(self.r0.address)));
        self.load_constant_into(second_destination, 0, 1);
        self.line(&format!("{done}:"));
        Ok(())
    }

    fn intrinsic_compare(&mut self, values: &[Storage]) {
        self.load_si(values[0].address);
        self.load_di(values[1].address);
        let length = values[2];
        let less = self.next_label("compare_less");
        let greater = self.next_label("compare_greater");
        let next = self.next_label("compare_next");
        let equal = self.next_label("compare_equal");
        let done = self.next_label("compare_done");
        let loop_label = self.next_label("compare_loop");
        self.line(&format!("{loop_label}:"));
        self.jump_storage_zero(length, 3, &equal);
        self.line("    mov al,[si]");
        self.line("    cmp al,[di]");
        self.branch_long("jb", &less);
        self.branch_long("ja", &greater);
        self.line(&format!("{next}:"));
        self.line("    inc si");
        self.line("    inc di");
        self.decrement_storage(length, 3);
        self.line(&format!("    jmp near {loop_label}"));
        self.line(&format!("{less}:"));
        self.load_constant(-1, 1);
        self.line(&format!("    jmp near {done}"));
        self.line(&format!("{greater}:"));
        self.load_constant(1, 1);
        self.line(&format!("    jmp near {done}"));
        self.line(&format!("{equal}:"));
        self.load_constant(0, 1);
        self.line(&format!("{done}:"));
    }

    fn decrement_storage(&mut self, storage: Storage, width: u8) {
        for offset in 0..u32::from(width) {
            self.load_al(storage.address + offset);
            self.line(&format!(
                "    {} al,{}",
                if offset == 0 { "sub" } else { "sbb" },
                imm(if offset == 0 { 1 } else { 0 })
            ));
            self.store_al(storage.address + offset);
        }
    }

    fn unary(&mut self, op: UnaryOp, width: u8) {
        match op {
            UnaryOp::BitNot => {
                for offset in 0..u32::from(width) {
                    self.load_al(self.r0.address + offset);
                    self.line("    not al");
                    self.store_al(self.r0.address + offset);
                }
            }
            UnaryOp::Neg => self.negate(self.r0, width),
            UnaryOp::Not => {
                let yes = self.next_label("not_true");
                let done = self.next_label("not_done");
                self.jump_storage_zero(self.r0, width, &yes);
                self.load_constant(0, width);
                self.line(&format!("    jmp near {done}"));
                self.line(&format!("{yes}:"));
                self.load_constant(1, width);
                self.line(&format!("{done}:"));
            }
        }
    }

    fn binary(
        &mut self,
        op: BinaryOp,
        width: u8,
        signed: bool,
        constant: Option<i64>,
    ) -> Result<(), Diagnostic> {
        if width == 4
            && let Some(value) = constant
            && self.binary_immediate(op, value, signed)?
        {
            return Ok(());
        }
        match op {
            BinaryOp::Add => self.add(width),
            BinaryOp::Sub => self.sub(width),
            BinaryOp::BitAnd | BinaryOp::BitOr | BinaryOp::BitXor => {
                let mnemonic = match op {
                    BinaryOp::BitAnd => "and",
                    BinaryOp::BitOr => "or",
                    _ => "xor",
                };
                if width == 4 {
                    for offset in [0, 2] {
                        self.load_ax(self.r0.address + offset);
                        self.line(&format!(
                            "    {mnemonic} ax,{}",
                            mem(self.r1.address + offset)
                        ));
                        self.line(&format!("    mov {},ax", mem(self.r0.address + offset)));
                    }
                } else {
                    for offset in 0..u32::from(width) {
                        self.load_al(self.r0.address + offset);
                        self.line(&format!(
                            "    {mnemonic} al,{}",
                            mem(self.r1.address + offset)
                        ));
                        self.store_al(self.r0.address + offset);
                    }
                }
            }
            BinaryOp::Mul => self.multiply(width, signed)?,
            BinaryOp::Div | BinaryOp::Mod => self.divide(width, op == BinaryOp::Mod, signed)?,
            BinaryOp::Shl | BinaryOp::Shr => {
                self.shift(width, op == BinaryOp::Shr, signed, constant)
            }
            BinaryOp::And | BinaryOp::Or => unreachable!("short-circuited"),
            op if is_comparison(op) => self.compare(op, width, signed),
            _ => return Err(Diagnostic::new("unsupported i8086 binary operation")),
        }
        Ok(())
    }

    fn binary_immediate(
        &mut self,
        op: BinaryOp,
        value: i64,
        signed: bool,
    ) -> Result<bool, Diagnostic> {
        let value = value as u32;
        match op {
            BinaryOp::Add | BinaryOp::Sub if value == 0 => return Ok(true),
            BinaryOp::BitOr | BinaryOp::BitXor if value == 0 => return Ok(true),
            BinaryOp::BitAnd if value == u32::MAX => return Ok(true),
            BinaryOp::Mul if value == 0 => {
                self.zero(self.r0);
                return Ok(true);
            }
            BinaryOp::Mul if value == 1 => return Ok(true),
            BinaryOp::Mul if value == u32::MAX => {
                self.negate(self.r0, 4);
                return Ok(true);
            }
            BinaryOp::Div if value == 1 => return Ok(true),
            BinaryOp::Div if signed && value == u32::MAX => {
                self.negate(self.r0, 4);
                return Ok(true);
            }
            BinaryOp::Mod if value == 1 || (signed && value == u32::MAX) => {
                self.zero(self.r0);
                return Ok(true);
            }
            BinaryOp::Shl | BinaryOp::Shr => {
                self.shift_fixed(op == BinaryOp::Shr, signed, value);
                return Ok(true);
            }
            BinaryOp::Div | BinaryOp::Mod
                if value.is_power_of_two() && (!signed || value <= i32::MAX as u32) =>
            {
                self.divide_power_of_two(value.trailing_zeros(), op == BinaryOp::Mod, signed)?;
                return Ok(true);
            }
            BinaryOp::Add | BinaryOp::Sub => {
                self.word_immediate_arithmetic(op, value);
                return Ok(true);
            }
            BinaryOp::BitAnd | BinaryOp::BitOr | BinaryOp::BitXor => {
                self.word_immediate_bitwise(op, value);
                return Ok(true);
            }
            op if is_comparison(op) => {
                self.compare_immediate(op, value, signed);
                return Ok(true);
            }
            _ => {}
        }
        Ok(false)
    }

    fn word_immediate_arithmetic(&mut self, op: BinaryOp, value: u32) {
        let (low_op, high_op) = if op == BinaryOp::Add {
            ("add", "adc")
        } else {
            ("sub", "sbb")
        };
        self.load_ax(self.r0.address);
        self.line(&format!("    {low_op} ax,{}", imm(value & 0xffff)));
        self.line(&format!("    mov {},ax", mem(self.r0.address)));
        self.load_ax(self.r0.address + 2);
        self.line(&format!("    {high_op} ax,{}", imm(value >> 16)));
        self.line(&format!("    mov {},ax", mem(self.r0.address + 2)));
    }

    fn divide_power_of_two(
        &mut self,
        shift: u32,
        remainder: bool,
        signed: bool,
    ) -> Result<(), Diagnostic> {
        if !signed {
            if remainder {
                self.word_immediate_bitwise(BinaryOp::BitAnd, (1u32 << shift) - 1);
            } else {
                self.shift_fixed(true, false, shift);
            }
            return Ok(());
        }

        let original = self.model.allocate(4)?;
        self.copy(self.r0, original, 4);
        let nonnegative = self.next_label("signed_power_two_nonnegative");
        self.load_ax(self.r0.address + 2);
        self.line("    test ax,8000h");
        self.branch_long("jz", &nonnegative);
        self.word_immediate_arithmetic(BinaryOp::Add, (1u32 << shift) - 1);
        self.line(&format!("{nonnegative}:"));
        self.shift_fixed(true, true, shift);
        if remainder {
            let quotient = self.model.allocate(4)?;
            self.copy(self.r0, quotient, 4);
            for _ in 0..shift {
                self.shift_storage_once(quotient, 4, false, false);
            }
            self.copy(original, self.r0, 4);
            self.sub_storage(self.r0, quotient, 4);
        }
        Ok(())
    }

    fn word_immediate_bitwise(&mut self, op: BinaryOp, value: u32) {
        let mnemonic = match op {
            BinaryOp::BitAnd => "and",
            BinaryOp::BitOr => "or",
            BinaryOp::BitXor => "xor",
            _ => unreachable!(),
        };
        for (offset, word) in [(0, value & 0xffff), (2, value >> 16)] {
            self.load_ax(self.r0.address + offset);
            self.line(&format!("    {mnemonic} ax,{}", imm(word)));
            self.line(&format!("    mov {},ax", mem(self.r0.address + offset)));
        }
    }

    fn compare_immediate(&mut self, op: BinaryOp, value: u32, signed: bool) {
        let yes = self.next_label("compare_immediate_true");
        let no = self.next_label("compare_immediate_false");
        let done = self.next_label("compare_immediate_done");
        let low = value & 0xffff;
        let high = value >> 16;
        match op {
            BinaryOp::Eq | BinaryOp::Ne => {
                self.load_ax(self.r0.address + 2);
                self.line(&format!("    cmp ax,{}", imm(high)));
                self.branch_long("jne", if op == BinaryOp::Ne { &yes } else { &no });
                self.load_ax(self.r0.address);
                self.line(&format!("    cmp ax,{}", imm(low)));
                self.branch_long("jne", if op == BinaryOp::Ne { &yes } else { &no });
                self.line(&format!(
                    "    jmp near {}",
                    if op == BinaryOp::Eq { &yes } else { &no }
                ));
            }
            _ => {
                self.load_ax(self.r0.address + 2);
                let ordered_high = if signed { high ^ 0x8000 } else { high };
                if signed {
                    self.line("    xor ax,8000h");
                }
                self.line(&format!("    cmp ax,{}", imm(ordered_high)));
                match op {
                    BinaryOp::Lt | BinaryOp::Le => {
                        self.branch_long("jb", &yes);
                        self.branch_long("jne", &no);
                    }
                    BinaryOp::Gt | BinaryOp::Ge => {
                        self.branch_long("jb", &no);
                        self.branch_long("jne", &yes);
                    }
                    _ => unreachable!(),
                }
                self.load_ax(self.r0.address);
                self.line(&format!("    cmp ax,{}", imm(low)));
                let condition = match op {
                    BinaryOp::Lt => "jb",
                    BinaryOp::Le => "jbe",
                    BinaryOp::Gt => "ja",
                    BinaryOp::Ge => "jae",
                    _ => unreachable!(),
                };
                self.branch_long(condition, &yes);
                self.line(&format!("    jmp near {no}"));
            }
        }
        self.line(&format!("{yes}:"));
        self.load_constant(1, 1);
        self.line(&format!("    jmp near {done}"));
        self.line(&format!("{no}:"));
        self.load_constant(0, 1);
        self.line(&format!("{done}:"));
    }

    fn add(&mut self, width: u8) {
        if width == 4 {
            self.load_ax(self.r0.address);
            self.line(&format!("    add ax,{}", mem(self.r1.address)));
            self.line(&format!("    mov {},ax", mem(self.r0.address)));
            self.load_ax(self.r0.address + 2);
            self.line(&format!("    adc ax,{}", mem(self.r1.address + 2)));
            self.line(&format!("    mov {},ax", mem(self.r0.address + 2)));
            return;
        }
        self.add_storage(self.r0, self.r1, width);
    }

    fn sub(&mut self, width: u8) {
        if width == 4 {
            self.load_ax(self.r0.address);
            self.line(&format!("    sub ax,{}", mem(self.r1.address)));
            self.line(&format!("    mov {},ax", mem(self.r0.address)));
            self.load_ax(self.r0.address + 2);
            self.line(&format!("    sbb ax,{}", mem(self.r1.address + 2)));
            self.line(&format!("    mov {},ax", mem(self.r0.address + 2)));
            return;
        }
        self.sub_storage(self.r0, self.r1, width);
    }

    fn add_storage(&mut self, target: Storage, source: Storage, width: u8) {
        if width == 4 {
            self.load_ax(target.address);
            self.line(&format!("    add ax,{}", mem(source.address)));
            self.line(&format!("    mov {},ax", mem(target.address)));
            self.load_ax(target.address + 2);
            self.line(&format!("    adc ax,{}", mem(source.address + 2)));
            self.line(&format!("    mov {},ax", mem(target.address + 2)));
        } else {
            for offset in 0..u32::from(width) {
                self.load_al(target.address + offset);
                self.line(&format!(
                    "    {} al,{}",
                    if offset == 0 { "add" } else { "adc" },
                    mem(source.address + offset)
                ));
                self.store_al(target.address + offset);
            }
        }
    }

    fn sub_storage(&mut self, target: Storage, source: Storage, width: u8) {
        if width == 4 {
            self.load_ax(target.address);
            self.line(&format!("    sub ax,{}", mem(source.address)));
            self.line(&format!("    mov {},ax", mem(target.address)));
            self.load_ax(target.address + 2);
            self.line(&format!("    sbb ax,{}", mem(source.address + 2)));
            self.line(&format!("    mov {},ax", mem(target.address + 2)));
        } else {
            for offset in 0..u32::from(width) {
                self.load_al(target.address + offset);
                self.line(&format!(
                    "    {} al,{}",
                    if offset == 0 { "sub" } else { "sbb" },
                    mem(source.address + offset)
                ));
                self.store_al(target.address + offset);
            }
        }
    }

    fn multiply(&mut self, width: u8, signed: bool) -> Result<(), Diagnostic> {
        if width <= 2 {
            let mnemonic = if signed { "imul" } else { "mul" };
            if width == 1 {
                self.load_al(self.r0.address);
                self.line(&format!("    mov bl,{}", mem(self.r1.address)));
                self.line(&format!("    {mnemonic} bl"));
                self.store_al(self.r0.address);
            } else {
                self.load_ax(self.r0.address);
                self.load_bx(self.r1.address);
                self.line(&format!("    {mnemonic} bx"));
                self.line(&format!("    mov {},ax", mem(self.r0.address)));
            }
            return Ok(());
        }

        debug_assert_eq!(width, 4);
        // Low 32 bits: a0*b0 + ((a0*b1 + a1*b0) << 16). Signedness does
        // not affect the low half of a two's-complement product.
        let _ = signed;
        self.load_ax(self.r0.address);
        self.line("    mov cx,ax");
        self.load_si(self.r0.address + 2);
        self.load_bx(self.r1.address);
        self.line("    mul bx");
        self.line(&format!("    mov {},ax", mem(self.r0.address)));
        self.line(&format!("    mov {},dx", mem(self.r0.address + 2)));
        self.line("    mov ax,cx");
        self.load_bx(self.r1.address + 2);
        self.line("    mul bx");
        self.line(&format!("    add {},ax", mem(self.r0.address + 2)));
        self.line("    mov ax,si");
        self.load_bx(self.r1.address);
        self.line("    mul bx");
        self.line(&format!("    add {},ax", mem(self.r0.address + 2)));
        Ok(())
    }

    fn divide(&mut self, width: u8, remainder: bool, signed: bool) -> Result<(), Diagnostic> {
        let generic = if width == 4 && !signed {
            let generic = self.next_label("divide_u32_generic");
            let done = self.next_label("divide_u32_fast_done");
            self.load_ax(self.r1.address + 2);
            self.line("    or ax,ax");
            self.branch_long("jnz", &generic);
            self.load_bx(self.r1.address);
            self.line("    or bx,bx");
            let zero = self.next_label("divide_u32_fast_zero");
            self.branch_long("jz", &zero);
            self.line("    xor dx,dx");
            self.load_ax(self.r0.address + 2);
            self.line("    div bx");
            if !remainder {
                self.line(&format!("    mov {},ax", mem(self.r0.address + 2)));
            }
            self.load_ax(self.r0.address);
            self.line("    div bx");
            if remainder {
                self.line(&format!("    mov {},dx", mem(self.r0.address)));
                self.line("    xor ax,ax");
                self.line(&format!("    mov {},ax", mem(self.r0.address + 2)));
            } else {
                self.line(&format!("    mov {},ax", mem(self.r0.address)));
            }
            self.line(&format!("    jmp near {done}"));
            self.line(&format!("{zero}:"));
            self.zero(self.r0);
            self.line(&format!("{done}:"));
            let finish = self.next_label("divide_u32_fast_return");
            self.line(&format!("    jmp near {finish}"));
            self.line(&format!("{generic}:"));
            Some(finish)
        } else {
            None
        };
        let quotient = self.r2;
        let quotient_negative = self.model.allocate(1)?;
        let remainder_negative = self.model.allocate(1)?;
        self.zero(quotient_negative);
        self.zero(remainder_negative);
        if signed {
            let positive = self.next_label("dividend_positive");
            self.load_al(self.r0.address + u32::from(width - 1));
            self.line("    test al,80h");
            self.branch_long("jz", &positive);
            self.toggle(quotient_negative);
            self.toggle(remainder_negative);
            self.negate(self.r0, width);
            self.line(&format!("{positive}:"));
            self.normalize_signed(self.r1, width, quotient_negative, true);
        }
        let divisor = self.model.allocate(u32::from(width))?;
        let remainder_value = self.model.allocate(u32::from(width))?;
        self.copy(self.r1, divisor, u32::from(width));
        self.copy(self.r0, quotient, u32::from(width));
        self.zero(remainder_value);
        let zero = self.next_label("divide_zero");
        let finish = self.next_label("divide_finish");
        self.jump_storage_zero(divisor, width, &zero);

        // Restoring division shifts one dividend bit into the remainder per
        // iteration. Runtime is bounded by the scalar width instead of its value.
        for _ in 0..u32::from(width) * 8 {
            self.shift_dividend_bit_into_remainder(quotient, remainder_value, width);
            let less = self.next_label("divide_less");
            self.jump_less(remainder_value, divisor, width, &less);
            self.sub_storage(remainder_value, divisor, width);
            self.load_al(quotient.address);
            self.line("    or al,1");
            self.store_al(quotient.address);
            self.line(&format!("{less}:"));
        }
        if remainder {
            self.copy(remainder_value, self.r0, u32::from(width));
        } else {
            self.copy(quotient, self.r0, u32::from(width));
        }
        self.line(&format!("    jmp near {finish}"));
        self.line(&format!("{zero}:"));
        self.zero(self.r0);
        self.line(&format!("{finish}:"));
        if signed {
            self.negate_if(
                if remainder {
                    remainder_negative
                } else {
                    quotient_negative
                },
                self.r0,
                width,
            );
        }
        if let Some(finish) = generic {
            self.line(&format!("{finish}:"));
        }
        Ok(())
    }

    fn shift(&mut self, width: u8, right: bool, signed: bool, constant: Option<i64>) {
        if let Some(count) = constant {
            if width == 4 {
                self.shift_fixed(right, signed, count as u32);
            } else {
                for _ in 0..(count as u32).min(u32::from(width) * 8) {
                    self.shift_once(width, right, signed);
                }
            }
            return;
        }

        let loop_label = self.next_label("shift_loop");
        let done = self.next_label("shift_done");
        self.load_cl(self.r1.address);
        self.line(&format!("{loop_label}:"));
        self.line("    or cl,cl");
        self.branch_long("jz", &done);
        self.shift_once(width, right, signed);
        self.line("    dec cl");
        self.line(&format!("    jmp near {loop_label}"));
        self.line(&format!("{done}:"));
    }

    fn shift_fixed(&mut self, right: bool, signed: bool, count: u32) {
        if count == 0 {
            return;
        }
        if count >= 32 {
            if right && signed {
                self.load_ax(self.r0.address + 2);
                for _ in 0..15 {
                    self.line("    sar ax,1");
                }
                self.line(&format!("    mov {},ax", mem(self.r0.address)));
                self.line(&format!("    mov {},ax", mem(self.r0.address + 2)));
            } else {
                self.zero(self.r0);
            }
            return;
        }
        if count == 16 {
            if right {
                self.load_ax(self.r0.address + 2);
                self.line(&format!("    mov {},ax", mem(self.r0.address)));
                if signed {
                    for _ in 0..15 {
                        self.line("    sar ax,1");
                    }
                    self.line(&format!("    mov {},ax", mem(self.r0.address + 2)));
                } else {
                    self.line("    xor ax,ax");
                    self.line(&format!("    mov {},ax", mem(self.r0.address + 2)));
                }
            } else {
                self.load_ax(self.r0.address);
                self.line(&format!("    mov {},ax", mem(self.r0.address + 2)));
                self.line("    xor ax,ax");
                self.line(&format!("    mov {},ax", mem(self.r0.address)));
            }
            return;
        }
        if count > 16 {
            self.shift_fixed(right, signed, 16);
            self.shift_fixed(right, signed, count - 16);
            return;
        }
        for _ in 0..count {
            self.shift_once(4, right, signed);
        }
    }

    fn shift_once(&mut self, width: u8, right: bool, signed: bool) {
        if width == 4 {
            if right {
                self.load_ax(self.r0.address + 2);
                self.line(if signed {
                    "    sar ax,1"
                } else {
                    "    shr ax,1"
                });
                self.line(&format!("    mov {},ax", mem(self.r0.address + 2)));
                self.load_ax(self.r0.address);
                self.line("    rcr ax,1");
                self.line(&format!("    mov {},ax", mem(self.r0.address)));
            } else {
                self.load_ax(self.r0.address);
                self.line("    shl ax,1");
                self.line(&format!("    mov {},ax", mem(self.r0.address)));
                self.load_ax(self.r0.address + 2);
                self.line("    rcl ax,1");
                self.line(&format!("    mov {},ax", mem(self.r0.address + 2)));
            }
        } else {
            self.shift_storage_once(self.r0, width, right, signed);
        }
    }

    fn shift_storage_once(&mut self, storage: Storage, width: u8, right: bool, signed: bool) {
        if width == 4 {
            if right {
                self.load_ax(storage.address + 2);
                self.line(if signed {
                    "    sar ax,1"
                } else {
                    "    shr ax,1"
                });
                self.line(&format!("    mov {},ax", mem(storage.address + 2)));
                self.load_ax(storage.address);
                self.line("    rcr ax,1");
                self.line(&format!("    mov {},ax", mem(storage.address)));
            } else {
                self.load_ax(storage.address);
                self.line("    shl ax,1");
                self.line(&format!("    mov {},ax", mem(storage.address)));
                self.load_ax(storage.address + 2);
                self.line("    rcl ax,1");
                self.line(&format!("    mov {},ax", mem(storage.address + 2)));
            }
            return;
        }
        if right {
            let top = storage.address + u32::from(width - 1);
            self.load_al(top);
            self.line(if signed {
                "    sar al,1"
            } else {
                "    shr al,1"
            });
            self.store_al(top);
            for offset in (0..u32::from(width - 1)).rev() {
                self.load_al(storage.address + offset);
                self.line("    rcr al,1");
                self.store_al(storage.address + offset);
            }
        } else {
            self.load_al(storage.address);
            self.line("    shl al,1");
            self.store_al(storage.address);
            for offset in 1..u32::from(width) {
                self.load_al(storage.address + offset);
                self.line("    rcl al,1");
                self.store_al(storage.address + offset);
            }
        }
    }

    fn shift_dividend_bit_into_remainder(
        &mut self,
        dividend: Storage,
        remainder: Storage,
        width: u8,
    ) {
        self.load_al(dividend.address);
        self.line("    shl al,1");
        self.store_al(dividend.address);
        for offset in 1..u32::from(width) {
            self.load_al(dividend.address + offset);
            self.line("    rcl al,1");
            self.store_al(dividend.address + offset);
        }
        for offset in 0..u32::from(width) {
            self.load_al(remainder.address + offset);
            self.line("    rcl al,1");
            self.store_al(remainder.address + offset);
        }
    }

    fn short_circuit(&mut self, left: &Expr, op: BinaryOp, right: &Expr) -> Result<(), Diagnostic> {
        if let Ok(value) = self.model.const_value(left) {
            let left_is_true = value != 0;
            let short_circuits = match op {
                BinaryOp::And => !left_is_true,
                BinaryOp::Or => left_is_true,
                _ => false,
            };
            if short_circuits {
                self.load_constant(i64::from(op == BinaryOp::Or), 1);
            } else {
                self.emit_expr(right, &bool_ty())?;
            }
            return Ok(());
        }

        let decisive = self.next_label("logical_decisive");
        let done = self.next_label("logical_done");
        self.emit_expr_preserving(left, &bool_ty(), &[right])?;
        if op == BinaryOp::And {
            self.jump_storage_zero(self.r0, 1, &decisive);
        } else {
            self.jump_storage_nonzero(self.r0, 1, &decisive);
        }
        self.emit_expr(right, &bool_ty())?;
        self.line(&format!("    jmp near {done}"));
        self.line(&format!("{decisive}:"));
        self.load_constant(i64::from(op == BinaryOp::Or), 1);
        self.line(&format!("{done}:"));
        Ok(())
    }

    fn compare(&mut self, op: BinaryOp, width: u8, signed: bool) {
        let yes = self.next_label("compare_true");
        let no = self.next_label("compare_false");
        let done = self.next_label("compare_done");
        self.compare_branch(op, width, signed, &yes, &no);
        self.line(&format!("{yes}:"));
        self.load_constant(1, 1);
        self.line(&format!("    jmp near {done}"));
        self.line(&format!("{no}:"));
        self.load_constant(0, 1);
        self.line(&format!("{done}:"));
    }

    fn compare_branch(&mut self, op: BinaryOp, width: u8, signed: bool, yes: &str, no: &str) {
        if signed && !matches!(op, BinaryOp::Eq | BinaryOp::Ne) {
            let top = u32::from(width - 1);
            self.load_al(self.r0.address + top);
            self.line("    xor al,80h");
            self.store_al(self.r0.address + top);
            self.load_al(self.r1.address + top);
            self.line("    xor al,80h");
            self.store_al(self.r1.address + top);
        }
        match op {
            BinaryOp::Eq => {
                self.jump_equal(self.r0, self.r1, width, yes);
                self.line(&format!("    jmp near {no}"));
            }
            BinaryOp::Ne => {
                self.jump_equal(self.r0, self.r1, width, no);
                self.line(&format!("    jmp near {yes}"));
            }
            BinaryOp::Lt => {
                self.jump_less(self.r0, self.r1, width, yes);
                self.line(&format!("    jmp near {no}"));
            }
            BinaryOp::Le => {
                self.jump_less(self.r0, self.r1, width, yes);
                self.jump_equal(self.r0, self.r1, width, yes);
                self.line(&format!("    jmp near {no}"));
            }
            BinaryOp::Gt => {
                self.jump_less(self.r0, self.r1, width, no);
                self.jump_equal(self.r0, self.r1, width, no);
                self.line(&format!("    jmp near {yes}"));
            }
            BinaryOp::Ge => {
                self.jump_less(self.r0, self.r1, width, no);
                self.line(&format!("    jmp near {yes}"));
            }
            _ => unreachable!(),
        }
    }

    fn materialize_place(&mut self, place: &Place) -> Result<MaterializedPlace, Diagnostic> {
        if let Place::Ident(name) = place
            && matches!(self.binding(name)?.location, BindingLocation::Bp)
        {
            return Ok(MaterializedPlace::Bp);
        }
        match self.place_address(place)? {
            Address::Direct(address) => Ok(MaterializedPlace::Direct(address)),
            Address::Indirect => {
                let address = self.model.allocate(2)?;
                self.line(&format!("    mov {},bx", mem(address.address)));
                Ok(MaterializedPlace::Indirect(address))
            }
        }
    }

    fn load_materialized_place(&mut self, place: MaterializedPlace, width: u8) {
        match place {
            MaterializedPlace::Direct(address) => self.copy(
                Storage {
                    address,
                    size: u32::from(width),
                },
                self.r0,
                u32::from(width),
            ),
            MaterializedPlace::Indirect(address) => {
                self.load_bx(address.address);
                self.load_indirect(width);
            }
            MaterializedPlace::Bp => {
                self.line("    mov ax,bp");
                self.line(&format!("    mov {},ax", mem(self.r0.address)));
            }
        }
    }

    fn store_materialized_place(&mut self, place: MaterializedPlace, source: Storage, size: u32) {
        match place {
            MaterializedPlace::Direct(address) => {
                self.copy(source, Storage { address, size }, size)
            }
            MaterializedPlace::Indirect(address) => {
                self.load_bx(address.address);
                self.copy_storage_to_indirect(source, size);
            }
            MaterializedPlace::Bp => {
                debug_assert_eq!(size, 2);
                self.load_ax(source.address);
                self.line("    mov bp,ax");
            }
        }
    }

    fn place_address(&mut self, place: &Place) -> Result<Address, Diagnostic> {
        match place {
            Place::Ident(name) => {
                let binding = self.binding(name)?;
                Ok(Address::Direct(self.static_storage(&binding)?.address))
            }
            Place::Index { name, index } => {
                self.named_index_address(name, index)?;
                Ok(Address::Indirect)
            }
            Place::Field { base, field } => Ok(self.named_field_address(base, field)?.0),
            Place::Access(path) => {
                self.access_address(path)?;
                Ok(Address::Indirect)
            }
            Place::Deref(expr) => {
                let Type::Ptr(inner) = self.model.resolved_type(&self.expr_type(expr)?)? else {
                    return Err(Diagnostic::new("dereference requires pointer"));
                };
                self.emit_expr(expr, &Type::Ptr(inner))?;
                self.result_to_bx();
                Ok(Address::Indirect)
            }
        }
    }

    fn place_type(&self, place: &Place) -> Result<Type, Diagnostic> {
        match place {
            Place::Ident(name) => Ok(self.binding(name)?.ty),
            Place::Index { name, .. } => element_type(&self.indexed_root_type(name)?),
            Place::Field { base, field } => {
                let root = self.access_root(base)?;
                let root_ty = match root {
                    AccessRoot::Storage(binding) => binding.ty,
                    AccessRoot::Constant { ty, .. } => ty,
                };
                let resolved = self.model.resolved_type(&root_ty)?;
                let ty = match resolved {
                    Type::Ptr(inner) => *inner,
                    ty => ty,
                };
                Ok(self.model.field(&ty, field)?.ty.clone())
            }
            Place::Access(path) => self.access_type(path),
            Place::Deref(expr) => match self.model.resolved_type(&self.expr_type(expr)?)? {
                Type::Ptr(inner) => Ok(*inner),
                _ => Err(Diagnostic::new("dereference requires pointer")),
            },
        }
    }

    fn indexed_root_type(&self, name: &str) -> Result<Type, Diagnostic> {
        let root = self.access_root(name)?;
        let ty = match root {
            AccessRoot::Storage(binding) => binding.ty,
            AccessRoot::Constant { ty, .. } => ty,
        };
        self.model.resolved_type(&ty)
    }

    fn named_field_address(
        &mut self,
        base: &str,
        field_name: &str,
    ) -> Result<(Address, crate::tbir::model::FieldLayout), Diagnostic> {
        let root = self.access_root(base)?;
        match root {
            AccessRoot::Storage(binding) => {
                let resolved = self.model.resolved_type(&binding.ty)?;
                if let Type::Ptr(inner) = resolved {
                    let field = self.model.field(&inner, field_name)?.clone();
                    self.load_binding_into_bx(&binding);
                    if field.offset != 0 {
                        self.line(&format!("    add bx,{}", imm(field.offset)));
                    }
                    Ok((Address::Indirect, field))
                } else {
                    let field = self.model.field(&resolved, field_name)?.clone();
                    Ok((
                        Address::Direct(self.static_storage(&binding)?.address + field.offset),
                        field,
                    ))
                }
            }
            AccessRoot::Constant { address, ty } => {
                let Type::Ptr(inner) = self.model.resolved_type(&ty)? else {
                    return Err(Diagnostic::new(format!(
                        "field access root `{base}` must be storage or a pointer constant"
                    )));
                };
                let field = self.model.field(&inner, field_name)?.clone();
                self.line(&format!("    mov bx,{}", imm(address)));
                if field.offset != 0 {
                    self.line(&format!("    add bx,{}", imm(field.offset)));
                }
                Ok((Address::Indirect, field))
            }
        }
    }

    fn named_index_address(&mut self, name: &str, index: &Expr) -> Result<Type, Diagnostic> {
        let root = self.access_root(name)?;
        let declared_ty = match &root {
            AccessRoot::Storage(binding) => binding.ty.clone(),
            AccessRoot::Constant { ty, .. } => ty.clone(),
        };
        let resolved = self.model.resolved_type(&declared_ty)?;
        let element = element_type(&resolved)?;
        let size = self.model.type_size(&element)?;
        match (root, &resolved) {
            (AccessRoot::Storage(binding), Type::Array { len, .. }) => {
                self.validate_const_array_index(index, len, name)?;
                self.line(&format!(
                    "    mov bx,{}",
                    imm(self.static_storage(&binding)?.address)
                ));
            }
            (AccessRoot::Storage(binding), Type::Ptr(_)) => self.load_binding_into_bx(&binding),
            (AccessRoot::Constant { address, .. }, Type::Ptr(_)) => {
                self.line(&format!("    mov bx,{}", imm(address)));
            }
            _ => return Err(Diagnostic::new("indexing requires array or pointer")),
        }
        self.add_index(index, size)?;
        Ok(element)
    }

    fn access_address(&mut self, path: &AccessPath) -> Result<Type, Diagnostic> {
        let root = self.access_root(&path.root)?;
        let mut ty = match root {
            AccessRoot::Storage(binding) => {
                let resolved = self.model.resolved_type(&binding.ty)?;
                if let Type::Ptr(inner) = resolved {
                    self.load_binding_into_bx(&binding);
                    *inner
                } else {
                    self.line(&format!(
                        "    mov bx,{}",
                        imm(self.static_storage(&binding)?.address)
                    ));
                    resolved
                }
            }
            AccessRoot::Constant { address, ty } => {
                let Type::Ptr(inner) = self.model.resolved_type(&ty)? else {
                    return Err(Diagnostic::new(format!(
                        "access root `{}` must be storage or a pointer constant",
                        path.root
                    )));
                };
                self.line(&format!("    mov bx,{}", imm(address)));
                *inner
            }
        };
        for segment in &path.segments {
            match segment {
                AccessSegment::Field(name) => {
                    let field = self.model.field(&ty, name)?.clone();
                    if field.offset != 0 {
                        self.line(&format!("    add bx,{}", imm(field.offset)));
                    }
                    ty = field.ty;
                }
                AccessSegment::Index(index) => {
                    let resolved = self.model.resolved_type(&ty)?;
                    if let Type::Array { len, .. } = &resolved {
                        self.validate_const_array_index(index, len, &path.root)?;
                    }
                    let element = element_type(&resolved)?;
                    let size = self.model.type_size(&element)?;
                    self.add_index(index, size)?;
                    ty = element;
                }
            }
        }
        Ok(ty)
    }

    fn validate_const_array_index(
        &self,
        index: &Expr,
        len: &Expr,
        root: &str,
    ) -> Result<(), Diagnostic> {
        let Ok(index) = self.model.const_value(index) else {
            return Ok(());
        };
        let len = self.model.const_value(len)?;
        if index < 0 || index >= len {
            return Err(Diagnostic::new(format!(
                "array index {index} is out of bounds for `{root}` with length {len}"
            )));
        }
        Ok(())
    }

    fn add_index(&mut self, index: &Expr, scale: u32) -> Result<(), Diagnostic> {
        self.validate_index_type(index)?;
        if let Ok(index) = self.model.const_value(index) {
            let offset = u32::try_from(index)
                .ok()
                .and_then(|value| value.checked_mul(scale))
                .ok_or_else(|| Diagnostic::new("array index offset overflow"))?;
            if offset != 0 {
                self.line(&format!("    add bx,{}", imm(offset)));
            }
            return Ok(());
        }
        self.line("    push bx");
        self.emit_expr(index, &Type::Named("u16".to_owned()))?;
        self.line("    pop bx");
        self.load_ax(self.r0.address);
        if scale != 1 {
            self.line(&format!("    mov cx,{}", imm(scale)));
            self.line("    mul cx");
        }
        self.line("    add bx,ax");
        Ok(())
    }

    fn emit_pointer_add(
        &mut self,
        left: &Expr,
        right: &Expr,
        expected: &Type,
    ) -> Result<bool, Diagnostic> {
        let left_ty = self.model.resolved_type(&self.expr_type(left)?)?;
        let right_ty = self.model.resolved_type(&self.expr_type(right)?)?;
        let (pointer, index, pointer_ty, index_ty, pointer_on_left) = if let Type::Ptr(_) = left_ty
        {
            (left, right, left_ty, right_ty, true)
        } else if let Type::Ptr(_) = right_ty {
            (right, left, right_ty, left_ty, false)
        } else {
            return Ok(false);
        };
        let Type::Ptr(inner) = &pointer_ty else {
            return Ok(false);
        };
        let element_size = self.model.type_size(inner)?;
        if element_size > u32::from(u16::MAX) {
            return Err(Diagnostic::new(
                "pointer element size exceeds i8086 address arithmetic",
            ));
        }
        self.validate_index_type(index)?;

        if pointer_on_left {
            if let Some(base) = self.constant_pointer_address(pointer) {
                self.line(&format!("    mov bx,{}", imm(base)));
            } else {
                self.emit_expr_preserving(pointer, &pointer_ty, &[index])?;
                self.result_to_bx();
            }
            let index_width = self.scalar_width(&index_ty)?;
            self.line("    push bx");
            self.emit_expr(index, &index_ty)?;
            self.line("    pop bx");
            self.load_index_ax(self.r0, index_width);
        } else {
            let index_width = self.scalar_width(&index_ty)?;
            self.emit_expr(index, &index_ty)?;
            let saved_index = self.model.allocate(u32::from(index_width))?;
            self.copy(self.r0, saved_index, u32::from(index_width));
            if let Some(base) = self.constant_pointer_address(pointer) {
                self.line(&format!("    mov bx,{}", imm(base)));
            } else {
                self.emit_expr(pointer, &pointer_ty)?;
                self.result_to_bx();
            }
            self.load_index_ax(saved_index, index_width);
        }
        if element_size == 1 {
            self.line("    add bx,ax");
        } else {
            self.line(&format!("    mov cx,{}", imm(element_size)));
            self.line("    mul cx");
            self.line("    add bx,ax");
        }
        let _ = expected;
        self.bx_to_result(2);
        Ok(true)
    }

    fn load_index_ax(&mut self, storage: Storage, width: u8) {
        if width == 1 {
            self.load_al(storage.address);
            self.line("    xor ah,ah");
        } else {
            self.load_ax(storage.address);
        }
    }

    fn validate_index_type(&self, index: &Expr) -> Result<(), Diagnostic> {
        if let Ok(value) = self.model.const_value(index)
            && value < 0
        {
            return Err(Diagnostic::new(
                "array or pointer index must be non-negative",
            ));
        }
        let ty = self.model.resolved_type(&self.expr_type(index)?)?;
        if matches!(ty, Type::Named(ref name) if matches!(name.as_str(), "u8" | "u16")) {
            Ok(())
        } else {
            Err(Diagnostic::new(
                "array or pointer index must have type `u8` or `u16`",
            ))
        }
    }

    fn load_direct_memory(&mut self, address: u32, width: u8) {
        let words = u32::from(width) / 2;
        for word in 0..words {
            let offset = word * 2;
            self.load_ax(address + offset);
            self.line(&format!("    mov {},ax", mem(self.r0.address + offset)));
        }
        if !u32::from(width).is_multiple_of(2) {
            self.load_al(address + u32::from(width) - 1);
            self.store_al(self.r0.address + u32::from(width) - 1);
        }
    }

    fn load_indirect(&mut self, width: u8) {
        for offset in 0..u32::from(width) {
            self.line(&format!("    mov al,{}", indexed_bx(offset)));
            self.store_al(self.r0.address + offset);
        }
    }

    fn copy_indirect_to_storage(&mut self, target: Storage, size: u32) {
        for offset in 0..size {
            self.line(&format!("    mov al,{}", indexed_bx(offset)));
            self.store_al(target.address + offset);
        }
    }

    fn copy_storage_to_indirect(&mut self, source: Storage, size: u32) {
        for offset in 0..size {
            self.load_al(source.address + offset);
            self.line(&format!("    mov {},al", indexed_bx(offset)));
        }
    }

    fn comparison_operand_type(
        &self,
        left: &Expr,
        op: BinaryOp,
        right: &Expr,
        expected: &Type,
    ) -> Result<Type, Diagnostic> {
        if is_untyped_integer_expr(left) && is_untyped_integer_expr(right) {
            let left = self.model.const_value(left)?;
            let right = self.model.const_value(right)?;
            return Ok(common_literal_type(left, right));
        }

        let left_ty = self.model.resolved_type(&self.expr_type(left)?)?;
        let right_ty = self.model.resolved_type(&self.expr_type(right)?)?;
        if is_untyped_integer_expr(left) {
            self.validate_literal_for_type(self.model.const_value(left)?, &right_ty)?;
            return Ok(right_ty);
        }
        if is_untyped_integer_expr(right) {
            self.validate_literal_for_type(self.model.const_value(right)?, &left_ty)?;
            return Ok(left_ty);
        }

        if matches!(left_ty, Type::Array { .. }) || matches!(right_ty, Type::Array { .. }) {
            return Err(Diagnostic::new("array value cannot be used as a scalar"));
        }
        if left_ty == bool_ty() || right_ty == bool_ty() {
            if !matches!(op, BinaryOp::Eq | BinaryOp::Ne) || left_ty != right_ty {
                return Err(Diagnostic::new("type mismatch in comparison"));
            }
            return Ok(bool_ty());
        }
        if matches!(left_ty, Type::Ptr(_)) || matches!(right_ty, Type::Ptr(_)) {
            if !matches!(op, BinaryOp::Eq | BinaryOp::Ne) {
                return Err(Diagnostic::new(
                    "pointer comparisons support only == and !=",
                ));
            }
            if left_ty != right_ty {
                return Err(Diagnostic::new("type mismatch in pointer comparison"));
            }
            return Ok(left_ty);
        }
        if self.type_is_signed(&left_ty)? != self.type_is_signed(&right_ty)? {
            return Err(Diagnostic::new("signed/unsigned mix without cast"));
        }
        if self.scalar_width(&left_ty)? != self.scalar_width(&right_ty)? {
            return Err(Diagnostic::new(
                "comparison operands must have same width without cast",
            ));
        }
        let _ = expected;
        Ok(left_ty)
    }

    fn validate_literal_for_type(&self, value: i64, ty: &Type) -> Result<(), Diagnostic> {
        let resolved = self.model.resolved_type(ty)?;
        let valid = match resolved {
            Type::Named(ref name) => match name.as_str() {
                "u8" => (0..=u8::MAX as i64).contains(&value),
                "i8" => (i8::MIN as i64..=i8::MAX as i64).contains(&value),
                "u16" => (0..=u16::MAX as i64).contains(&value),
                "i16" => (i16::MIN as i64..=i16::MAX as i64).contains(&value),
                "u24" | "ptr" => (0..=0xFF_FFFF).contains(&value),
                "i24" => (-0x80_0000..=0x7F_FFFF).contains(&value),
                "u32" => (0..=u32::MAX as i64).contains(&value),
                "i32" => (i32::MIN as i64..=i32::MAX as i64).contains(&value),
                _ => false,
            },
            _ => false,
        };
        if valid {
            Ok(())
        } else {
            Err(Diagnostic::new(format!(
                "literal {value} is outside type `{}`",
                type_display(ty)
            )))
        }
    }

    fn type_is_signed(&self, ty: &Type) -> Result<bool, Diagnostic> {
        Ok(matches!(
            self.model.resolved_type(ty)?,
            Type::Named(name) if matches!(name.as_str(), "i8" | "i16" | "i24" | "i32")
        ))
    }

    fn expr_type(&self, expr: &Expr) -> Result<Type, Diagnostic> {
        match expr {
            Expr::Int(value) => Ok(integer_value_type(*value)),
            Expr::TypedInt(_, ty) | Expr::Cast { ty, .. } => Ok(ty.clone()),
            Expr::Bool(_) => Ok(bool_ty()),
            Expr::Char(_) | Expr::In(_) => Ok(Type::Named("u8".to_owned())),
            Expr::String(_) => Ok(byte_ptr()),
            Expr::Ident(name) => self
                .model
                .constant_types
                .get(name)
                .cloned()
                .or_else(|| self.binding(name).ok().map(|binding| binding.ty))
                .ok_or_else(|| Diagnostic::new(format!("unknown value `{name}`"))),
            Expr::Index { name, .. } => element_type(&self.indexed_root_type(name)?),
            Expr::Field { base, field } => {
                let constant = format!("{base}.{field}");
                self.model
                    .constant_types
                    .get(&constant)
                    .cloned()
                    .map(Ok)
                    .unwrap_or_else(|| {
                        let root = self.access_root(base)?;
                        let root_ty = match root {
                            AccessRoot::Storage(binding) => binding.ty,
                            AccessRoot::Constant { ty, .. } => ty,
                        };
                        let resolved = self.model.resolved_type(&root_ty)?;
                        let ty = match resolved {
                            Type::Ptr(inner) => *inner,
                            ty => ty,
                        };
                        Ok(self.model.field(&ty, field)?.ty.clone())
                    })
            }
            Expr::AddressOfIndex { name, .. } => Ok(Type::Ptr(Box::new(element_type(
                &self.indexed_root_type(name)?,
            )?))),
            Expr::AddressOfField { base, field } => {
                let root = self.access_root(base)?;
                let root_ty = match root {
                    AccessRoot::Storage(binding) => binding.ty,
                    AccessRoot::Constant { ty, .. } => ty,
                };
                let resolved = self.model.resolved_type(&root_ty)?;
                let ty = match resolved {
                    Type::Ptr(inner) => *inner,
                    ty => ty,
                };
                Ok(Type::Ptr(Box::new(
                    self.model.field(&ty, field)?.ty.clone(),
                )))
            }
            Expr::Access(path) => self.access_type(path),
            Expr::AddressOfAccess(path) => Ok(Type::Ptr(Box::new(self.access_type(path)?))),
            Expr::AddressOf(name) => {
                if let Some(function_ty) = self.function_value_type(name) {
                    Ok(Type::Ptr(Box::new(function_ty)))
                } else if self
                    .model
                    .functions
                    .get(name)
                    .is_some_and(|signature| signature.second_return_type.is_some())
                {
                    Err(Diagnostic::new(format!(
                        "8086 function pointer cannot reference two-result function `{name}`"
                    )))
                } else {
                    Ok(Type::Ptr(Box::new(self.binding(name)?.ty)))
                }
            }
            Expr::StructInit { ty, .. } => Ok(Type::Named(ty.clone())),
            Expr::BankedPointer { pointer, .. } => self.expr_type(pointer),
            Expr::Deref(expr) => match self.model.resolved_type(&self.expr_type(expr)?)? {
                Type::Ptr(inner) => Ok(*inner),
                _ => Err(Diagnostic::new("dereference requires pointer")),
            },
            Expr::Call { path, args } => {
                let name = path.join(".");
                if let Some(descriptor) = CATALOG.lookup(&name) {
                    let argument_types = args
                        .iter()
                        .map(|arg| self.model.resolved_type(&self.expr_type(arg)?))
                        .collect::<Result<Vec<_>, _>>()?;
                    let result = CATALOG
                        .infer_result_types(descriptor.canonical_name, &argument_types)
                        .map_err(|error| Diagnostic::new(error.to_string()))?;
                    return result.into_iter().next().ok_or_else(|| {
                        Diagnostic::new(format!("intrinsic `{name}` has no scalar result"))
                    });
                }
                if let Some(name) = resolve_function(path, &self.model) {
                    return self.model.functions[&name]
                        .return_type
                        .clone()
                        .ok_or_else(|| Diagnostic::new("void function has no value"));
                }
                if path.len() == 1
                    && let Type::Ptr(inner) =
                        self.model.resolved_type(&self.binding(&path[0])?.ty)?
                    && let Type::Function { return_type, .. } = *inner
                {
                    return return_type
                        .map(|ty| *ty)
                        .ok_or_else(|| Diagnostic::new("void function has no value"));
                }
                Err(Diagnostic::new(format!(
                    "unknown function `{}`",
                    path.join(".")
                )))
            }
            Expr::Unary {
                op: UnaryOp::Not, ..
            } => Ok(bool_ty()),
            Expr::Unary { expr, .. } => self.expr_type(expr),
            Expr::Binary { op, .. }
                if is_comparison(*op) || matches!(op, BinaryOp::And | BinaryOp::Or) =>
            {
                Ok(bool_ty())
            }
            Expr::Binary { left, .. } => self.expr_type(left),
            Expr::Array(_) => Err(Diagnostic::new("array type requires context")),
        }
    }

    fn access_type(&self, path: &AccessPath) -> Result<Type, Diagnostic> {
        let root = self.access_root(&path.root)?;
        let root_ty = match root {
            AccessRoot::Storage(binding) => binding.ty,
            AccessRoot::Constant { ty, .. } => ty,
        };
        let mut ty = self.model.resolved_type(&root_ty)?;
        if let Type::Ptr(inner) = ty {
            ty = *inner;
        }
        for segment in &path.segments {
            ty = match segment {
                AccessSegment::Field(name) => self.model.field(&ty, name)?.ty.clone(),
                AccessSegment::Index(index) => {
                    let resolved = self.model.resolved_type(&ty)?;
                    if let Type::Array { len, .. } = &resolved {
                        self.validate_const_array_index(index, len, &path.root)?;
                    }
                    element_type(&resolved)?
                }
            };
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
        let register_inputs = inputs
            .iter()
            .filter(|input| matches!(input.class.as_str(), "reg8" | "reg16"))
            .count();
        let register_outputs = outputs
            .iter()
            .filter(|output| matches!(output.class.as_str(), "reg8" | "reg16"))
            .count();
        if register_inputs > 1 || register_outputs > 1 {
            return Err(Diagnostic::new(
                "i8086 inline asm supports at most one fixed AX-family register input and output",
            ));
        }

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
            self.validate_inline_asm_input(input)?;
            let binding = self.inline_asm_input_binding(input)?;
            operands.insert(input.name.clone(), binding);
        }
        for output in outputs {
            if operands.contains_key(&output.name) {
                return Err(Diagnostic::new(format!(
                    "duplicate inline asm operand `{}`",
                    output.name
                )));
            }
            self.validate_inline_asm_output(output)?;
            let binding = self.inline_asm_output_binding(output)?;
            operands.insert(output.name.clone(), binding);
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
        let substituted = lines
            .iter()
            .map(|line| substitute_inline_asm_operands(line, &operands))
            .collect::<Result<Vec<_>, _>>()?;

        for input in inputs {
            self.load_inline_asm_input(input)?;
        }
        for line in substituted {
            self.line(&format!("    {line}"));
        }
        for output in outputs {
            self.store_inline_asm_output(output)?;
        }
        Ok(())
    }

    fn validate_inline_asm_input(&self, input: &crate::ast::AsmInput) -> Result<(), Diagnostic> {
        let bound = self.named_value_type(&input.name)?;
        if self.model.resolved_type(&input.ty)? != self.model.resolved_type(&bound)? {
            return Err(Diagnostic::new(format!(
                "inline asm input `{}` declared type `{}` does not match bound type `{}`",
                input.name,
                type_display(&input.ty),
                type_display(&bound)
            )));
        }
        self.validate_inline_asm_class(&input.name, &input.ty, &input.class, false)
    }

    fn validate_inline_asm_output(&self, output: &crate::ast::AsmOutput) -> Result<(), Diagnostic> {
        let bound = self.binding(&output.name)?.ty;
        if self.model.resolved_type(&output.ty)? != self.model.resolved_type(&bound)? {
            return Err(Diagnostic::new(format!(
                "inline asm output `{}` declared type `{}` does not match bound type `{}`",
                output.name,
                type_display(&output.ty),
                type_display(&bound)
            )));
        }
        self.validate_inline_asm_class(&output.name, &output.ty, &output.class, true)
    }

    fn validate_inline_asm_class(
        &self,
        name: &str,
        ty: &Type,
        class: &str,
        output: bool,
    ) -> Result<(), Diagnostic> {
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
                "inline asm operand class `reg24` is not supported on i8086",
            ))
        } else if class == "imm" {
            Err(Diagnostic::new(format!(
                "inline asm immediate `{name}` must be a compile-time constant"
            )))
        } else {
            Err(Diagnostic::new(format!(
                "inline asm operand `{name}` has incompatible or unsupported class `{class}`"
            )))
        }
    }

    fn inline_asm_input_binding(&self, input: &crate::ast::AsmInput) -> Result<String, Diagnostic> {
        match input.class.as_str() {
            "reg8" => Ok("al".to_owned()),
            "reg16" => Ok("ax".to_owned()),
            "mem" => {
                let binding = self.binding(&input.name)?;
                Ok(mem(self.static_storage(&binding)?.address))
            }
            "imm" => {
                let width = self.model.type_width(&input.ty)?;
                let value = self.model.constants[&input.name];
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
            "reg8" => Ok("al".to_owned()),
            "reg16" => Ok("ax".to_owned()),
            "mem" => {
                let binding = self.binding(&output.name)?;
                Ok(mem(self.static_storage(&binding)?.address))
            }
            _ => Err(Diagnostic::new(format!(
                "unsupported inline asm output class `{}`",
                output.class
            ))),
        }
    }

    fn load_inline_asm_input(&mut self, input: &crate::ast::AsmInput) -> Result<(), Diagnostic> {
        match input.class.as_str() {
            "reg8" => {
                if let Ok(binding) = self.binding(&input.name) {
                    let storage = self.static_storage(&binding)?;
                    self.load_al(storage.address);
                } else {
                    self.line(&format!(
                        "    mov al,{}",
                        format_immediate(self.model.constants[&input.name], 1)
                    ));
                }
            }
            "reg16" => {
                if let Ok(binding) = self.binding(&input.name) {
                    match binding.location {
                        BindingLocation::Static(storage) => self.load_ax(storage.address),
                        BindingLocation::Bp => self.line("    mov ax,bp"),
                    }
                } else {
                    self.line(&format!(
                        "    mov ax,{}",
                        format_immediate(self.model.constants[&input.name], 2)
                    ));
                }
            }
            "mem" | "imm" => {}
            _ => unreachable!("validated inline asm input class"),
        }
        Ok(())
    }

    fn store_inline_asm_output(
        &mut self,
        output: &crate::ast::AsmOutput,
    ) -> Result<(), Diagnostic> {
        let binding = self.binding(&output.name)?;
        match output.class.as_str() {
            "reg8" => {
                let storage = self.static_storage(&binding)?;
                self.store_al(storage.address);
            }
            "reg16" => match binding.location {
                BindingLocation::Static(storage) => {
                    self.line(&format!("    mov {},ax", mem(storage.address)))
                }
                BindingLocation::Bp => self.line("    mov bp,ax"),
            },
            "mem" => {}
            _ => unreachable!("validated inline asm output class"),
        }
        Ok(())
    }

    fn validate_inline_asm_clobbers(&self, clobbers: &[String]) -> Result<(), Diagnostic> {
        let mut seen = HashSet::new();
        for clobber in clobbers {
            let canonical = canonical_i8086_clobber(clobber);
            if !seen.insert(canonical) {
                return Err(Diagnostic::new(format!(
                    "duplicate or overlapping inline asm clobber `{clobber}`"
                )));
            }
            if matches!(clobber.as_str(), "sp" | "ip" | "cs" | "ds" | "es" | "ss") {
                let context = if self.function_states.last().is_some_and(|state| state.naked) {
                    "naked 8086 functions"
                } else {
                    "the 8086 generated-code ABI"
                };
                return Err(Diagnostic::new(format!(
                    "inline asm cannot clobber ABI-critical register `{clobber}` in {context}"
                )));
            }
            if !matches!(
                clobber.as_str(),
                "al" | "ah"
                    | "ax"
                    | "bl"
                    | "bh"
                    | "bx"
                    | "cl"
                    | "ch"
                    | "cx"
                    | "dl"
                    | "dh"
                    | "dx"
                    | "si"
                    | "di"
                    | "bp"
                    | "flags"
                    | "memory"
                    | "ports"
            ) {
                return Err(Diagnostic::new(format!(
                    "unsupported i8086 inline asm clobber `{clobber}`"
                )));
            }
        }
        Ok(())
    }

    fn scalar_width(&self, ty: &Type) -> Result<u8, Diagnostic> {
        let width = self.model.type_width(ty)?;
        if u32::from(width) > SCRATCH_BYTES {
            Err(Diagnostic::new(
                "scalar is too wide for i8086 emitter scratch ABI",
            ))
        } else {
            Ok(width)
        }
    }

    fn bind(
        &mut self,
        name: String,
        location: BindingLocation,
        ty: Type,
    ) -> Result<(), Diagnostic> {
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
            .insert(name, Binding { location, ty });
        Ok(())
    }

    fn constant_pointer_address(&self, expr: &Expr) -> Option<u32> {
        if let Ok(value) = self.model.const_value(expr) {
            return u32::try_from(value).ok();
        }
        if let Expr::Ident(name) = expr {
            return self.model.mmio.get(name).map(|(address, _, _)| *address);
        }
        None
    }

    fn access_root(&self, name: &str) -> Result<AccessRoot, Diagnostic> {
        if let Ok(binding) = self.binding(name) {
            return Ok(AccessRoot::Storage(binding));
        }
        if let (Some(value), Some(ty)) = (
            self.model.constants.get(name),
            self.model.constant_types.get(name),
        ) {
            let address = u32::try_from(*value).map_err(|_| {
                Diagnostic::new(format!(
                    "pointer constant `{name}` is outside address space"
                ))
            })?;
            return Ok(AccessRoot::Constant {
                address,
                ty: ty.clone(),
            });
        }
        Err(Diagnostic::new(format!("unknown access root `{name}`")))
    }

    fn named_value_type(&self, name: &str) -> Result<Type, Diagnostic> {
        self.model
            .constant_types
            .get(name)
            .cloned()
            .or_else(|| self.binding(name).ok().map(|binding| binding.ty))
            .ok_or_else(|| Diagnostic::new(format!("unknown value `{name}`")))
    }

    fn binding(&self, name: &str) -> Result<Binding, Diagnostic> {
        if let Some(binding) = self.scopes.iter().rev().find_map(|scope| scope.get(name)) {
            return Ok(binding.clone());
        }
        if let Some(storage) = self.model.globals.get(name) {
            return Ok(Binding {
                location: BindingLocation::Static(*storage),
                ty: self.model.global_types[name].clone(),
            });
        }
        Err(Diagnostic::new(format!("unknown variable `{name}`")))
    }

    fn port(&self, name: &str) -> Result<u8, Diagnostic> {
        self.ports
            .get(name)
            .copied()
            .ok_or_else(|| Diagnostic::new(format!("unknown port `{name}`")))
    }

    fn static_storage(&self, binding: &Binding) -> Result<Storage, Diagnostic> {
        match binding.location {
            BindingLocation::Static(storage) => Ok(storage),
            BindingLocation::Bp => Err(Diagnostic::new(
                "BP local unexpectedly requires addressable storage",
            )),
        }
    }

    fn copy_binding_to_storage(&mut self, binding: &Binding, target: Storage, size: u32) {
        match binding.location {
            BindingLocation::Static(source) => self.copy(source, target, size),
            BindingLocation::Bp => {
                debug_assert_eq!(size, 2);
                self.line("    mov ax,bp");
                self.line(&format!("    mov {},ax", mem(target.address)));
            }
        }
    }

    fn load_binding_into_bx(&mut self, binding: &Binding) {
        match binding.location {
            BindingLocation::Static(storage) => self.load_bx(storage.address),
            BindingLocation::Bp => self.line("    mov bx,bp"),
        }
    }

    fn copy(&mut self, source: Storage, target: Storage, size: u32) {
        let words = size / 2;
        for word in 0..words {
            let offset = word * 2;
            self.load_ax(source.address + offset);
            self.line(&format!("    mov {},ax", mem(target.address + offset)));
        }
        if !size.is_multiple_of(2) {
            self.load_al(source.address + size - 1);
            self.store_al(target.address + size - 1);
        }
    }

    fn zero(&mut self, storage: Storage) {
        self.line("    xor ax,ax");
        let words = storage.size / 2;
        for word in 0..words {
            self.line(&format!("    mov {},ax", mem(storage.address + word * 2)));
        }
        if !storage.size.is_multiple_of(2) {
            self.store_al(storage.address + storage.size - 1);
        }
    }

    fn load_constant(&mut self, value: i64, width: u8) {
        self.load_constant_into(self.r0, value, width);
    }

    fn load_constant_into(&mut self, target: Storage, value: i64, width: u8) {
        let value = value as u64;
        for offset in (0..u32::from(width)).step_by(2) {
            let bytes = u32::from(width) - offset;
            if bytes >= 2 {
                self.line(&format!(
                    "    mov ax,{}",
                    imm(((value >> (offset * 8)) & 0xffff) as u32)
                ));
                self.line(&format!("    mov {},ax", mem(target.address + offset)));
            } else {
                self.line(&format!(
                    "    mov al,{}",
                    imm(((value >> (offset * 8)) & 0xff) as u32)
                ));
                self.store_al(target.address + offset);
            }
        }
    }

    fn extend_result(&mut self, source: u8, target: u8, signed: bool) {
        if target <= source {
            return;
        }
        if signed {
            self.load_al(self.r0.address + u32::from(source - 1));
            self.line("    sar al,1");
            self.line("    sar al,1");
            self.line("    sar al,1");
            self.line("    sar al,1");
            self.line("    sar al,1");
            self.line("    sar al,1");
            self.line("    sar al,1");
        } else {
            self.line("    xor al,al");
        }
        for offset in u32::from(source)..u32::from(target) {
            self.store_al(self.r0.address + offset);
        }
    }

    fn scale(&mut self, storage: Storage, width: u8, amount: u32) -> Result<(), Diagnostic> {
        if amount <= 1 {
            return Ok(());
        }
        let source = self.model.allocate(u32::from(width))?;
        let result = self.model.allocate(u32::from(width))?;
        self.copy(storage, source, u32::from(width));
        self.zero(result);
        for _ in 0..amount {
            self.copy(result, self.r0, u32::from(width));
            self.copy(source, self.r1, u32::from(width));
            self.add(width);
            self.copy(self.r0, result, u32::from(width));
        }
        self.copy(result, storage, u32::from(width));
        Ok(())
    }

    fn negate(&mut self, storage: Storage, width: u8) {
        if width == 4 {
            let low_nonzero = self.next_label("negate_low_nonzero");
            let done = self.next_label("negate_done");
            self.load_ax(storage.address);
            self.line("    neg ax");
            self.line(&format!("    mov {},ax", mem(storage.address)));
            self.load_ax(storage.address + 2);
            self.branch_long("jnz", &low_nonzero);
            self.line("    neg ax");
            self.line(&format!("    jmp near {done}"));
            self.line(&format!("{low_nonzero}:"));
            self.line("    not ax");
            self.line(&format!("{done}:"));
            self.line(&format!("    mov {},ax", mem(storage.address + 2)));
            return;
        }
        for offset in 0..u32::from(width) {
            self.load_al(storage.address + offset);
            self.line("    not al");
            self.store_al(storage.address + offset);
        }
        self.line("    stc");
        for offset in 0..u32::from(width) {
            self.load_al(storage.address + offset);
            self.line("    adc al,0");
            self.store_al(storage.address + offset);
        }
    }

    fn normalize_signed(&mut self, storage: Storage, width: u8, flag: Storage, toggle: bool) {
        let positive = self.next_label("signed_positive");
        self.load_al(storage.address + u32::from(width - 1));
        self.line("    test al,80h");
        self.branch_long("jz", &positive);
        if toggle {
            self.toggle(flag);
        } else {
            self.store_immediate(flag.address, 1);
        }
        self.negate(storage, width);
        self.line(&format!("{positive}:"));
    }

    fn negate_if(&mut self, flag: Storage, storage: Storage, width: u8) {
        let done = self.next_label("sign_done");
        self.jump_storage_zero(flag, 1, &done);
        self.negate(storage, width);
        self.line(&format!("{done}:"));
    }

    fn toggle(&mut self, storage: Storage) {
        self.load_al(storage.address);
        self.line("    xor al,1");
        self.store_al(storage.address);
    }

    fn jump_storage_zero(&mut self, storage: Storage, width: u8, target: &str) {
        let nonzero = self.next_label("nonzero");
        if width == 4 {
            self.load_ax(storage.address);
            self.line(&format!("    or ax,{}", mem(storage.address + 2)));
            self.branch_long("jne", &nonzero);
        } else {
            for offset in 0..u32::from(width) {
                self.load_al(storage.address + offset);
                self.line("    or al,al");
                self.branch_long("jne", &nonzero);
            }
        }
        self.line(&format!("    jmp near {target}"));
        self.line(&format!("{nonzero}:"));
    }

    fn jump_storage_nonzero(&mut self, storage: Storage, width: u8, target: &str) {
        if width == 4 {
            self.load_ax(storage.address);
            self.line(&format!("    or ax,{}", mem(storage.address + 2)));
            self.branch_long("jne", target);
        } else {
            for offset in 0..u32::from(width) {
                self.load_al(storage.address + offset);
                self.line("    or al,al");
                self.branch_long("jne", target);
            }
        }
    }

    fn jump_equal(&mut self, left: Storage, right: Storage, width: u8, target: &str) {
        let different = self.next_label("different");
        if width == 4 {
            for offset in [0, 2] {
                self.load_ax(left.address + offset);
                self.line(&format!("    cmp ax,{}", mem(right.address + offset)));
                self.branch_long("jne", &different);
            }
        } else {
            for offset in 0..u32::from(width) {
                self.load_al(left.address + offset);
                self.line(&format!("    cmp al,{}", mem(right.address + offset)));
                self.branch_long("jne", &different);
            }
        }
        self.line(&format!("    jmp near {target}"));
        self.line(&format!("{different}:"));
    }

    fn jump_less(&mut self, left: Storage, right: Storage, width: u8, target: &str) {
        let done = self.next_label("ordered");
        if width == 4 {
            for offset in [2, 0] {
                self.load_ax(left.address + offset);
                self.line(&format!("    cmp ax,{}", mem(right.address + offset)));
                self.branch_long("jb", target);
                self.branch_long("jne", &done);
            }
        } else {
            for offset in (0..u32::from(width)).rev() {
                self.load_al(left.address + offset);
                self.line(&format!("    cmp al,{}", mem(right.address + offset)));
                self.branch_long("jb", target);
                self.branch_long("jne", &done);
            }
        }
        self.line(&format!("{done}:"));
    }

    fn branch_long(&mut self, condition: &str, target: &str) {
        let skip = self.next_label("branch_skip");
        let inverse = match condition {
            "jz" => "jnz",
            "jnz" => "jz",
            "je" => "jne",
            "jne" => "je",
            "jb" => "jae",
            "jbe" => "ja",
            "ja" => "jbe",
            "jae" => "jb",
            "jc" => "jnc",
            "jnc" => "jc",
            "js" => "jns",
            "jns" => "js",
            _ => unreachable!("unsupported 8086 condition"),
        };
        self.line(&format!("    {inverse} short {skip}"));
        self.line(&format!("    jmp near {target}"));
        self.line(&format!("{skip}:"));
    }

    fn live_storage_segments(
        &self,
        live: Storage,
        args: &[Expr],
        live_after: &HashSet<String>,
        save_all: bool,
        protected: &[Storage],
    ) -> Vec<Storage> {
        let mut ranges = if save_all {
            vec![(live.address, live.address.saturating_add(live.size))]
        } else {
            self.scopes
                .iter()
                .flat_map(|scope| scope.iter())
                .filter(|(name, _)| live_after.contains(*name))
                .filter_map(|(_, binding)| {
                    let BindingLocation::Static(storage) = binding.location else {
                        return None;
                    };
                    let start = storage.address.max(live.address);
                    let end = storage
                        .address
                        .saturating_add(storage.size)
                        .min(live.address.saturating_add(live.size));
                    (start < end).then_some((start, end))
                })
                .collect::<Vec<_>>()
        };
        if ranges.is_empty() {
            return Vec::new();
        }
        ranges.sort_unstable();
        ranges = merge_ranges(ranges);

        let mut excluded = args
            .iter()
            .filter_map(|arg| match arg {
                // The callee can mutate this storage through its pointer parameter.
                // Restoring a pre-call snapshot would discard that mutation.
                Expr::AddressOf(name) => self.binding(name).ok().and_then(|binding| match binding
                    .location
                {
                    BindingLocation::Static(storage) => Some(storage),
                    BindingLocation::Bp => None,
                }),
                _ => None,
            })
            .filter_map(|storage| {
                let start = storage.address.max(live.address);
                let end = storage
                    .address
                    .saturating_add(storage.size)
                    .min(live.address.saturating_add(live.size));
                (start < end).then_some((start, end))
            })
            .collect::<Vec<_>>();
        excluded.extend(protected.iter().copied().filter_map(|storage| {
            let start = storage.address.max(live.address);
            let end = storage
                .address
                .saturating_add(storage.size)
                .min(live.address.saturating_add(live.size));
            (start < end).then_some((start, end))
        }));
        excluded.sort_unstable();
        excluded = merge_ranges(excluded);

        let mut saved = Vec::new();
        for (range_start, range_end) in ranges {
            let mut cursor = range_start;
            for (start, end) in &excluded {
                if *end <= cursor || *start >= range_end {
                    continue;
                }
                if *start > cursor {
                    saved.push(Storage {
                        address: cursor,
                        size: *start - cursor,
                    });
                }
                cursor = cursor.max(*end);
                if cursor >= range_end {
                    break;
                }
            }
            if cursor < range_end {
                saved.push(Storage {
                    address: cursor,
                    size: range_end - cursor,
                });
            }
        }
        saved
    }

    fn push_bytes(&mut self, storage: Storage) {
        let words = storage.size / 2;
        for word in 0..words {
            self.load_ax(storage.address + word * 2);
            self.line("    push ax");
        }
        if !storage.size.is_multiple_of(2) {
            self.load_al(storage.address + storage.size - 1);
            self.line("    xor ah,ah");
            self.line("    push ax");
        }
    }

    fn pop_bytes(&mut self, storage: Storage) {
        if !storage.size.is_multiple_of(2) {
            self.line("    pop ax");
            self.store_al(storage.address + storage.size - 1);
        }
        for word in (0..storage.size / 2).rev() {
            self.line("    pop ax");
            self.line(&format!("    mov {},ax", mem(storage.address + word * 2)));
        }
    }

    fn result_to_bx(&mut self) {
        self.load_bx(self.r0.address);
    }
    fn bx_to_result(&mut self, width: u8) {
        self.line(&format!("    mov {},bx", mem(self.r0.address)));
        if width > 2 {
            self.line("    xor al,al");
            for offset in 2..u32::from(width) {
                self.store_al(self.r0.address + offset);
            }
        }
    }
    fn load_al(&mut self, address: u32) {
        self.line(&format!("    mov al,{}", mem(address)));
    }
    fn store_al(&mut self, address: u32) {
        self.line(&format!("    mov {},al", mem(address)));
    }
    fn store_immediate(&mut self, address: u32, value: u8) {
        self.line(&format!("    mov al,{}", imm(u32::from(value))));
        self.store_al(address);
    }
    fn load_ax(&mut self, address: u32) {
        self.line(&format!("    mov ax,{}", mem(address)));
    }
    fn load_bx(&mut self, address: u32) {
        self.line(&format!("    mov bx,{}", mem(address)));
    }
    fn load_cx(&mut self, address: u32) {
        self.line(&format!("    mov cx,{}", mem(address)));
    }
    fn load_si(&mut self, address: u32) {
        self.line(&format!("    mov si,{}", mem(address)));
    }
    fn load_di(&mut self, address: u32) {
        self.line(&format!("    mov di,{}", mem(address)));
    }
    fn load_cl(&mut self, address: u32) {
        self.line(&format!("    mov cl,{}", mem(address)));
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

const I8086_WORD_CLASS: RegClass = RegClass(0);
const I8086_BYTE_CLASS: RegClass = RegClass(1);
const I8086_STATIC_SPILL_CLASS: SpillClassId = SpillClassId(0);
const I8086_BP_REGISTER: PhysReg = PhysReg(14);

fn i8086_local_target() -> Target {
    Target {
        units: [
            "al", "ah", "bl", "bh", "cl", "ch", "dl", "dh", "si", "di", "bp",
        ]
        .into_iter()
        .map(RegisterUnit::new)
        .collect(),
        registers: vec![
            PhysicalRegister::new("ax", vec![RegUnit(0), RegUnit(1)]),
            PhysicalRegister::new("al", vec![RegUnit(0)]),
            PhysicalRegister::new("ah", vec![RegUnit(1)]),
            PhysicalRegister::new("bx", vec![RegUnit(2), RegUnit(3)]),
            PhysicalRegister::new("bl", vec![RegUnit(2)]),
            PhysicalRegister::new("bh", vec![RegUnit(3)]),
            PhysicalRegister::new("cx", vec![RegUnit(4), RegUnit(5)]),
            PhysicalRegister::new("cl", vec![RegUnit(4)]),
            PhysicalRegister::new("ch", vec![RegUnit(5)]),
            PhysicalRegister::new("dx", vec![RegUnit(6), RegUnit(7)]),
            PhysicalRegister::new("dl", vec![RegUnit(6)]),
            PhysicalRegister::new("dh", vec![RegUnit(7)]),
            PhysicalRegister::new("si", vec![RegUnit(8)]),
            PhysicalRegister::new("di", vec![RegUnit(9)]),
            PhysicalRegister::new("bp", vec![RegUnit(10)]),
        ],
        register_classes: vec![
            RegisterClass::new("local-word", vec![I8086_BP_REGISTER]),
            RegisterClass::new("byte", vec![]),
        ],
        spill_classes: vec![SpillClass::new("static", None, 1).with_base_alignment(1)],
    }
}

fn plan_function_locals(
    function: &Function,
    model: &SemanticModel,
) -> Result<FunctionLocals, Diagnostic> {
    let mut source_locals = Vec::new();
    let mut local_types = HashMap::new();
    collect_i8086_locals(&function.body, model, &mut source_locals, &mut local_types)?;
    let allocation = allocate_source_locals(
        &i8086_local_target(),
        &source_locals,
        &function.body,
        &[I8086_BP_REGISTER],
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

    let spill_sizes = allocation
        .allocation
        .spill_slots
        .iter()
        .map(|slot| {
            debug_assert_eq!(slot.class, I8086_STATIC_SPILL_CLASS);
            slot.size
        })
        .collect();
    let mut bindings = HashMap::new();
    for (name, ty) in local_types {
        let vreg = allocation
            .locals
            .vreg(&name)
            .ok_or_else(|| Diagnostic::new(format!("missing allocation for local `{name}`")))?;
        let location = match allocation.allocation.location(vreg) {
            Some(Location::Register(register)) if register == I8086_BP_REGISTER => {
                PlannedLocation::Bp
            }
            Some(Location::Spill(slot)) => PlannedLocation::Spill(slot),
            Some(Location::Register(_)) => {
                return Err(Diagnostic::new(format!(
                    "invalid i8086 local register for `{name}`"
                )));
            }
            Some(Location::Unused) | None => {
                return Err(Diagnostic::new(format!(
                    "source allocator did not place local `{name}`"
                )));
            }
        };
        bindings.insert(name, PlannedLocal { location, ty });
    }
    Ok(FunctionLocals {
        bindings,
        spill_sizes,
    })
}

fn collect_i8086_locals(
    body: &[Stmt],
    model: &SemanticModel,
    locals: &mut Vec<SourceLocal>,
    local_types: &mut HashMap<String, Type>,
) -> Result<(), Diagnostic> {
    for stmt in body {
        match stmt {
            Stmt::Let { name, ty, .. } => {
                collect_i8086_local(name, ty, false, body, model, locals, local_types)?;
            }
            Stmt::LetTwo {
                first_name,
                first_ty,
                second_name,
                second_ty,
                ..
            } => {
                collect_i8086_local(
                    first_name,
                    first_ty,
                    false,
                    body,
                    model,
                    locals,
                    local_types,
                )?;
                collect_i8086_local(
                    second_name,
                    second_ty,
                    true,
                    body,
                    model,
                    locals,
                    local_types,
                )?;
            }
            Stmt::If {
                then_body,
                else_body,
                ..
            } => {
                collect_i8086_locals(then_body, model, locals, local_types)?;
                collect_i8086_locals(else_body, model, locals, local_types)?;
            }
            Stmt::While { body, .. } | Stmt::Loop { body } => {
                collect_i8086_locals(body, model, locals, local_types)?;
            }
            _ => {}
        }
    }
    Ok(())
}

fn collect_i8086_local(
    name: &str,
    ty: &Type,
    force_memory: bool,
    body: &[Stmt],
    model: &SemanticModel,
    locals: &mut Vec<SourceLocal>,
    local_types: &mut HashMap<String, Type>,
) -> Result<(), Diagnostic> {
    let resolved = model.resolved_type(ty)?;
    let aggregate = matches!(&resolved, Type::Array { .. })
        || matches!(&resolved, Type::Named(name) if model.structs.contains_key(name));
    let function_pointer = matches!(&resolved, Type::Function { .. })
        || matches!(&resolved, Type::Ptr(inner) if matches!(inner.as_ref(), Type::Function { .. }));
    let size = model.type_size(&resolved)?;
    let word = !aggregate && model.type_width(&resolved).ok() == Some(2);
    if local_types.insert(name.to_owned(), ty.clone()).is_some() {
        return Err(Diagnostic::new(format!("duplicate local `{name}")));
    }
    locals.push(
        SourceLocal::new(
            name.to_owned(),
            size,
            1,
            if word {
                I8086_WORD_CLASS
            } else {
                I8086_BYTE_CLASS
            },
        )
        .with_spill_classes(vec![I8086_STATIC_SPILL_CLASS])
        .with_force_memory(
            force_memory || aggregate || function_pointer || local_is_inline_asm_memory(body, name),
        ),
    );
    Ok(())
}

fn local_is_inline_asm_memory(body: &[Stmt], name: &str) -> bool {
    body.iter().any(|stmt| match stmt {
        Stmt::Asm {
            inputs, outputs, ..
        } => {
            inputs
                .iter()
                .any(|input| input.name == name && input.class == "mem")
                || outputs
                    .iter()
                    .any(|output| output.name == name && output.class == "mem")
        }
        Stmt::If {
            then_body,
            else_body,
            ..
        } => {
            local_is_inline_asm_memory(then_body, name)
                || local_is_inline_asm_memory(else_body, name)
        }
        Stmt::While { body, .. } | Stmt::Loop { body } => local_is_inline_asm_memory(body, name),
        _ => false,
    })
}

fn bool_ty() -> Type {
    Type::Named("bool".to_owned())
}
fn byte_ptr() -> Type {
    Type::Ptr(Box::new(Type::Named("u8".to_owned())))
}
fn mem(address: u32) -> String {
    format!("[{}]", imm(address))
}
fn indexed_bx(offset: u32) -> String {
    if offset == 0 {
        "[bx]".to_owned()
    } else {
        format!("[bx+{}]", imm(offset))
    }
}
fn imm(value: u32) -> String {
    format!("0{value:X}h")
}

fn intrinsic_mask(bits: u32) -> u32 {
    if bits >= 32 {
        u32::MAX
    } else if bits == 0 {
        0
    } else {
        (1_u32 << bits) - 1
    }
}

fn element_type(ty: &Type) -> Result<Type, Diagnostic> {
    match ty {
        Type::Array { element, .. } | Type::Ptr(element) => Ok((**element).clone()),
        _ => Err(Diagnostic::new("indexing requires array or pointer")),
    }
}
fn integer_value_type(value: i64) -> Type {
    if value < 0 {
        if value >= i8::MIN as i64 {
            Type::Named("i8".to_owned())
        } else if value >= i16::MIN as i64 {
            Type::Named("i16".to_owned())
        } else if value >= -0x80_0000 {
            Type::Named("i24".to_owned())
        } else {
            Type::Named("i32".to_owned())
        }
    } else if value <= u8::MAX as i64 {
        Type::Named("u8".to_owned())
    } else if value <= u16::MAX as i64 {
        Type::Named("u16".to_owned())
    } else if value <= 0xFF_FFFF {
        Type::Named("u24".to_owned())
    } else {
        Type::Named("u32".to_owned())
    }
}

fn common_literal_type(left: i64, right: i64) -> Type {
    let minimum = left.min(right);
    let maximum = left.max(right);
    if minimum < 0 {
        if minimum >= i8::MIN as i64 && maximum <= i8::MAX as i64 {
            Type::Named("i8".to_owned())
        } else if minimum >= i16::MIN as i64 && maximum <= i16::MAX as i64 {
            Type::Named("i16".to_owned())
        } else if minimum >= -0x80_0000 && maximum <= 0x7F_FFFF {
            Type::Named("i24".to_owned())
        } else {
            Type::Named("i32".to_owned())
        }
    } else if maximum <= u8::MAX as i64 {
        Type::Named("u8".to_owned())
    } else if maximum <= u16::MAX as i64 {
        Type::Named("u16".to_owned())
    } else if maximum <= 0xFF_FFFF {
        Type::Named("u24".to_owned())
    } else {
        Type::Named("u32".to_owned())
    }
}

fn is_untyped_integer_expr(expr: &Expr) -> bool {
    matches!(expr, Expr::Int(_))
        || matches!(expr, Expr::Unary { op: UnaryOp::Neg, expr } if is_untyped_integer_expr(expr))
}

fn is_comparison(op: BinaryOp) -> bool {
    matches!(
        op,
        BinaryOp::Eq | BinaryOp::Ne | BinaryOp::Lt | BinaryOp::Le | BinaryOp::Gt | BinaryOp::Ge
    )
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
fn resolve_function(path: &[String], model: &SemanticModel) -> Option<String> {
    let qualified = path.join(".");
    if model.functions.contains_key(&qualified) {
        Some(qualified)
    } else {
        path.last()
            .filter(|name| model.functions.contains_key(*name))
            .cloned()
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
                let mut references = Vec::new();
                collect_stmt_function_references(&function.body, &mut references);
                roots.extend(
                    references
                        .into_iter()
                        .filter(|name| model.functions.contains_key(name)),
                );
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
                let mut references = Vec::new();
                collect_expr_function_references(&global.value, &mut references);
                roots.extend(
                    references
                        .into_iter()
                        .filter(|name| model.functions.contains_key(name)),
                );
            }
            _ => {}
        }
    }

    // Inline assembly references use emitted labels rather than source calls. Retain
    // exactly the local or module-qualified functions named by an assembly token.
    for declaration in &program.declarations {
        let Declaration::Function(function) = declaration else {
            continue;
        };
        for candidate in &program.declarations {
            let Declaration::Function(candidate) = candidate else {
                continue;
            };
            if block_mentions_asm_symbol(&function.body, &function_label(&candidate.name)) {
                roots.push(candidate.name.clone());
            }
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

fn block_mentions_asm_symbol(stmts: &[Stmt], symbol: &str) -> bool {
    stmts.iter().any(|stmt| match stmt {
        Stmt::Asm { lines, .. } => lines.iter().any(|line| {
            line.split(|ch: char| !(ch.is_ascii_alphanumeric() || ch == '_'))
                .any(|token| token == symbol)
        }),
        Stmt::If {
            then_body,
            else_body,
            ..
        } => {
            block_mentions_asm_symbol(then_body, symbol)
                || block_mentions_asm_symbol(else_body, symbol)
        }
        Stmt::While { body, .. } | Stmt::Loop { body } => block_mentions_asm_symbol(body, symbol),
        _ => false,
    })
}

fn collect_stmt_calls(stmts: &[Stmt], calls: &mut Vec<Vec<String>>) {
    for stmt in stmts {
        match stmt {
            Stmt::Let { value, .. }
            | Stmt::LetTwo { value, .. }
            | Stmt::Return(Some(value))
            | Stmt::Expr(value) => {
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

fn collect_stmt_function_references(stmts: &[Stmt], references: &mut Vec<String>) {
    for stmt in stmts {
        match stmt {
            Stmt::Let { value, .. }
            | Stmt::LetTwo { value, .. }
            | Stmt::Return(Some(value))
            | Stmt::Expr(value)
            | Stmt::Out { value, .. } => {
                collect_expr_function_references(value, references);
            }
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

fn collect_expr_function_references(expr: &Expr, references: &mut Vec<String>) {
    match expr {
        Expr::AddressOf(name) => references.push(name.clone()),
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

fn statement_live_after(stmts: &[Stmt], inherited: &HashSet<String>) -> Vec<HashSet<String>> {
    let mut result = vec![HashSet::new(); stmts.len()];
    let mut live = inherited.clone();
    for index in (0..stmts.len()).rev() {
        result[index] = live.clone();
        let mut uses = HashSet::new();
        stmt_uses(&stmts[index], &mut uses);
        let mut defs = HashSet::new();
        stmt_defs(&stmts[index], &mut defs);
        for name in defs {
            live.remove(&name);
        }
        live.extend(uses);
    }
    result
}

fn block_live_entry(stmts: &[Stmt], inherited: &HashSet<String>) -> HashSet<String> {
    let mut entry = inherited.clone();
    loop {
        let mut candidate = entry.clone();
        for stmt in stmts.iter().rev() {
            let mut defs = HashSet::new();
            stmt_defs(stmt, &mut defs);
            for name in defs {
                candidate.remove(&name);
            }
            stmt_uses(stmt, &mut candidate);
        }
        if candidate == entry {
            return entry;
        }
        entry = candidate;
    }
}

fn statements_uses(stmts: &[Stmt]) -> HashSet<String> {
    let mut uses = HashSet::new();
    for stmt in stmts {
        stmt_uses(stmt, &mut uses);
    }
    uses
}

fn stmt_defs(stmt: &Stmt, defs: &mut HashSet<String>) {
    match stmt {
        Stmt::Let { name, .. } => {
            defs.insert(name.clone());
        }
        Stmt::LetTwo {
            first_name,
            second_name,
            ..
        } => {
            defs.insert(first_name.clone());
            defs.insert(second_name.clone());
        }
        Stmt::Assign {
            target: Place::Ident(name),
            ..
        } => {
            defs.insert(name.clone());
        }
        // Definitions inside branches and loops are not definite definitions at
        // the enclosing control-flow join. Their nested blocks are analyzed
        // independently with the enclosing live set as their continuation.
        _ => {}
    }
}

fn stmt_uses(stmt: &Stmt, uses: &mut HashSet<String>) {
    match stmt {
        Stmt::Let { value, .. }
        | Stmt::LetTwo { value, .. }
        | Stmt::Return(Some(value))
        | Stmt::Expr(value) => expr_uses(value, uses),
        Stmt::ReturnTwo { first, second } => {
            expr_uses(first, uses);
            expr_uses(second, uses);
        }
        Stmt::Assign { target, op, value } => {
            place_uses(target, uses);
            if *op == AssignOp::Set
                && let Place::Ident(name) = target
            {
                uses.remove(name);
            }
            expr_uses(value, uses);
        }
        Stmt::If {
            condition,
            then_body,
            else_body,
        } => {
            expr_uses(condition, uses);
            for nested in then_body.iter().chain(else_body) {
                stmt_uses(nested, uses);
            }
        }
        Stmt::While { condition, body } => {
            expr_uses(condition, uses);
            for nested in body {
                stmt_uses(nested, uses);
            }
        }
        Stmt::Loop { body } => {
            for nested in body {
                stmt_uses(nested, uses);
            }
        }
        Stmt::Asm {
            inputs, outputs, ..
        } => {
            for operand in inputs {
                uses.insert(operand.name.clone());
            }
            for operand in outputs {
                uses.insert(operand.name.clone());
            }
        }
        Stmt::Out { value, .. } => expr_uses(value, uses),
        Stmt::Return(None) | Stmt::Break | Stmt::Continue => {}
    }
}

fn place_uses(place: &Place, uses: &mut HashSet<String>) {
    match place {
        Place::Ident(name) | Place::Field { base: name, .. } => {
            uses.insert(name.clone());
        }
        Place::Index { name, index } => {
            uses.insert(name.clone());
            expr_uses(index, uses);
        }
        Place::Access(path) => {
            uses.insert(path.root.clone());
            for segment in &path.segments {
                if let AccessSegment::Index(index) = segment {
                    expr_uses(index, uses);
                }
            }
        }
        Place::Deref(expr) => expr_uses(expr, uses),
    }
}

fn expr_uses(expr: &Expr, uses: &mut HashSet<String>) {
    match expr {
        Expr::Ident(name) | Expr::AddressOf(name) => {
            uses.insert(name.clone());
        }
        Expr::Index { name, index } | Expr::AddressOfIndex { name, index } => {
            uses.insert(name.clone());
            expr_uses(index, uses);
        }
        Expr::Field { base, .. } | Expr::AddressOfField { base, .. } => {
            uses.insert(base.clone());
        }
        Expr::Access(path) | Expr::AddressOfAccess(path) => {
            uses.insert(path.root.clone());
            for segment in &path.segments {
                if let AccessSegment::Index(index) = segment {
                    expr_uses(index, uses);
                }
            }
        }
        Expr::Deref(expr)
        | Expr::BankedPointer { pointer: expr, .. }
        | Expr::Unary { expr, .. }
        | Expr::Cast { expr, .. } => expr_uses(expr, uses),
        Expr::Call { args, .. } => {
            for arg in args {
                expr_uses(arg, uses);
            }
        }
        Expr::Array(values) => {
            for value in values {
                expr_uses(value, uses);
            }
        }
        Expr::StructInit { fields, .. } => {
            for (_, value) in fields {
                expr_uses(value, uses);
            }
        }
        Expr::Binary { left, right, .. } => {
            expr_uses(left, uses);
            expr_uses(right, uses);
        }
        Expr::Int(_)
        | Expr::TypedInt(_, _)
        | Expr::Bool(_)
        | Expr::Char(_)
        | Expr::String(_)
        | Expr::In(_) => {}
    }
}

fn block_contains_memory_barrier_asm(stmts: &[Stmt]) -> bool {
    stmts.iter().any(|stmt| match stmt {
        Stmt::Asm { clobbers, .. } => clobbers.iter().any(|clobber| clobber == "memory"),
        Stmt::If {
            then_body,
            else_body,
            ..
        } => {
            block_contains_memory_barrier_asm(then_body)
                || block_contains_memory_barrier_asm(else_body)
        }
        Stmt::While { body, .. } | Stmt::Loop { body } => block_contains_memory_barrier_asm(body),
        _ => false,
    })
}

fn is_i8086_intrinsic_call(path: &[String]) -> bool {
    CATALOG.lookup(&path.join(".")).is_some()
}

fn merge_ranges(mut ranges: Vec<(u32, u32)>) -> Vec<(u32, u32)> {
    if ranges.is_empty() {
        return ranges;
    }
    ranges.sort_unstable();
    let mut merged = Vec::with_capacity(ranges.len());
    for (start, end) in ranges {
        if let Some((_, previous_end)) = merged.last_mut()
            && start <= *previous_end
        {
            *previous_end = (*previous_end).max(end);
        } else {
            merged.push((start, end));
        }
    }
    merged
}

fn relax_i8086_branches(assembly: &str) -> String {
    let had_trailing_newline = assembly.ends_with('\n');
    let original = assembly.to_owned();
    let mut lines = assembly.lines().map(str::to_owned).collect::<Vec<_>>();

    // Recompute addresses after every rewrite. This is intentionally small and
    // conservative: only the branch forms emitted by `branch_long` and plain
    // unconditional near jumps are relaxed.
    for _ in 0..16 {
        let (Some(labels), Some(addresses)) =
            (i8086_label_addresses(&lines), i8086_line_addresses(&lines))
        else {
            // Raw inline assembly may contain data directives or target-specific
            // instructions whose length this backend cannot prove. Do not guess;
            // retaining near branches is safer than producing a bad short branch.
            return original;
        };
        let mut changed = false;

        let mut index = 0;
        while index + 2 < lines.len() {
            let Some((inverse, _, skip)) = i8086_branch_parts(&lines[index]) else {
                index += 1;
                continue;
            };
            let Some((_, _, target)) = i8086_branch_parts(&lines[index + 1]) else {
                index += 1;
                continue;
            };
            if !target.starts_with('.') && !target.starts_with('_') {
                index += 1;
                continue;
            }
            let skip_label = format!("{skip}:");
            if lines[index + 2].trim() != skip_label
                || lines.iter().filter(|line| line.contains(&skip)).count() > 2
            {
                index += 1;
                continue;
            }
            let Some(&target_address) = labels.get(&target) else {
                index += 1;
                continue;
            };
            let direct = inverse_condition(inverse);
            if !(-128..=127).contains(&((target_address as i64) - (addresses[index] as i64 + 2))) {
                index += 1;
                continue;
            }
            lines[index] = format!("    {direct} short {target}");
            lines.remove(index + 2);
            lines.remove(index + 1);
            changed = true;
            break;
        }
        if changed {
            continue;
        }

        for index in 0..lines.len() {
            let Some((mnemonic, distance, target)) = i8086_branch_parts(&lines[index]) else {
                continue;
            };
            let next = lines
                .iter()
                .skip(index + 1)
                .find(|line| {
                    let trimmed = line.trim();
                    !trimmed.is_empty() && !trimmed.starts_with(';')
                })
                .map(|line| line.trim().to_owned());
            if mnemonic == "jmp"
                && next
                    .as_deref()
                    .is_some_and(|line| line == format!("{target}:"))
            {
                lines.remove(index);
                changed = true;
                break;
            }
            if mnemonic != "jmp" || distance != "near" {
                continue;
            }
            let Some(&target_address) = labels.get(&target) else {
                continue;
            };
            if (-128..=127).contains(&((target_address as i64) - (addresses[index] as i64 + 2))) {
                lines[index] = format!("    jmp short {target}");
                changed = true;
                break;
            }
        }
        if !changed {
            break;
        }
    }

    let mut output = lines.join("\n");
    if had_trailing_newline {
        output.push('\n');
    }
    output
}

fn i8086_branch_parts(line: &str) -> Option<(&str, &str, String)> {
    let mut parts = line.split_whitespace();
    let mnemonic = parts.next()?;
    let distance = parts.next()?;
    let target = parts.next()?.to_owned();
    if parts.next().is_some()
        || !matches!(distance, "short" | "near")
        || !(mnemonic == "jmp"
            || matches!(
                mnemonic,
                "jae" | "jb" | "jbe" | "ja" | "je" | "jne" | "jz" | "jnz" | "js" | "jns"
            ))
    {
        return None;
    }
    Some((mnemonic, distance, target))
}

fn i8086_label_addresses(lines: &[String]) -> Option<HashMap<String, u32>> {
    let addresses = i8086_line_addresses(lines)?;
    Some(
        lines
            .iter()
            .enumerate()
            .filter_map(|(index, line)| {
                let label = line.trim().strip_suffix(':')?;
                Some((label.to_owned(), addresses[index]))
            })
            .collect(),
    )
}

fn i8086_line_addresses(lines: &[String]) -> Option<Vec<u32>> {
    let mut address = 0u32;
    let mut result = Vec::with_capacity(lines.len());
    for line in lines {
        result.push(address);
        let trimmed = line.trim();
        if trimmed.is_empty()
            || trimmed.starts_with(';')
            || trimmed.starts_with("section ")
            || trimmed.ends_with(':')
        {
            continue;
        }
        address = address.saturating_add(instruction_len(trimmed).ok()? as u32);
    }
    Some(result)
}

fn inverse_condition(condition: &str) -> &'static str {
    match condition {
        "jz" | "je" => "jnz",
        "jnz" | "jne" => "jz",
        "jb" => "jae",
        "jae" => "jb",
        "jbe" => "ja",
        "ja" => "jbe",
        "js" => "jns",
        "jns" => "js",
        _ => unreachable!("unsupported i8086 condition"),
    }
}

fn function_label(name: &str) -> String {
    format!("_{}", sanitize(&name.replace('.', "__")))
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

fn type_display(ty: &Type) -> String {
    match ty {
        Type::Named(name) => name.clone(),
        Type::Ptr(inner) => format!("ptr<{}>", type_display(inner)),
        Type::Function {
            params,
            return_type,
        } => {
            let params = params
                .iter()
                .map(type_display)
                .collect::<Vec<_>>()
                .join(", ");
            match return_type {
                Some(ty) => format!("fn({params}){}", type_display(ty)),
                None => format!("fn({params})"),
            }
        }
        Type::Array { element, .. } => format!("[{}; ...]", type_display(element)),
    }
}

fn canonical_i8086_clobber(clobber: &str) -> &str {
    match clobber {
        "al" | "ah" | "ax" => "ax",
        "bl" | "bh" | "bx" => "bx",
        "cl" | "ch" | "cx" => "cx",
        "dl" | "dh" | "dx" => "dx",
        other => other,
    }
}

fn format_immediate(value: i64, width: u8) -> String {
    let bits = u32::from(width) * 8;
    let mask = if bits >= 64 {
        u64::MAX
    } else {
        (1u64 << bits) - 1
    };
    imm(((value as u64) & mask) as u32)
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

fn contains_two_result_statement(stmts: &[Stmt]) -> bool {
    stmts.iter().any(|stmt| match stmt {
        Stmt::ReturnTwo { .. } => true,
        Stmt::If {
            then_body,
            else_body,
            ..
        } => contains_two_result_statement(then_body) || contains_two_result_statement(else_body),
        Stmt::While { body, .. } | Stmt::Loop { body } => contains_two_result_statement(body),
        _ => false,
    })
}

fn block_contains_empty_return(stmts: &[Stmt]) -> bool {
    stmts.iter().any(|stmt| match stmt {
        Stmt::Return(None) => true,
        Stmt::If {
            then_body,
            else_body,
            ..
        } => block_contains_empty_return(then_body) || block_contains_empty_return(else_body),
        Stmt::While { body, .. } | Stmt::Loop { body } => block_contains_empty_return(body),
        _ => false,
    })
}

fn block_can_complete_normally(stmts: &[Stmt], model: &SemanticModel) -> bool {
    let mut reachable = true;
    for stmt in stmts {
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

fn block_can_break_current_loop(stmts: &[Stmt], model: &SemanticModel) -> bool {
    let mut reachable = true;
    for stmt in stmts {
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

#[cfg(test)]
mod tests;
