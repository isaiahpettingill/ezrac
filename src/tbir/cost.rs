//! Target-independent costs for choosing equivalent low-level operations.
//!
//! A target-specific lowering pass supplies the measured cost of each candidate;
//! this module only compares those candidates. It does not prove that two forms
//! are equivalent, and callers must account for volatile accesses and required
//! flag results before presenting candidates here.

use core::cmp::Ordering;

/// A set of architectural flags used by an operation.
///
/// The set is intentionally not tied to one CPU. Targets can use the common
/// names below or reserve bits from [`FlagSet::from_bits`] for other flags.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub struct FlagSet(u16);

impl FlagSet {
    pub const NONE: Self = Self(0);
    pub const CARRY: Self = Self(1 << 0);
    pub const ZERO: Self = Self(1 << 1);
    pub const NEGATIVE: Self = Self(1 << 2);
    pub const OVERFLOW: Self = Self(1 << 3);
    pub const HALF_CARRY: Self = Self(1 << 4);
    pub const SIGN: Self = Self(1 << 5);
    pub const PARITY: Self = Self(1 << 6);
    pub const AUXILIARY_CARRY: Self = Self(1 << 7);
    pub const ALL: Self = Self(u16::MAX);

    pub const fn from_bits(bits: u16) -> Self {
        Self(bits)
    }

    pub const fn bits(self) -> u16 {
        self.0
    }

    pub const fn contains(self, flag: Self) -> bool {
        self.intersection(flag).0 == flag.0
    }

    pub const fn is_empty(self) -> bool {
        self.0 == 0
    }

    pub const fn union(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }

    pub const fn intersection(self, other: Self) -> Self {
        Self(self.0 & other.0)
    }

    pub const fn difference(self, other: Self) -> Self {
        Self(self.0 & !other.0)
    }

    pub fn count(self) -> u32 {
        self.0.count_ones()
    }
}

/// Flags read and written by an operation.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct FlagEffects {
    pub reads: FlagSet,
    pub writes: FlagSet,
}

impl FlagEffects {
    pub const fn new(reads: FlagSet, writes: FlagSet) -> Self {
        Self { reads, writes }
    }

    pub const fn none() -> Self {
        Self::new(FlagSet::NONE, FlagSet::NONE)
    }

    pub const fn reads(flags: FlagSet) -> Self {
        Self::new(flags, FlagSet::NONE)
    }

    pub const fn writes(flags: FlagSet) -> Self {
        Self::new(FlagSet::NONE, flags)
    }

    pub const fn merge(self, other: Self) -> Self {
        Self::new(
            self.reads.union(other.reads),
            self.writes.union(other.writes),
        )
    }

    pub const fn clobbers(self, flags: FlagSet) -> FlagSet {
        self.writes.intersection(flags)
    }
}

/// Aggregate cost for one candidate operation form.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct InstructionCost {
    pub bytes: u16,
    pub cycles: u32,
    pub temporaries: u8,
    pub flags: FlagEffects,
}

impl InstructionCost {
    pub const fn new(bytes: u16, cycles: u32, temporaries: u8, flags: FlagEffects) -> Self {
        Self {
            bytes,
            cycles,
            temporaries,
            flags,
        }
    }

    pub const fn with_cycles(self, cycles: u32) -> Self {
        Self { cycles, ..self }
    }
}

/// A named operation form supplied by a lowering pass.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CostCandidate<'a> {
    pub name: &'a str,
    pub cost: InstructionCost,
}

impl<'a> CostCandidate<'a> {
    pub const fn new(name: &'a str, cost: InstructionCost) -> Self {
        Self { name, cost }
    }
}

/// Weights and flag liveness used to rank candidates.
///
/// `live_flags` identifies flags that must survive the operation. Flag reads
/// are also counted because they add a dependency on the incoming flag state.
/// The default keeps all known flags live, which is conservative; a caller that
/// has proved that no flags are observable can set this to [`FlagSet::NONE`].
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CostModel {
    pub bytes_weight: u32,
    pub cycles_weight: u32,
    pub temporaries_weight: u32,
    pub flags_weight: u32,
    pub live_flags: FlagSet,
}

