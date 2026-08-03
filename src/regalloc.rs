//! A self-contained, target-neutral register allocator.
//!
//! Targets describe physical registers in terms of the register units they own.
//! Two registers alias when their unit sets intersect. Register classes limit the
//! registers available to a virtual register, while fixed operands and clobbers
//! add per-instruction constraints.
//!
//! The allocator computes CFG liveness, builds conservative live intervals, and
//! applies deterministic linear scan. Intervals do not contain holes: a value
//! live in separated parts of the CFG occupies its register over the full range.
//! This costs some allocation quality but makes aliasing, fixed operands, and
//! control flow safe without target-specific logic.

extern crate alloc;

use alloc::{
    collections::{BTreeMap, BTreeSet},
    format,
    string::String,
    vec,
    vec::Vec,
};
use core::fmt;

pub mod source;

/// Index of an indivisible physical register storage unit.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Hash)]
pub struct RegUnit(pub usize);

/// Index of a physical register in [`Target::registers`].
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Hash)]
pub struct PhysReg(pub usize);

/// Index of a register class in [`Target::register_classes`].
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Hash)]
pub struct RegClass(pub usize);

/// Index of a target-defined spill class in [`Target::spill_classes`].
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Hash)]
pub struct SpillClassId(pub usize);

/// Index of a virtual register in [`Function::virtual_registers`].
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Hash)]
pub struct VReg(pub usize);

/// Index of a basic block in [`Function::blocks`].
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Hash)]
pub struct BlockId(pub usize);

/// A stable instruction position used by liveness and interval results.
///
/// Uses are at even positions and definitions are at the following odd
/// position. This permits a copy destination to reuse a source register when
/// the source dies at the copy.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Hash)]
pub struct ProgramPoint(pub u32);

/// An indivisible piece of physical register storage.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RegisterUnit {
    pub name: String,
}

impl RegisterUnit {
    pub fn new(name: impl Into<String>) -> Self {
        Self { name: name.into() }
    }
}

/// A physical register and all storage units it covers.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PhysicalRegister {
    pub name: String,
    pub units: Vec<RegUnit>,
}

impl PhysicalRegister {
    pub fn new(name: impl Into<String>, units: Vec<RegUnit>) -> Self {
        Self {
            name: name.into(),
            units,
        }
    }
}

/// A deterministic, ordered set of registers legal for one kind of value.
///
/// An empty register list models a memory-only class. Values in that class are
/// spilled without requiring dummy physical registers.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RegisterClass {
    pub name: String,
    /// Registers are tried in this order after any safe copy preference.
    pub registers: Vec<PhysReg>,
}

impl RegisterClass {
    pub fn new(name: impl Into<String>, registers: Vec<PhysReg>) -> Self {
        Self {
            name: name.into(),
            registers,
        }
    }
}

/// A target-defined storage area for spilled values.
///
/// Names may be values such as `"stack"`, `"static"`, or `"zero-page"`.
/// Empty `register_classes` means that the spill class accepts every register
/// class. Capacity is measured in bytes and is independent for each spill class.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SpillClass {
    pub name: String,
    pub capacity: Option<u32>,
    pub base_alignment: u32,
    pub cost: u32,
    pub register_classes: Vec<RegClass>,
}

impl SpillClass {
    pub fn new(name: impl Into<String>, capacity: Option<u32>, cost: u32) -> Self {
        Self {
            name: name.into(),
            capacity,
            base_alignment: 1,
            cost,
            register_classes: Vec::new(),
        }
    }

    pub fn with_base_alignment(mut self, alignment: u32) -> Self {
        self.base_alignment = alignment;
        self
    }

    pub fn for_register_classes(mut self, classes: Vec<RegClass>) -> Self {
        self.register_classes = classes;
        self
    }
}

/// All register and spill information supplied by a source backend.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Target {
    pub units: Vec<RegisterUnit>,
    pub registers: Vec<PhysicalRegister>,
    pub register_classes: Vec<RegisterClass>,
    pub spill_classes: Vec<SpillClass>,
}

impl Target {
    /// Returns whether two physical register names cover any common unit.
    /// Invalid IDs return `false`; [`validate`] reports them in target data.
    pub fn registers_alias(&self, left: PhysReg, right: PhysReg) -> bool {
        let (Some(left), Some(right)) = (self.registers.get(left.0), self.registers.get(right.0))
        else {
            return false;
        };
        left.units.iter().any(|unit| right.units.contains(unit))
    }
}

