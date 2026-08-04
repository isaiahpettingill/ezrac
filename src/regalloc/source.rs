//! Conservative source-AST lowering for local register-allocation planning.
//!
//! This module records only effects needed by the allocator. It does not try to
//! model target instruction selection. Each source expression or statement is
//! represented by its local uses, local definitions, opaque clobbers, and CFG
//! edges. Only names supplied in [`SourceLocal`] are treated as locals.

extern crate alloc;

use alloc::{
    collections::{BTreeMap, BTreeSet},
    format,
    string::String,
    vec,
    vec::Vec,
};

use crate::ast::{AccessPath, AccessSegment, AssignOp, Expr, Place, Stmt};

use super::{
    Allocation, BasicBlock, BlockId, Diagnostic, DiagnosticCode, Function, Instruction, PhysReg,
    RegClass, SpillClassId, Target, VReg, VirtualRegister, allocate,
};

/// Source-local storage requirements supplied by the caller.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SourceLocal {
    pub name: String,
    pub size: u32,
    pub alignment: u32,
    pub class: RegClass,
    /// Empty means every compatible target spill class is allowed.
    pub spill_classes: Vec<SpillClassId>,
    /// Forces memory storage for aggregates, address-sensitive values, or other
    /// locals the caller knows cannot safely live only in a register.
    pub force_memory: bool,
}

impl SourceLocal {
    pub fn new(name: impl Into<String>, size: u32, alignment: u32, class: RegClass) -> Self {
        Self {
            name: name.into(),
            size,
            alignment,
            class,
            spill_classes: Vec::new(),
            force_memory: false,
        }
    }

    pub fn with_spill_classes(mut self, spill_classes: Vec<SpillClassId>) -> Self {
        self.spill_classes = spill_classes;
        self
    }

    pub fn with_force_memory(mut self, force_memory: bool) -> Self {
        self.force_memory = force_memory;
        self
    }
}

/// Stable source-name and virtual-register mappings.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SourceLocalMap {
    pub by_name: BTreeMap<String, VReg>,
    pub by_vreg: Vec<String>,
}

impl SourceLocalMap {
    pub fn vreg(&self, name: &str) -> Option<VReg> {
        self.by_name.get(name).copied()
    }

    pub fn name(&self, vreg: VReg) -> Option<&str> {
        self.by_vreg.get(vreg.0).map(String::as_str)
    }
}

/// Source mappings and the machine-effect function produced before allocation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LoweredSourceFunction {
    pub locals: SourceLocalMap,
    pub function: Function,
}

/// Source mappings paired with the allocator result.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SourceAllocation {
    pub locals: SourceLocalMap,
    pub allocation: Allocation,
}

/// Lowers source statements to the allocator's target-neutral machine-effect IR.
///
/// Calls and inline assembly clobber every register in `opaque_clobbers`.
/// Explicit address-taking automatically forces the referenced local to memory.
/// Field and index roots should also be marked with [`SourceLocal::force_memory`]
/// when their source type requires addressable aggregate storage.
pub fn lower_source_function(
    locals: &[SourceLocal],
    body: &[Stmt],
    opaque_clobbers: &[PhysReg],
) -> Result<LoweredSourceFunction, Vec<Diagnostic>> {
    let mappings = build_mappings(locals)?;
    validate_loop_control(body)?;
    let virtual_registers = locals
        .iter()
        .map(|local| {
            VirtualRegister::new(local.name.clone(), local.size, local.alignment, local.class)
                .with_spill_classes(local.spill_classes.clone())
                .with_must_spill(local.force_memory)
        })
        .collect();

    let mut builder = Builder {
        mappings: &mappings,
        opaque_clobbers: deduplicate_clobbers(opaque_clobbers),
        blocks: vec![BasicBlock::default()],
        address_taken: BTreeSet::new(),
        opaque_locals: BTreeSet::new(),
    };
    builder.lower_statements(body, Some(BlockId(0)), None);

    let mut function = Function::new(virtual_registers, builder.blocks);
    for vreg in builder.address_taken {
        function.virtual_registers[vreg.0].must_spill = true;
        function.virtual_registers[vreg.0].spill_slot_reusable = false;
    }
    for vreg in builder.opaque_locals {
        function.virtual_registers[vreg.0].must_spill = true;
    }

    Ok(LoweredSourceFunction {
        locals: mappings,
        function,
    })
}