impl CostModel {
    pub const fn new(
        bytes_weight: u32,
        cycles_weight: u32,
        temporaries_weight: u32,
        flags_weight: u32,
        live_flags: FlagSet,
    ) -> Self {
        Self {
            bytes_weight,
            cycles_weight,
            temporaries_weight,
            flags_weight,
            live_flags,
        }
    }

    /// A balanced ranking for code size and execution time.
    pub const fn balanced() -> Self {
        Self::new(1, 1, 2, 2, FlagSet::ALL)
    }

    /// Prefer smaller forms while retaining cycle and resource costs.
    pub const fn code_size() -> Self {
        Self::new(4, 1, 2, 2, FlagSet::ALL)
    }

    /// Prefer faster forms while retaining code size and resource costs.
    pub const fn speed() -> Self {
        Self::new(1, 4, 2, 2, FlagSet::ALL)
    }

    pub const fn with_live_flags(self, live_flags: FlagSet) -> Self {
        Self { live_flags, ..self }
    }

    pub fn score(&self, cost: InstructionCost) -> CostScore {
        let bytes = u32::from(cost.bytes);
        let cycles = cost.cycles;
        let temporaries = u32::from(cost.temporaries);
        let flag_clobbers = cost.flags.clobbers(self.live_flags).count();
        let flag_reads = cost.flags.reads.count();
        let flag_units = flag_clobbers.saturating_add(flag_reads);
        let weighted = u64::from(bytes)
            .saturating_mul(u64::from(self.bytes_weight))
            .saturating_add(u64::from(cycles).saturating_mul(u64::from(self.cycles_weight)))
            .saturating_add(
                u64::from(temporaries).saturating_mul(u64::from(self.temporaries_weight)),
            )
            .saturating_add(u64::from(flag_units).saturating_mul(u64::from(self.flags_weight)));
        CostScore {
            weighted,
            bytes,
            cycles,
            temporaries,
            flag_clobbers,
            flag_reads,
        }
    }

    pub fn compare(&self, left: &CostCandidate<'_>, right: &CostCandidate<'_>) -> Ordering {
        self.compare_costs(left.cost, right.cost)
    }

    pub fn compare_costs(&self, left: InstructionCost, right: InstructionCost) -> Ordering {
        self.score(left).cmp(&self.score(right))
    }

    pub fn choose_index(&self, candidates: &[CostCandidate<'_>]) -> Option<usize> {
        let mut best = None;
        for (index, candidate) in candidates.iter().enumerate() {
            if best.is_none_or(|best_index| {
                self.compare(candidate, &candidates[best_index]) == Ordering::Less
            }) {
                best = Some(index);
            }
        }
        best
    }

    pub fn choose<'a, 'name>(
        &self,
        candidates: &'a [CostCandidate<'name>],
    ) -> Option<&'a CostCandidate<'name>> {
        self.choose_index(candidates)
            .map(|index| &candidates[index])
    }

    pub fn choose_pair(
        &self,
        first: &CostCandidate<'_>,
        second: &CostCandidate<'_>,
    ) -> CandidateChoice {
        match self.compare(first, second) {
            Ordering::Less => CandidateChoice::First,
            Ordering::Greater => CandidateChoice::Second,
            Ordering::Equal => CandidateChoice::Tie,
        }
    }
}

impl Default for CostModel {
    fn default() -> Self {
        Self::balanced()
    }
}

/// The ranked result for a pair of candidates.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CandidateChoice {
    First,
    Second,
    Tie,
}

impl CandidateChoice {
    pub const fn is_tie(self) -> bool {
        matches!(self, Self::Tie)
    }
}

/// The two cycle counts of a conditional branch.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BranchTiming {
    pub not_taken_cycles: u32,
    pub taken_cycles: u32,
}

impl BranchTiming {
    pub const fn new(not_taken_cycles: u32, taken_cycles: u32) -> Self {
        Self {
            not_taken_cycles,
            taken_cycles,
        }
    }