/// A virtual value to place in a register or spill slot.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct VirtualRegister {
    pub name: String,
    pub size: u32,
    pub alignment: u32,
    pub class: RegClass,
    /// Always place this value in a spill slot, even if a register is available.
    pub must_spill: bool,
    /// Whether this value's spill slot may be shared with nonoverlapping values.
    /// Escaped addresses require a dedicated, non-reusable slot.
    pub spill_slot_reusable: bool,
    /// Empty means every compatible target spill class is allowed.
    pub spill_classes: Vec<SpillClassId>,
}

impl VirtualRegister {
    pub fn new(name: impl Into<String>, size: u32, alignment: u32, class: RegClass) -> Self {
        Self {
            name: name.into(),
            size,
            alignment,
            class,
            must_spill: false,
            spill_slot_reusable: true,
            spill_classes: Vec::new(),
        }
    }

    pub fn with_spill_classes(mut self, classes: Vec<SpillClassId>) -> Self {
        self.spill_classes = classes;
        self
    }

    pub fn with_must_spill(mut self, must_spill: bool) -> Self {
        self.must_spill = must_spill;
        self
    }

    pub fn with_spill_slot_reuse(mut self, reusable: bool) -> Self {
        self.spill_slot_reusable = reusable;
        self
    }
}

/// A virtual operand required to occupy a specific physical register.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FixedOperand {
    pub vreg: VReg,
    pub register: PhysReg,
}

impl FixedOperand {
    pub fn new(vreg: VReg, register: PhysReg) -> Self {
        Self { vreg, register }
    }
}

/// A copy relation used both as instruction semantics and as an allocation hint.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CopyOperand {
    pub from: VReg,
    pub to: VReg,
}

impl CopyOperand {
    pub fn new(from: VReg, to: VReg) -> Self {
        Self { from, to }
    }
}

/// Machine-level operand effects needed by the allocator.
///
/// Effects are ordered as uses, clobbers, then definitions. A value redefined
/// by an instruction may therefore use a clobbered register for its result.
/// Copy operands count as uses and definitions; callers do not need to repeat
/// them in `uses` and `defs`. Fixed operands likewise count as normal uses or
/// definitions in addition to imposing their physical-register constraint.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Instruction {
    pub uses: Vec<VReg>,
    pub defs: Vec<VReg>,
    pub fixed_uses: Vec<FixedOperand>,
    pub fixed_defs: Vec<FixedOperand>,
    pub clobbers: Vec<PhysReg>,
    pub copies: Vec<CopyOperand>,
}

impl Instruction {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_uses(mut self, uses: Vec<VReg>) -> Self {
        self.uses = uses;
        self
    }

    pub fn with_defs(mut self, defs: Vec<VReg>) -> Self {
        self.defs = defs;
        self
    }

    pub fn with_fixed_uses(mut self, uses: Vec<FixedOperand>) -> Self {
        self.fixed_uses = uses;
        self
    }

    pub fn with_fixed_defs(mut self, defs: Vec<FixedOperand>) -> Self {
        self.fixed_defs = defs;
        self
    }

    pub fn with_clobbers(mut self, clobbers: Vec<PhysReg>) -> Self {
        self.clobbers = clobbers;
        self
    }

    pub fn with_copies(mut self, copies: Vec<CopyOperand>) -> Self {
        self.copies = copies;
        self
    }

    fn all_uses(&self) -> impl Iterator<Item = VReg> + '_ {
        self.uses
            .iter()
            .copied()
            .chain(self.fixed_uses.iter().map(|operand| operand.vreg))
            .chain(self.copies.iter().map(|copy| copy.from))
    }

    fn all_defs(&self) -> impl Iterator<Item = VReg> + '_ {
        self.defs
            .iter()
            .copied()
            .chain(self.fixed_defs.iter().map(|operand| operand.vreg))
            .chain(self.copies.iter().map(|copy| copy.to))
    }
}

/// A basic block. Blocks and successors use their indices in [`Function::blocks`].
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct BasicBlock {
    pub instructions: Vec<Instruction>,
    pub successors: Vec<BlockId>,
}

impl BasicBlock {
    pub fn new(instructions: Vec<Instruction>, successors: Vec<BlockId>) -> Self {
        Self {
            instructions,
            successors,
        }
    }
}

/// Input to liveness and allocation.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Function {
    pub virtual_registers: Vec<VirtualRegister>,
    pub blocks: Vec<BasicBlock>,
}

impl Function {
    pub fn new(virtual_registers: Vec<VirtualRegister>, blocks: Vec<BasicBlock>) -> Self {
        Self {
            virtual_registers,
            blocks,
        }
    }
}

