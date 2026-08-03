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
    let preserves_carry = CostCandidate::new("preserve-carry", cost(2, 4, 0, FlagEffects::none()));
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