    fn cycles_for(self, probability: BranchProbability) -> u32 {
        match probability.taken_percent() {
            None => self.not_taken_cycles.max(self.taken_cycles),
            Some(taken_percent) => {
                let not_taken_percent = 100 - u32::from(taken_percent);
                let weighted = u64::from(self.not_taken_cycles)
                    .saturating_mul(u64::from(not_taken_percent))
                    .saturating_add(
                        u64::from(self.taken_cycles).saturating_mul(u64::from(taken_percent)),
                    );
                let rounded = weighted.saturating_add(99) / 100;
                if rounded > u64::from(u32::MAX) {
                    u32::MAX
                } else {
                    rounded as u32
                }
            }
        }
    }
}

/// A branch candidate with separate taken and not-taken timings.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BranchCandidate<'a> {
    pub candidate: CostCandidate<'a>,
    pub timing: BranchTiming,
}

impl<'a> BranchCandidate<'a> {
    pub const fn new(candidate: CostCandidate<'a>, timing: BranchTiming) -> Self {
        Self { candidate, timing }
    }

    pub fn cost_for(self, probability: BranchProbability) -> InstructionCost {
        self.candidate
            .cost
            .with_cycles(self.timing.cycles_for(probability))
    }
}

/// A proven or unknown taken probability for a conditional branch.
///
/// An unknown probability deliberately uses the branch's worst-case timing in
/// [`choose_branch_vs_branchless`]. This avoids assuming that a branch is cheap
/// merely because its not-taken path is cheap.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BranchProbability(Option<u8>);

impl BranchProbability {
    pub const UNKNOWN: Self = Self(None);

    pub const fn known_taken_percent(percent: u8) -> Option<Self> {
        if percent <= 100 {
            Some(Self(Some(percent)))
        } else {
            None
        }
    }

    pub const fn taken_percent(self) -> Option<u8> {
        self.0
    }
}

/// Compare a constant-shift form with an add form without assuming that a
/// power-of-two operation is always cheaper as a shift.
pub fn choose_shift_vs_add(
    model: &CostModel,
    shift: &CostCandidate<'_>,
    add: &CostCandidate<'_>,
) -> CandidateChoice {
    model.choose_pair(shift, add)
}

/// Compare a conditional branch with a branchless form.
///
/// With [`BranchProbability::UNKNOWN`], the branch uses its worst-case cycle
/// count. A known probability may be used only when it comes from a sound
/// range/profile fact; the result rounds expected cycles up to stay integral.
pub fn choose_branch_vs_branchless(
    model: &CostModel,
    branch: &BranchCandidate<'_>,
    branchless: &CostCandidate<'_>,
    probability: BranchProbability,
) -> CandidateChoice {
    let branch_cost = branch.cost_for(probability);
    model
        .compare_costs(branch_cost, branchless.cost)
        .then_choice()
}

trait OrderingChoice {
    fn then_choice(self) -> CandidateChoice;
}

impl OrderingChoice for Ordering {
    fn then_choice(self) -> CandidateChoice {
        match self {
            Self::Less => CandidateChoice::First,
            Self::Greater => CandidateChoice::Second,
            Self::Equal => CandidateChoice::Tie,
        }
    }
}

/// The score used by [`CostModel`] to rank one candidate.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CostScore {
    pub weighted: u64,
    pub bytes: u32,
    pub cycles: u32,
    pub temporaries: u32,
    pub flag_clobbers: u32,
    pub flag_reads: u32,
}

impl Ord for CostScore {
    fn cmp(&self, other: &Self) -> Ordering {
        (
            self.weighted,
            self.bytes,
            self.cycles,
            self.temporaries,
            self.flag_clobbers,
            self.flag_reads,
        )
            .cmp(&(
                other.weighted,
                other.bytes,
                other.cycles,
                other.temporaries,
                other.flag_clobbers,
                other.flag_reads,
            ))
    }
}

impl PartialOrd for CostScore {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cost(bytes: u16, cycles: u32, temporaries: u8, flags: FlagEffects) -> InstructionCost {
        InstructionCost::new(bytes, cycles, temporaries, flags)
    }