/// A stable diagnostic category suitable for backend tests or UI reporting.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DiagnosticCode {
    InvalidTarget,
    InvalidFunction,
    InvalidOperand,
    ConflictingFixedRegisters,
    RegisterConflict,
    SpillExhausted,
}

/// A validation or allocation failure. Public entry points return diagnostics
/// rather than panicking for malformed target or function data.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Diagnostic {
    pub code: DiagnosticCode,
    pub message: String,
}

impl Diagnostic {
    fn new(code: DiagnosticCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }
}

impl fmt::Display for Diagnostic {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{:?}: {}", self.code, self.message)
    }
}

/// Live-in and live-out sets for every block, in block order.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Liveness {
    pub live_in: Vec<BTreeSet<VReg>>,
    pub live_out: Vec<BTreeSet<VReg>>,
}

/// One conservative, inclusive live range.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LiveInterval {
    pub vreg: VReg,
    pub start: ProgramPoint,
    pub end: ProgramPoint,
}

impl LiveInterval {
    pub fn overlaps(self, other: Self) -> bool {
        self.start <= other.end && other.start <= self.end
    }
}

/// A physical or spilled location assigned to a virtual register.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Location {
    Register(PhysReg),
    Spill(usize),
    /// The virtual register never appears in an instruction or live set.
    Unused,
}

/// One spill slot. Offsets are relative to the target-defined spill area.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SpillSlot {
    pub class: SpillClassId,
    pub offset: u32,
    pub size: u32,
    pub alignment: u32,
    /// Whether another nonoverlapping reusable value may share this slot.
    pub reusable: bool,
    /// Values sharing this slot, ordered by interval start.
    pub values: Vec<VReg>,
}

/// Complete deterministic allocation output.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Allocation {
    pub locations: Vec<Location>,
    pub spill_slots: Vec<SpillSlot>,
    pub liveness: Liveness,
    pub intervals: Vec<LiveInterval>,
}

impl Allocation {
    pub fn location(&self, vreg: VReg) -> Option<Location> {
        self.locations.get(vreg.0).copied()
    }

    pub fn interval(&self, vreg: VReg) -> Option<LiveInterval> {
        self.intervals
            .iter()
            .find(|interval| interval.vreg == vreg)
            .copied()
    }
}