/// Lowers source-local effects and runs the shared register allocator.
///
/// Source mapping errors use the allocator's `InvalidFunction` diagnostic code.
/// Diagnostics returned by the allocator are otherwise passed through unchanged.
pub fn allocate_source_locals(
    target: &Target,
    locals: &[SourceLocal],
    body: &[Stmt],
    opaque_clobbers: &[PhysReg],
) -> Result<SourceAllocation, Vec<Diagnostic>> {
    let lowered = lower_source_function(locals, body, opaque_clobbers)?;
    let allocation = allocate(target, &lowered.function)?;
    Ok(SourceAllocation {
        locals: lowered.locals,
        allocation,
    })
}

fn build_mappings(locals: &[SourceLocal]) -> Result<SourceLocalMap, Vec<Diagnostic>> {
    let mut by_name = BTreeMap::new();
    let mut by_vreg = Vec::with_capacity(locals.len());
    let mut diagnostics = Vec::new();

    for (index, local) in locals.iter().enumerate() {
        let vreg = VReg(index);
        if let Some(first) = by_name.insert(local.name.clone(), vreg) {
            diagnostics.push(Diagnostic {
                code: DiagnosticCode::InvalidFunction,
                message: format!(
                    "source local {:?} is listed more than once (virtual registers {} and {})",
                    local.name, first.0, index
                ),
            });
        }
        by_vreg.push(local.name.clone());
    }

    if diagnostics.is_empty() {
        Ok(SourceLocalMap { by_name, by_vreg })
    } else {
        Err(diagnostics)
    }
}

fn validate_loop_control(body: &[Stmt]) -> Result<(), Vec<Diagnostic>> {
    fn visit(statements: &[Stmt], loop_depth: usize, diagnostics: &mut Vec<Diagnostic>) {
        for statement in statements {
            match statement {
                Stmt::If {
                    then_body,
                    else_body,
                    ..
                } => {
                    visit(then_body, loop_depth, diagnostics);
                    visit(else_body, loop_depth, diagnostics);
                }
                Stmt::While { body, .. } | Stmt::Loop { body } => {
                    visit(body, loop_depth + 1, diagnostics);
                }
                Stmt::Break if loop_depth == 0 => diagnostics.push(Diagnostic {
                    code: DiagnosticCode::InvalidFunction,
                    message: "break appears outside a loop".into(),
                }),
                Stmt::Continue if loop_depth == 0 => diagnostics.push(Diagnostic {
                    code: DiagnosticCode::InvalidFunction,
                    message: "continue appears outside a loop".into(),
                }),
                _ => {}
            }
        }
    }

    let mut diagnostics = Vec::new();
    visit(body, 0, &mut diagnostics);
    if diagnostics.is_empty() {
        Ok(())
    } else {
        Err(diagnostics)
    }
}

fn deduplicate_clobbers(clobbers: &[PhysReg]) -> Vec<PhysReg> {
    let mut result = Vec::new();
    for clobber in clobbers {
        push_unique(&mut result, *clobber);
    }
    result
}

#[derive(Clone, Copy)]
struct LoopTargets {
    break_target: BlockId,
    continue_target: BlockId,
}

struct Builder<'a> {
    mappings: &'a SourceLocalMap,
    opaque_clobbers: Vec<PhysReg>,
    blocks: Vec<BasicBlock>,
    address_taken: BTreeSet<VReg>,
    opaque_locals: BTreeSet<VReg>,
}

