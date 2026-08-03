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
                self.push_instruction(block, instruction, has_call);
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
mod tests {
    use alloc::{boxed::Box, string::ToString, vec};

    use crate::ast::{AsmInput, AsmOutput, BinaryOp, Type};

    use super::*;
    use crate::regalloc::{
        Location, PhysicalRegister, RegisterClass, RegisterUnit, SpillClass, SpillClassId,
    };

    fn local(name: &str) -> SourceLocal {
        SourceLocal::new(name, 1, 1, RegClass(0)).with_spill_classes(vec![SpillClassId(0)])
    }

    fn target() -> Target {
        Target {
            units: vec![RegisterUnit::new("r0"), RegisterUnit::new("r1")],
            registers: vec![
                PhysicalRegister::new("r0", vec![super::super::RegUnit(0)]),
                PhysicalRegister::new("r1", vec![super::super::RegUnit(1)]),
            ],
            register_classes: vec![RegisterClass::new("byte", vec![PhysReg(0), PhysReg(1)])],
            spill_classes: vec![SpillClass::new("stack", None, 1)],
        }
    }

    #[test]
    fn builds_exact_branch_successors_and_ignores_global_names() {
        let body = vec![Stmt::If {
            condition: Expr::Ident("condition".to_string()),
            then_body: vec![Stmt::Assign {
                target: Place::Ident("value".to_string()),
                op: AssignOp::Set,
                value: Expr::Ident("GLOBAL".to_string()),
            }],
            else_body: vec![Stmt::Assign {
                target: Place::Ident("value".to_string()),
                op: AssignOp::Set,
                value: Expr::Int(2),
            }],
        }];
        let lowered = lower_source_function(&[local("condition"), local("value")], &body, &[])
            .expect("lowering should succeed");

        assert_eq!(lowered.function.blocks.len(), 4);
        assert_eq!(
            lowered.function.blocks[0].successors,
            vec![BlockId(1), BlockId(2)]
        );
        assert_eq!(lowered.function.blocks[1].successors, vec![BlockId(3)]);
        assert_eq!(lowered.function.blocks[2].successors, vec![BlockId(3)]);
        assert!(lowered.function.blocks[3].successors.is_empty());
        assert_eq!(lowered.function.blocks[1].instructions[0].uses, vec![]);
        assert_eq!(
            lowered.function.blocks[1].instructions[0].defs,
            vec![VReg(1)]
        );
    }

    #[test]
    fn builds_while_and_loop_break_continue_edges() {
        let body = vec![
            Stmt::While {
                condition: Expr::Ident("condition".to_string()),
                body: vec![Stmt::Continue],
            },
            Stmt::Loop {
                body: vec![Stmt::If {
                    condition: Expr::Ident("condition".to_string()),
                    then_body: vec![Stmt::Continue],
                    else_body: vec![Stmt::Break],
                }],
            },
        ];
        let lowered = lower_source_function(&[local("condition")], &body, &[])
            .expect("lowering should succeed");
        let blocks = &lowered.function.blocks;

        assert_eq!(blocks[0].successors, vec![BlockId(1), BlockId(2)]);
        assert_eq!(blocks[1].successors, vec![BlockId(0)]);
        assert_eq!(blocks[2].successors, vec![BlockId(3)]);
        assert_eq!(blocks[3].successors, vec![BlockId(5), BlockId(6)]);
        assert_eq!(blocks[5].successors, vec![BlockId(3)]);
        assert_eq!(blocks[6].successors, vec![BlockId(4)]);
        assert!(blocks[4].successors.is_empty());
        assert!(blocks[7].successors.is_empty());
    }

    #[test]
    fn call_clobber_moves_a_live_local_off_the_clobbered_register() {
        let body = vec![
            Stmt::Let {
                name: "live".to_string(),
                ty: Type::Named("u8".to_string()),
                value: Expr::Int(1),
            },
            Stmt::Expr(Expr::Call {
                path: vec!["opaque".to_string()],
                args: vec![],
            }),
            Stmt::Return(Some(Expr::Ident("live".to_string()))),
        ];
        let result = allocate_source_locals(&target(), &[local("live")], &body, &[PhysReg(0)])
            .expect("allocation should succeed");

        assert_eq!(
            result.allocation.location(VReg(0)),
            Some(Location::Register(PhysReg(1)))
        );
    }

    #[test]
    fn address_taken_and_caller_forced_locals_spill() {
        let locals = vec![
            local("addressed"),
            local("aggregate").with_force_memory(true),
        ];
        let body = vec![
            Stmt::Expr(Expr::AddressOf("addressed".to_string())),
            Stmt::Expr(Expr::Field {
                base: "aggregate".to_string(),
                field: "member".to_string(),
            }),
        ];
        let result = allocate_source_locals(&target(), &locals, &body, &[])
            .expect("allocation should succeed");

        assert!(matches!(
            result.allocation.location(VReg(0)),
            Some(Location::Spill(_))
        ));
        assert!(matches!(
            result.allocation.location(VReg(1)),
            Some(Location::Spill(_))
        ));
        let addressed_slot = match result.allocation.location(VReg(0)) {
            Some(Location::Spill(slot)) => slot,
            _ => unreachable!(),
        };
        let aggregate_slot = match result.allocation.location(VReg(1)) {
            Some(Location::Spill(slot)) => slot,
            _ => unreachable!(),
        };
        assert_ne!(addressed_slot, aggregate_slot);
        assert!(!result.allocation.spill_slots[addressed_slot].reusable);
        assert!(result.allocation.spill_slots[aggregate_slot].reusable);
    }