/// Validates all IDs, shapes, classes, alignments, and fixed constraints that
/// do not depend on liveness.
pub fn validate(target: &Target, function: &Function) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();

    let mut unit_names = BTreeSet::new();
    for (index, unit) in target.units.iter().enumerate() {
        if unit.name.is_empty() {
            diagnostics.push(Diagnostic::new(
                DiagnosticCode::InvalidTarget,
                format!("register unit {index} has an empty name"),
            ));
        }
        if !unit_names.insert(unit.name.as_str()) {
            diagnostics.push(Diagnostic::new(
                DiagnosticCode::InvalidTarget,
                format!("duplicate register unit name {:?}", unit.name),
            ));
        }
    }

    let mut register_names = BTreeSet::new();
    for (index, register) in target.registers.iter().enumerate() {
        if register.name.is_empty() {
            diagnostics.push(Diagnostic::new(
                DiagnosticCode::InvalidTarget,
                format!("physical register {index} has an empty name"),
            ));
        }
        if !register_names.insert(register.name.as_str()) {
            diagnostics.push(Diagnostic::new(
                DiagnosticCode::InvalidTarget,
                format!("duplicate physical register name {:?}", register.name),
            ));
        }
        if register.units.is_empty() {
            diagnostics.push(Diagnostic::new(
                DiagnosticCode::InvalidTarget,
                format!("physical register {:?} has no units", register.name),
            ));
        }
        let mut seen = BTreeSet::new();
        for unit in &register.units {
            if unit.0 >= target.units.len() {
                diagnostics.push(Diagnostic::new(
                    DiagnosticCode::InvalidTarget,
                    format!(
                        "physical register {:?} refers to unknown unit {}",
                        register.name, unit.0
                    ),
                ));
            } else if !seen.insert(*unit) {
                diagnostics.push(Diagnostic::new(
                    DiagnosticCode::InvalidTarget,
                    format!(
                        "physical register {:?} contains unit {} more than once",
                        register.name, unit.0
                    ),
                ));
            }
        }
    }

    let mut class_names = BTreeSet::new();
    for (index, class) in target.register_classes.iter().enumerate() {
        if class.name.is_empty() {
            diagnostics.push(Diagnostic::new(
                DiagnosticCode::InvalidTarget,
                format!("register class {index} has an empty name"),
            ));
        }
        if !class_names.insert(class.name.as_str()) {
            diagnostics.push(Diagnostic::new(
                DiagnosticCode::InvalidTarget,
                format!("duplicate register class name {:?}", class.name),
            ));
        }
        let mut seen = BTreeSet::new();
        for register in &class.registers {
            if register.0 >= target.registers.len() {
                diagnostics.push(Diagnostic::new(
                    DiagnosticCode::InvalidTarget,
                    format!(
                        "register class {:?} refers to unknown register {}",
                        class.name, register.0
                    ),
                ));
            } else if !seen.insert(*register) {
                diagnostics.push(Diagnostic::new(
                    DiagnosticCode::InvalidTarget,
                    format!(
                        "register class {:?} contains register {} more than once",
                        class.name, register.0
                    ),
                ));
            }
        }
    }

    let mut spill_names = BTreeSet::new();
    for (index, spill) in target.spill_classes.iter().enumerate() {
        if spill.name.is_empty() {
            diagnostics.push(Diagnostic::new(
                DiagnosticCode::InvalidTarget,
                format!("spill class {index} has an empty name"),
            ));
        }
        if !spill_names.insert(spill.name.as_str()) {
            diagnostics.push(Diagnostic::new(
                DiagnosticCode::InvalidTarget,
                format!("duplicate spill class name {:?}", spill.name),
            ));
        }
        if !is_power_of_two(spill.base_alignment) {
            diagnostics.push(Diagnostic::new(
                DiagnosticCode::InvalidTarget,
                format!(
                    "spill class {:?} has invalid base alignment {}",
                    spill.name, spill.base_alignment
                ),
            ));
        }
        let mut seen = BTreeSet::new();
        for class in &spill.register_classes {
            if class.0 >= target.register_classes.len() {
                diagnostics.push(Diagnostic::new(
                    DiagnosticCode::InvalidTarget,
                    format!(
                        "spill class {:?} refers to unknown register class {}",
                        spill.name, class.0
                    ),
                ));
            } else if !seen.insert(*class) {
                diagnostics.push(Diagnostic::new(
                    DiagnosticCode::InvalidTarget,
                    format!(
                        "spill class {:?} repeats register class {}",
                        spill.name, class.0
                    ),
                ));
            }
        }
    }

    for (index, vreg) in function.virtual_registers.iter().enumerate() {
        if vreg.size == 0 {
            diagnostics.push(Diagnostic::new(
                DiagnosticCode::InvalidFunction,
                format!("virtual register {index} has size zero"),
            ));
        }
        if !is_power_of_two(vreg.alignment) {
            diagnostics.push(Diagnostic::new(
                DiagnosticCode::InvalidFunction,
                format!(
                    "virtual register {index} has invalid alignment {}",
                    vreg.alignment
                ),
            ));
        }
        if vreg.class.0 >= target.register_classes.len() {
            diagnostics.push(Diagnostic::new(
                DiagnosticCode::InvalidFunction,
                format!(
                    "virtual register {index} refers to unknown register class {}",
                    vreg.class.0
                ),
            ));
        }
        let mut seen = BTreeSet::new();
        for spill in &vreg.spill_classes {
            if spill.0 >= target.spill_classes.len() {
                diagnostics.push(Diagnostic::new(
                    DiagnosticCode::InvalidFunction,
                    format!(
                        "virtual register {index} refers to unknown spill class {}",
                        spill.0
                    ),
                ));
            } else if !seen.insert(*spill) {
                diagnostics.push(Diagnostic::new(
                    DiagnosticCode::InvalidFunction,
                    format!("virtual register {index} repeats spill class {}", spill.0),
                ));
            }
        }
    }

    diagnostics.extend(validate_function_ids(target, function));
    diagnostics
}