impl Builder<'_> {
    fn new_block(&mut self) -> BlockId {
        let id = BlockId(self.blocks.len());
        self.blocks.push(BasicBlock::default());
        id
    }

    fn add_successor(&mut self, block: BlockId, successor: BlockId) {
        push_unique(&mut self.blocks[block.0].successors, successor);
    }

    fn push_instruction(
        &mut self,
        block: BlockId,
        instruction: Instruction,
        has_opaque_effect: bool,
    ) {
        if has_opaque_effect {
            self.opaque_locals
                .extend(instruction.uses.iter().chain(&instruction.defs).copied());
        }
        self.blocks[block.0].instructions.push(instruction);
    }

    fn lower_statements(
        &mut self,
        statements: &[Stmt],
        mut current: Option<BlockId>,
        loop_targets: Option<LoopTargets>,
    ) -> Option<BlockId> {
        for statement in statements {
            let Some(block) = current else {
                break;
            };
            current = self.lower_statement(statement, block, loop_targets);
        }
        current
    }

    fn lower_statement(
        &mut self,
        statement: &Stmt,
        block: BlockId,
        loop_targets: Option<LoopTargets>,
    ) -> Option<BlockId> {
        match statement {
            Stmt::Let { name, value, .. } => {
                let mut instruction = Instruction::new();
                let has_call = self.collect_expr(value, &mut instruction);
                self.add_def(name, &mut instruction);
                if has_call {
                    // A let result is defined after its call; only the call's
                    // source locals need opaque storage.
                    self.opaque_locals.extend(instruction.uses.iter().copied());
                }
                self.push_instruction(block, instruction, false);
                Some(block)
            }
            Stmt::Assign { target, op, value } => {
                let mut instruction = Instruction::new();
                let value_has_call = self.collect_expr(value, &mut instruction);
                let place_has_call =
                    self.collect_place(target, *op != AssignOp::Set, &mut instruction);
                self.push_instruction(block, instruction, value_has_call || place_has_call);
                Some(block)
            }
            Stmt::If {
                condition,
                then_body,
                else_body,
            } => {
                let mut instruction = Instruction::new();
                let has_call = self.collect_expr(condition, &mut instruction);
                self.push_instruction(block, instruction, has_call);

                let then_block = self.new_block();
                let else_block = self.new_block();
                let join_block = self.new_block();
                self.add_successor(block, then_block);
                self.add_successor(block, else_block);

                let then_end = self.lower_statements(then_body, Some(then_block), loop_targets);
                let else_end = self.lower_statements(else_body, Some(else_block), loop_targets);
                if let Some(end) = then_end {
                    self.add_successor(end, join_block);
                }
                if let Some(end) = else_end {
                    self.add_successor(end, join_block);
                }
                (then_end.is_some() || else_end.is_some()).then_some(join_block)
            }
            Stmt::While { condition, body } => {
                let condition_block = block;
                let mut instruction = Instruction::new();
                let has_call = self.collect_expr(condition, &mut instruction);
                self.push_instruction(condition_block, instruction, has_call);

                let body_block = self.new_block();
                let after_block = self.new_block();
                self.add_successor(condition_block, body_block);
                self.add_successor(condition_block, after_block);
                let targets = LoopTargets {
                    break_target: after_block,
                    continue_target: condition_block,
                };
                if let Some(body_end) = self.lower_statements(body, Some(body_block), Some(targets))
                {
                    self.add_successor(body_end, condition_block);
                }
                Some(after_block)
            }
            Stmt::Loop { body } => {
                let body_block = self.new_block();
                let after_block = self.new_block();
                self.add_successor(block, body_block);
                let targets = LoopTargets {
                    break_target: after_block,
                    continue_target: body_block,
                };
                if let Some(body_end) = self.lower_statements(body, Some(body_block), Some(targets))
                {
                    self.add_successor(body_end, body_block);
                }
                Some(after_block)
            }
            Stmt::Break => {
                if let Some(targets) = loop_targets {
                    self.add_successor(block, targets.break_target);
                }
                None
            }
            Stmt::Continue => {
                if let Some(targets) = loop_targets {
                    self.add_successor(block, targets.continue_target);
                }
                None
            }
            Stmt::Return(value) => {
                if let Some(value) = value {
                    let mut instruction = Instruction::new();
                    let has_call = self.collect_expr(value, &mut instruction);
                    self.push_instruction(block, instruction, has_call);
                }
                None
            }
            Stmt::Asm {
                inputs, outputs, ..
            } => {
                let mut instruction = Instruction::new();
                for input in inputs {
                    self.add_use(&input.name, &mut instruction);
                }
                for output in outputs {
                    self.add_def(&output.name, &mut instruction);
                }
                instruction.clobbers = self.opaque_clobbers.clone();
                self.push_instruction(block, instruction, true);
                Some(block)
            }
            Stmt::Out { value, .. } | Stmt::Expr(value) => {
                let mut instruction = Instruction::new();
                let has_call = self.collect_expr(value, &mut instruction);
                self.push_instruction(block, instruction, has_call);
                Some(block)
            }
        }
    }

    fn collect_place(
        &mut self,
        place: &Place,
        compound: bool,
        instruction: &mut Instruction,
    ) -> bool {
        match place {
            Place::Ident(name) => {
                self.add_place_root(name, compound, instruction);
                false
            }
            Place::Index { name, index } => {
                let has_call = self.collect_expr(index, instruction);
                self.add_place_root(name, compound, instruction);
                has_call
            }
            Place::Field { base, .. } => {
                self.add_place_root(base, compound, instruction);
                false
            }
            Place::Access(path) => {
                let has_call = self.collect_access_indices(path, instruction);
                self.add_place_root(&path.root, compound, instruction);
                has_call
            }
            Place::Deref(pointer) => self.collect_expr(pointer, instruction),
        }
    }

    fn add_place_root(&self, name: &str, compound: bool, instruction: &mut Instruction) {
        if compound {
            self.add_use(name, instruction);
        }
        self.add_def(name, instruction);
    }

    fn collect_expr(&mut self, expression: &Expr, instruction: &mut Instruction) -> bool {
        match expression {
            Expr::Ident(name) | Expr::In(name) => {
                self.add_use(name, instruction);
                false
            }
            Expr::Index { name, index } => {
                self.add_use(name, instruction);
                self.collect_expr(index, instruction)
            }
            Expr::Field { base, .. } => {
                self.add_use(base, instruction);
                false
            }
            Expr::AddressOfIndex { name, index } => {
                self.add_address_use(name, instruction);
                self.collect_expr(index, instruction)
            }
            Expr::AddressOfField { base, .. } | Expr::AddressOf(base) => {
                self.add_address_use(base, instruction);
                false
            }
            Expr::Access(path) => {
                self.add_use(&path.root, instruction);
                self.collect_access_indices(path, instruction)
            }
            Expr::AddressOfAccess(path) => {
                self.add_address_use(&path.root, instruction);
                self.collect_access_indices(path, instruction)
            }
            Expr::Array(values) => values.iter().fold(false, |has_call, value| {
                self.collect_expr(value, instruction) || has_call
            }),
            Expr::StructInit { fields, .. } => fields.iter().fold(false, |has_call, (_, value)| {
                self.collect_expr(value, instruction) || has_call
            }),
            Expr::Deref(pointer)
            | Expr::BankedPointer { pointer, .. }
            | Expr::Unary { expr: pointer, .. }
            | Expr::Cast { expr: pointer, .. } => self.collect_expr(pointer, instruction),
            Expr::Call { path, args } => {
                if let Some(root) = path.first() {
                    self.add_use(root, instruction);
                }
                for argument in args {
                    self.collect_expr(argument, instruction);
                }
                instruction.clobbers = self.opaque_clobbers.clone();
                true
            }
            Expr::Binary { left, right, .. } => {
                let left_has_call = self.collect_expr(left, instruction);
                self.collect_expr(right, instruction) || left_has_call
            }
            Expr::Int(_)
            | Expr::TypedInt(_, _)
            | Expr::Bool(_)
            | Expr::Char(_)
            | Expr::String(_) => false,
        }
    }

    fn collect_access_indices(&mut self, path: &AccessPath, instruction: &mut Instruction) -> bool {
        path.segments.iter().fold(false, |has_call, segment| {
            if let AccessSegment::Index(index) = segment {
                self.collect_expr(index, instruction) || has_call
            } else {
                has_call
            }
        })
    }

    fn add_use(&self, name: &str, instruction: &mut Instruction) {
        if let Some(vreg) = self.mappings.vreg(name) {
            push_unique(&mut instruction.uses, vreg);
        }
    }

    fn add_def(&self, name: &str, instruction: &mut Instruction) {
        if let Some(vreg) = self.mappings.vreg(name) {
            push_unique(&mut instruction.defs, vreg);
        }
    }

    fn add_address_use(&mut self, name: &str, instruction: &mut Instruction) {
        if let Some(vreg) = self.mappings.vreg(name) {
            self.address_taken.insert(vreg);
            push_unique(&mut instruction.uses, vreg);
        }
    }
}

fn push_unique<T: Copy + PartialEq>(values: &mut Vec<T>, value: T) {
    if !values.contains(&value) {
        values.push(value);
    }
}

#[cfg(test)]
mod tests;