    #[test]
    fn dead_and_unused_locals_remain_unused() {
        let result = allocate_source_locals(&target(), &[local("unused")], &[], &[])
            .expect("allocation should succeed");

        assert_eq!(result.locals.vreg("unused"), Some(VReg(0)));
        assert_eq!(result.locals.name(VReg(0)), Some("unused"));
        assert_eq!(result.allocation.location(VReg(0)), Some(Location::Unused));
    }

    #[test]
    fn compound_assignment_uses_and_defines_its_target() {
        let body = vec![Stmt::Assign {
            target: Place::Index {
                name: "value".to_string(),
                index: Box::new(Expr::Ident("index".to_string())),
            },
            op: AssignOp::Add,
            value: Expr::Binary {
                left: Box::new(Expr::Ident("rhs".to_string())),
                op: BinaryOp::Add,
                right: Box::new(Expr::Int(1)),
            },
        }];
        let lowered =
            lower_source_function(&[local("value"), local("index"), local("rhs")], &body, &[])
                .expect("lowering should succeed");
        let instruction = &lowered.function.blocks[0].instructions[0];

        assert_eq!(instruction.uses, vec![VReg(2), VReg(1), VReg(0)]);
        assert_eq!(instruction.defs, vec![VReg(0)]);
    }

    #[test]
    fn locals_on_either_side_of_a_nested_call_force_memory() {
        for expression in [
            Expr::Binary {
                left: Box::new(Expr::Call {
                    path: vec!["callee".to_string()],
                    args: vec![],
                }),
                op: BinaryOp::Add,
                right: Box::new(Expr::Ident("local".to_string())),
            },
            Expr::Binary {
                left: Box::new(Expr::Ident("local".to_string())),
                op: BinaryOp::Add,
                right: Box::new(Expr::Call {
                    path: vec!["callee".to_string()],
                    args: vec![],
                }),
            },
        ] {
            let lowered =
                lower_source_function(&[local("local")], &[Stmt::Expr(expression)], &[PhysReg(0)])
                    .expect("lowering should succeed");
            assert!(lowered.function.virtual_registers[0].must_spill);
        }
    }

    #[test]
    fn compound_assignment_with_a_call_forces_its_target_to_memory() {
        let body = vec![Stmt::Assign {
            target: Place::Ident("target".to_string()),
            op: AssignOp::Add,
            value: Expr::Call {
                path: vec!["callee".to_string()],
                args: vec![],
            },
        }];
        let lowered = lower_source_function(&[local("target")], &body, &[PhysReg(0)])
            .expect("lowering should succeed");

        assert_eq!(
            lowered.function.blocks[0].instructions[0].uses,
            vec![VReg(0)]
        );
        assert_eq!(
            lowered.function.blocks[0].instructions[0].defs,
            vec![VReg(0)]
        );
        assert!(lowered.function.virtual_registers[0].must_spill);
    }

    #[test]
    fn indexed_target_with_a_call_forces_all_statement_locals_to_memory() {
        let body = vec![Stmt::Assign {
            target: Place::Index {
                name: "array".to_string(),
                index: Box::new(Expr::Ident("index".to_string())),
            },
            op: AssignOp::Set,
            value: Expr::Call {
                path: vec!["callee".to_string()],
                args: vec![Expr::Ident("argument".to_string())],
            },
        }];
        let lowered = lower_source_function(
            &[local("array"), local("index"), local("argument")],
            &body,
            &[PhysReg(0)],
        )
        .expect("lowering should succeed");

        assert!(
            lowered
                .function
                .virtual_registers
                .iter()
                .all(|vreg| vreg.must_spill)
        );
    }

    #[test]
    fn local_function_pointer_call_records_and_spills_the_path_root() {
        let body = vec![Stmt::Expr(Expr::Call {
            path: vec!["callback".to_string()],
            args: vec![],
        })];
        let lowered = lower_source_function(&[local("callback")], &body, &[PhysReg(0)])
            .expect("lowering should succeed");

        assert_eq!(
            lowered.function.blocks[0].instructions[0].uses,
            vec![VReg(0)]
        );
        assert!(lowered.function.virtual_registers[0].must_spill);
    }

    #[test]
    fn break_and_continue_outside_loops_are_diagnostics() {
        let errors = lower_source_function(&[], &[Stmt::Break, Stmt::Continue], &[])
            .expect_err("invalid loop control should fail");

        assert_eq!(errors.len(), 2);
        assert!(
            errors
                .iter()
                .all(|error| error.code == DiagnosticCode::InvalidFunction)
        );
    }

    #[test]
    fn asm_names_are_effects_and_asm_is_opaque() {
        let body = vec![Stmt::Asm {
            volatile: false,
            inputs: vec![AsmInput {
                name: "input".to_string(),
                ty: Type::Named("u8".to_string()),
                class: "byte".to_string(),
            }],
            outputs: vec![AsmOutput {
                name: "output".to_string(),
                ty: Type::Named("u8".to_string()),
                class: "byte".to_string(),
            }],
            clobbers: vec![],
            lines: vec![],
        }];
        let lowered =
            lower_source_function(&[local("input"), local("output")], &body, &[PhysReg(0)])
                .expect("lowering should succeed");
        let instruction = &lowered.function.blocks[0].instructions[0];

        assert_eq!(instruction.uses, vec![VReg(0)]);
        assert_eq!(instruction.defs, vec![VReg(1)]);
        assert_eq!(instruction.clobbers, vec![PhysReg(0)]);
        assert!(
            lowered
                .function
                .virtual_registers
                .iter()
                .all(|vreg| vreg.must_spill)
        );
    }
}