fn validate_function_ids(target: &Target, function: &Function) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();
    for (block_index, block) in function.blocks.iter().enumerate() {
        let mut successors = BTreeSet::new();
        for successor in &block.successors {
            if successor.0 >= function.blocks.len() {
                diagnostics.push(Diagnostic::new(
                    DiagnosticCode::InvalidFunction,
                    format!(
                        "block {block_index} refers to unknown successor {}",
                        successor.0
                    ),
                ));
            } else if !successors.insert(*successor) {
                diagnostics.push(Diagnostic::new(
                    DiagnosticCode::InvalidFunction,
                    format!("block {block_index} repeats successor {}", successor.0),
                ));
            }
        }

        for (instruction_index, instruction) in block.instructions.iter().enumerate() {
            let context = format!("block {block_index}, instruction {instruction_index}");
            for vreg in instruction.all_uses().chain(instruction.all_defs()) {
                if vreg.0 >= function.virtual_registers.len() {
                    diagnostics.push(Diagnostic::new(
                        DiagnosticCode::InvalidOperand,
                        format!("{context} refers to unknown virtual register {}", vreg.0),
                    ));
                }
            }
            for operand in instruction
                .fixed_uses
                .iter()
                .chain(instruction.fixed_defs.iter())
            {
                if operand.register.0 >= target.registers.len() {
                    diagnostics.push(Diagnostic::new(
                        DiagnosticCode::InvalidOperand,
                        format!(
                            "{context} refers to unknown physical register {}",
                            operand.register.0
                        ),
                    ));
                    continue;
                }
                let Some(vreg) = function.virtual_registers.get(operand.vreg.0) else {
                    continue;
                };
                let Some(class) = target.register_classes.get(vreg.class.0) else {
                    continue;
                };
                if vreg.must_spill {
                    diagnostics.push(Diagnostic::new(
                        DiagnosticCode::InvalidOperand,
                        format!(
                            "{context} fixes virtual register {} to a physical register, but the value must spill",
                            operand.vreg.0
                        ),
                    ));
                }
                if !class.registers.contains(&operand.register) {
                    diagnostics.push(Diagnostic::new(
                        DiagnosticCode::InvalidOperand,
                        format!(
                            "{context} fixes virtual register {} to register {}, which is not in class {:?}",
                            operand.vreg.0, operand.register.0, class.name
                        ),
                    ));
                }
            }
            for register in &instruction.clobbers {
                if register.0 >= target.registers.len() {
                    diagnostics.push(Diagnostic::new(
                        DiagnosticCode::InvalidOperand,
                        format!(
                            "{context} clobbers unknown physical register {}",
                            register.0
                        ),
                    ));
                }
            }
        }
    }
    diagnostics
}

fn validate_function_shape(function: &Function) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();
    for (block_index, block) in function.blocks.iter().enumerate() {
        let mut successors = BTreeSet::new();
        for successor in &block.successors {
            if successor.0 >= function.blocks.len() {
                diagnostics.push(Diagnostic::new(
                    DiagnosticCode::InvalidFunction,
                    format!(
                        "block {block_index} refers to unknown successor {}",
                        successor.0
                    ),
                ));
            } else if !successors.insert(*successor) {
                diagnostics.push(Diagnostic::new(
                    DiagnosticCode::InvalidFunction,
                    format!("block {block_index} repeats successor {}", successor.0),
                ));
            }
        }
        for (instruction_index, instruction) in block.instructions.iter().enumerate() {
            let context = format!("block {block_index}, instruction {instruction_index}");
            for vreg in instruction.all_uses().chain(instruction.all_defs()) {
                if vreg.0 >= function.virtual_registers.len() {
                    diagnostics.push(Diagnostic::new(
                        DiagnosticCode::InvalidOperand,
                        format!("{context} refers to unknown virtual register {}", vreg.0),
                    ));
                }
            }
        }
    }
    diagnostics
}