    #[test]
    fn models_instruction_resources_and_flag_effects() {
        let effects = FlagEffects::new(FlagSet::CARRY, FlagSet::ZERO.union(FlagSet::NEGATIVE));
        let instruction = cost(3, 11, 2, effects);

        assert_eq!(instruction.bytes, 3);
        assert_eq!(instruction.cycles, 11);
        assert_eq!(instruction.temporaries, 2);
        assert_eq!(instruction.flags, effects);
        assert_eq!(effects.clobbers(FlagSet::ALL).count(), 2);
        assert!(effects.reads.contains(FlagSet::CARRY));
    }

    #[test]
    fn ranks_candidates_with_configurable_size_and_speed_preferences() {
        let shift = CostCandidate::new("shift", cost(2, 8, 0, FlagEffects::writes(FlagSet::ZERO)));
        let add = CostCandidate::new("add", cost(3, 5, 1, FlagEffects::writes(FlagSet::ZERO)));
        let candidates = [shift, add];

        assert_eq!(
            CostModel::code_size().choose(&candidates).unwrap().name,
            "shift"
        );
        assert_eq!(CostModel::speed().choose(&candidates).unwrap().name, "add");
    }

    #[test]
    fn live_flag_pressure_breaks_an_otherwise_equal_cost() {
        let preserves_carry =
            CostCandidate::new("preserve-carry", cost(2, 4, 0, FlagEffects::none()));
        let clobbers_carry = CostCandidate::new(
            "clobber-carry",
            cost(2, 4, 0, FlagEffects::writes(FlagSet::CARRY)),
        );
        let model = CostModel::new(1, 1, 1, 10, FlagSet::CARRY);

        assert_eq!(
            model.choose_pair(&preserves_carry, &clobbers_carry),
            CandidateChoice::First
        );
        assert_eq!(model.score(clobbers_carry.cost).flag_clobbers, 1);
    }

    #[test]
    fn chooses_the_first_candidate_on_a_complete_tie() {
        let first = CostCandidate::new("first", InstructionCost::default());
        let second = CostCandidate::new("second", InstructionCost::default());
        let candidates = [first, second];
        let model = CostModel::default();

        assert_eq!(model.choose_index(&candidates), Some(0));
        assert_eq!(model.choose_pair(&first, &second), CandidateChoice::Tie);
        assert_eq!(model.choose_index(&[]), None);
    }

    #[test]
    fn shift_vs_add_does_not_force_a_shift() {
        let shift = CostCandidate::new("shift", cost(2, 8, 0, FlagEffects::none()));
        let add = CostCandidate::new("add", cost(3, 5, 0, FlagEffects::none()));

        assert_eq!(
            choose_shift_vs_add(&CostModel::speed(), &shift, &add),
            CandidateChoice::Second
        );
        assert_eq!(
            choose_shift_vs_add(&CostModel::code_size(), &shift, &add),
            CandidateChoice::First
        );
    }

    #[test]
    fn unknown_branch_probability_uses_worst_case_cycles() {
        let branch = BranchCandidate::new(
            CostCandidate::new("branch", cost(2, 0, 0, FlagEffects::none())),
            BranchTiming::new(2, 20),
        );
        let branchless = CostCandidate::new("branchless", cost(5, 8, 1, FlagEffects::none()));
        let model = CostModel::new(1, 1, 1, 0, FlagSet::NONE);

        assert_eq!(
            choose_branch_vs_branchless(&model, &branch, &branchless, BranchProbability::UNKNOWN,),
            CandidateChoice::Second
        );
        assert_eq!(
            choose_branch_vs_branchless(
                &model,
                &branch,
                &branchless,
                BranchProbability::known_taken_percent(0).unwrap(),
            ),
            CandidateChoice::First
        );
    }

    #[test]
    fn known_branch_probability_rounds_expected_cycles_up() {
        let timing = BranchTiming::new(3, 10);

        assert_eq!(
            timing.cycles_for(BranchProbability::known_taken_percent(50).unwrap()),
            7
        );
        assert_eq!(BranchProbability::known_taken_percent(101), None);
    }
}