/// Computes standard backward dataflow liveness for explicit CFG successors.
pub fn analyze_liveness(function: &Function) -> Result<Liveness, Vec<Diagnostic>> {
    let diagnostics = validate_function_shape(function);
    if !diagnostics.is_empty() {
        return Err(diagnostics);
    }

    let block_count = function.blocks.len();
    let mut uses = vec![BTreeSet::new(); block_count];
    let mut defs = vec![BTreeSet::new(); block_count];

    for (block_index, block) in function.blocks.iter().enumerate() {
        for instruction in &block.instructions {
            for vreg in instruction.all_uses() {
                if !defs[block_index].contains(&vreg) {
                    uses[block_index].insert(vreg);
                }
            }
            defs[block_index].extend(instruction.all_defs());
        }
    }

    let mut live_in = vec![BTreeSet::new(); block_count];
    let mut live_out = vec![BTreeSet::new(); block_count];
    loop {
        let mut changed = false;
        for block_index in (0..block_count).rev() {
            let mut new_out = BTreeSet::new();
            for successor in &function.blocks[block_index].successors {
                new_out.extend(live_in[successor.0].iter().copied());
            }
            let mut new_in = uses[block_index].clone();
            new_in.extend(new_out.difference(&defs[block_index]).copied());
            if new_out != live_out[block_index] || new_in != live_in[block_index] {
                live_out[block_index] = new_out;
                live_in[block_index] = new_in;
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }

    Ok(Liveness { live_in, live_out })
}

/// Builds one conservative interval per used virtual register.
pub fn build_live_intervals(
    function: &Function,
    liveness: &Liveness,
) -> Result<Vec<LiveInterval>, Vec<Diagnostic>> {
    let mut diagnostics = validate_function_shape(function);
    if liveness.live_in.len() != function.blocks.len()
        || liveness.live_out.len() != function.blocks.len()
    {
        diagnostics.push(Diagnostic::new(
            DiagnosticCode::InvalidFunction,
            "liveness block count does not match the function",
        ));
    } else {
        for (block_index, (live_in, live_out)) in
            liveness.live_in.iter().zip(&liveness.live_out).enumerate()
        {
            for vreg in live_in.union(live_out) {
                if vreg.0 >= function.virtual_registers.len() {
                    diagnostics.push(Diagnostic::new(
                        DiagnosticCode::InvalidOperand,
                        format!(
                            "liveness for block {block_index} refers to unknown virtual register {}",
                            vreg.0
                        ),
                    ));
                }
            }
        }
    }
    if !diagnostics.is_empty() {
        return Err(diagnostics);
    }

    let layout = instruction_layout(function)?;
    let mut bounds: Vec<Option<(ProgramPoint, ProgramPoint)>> =
        vec![None; function.virtual_registers.len()];

    for (block_index, block) in function.blocks.iter().enumerate() {
        let (block_start, block_end) = layout.block_ranges[block_index];
        for vreg in &liveness.live_in[block_index] {
            extend_interval(&mut bounds[vreg.0], block_start);
        }
        for (instruction_index, instruction) in block.instructions.iter().enumerate() {
            let (use_point, def_point) = layout.instruction_points[block_index][instruction_index];
            for vreg in instruction.all_uses() {
                extend_interval(&mut bounds[vreg.0], use_point);
            }
            for vreg in instruction.all_defs() {
                extend_interval(&mut bounds[vreg.0], def_point);
            }
        }
        for vreg in &liveness.live_out[block_index] {
            extend_interval(&mut bounds[vreg.0], block_end);
        }
    }

    Ok(bounds
        .into_iter()
        .enumerate()
        .filter_map(|(index, bounds)| {
            bounds.map(|(start, end)| LiveInterval {
                vreg: VReg(index),
                start,
                end,
            })
        })
        .collect())
}

fn extend_interval(bounds: &mut Option<(ProgramPoint, ProgramPoint)>, point: ProgramPoint) {
    match bounds {
        Some((start, end)) => {
            *start = (*start).min(point);
            *end = (*end).max(point);
        }
        None => *bounds = Some((point, point)),
    }
}

#[derive(Clone, Debug)]
struct InstructionLayout {
    instruction_points: Vec<Vec<(ProgramPoint, ProgramPoint)>>,
    block_ranges: Vec<(ProgramPoint, ProgramPoint)>,
}

fn instruction_layout(function: &Function) -> Result<InstructionLayout, Vec<Diagnostic>> {
    let instruction_count: usize = function
        .blocks
        .iter()
        .map(|block| block.instructions.len())
        .sum();
    let Some(max_point) = instruction_count.checked_mul(2) else {
        return Err(vec![Diagnostic::new(
            DiagnosticCode::InvalidFunction,
            "function has too many instructions",
        )]);
    };
    if max_point > u32::MAX as usize {
        return Err(vec![Diagnostic::new(
            DiagnosticCode::InvalidFunction,
            "function has too many instructions",
        )]);
    }

    let mut next = 0_u32;
    let mut instruction_points = Vec::with_capacity(function.blocks.len());
    let mut block_ranges = Vec::with_capacity(function.blocks.len());
    for block in &function.blocks {
        let block_start = ProgramPoint(next);
        let mut points = Vec::with_capacity(block.instructions.len());
        for _ in &block.instructions {
            points.push((ProgramPoint(next), ProgramPoint(next + 1)));
            next += 2;
        }
        let block_end = points
            .last()
            .map(|(_, def_point)| *def_point)
            .unwrap_or(block_start);
        instruction_points.push(points);
        block_ranges.push((block_start, block_end));
    }
    Ok(InstructionLayout {
        instruction_points,
        block_ranges,
    })
}

/// Allocates every used virtual register or returns all validation/allocation
/// diagnostics found. The result is deterministic for a fixed input order.
pub fn allocate(target: &Target, function: &Function) -> Result<Allocation, Vec<Diagnostic>> {
    let diagnostics = validate(target, function);
    if !diagnostics.is_empty() {
        return Err(diagnostics);
    }

    let liveness = analyze_liveness(function)?;
    let mut intervals = build_live_intervals(function, &liveness)?;
    intervals.sort_by_key(|interval| (interval.start, interval.vreg));
    let layout = instruction_layout(function)?;

    let mut interval_by_vreg = vec![None; function.virtual_registers.len()];
    for interval in &intervals {
        interval_by_vreg[interval.vreg.0] = Some(*interval);
    }

    let fixed = collect_fixed_constraints(target, function)?;
    let copy_preferences = collect_copy_preferences(function);
    let mut locations = vec![Location::Unused; function.virtual_registers.len()];
    let mut register_assignments: Vec<(LiveInterval, PhysReg)> = Vec::new();
    let mut spilled = Vec::new();

    for interval in &intervals {
        let vreg = &function.virtual_registers[interval.vreg.0];
        if vreg.must_spill {
            spilled.push(*interval);
            continue;
        }
        let class = &target.register_classes[vreg.class.0];
        let required = fixed.get(&interval.vreg).copied();
        let mut candidates = Vec::new();

        if let Some(register) = required {
            candidates.push(register);
        } else {
            for source in copy_preferences.get(&interval.vreg).into_iter().flatten() {
                let Some(source_interval) = interval_by_vreg[source.0] else {
                    continue;
                };
                let Location::Register(register) = locations[source.0] else {
                    continue;
                };
                if !interval.overlaps(source_interval)
                    && class.registers.contains(&register)
                    && !candidates.contains(&register)
                {
                    candidates.push(register);
                }
            }
            for register in &class.registers {
                if !candidates.contains(register) {
                    candidates.push(*register);
                }
            }
        }

        let chosen = candidates.into_iter().find(|candidate| {
            !register_is_clobbered_across(target, function, &layout, *interval, *candidate)
                && !register_assignments.iter().any(|(other, register)| {
                    interval.overlaps(*other) && target.registers_alias(*candidate, *register)
                })
                && !fixed.iter().any(|(other_vreg, other_register)| {
                    if *other_vreg == interval.vreg {
                        return false;
                    }
                    let Some(other_interval) = interval_by_vreg[other_vreg.0] else {
                        return false;
                    };
                    interval.overlaps(other_interval)
                        && target.registers_alias(*candidate, *other_register)
                })
        });

        match chosen {
            Some(register) => {
                locations[interval.vreg.0] = Location::Register(register);
                register_assignments.push((*interval, register));
            }
            None if required.is_some() => {
                return Err(vec![Diagnostic::new(
                    DiagnosticCode::RegisterConflict,
                    format!(
                        "fixed virtual register {} cannot occupy physical register {} over {:?}..{:?}",
                        interval.vreg.0,
                        required.unwrap().0,
                        interval.start,
                        interval.end
                    ),
                )]);
            }
            None => spilled.push(*interval),
        }
    }

    let (spill_slots, spill_locations, spill_diagnostics) =
        allocate_spill_slots(target, function, &spilled);
    if !spill_diagnostics.is_empty() {
        return Err(spill_diagnostics);
    }
    for (vreg, slot) in spill_locations {
        locations[vreg.0] = Location::Spill(slot);
    }

    intervals.sort_by_key(|interval| interval.vreg);
    Ok(Allocation {
        locations,
        spill_slots,
        liveness,
        intervals,
    })
}

fn collect_fixed_constraints(
    target: &Target,
    function: &Function,
) -> Result<BTreeMap<VReg, PhysReg>, Vec<Diagnostic>> {
    let mut fixed = BTreeMap::new();
    let mut diagnostics = Vec::new();
    for (block_index, block) in function.blocks.iter().enumerate() {
        for (instruction_index, instruction) in block.instructions.iter().enumerate() {
            for operand in instruction
                .fixed_uses
                .iter()
                .chain(instruction.fixed_defs.iter())
            {
                if let Some(previous) = fixed.insert(operand.vreg, operand.register) {
                    if previous != operand.register {
                        diagnostics.push(Diagnostic::new(
                            DiagnosticCode::ConflictingFixedRegisters,
                            format!(
                                "virtual register {} is fixed to both {:?} and {:?}; second constraint is at block {}, instruction {}",
                                operand.vreg.0,
                                target.registers[previous.0].name,
                                target.registers[operand.register.0].name,
                                block_index,
                                instruction_index
                            ),
                        ));
                    }
                }
            }
        }
    }
    if diagnostics.is_empty() {
        Ok(fixed)
    } else {
        Err(diagnostics)
    }
}

fn collect_copy_preferences(function: &Function) -> BTreeMap<VReg, Vec<VReg>> {
    let mut preferences: BTreeMap<VReg, Vec<VReg>> = BTreeMap::new();
    for block in &function.blocks {
        for instruction in &block.instructions {
            for copy in &instruction.copies {
                let sources = preferences.entry(copy.to).or_default();
                if !sources.contains(&copy.from) {
                    sources.push(copy.from);
                }
            }
        }
    }
    preferences
}

fn register_is_clobbered_across(
    target: &Target,
    function: &Function,
    layout: &InstructionLayout,
    interval: LiveInterval,
    candidate: PhysReg,
) -> bool {
    for (block_index, block) in function.blocks.iter().enumerate() {
        for (instruction_index, instruction) in block.instructions.iter().enumerate() {
            if instruction.clobbers.is_empty() {
                continue;
            }
            let (use_point, def_point) = layout.instruction_points[block_index][instruction_index];
            let used = instruction.all_uses().any(|vreg| vreg == interval.vreg);
            let defined = instruction.all_defs().any(|vreg| vreg == interval.vreg);
            if interval.start <= use_point
                && interval.end >= def_point
                && !(defined && !used)
                && instruction
                    .clobbers
                    .iter()
                    .any(|clobber| target.registers_alias(candidate, *clobber))
            {
                return true;
            }
        }
    }
    false
}

fn allocate_spill_slots(
    target: &Target,
    function: &Function,
    spilled: &[LiveInterval],
) -> (Vec<SpillSlot>, BTreeMap<VReg, usize>, Vec<Diagnostic>) {
    let mut slots: Vec<SpillSlot> = Vec::new();
    let mut slot_last_end: Vec<ProgramPoint> = Vec::new();
    let mut cursors = vec![0_u32; target.spill_classes.len()];
    let mut locations = BTreeMap::new();
    let mut diagnostics = Vec::new();

    for interval in spilled {
        let vreg = &function.virtual_registers[interval.vreg.0];
        let mut classes: Vec<SpillClassId> = target
            .spill_classes
            .iter()
            .enumerate()
            .filter_map(|(index, class)| {
                let id = SpillClassId(index);
                let allowed_by_value =
                    vreg.spill_classes.is_empty() || vreg.spill_classes.contains(&id);
                let supports_class = class.register_classes.is_empty()
                    || class.register_classes.contains(&vreg.class);
                (allowed_by_value && supports_class).then_some(id)
            })
            .collect();
        classes.sort_by_key(|id| (target.spill_classes[id.0].cost, id.0));

        let mut assigned = None;
        for class_id in classes {
            let spill_class = &target.spill_classes[class_id.0];
            let alignment = vreg.alignment.max(spill_class.base_alignment);

            if let Some((slot_index, slot)) = slots.iter_mut().enumerate().find(|(index, slot)| {
                vreg.spill_slot_reusable
                    && slot.reusable
                    && slot.class == class_id
                    && slot_last_end[*index] < interval.start
                    && slot.size >= vreg.size
                    && slot.offset % alignment == 0
            }) {
                slot.alignment = slot.alignment.max(alignment);
                slot.values.push(interval.vreg);
                slot_last_end[slot_index] = interval.end;
                assigned = Some(slot_index);
                break;
            }

            let Some(offset) = align_up(cursors[class_id.0], alignment) else {
                continue;
            };
            let Some(end) = offset.checked_add(vreg.size) else {
                continue;
            };
            if spill_class.capacity.is_some_and(|capacity| end > capacity) {
                continue;
            }
            let slot_index = slots.len();
            slots.push(SpillSlot {
                class: class_id,
                offset,
                size: vreg.size,
                alignment,
                reusable: vreg.spill_slot_reusable,
                values: vec![interval.vreg],
            });
            slot_last_end.push(interval.end);
            cursors[class_id.0] = end;
            assigned = Some(slot_index);
            break;
        }

        match assigned {
            Some(slot) => {
                locations.insert(interval.vreg, slot);
            }
            None => diagnostics.push(Diagnostic::new(
                DiagnosticCode::SpillExhausted,
                format!(
                    "no allowed spill class has {} bytes aligned to {} for virtual register {}",
                    vreg.size, vreg.alignment, interval.vreg.0
                ),
            )),
        }
    }

    (slots, locations, diagnostics)
}

fn is_power_of_two(value: u32) -> bool {
    value != 0 && value.is_power_of_two()
}

fn align_up(value: u32, alignment: u32) -> Option<u32> {
    debug_assert!(is_power_of_two(alignment));
    value
        .checked_add(alignment - 1)
        .map(|value| value & !(alignment - 1))
}

#[cfg(test)]
mod tests;
