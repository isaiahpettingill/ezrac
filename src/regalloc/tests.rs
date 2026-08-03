    use super::*;

    fn unit(name: &str) -> RegisterUnit {
        RegisterUnit::new(name)
    }

    fn vreg(name: &str, class: usize) -> VirtualRegister {
        VirtualRegister::new(name, 1, 1, RegClass(class))
    }

    fn static_spills() -> Vec<SpillClass> {
        vec![SpillClass::new("static", Some(64), 10)]
    }

    fn z80_target() -> Target {
        Target {
            units: vec![unit("H"), unit("L"), unit("D"), unit("E")],
            registers: vec![
                PhysicalRegister::new("HL", vec![RegUnit(0), RegUnit(1)]),
                PhysicalRegister::new("H", vec![RegUnit(0)]),
                PhysicalRegister::new("L", vec![RegUnit(1)]),
                PhysicalRegister::new("DE", vec![RegUnit(2), RegUnit(3)]),
                PhysicalRegister::new("D", vec![RegUnit(2)]),
            ],
            register_classes: vec![
                RegisterClass::new("pair", vec![PhysReg(0), PhysReg(3)]),
                RegisterClass::new("byte", vec![PhysReg(1), PhysReg(2), PhysReg(4)]),
            ],
            spill_classes: static_spills(),
        }
    }

    fn x86_target() -> Target {
        Target {
            units: vec![unit("AL"), unit("AH"), unit("BL"), unit("BH")],
            registers: vec![
                PhysicalRegister::new("AX", vec![RegUnit(0), RegUnit(1)]),
                PhysicalRegister::new("AL", vec![RegUnit(0)]),
                PhysicalRegister::new("AH", vec![RegUnit(1)]),
                PhysicalRegister::new("BX", vec![RegUnit(2), RegUnit(3)]),
            ],
            register_classes: vec![
                RegisterClass::new("word", vec![PhysReg(0), PhysReg(3)]),
                RegisterClass::new("byte", vec![PhysReg(1), PhysReg(2)]),
            ],
            spill_classes: static_spills(),
        }
    }

    #[test]
    fn hl_excludes_h_and_l_but_not_disjoint_registers() {
        let target = z80_target();
        assert!(target.registers_alias(PhysReg(0), PhysReg(1)));
        assert!(target.registers_alias(PhysReg(0), PhysReg(2)));
        assert!(!target.registers_alias(PhysReg(1), PhysReg(2)));
        assert!(!target.registers_alias(PhysReg(0), PhysReg(3)));

        let function = Function::new(
            vec![vreg("pair", 0), vreg("byte", 1)],
            vec![BasicBlock::new(
                vec![
                    Instruction::new().with_defs(vec![VReg(0)]),
                    Instruction::new().with_defs(vec![VReg(1)]),
                    Instruction::new().with_uses(vec![VReg(0), VReg(1)]),
                ],
                vec![],
            )],
        );
        let allocation = allocate(&target, &function).unwrap();
        assert_eq!(
            allocation.location(VReg(0)),
            Some(Location::Register(PhysReg(0)))
        );
        assert_eq!(
            allocation.location(VReg(1)),
            Some(Location::Register(PhysReg(4)))
        );
    }

    #[test]
    fn ax_excludes_al_and_ah() {
        let target = x86_target();
        assert!(target.registers_alias(PhysReg(0), PhysReg(1)));
        assert!(target.registers_alias(PhysReg(0), PhysReg(2)));
        assert!(!target.registers_alias(PhysReg(1), PhysReg(2)));

        let function = Function::new(
            vec![vreg("word", 0), vreg("byte", 1)],
            vec![BasicBlock::new(
                vec![
                    Instruction::new().with_defs(vec![VReg(0)]),
                    Instruction::new().with_defs(vec![VReg(1)]),
                    Instruction::new().with_uses(vec![VReg(0), VReg(1)]),
                ],
                vec![],
            )],
        );
        let allocation = allocate(&target, &function).unwrap();
        assert_eq!(
            allocation.location(VReg(0)),
            Some(Location::Register(PhysReg(0)))
        );
        assert_eq!(allocation.location(VReg(1)), Some(Location::Spill(0)));
    }

    #[test]
    fn fixed_operand_reserves_aliased_register_before_it_starts() {
        let target = x86_target();
        let function = Function::new(
            vec![vreg("ordinary", 0), vreg("fixed", 0)],
            vec![BasicBlock::new(
                vec![
                    Instruction::new().with_defs(vec![VReg(0)]),
                    Instruction::new()
                        .with_fixed_defs(vec![FixedOperand::new(VReg(1), PhysReg(0))]),
                    Instruction::new().with_uses(vec![VReg(0), VReg(1)]),
                ],
                vec![],
            )],
        );
        let allocation = allocate(&target, &function).unwrap();
        assert_eq!(
            allocation.location(VReg(1)),
            Some(Location::Register(PhysReg(0)))
        );
        assert_eq!(
            allocation.location(VReg(0)),
            Some(Location::Register(PhysReg(3)))
        );
    }

    #[test]
    fn forced_spill_with_a_fixed_operand_is_a_diagnostic() {
        let target = x86_target();
        let function = Function::new(
            vec![vreg("memory", 0).with_must_spill(true)],
            vec![BasicBlock::new(
                vec![
                    Instruction::new()
                        .with_fixed_defs(vec![FixedOperand::new(VReg(0), PhysReg(0))]),
                ],
                vec![],
            )],
        );
        let errors = allocate(&target, &function).unwrap_err();
        assert!(errors.iter().any(|error| {
            error.code == DiagnosticCode::InvalidOperand && error.message.contains("must spill")
        }));
    }

    #[test]
    fn conflicting_fixed_operands_are_diagnostics() {
        let target = x86_target();
        let function = Function::new(
            vec![vreg("fixed", 0)],
            vec![BasicBlock::new(
                vec![
                    Instruction::new()
                        .with_fixed_defs(vec![FixedOperand::new(VReg(0), PhysReg(0))]),
                    Instruction::new()
                        .with_fixed_uses(vec![FixedOperand::new(VReg(0), PhysReg(3))]),
                ],
                vec![],
            )],
        );
        let errors = allocate(&target, &function).unwrap_err();
        assert!(
            errors
                .iter()
                .any(|error| error.code == DiagnosticCode::ConflictingFixedRegisters)
        );
    }

    #[test]
    fn cfg_liveness_and_intervals_conservatively_cover_layout_holes() {
        let target = Target {
            units: vec![unit("R0"), unit("R1")],
            registers: vec![
                PhysicalRegister::new("R0", vec![RegUnit(0)]),
                PhysicalRegister::new("R1", vec![RegUnit(1)]),
            ],
            register_classes: vec![RegisterClass::new("general", vec![PhysReg(0), PhysReg(1)])],
            spill_classes: static_spills(),
        };
        // v0 is defined in the entry and used only in block 3. Block 1 is a
        // separate branch in layout order, so the conservative interval spans it.
        let function = Function::new(
            vec![vreg("across", 0), vreg("branch-local", 0)],
            vec![
                BasicBlock::new(
                    vec![Instruction::new().with_defs(vec![VReg(0)])],
                    vec![BlockId(1), BlockId(2)],
                ),
                BasicBlock::new(
                    vec![
                        Instruction::new().with_defs(vec![VReg(1)]),
                        Instruction::new().with_uses(vec![VReg(1)]),
                    ],
                    vec![BlockId(3)],
                ),
                BasicBlock::new(vec![], vec![BlockId(3)]),
                BasicBlock::new(vec![Instruction::new().with_uses(vec![VReg(0)])], vec![]),
            ],
        );
        let allocation = allocate(&target, &function).unwrap();
        assert!(allocation.liveness.live_out[0].contains(&VReg(0)));
        let across = allocation.intervals[VReg(0).0];
        let local = allocation.intervals[VReg(1).0];
        assert!(across.overlaps(local));
        assert_ne!(allocation.location(VReg(0)), allocation.location(VReg(1)));
    }

    #[test]
    fn must_spill_forces_memory_when_a_register_is_available() {
        let target = Target {
            units: vec![unit("R")],
            registers: vec![PhysicalRegister::new("R", vec![RegUnit(0)])],
            register_classes: vec![RegisterClass::new("general", vec![PhysReg(0)])],
            spill_classes: static_spills(),
        };
        let function = Function::new(
            vec![vreg("forced", 0).with_must_spill(true)],
            vec![BasicBlock::new(
                vec![
                    Instruction::new().with_defs(vec![VReg(0)]),
                    Instruction::new().with_uses(vec![VReg(0)]),
                ],
                vec![],
            )],
        );
        let allocation = allocate(&target, &function).unwrap();
        assert_eq!(allocation.location(VReg(0)), Some(Location::Spill(0)));
    }

    #[test]
    fn empty_register_class_allocates_memory_only_values() {
        let target = Target {
            units: vec![],
            registers: vec![],
            register_classes: vec![RegisterClass::new("memory", vec![])],
            spill_classes: static_spills(),
        };
        let function = Function::new(
            vec![vreg("memory", 0)],
            vec![BasicBlock::new(
                vec![
                    Instruction::new().with_defs(vec![VReg(0)]),
                    Instruction::new().with_uses(vec![VReg(0)]),
                ],
                vec![],
            )],
        );
        let allocation = allocate(&target, &function).unwrap();
        assert_eq!(allocation.location(VReg(0)), Some(Location::Spill(0)));
        assert_eq!(allocation.spill_slots[0].values, vec![VReg(0)]);
    }

    #[test]
    fn nonoverlapping_spills_reuse_a_slot_with_size_and_alignment() {
        let target = Target {
            units: vec![unit("R")],
            registers: vec![PhysicalRegister::new("R", vec![RegUnit(0)])],
            register_classes: vec![RegisterClass::new("general", vec![PhysReg(0)])],
            spill_classes: vec![
                SpillClass::new("zero-page", Some(2), 1),
                SpillClass::new("stack", Some(64), 5).with_base_alignment(2),
            ],
        };
        let mut first = VirtualRegister::new("first", 2, 1, RegClass(0));
        first.spill_classes = vec![SpillClassId(0), SpillClassId(1)];
        let second = VirtualRegister::new("second", 1, 2, RegClass(0));
        let function = Function::new(
            vec![vreg("occupy", 0), first, second],
            vec![BasicBlock::new(
                vec![
                    Instruction::new().with_defs(vec![VReg(0), VReg(1)]),
                    Instruction::new().with_uses(vec![VReg(0), VReg(1)]),
                    Instruction::new().with_defs(vec![VReg(2)]),
                    Instruction::new().with_uses(vec![VReg(0), VReg(2)]),
                ],
                vec![],
            )],
        );
        let allocation = allocate(&target, &function).unwrap();
        assert_eq!(allocation.location(VReg(1)), Some(Location::Spill(0)));
        assert_eq!(allocation.location(VReg(2)), Some(Location::Spill(0)));
        assert_eq!(allocation.spill_slots.len(), 1);
        assert_eq!(allocation.spill_slots[0].class, SpillClassId(0));
        assert_eq!(allocation.spill_slots[0].size, 2);
        assert_eq!(allocation.spill_slots[0].alignment, 2);
        assert!(allocation.spill_slots[0].reusable);
        assert_eq!(allocation.spill_slots[0].values, vec![VReg(1), VReg(2)]);
    }

    #[test]
    fn non_reusable_spill_gets_a_dedicated_slot() {
        let target = Target {
            units: vec![],
            registers: vec![],
            register_classes: vec![RegisterClass::new("memory", vec![])],
            spill_classes: static_spills(),
        };
        let escaped = vreg("escaped", 0).with_spill_slot_reuse(false);
        let function = Function::new(
            vec![escaped, vreg("later", 0)],
            vec![BasicBlock::new(
                vec![
                    Instruction::new().with_defs(vec![VReg(0)]),
                    Instruction::new().with_uses(vec![VReg(0)]),
                    Instruction::new().with_defs(vec![VReg(1)]),
                    Instruction::new().with_uses(vec![VReg(1)]),
                ],
                vec![],
            )],
        );
        let allocation = allocate(&target, &function).unwrap();

        assert_eq!(allocation.location(VReg(0)), Some(Location::Spill(0)));
        assert_eq!(allocation.location(VReg(1)), Some(Location::Spill(1)));
        assert!(!allocation.spill_slots[0].reusable);
        assert_eq!(allocation.spill_slots[0].values, vec![VReg(0)]);
    }

    #[test]
    fn overlapping_spills_use_capacity_then_fall_back_by_cost() {
        let target = Target {
            units: vec![unit("R")],
            registers: vec![PhysicalRegister::new("R", vec![RegUnit(0)])],
            register_classes: vec![RegisterClass::new("general", vec![PhysReg(0)])],
            spill_classes: vec![
                SpillClass::new("zero-page", Some(1), 1),
                SpillClass::new("static", Some(16), 3),
            ],
        };
        let function = Function::new(
            vec![vreg("r", 0), vreg("zp", 0), vreg("static", 0)],
            vec![BasicBlock::new(
                vec![
                    Instruction::new().with_defs(vec![VReg(0), VReg(1), VReg(2)]),
                    Instruction::new().with_uses(vec![VReg(0), VReg(1), VReg(2)]),
                ],
                vec![],
            )],
        );
        let allocation = allocate(&target, &function).unwrap();
        assert_eq!(allocation.spill_slots.len(), 2);
        assert_eq!(allocation.spill_slots[0].class, SpillClassId(0));
        assert_eq!(allocation.spill_slots[1].class, SpillClassId(1));
    }

    #[test]
    fn copy_destination_safely_reuses_a_dead_source_register() {
        let target = Target {
            units: vec![unit("R0"), unit("R1")],
            registers: vec![
                PhysicalRegister::new("R0", vec![RegUnit(0)]),
                PhysicalRegister::new("R1", vec![RegUnit(1)]),
            ],
            register_classes: vec![RegisterClass::new("general", vec![PhysReg(0), PhysReg(1)])],
            spill_classes: static_spills(),
        };
        let function = Function::new(
            vec![vreg("source", 0), vreg("destination", 0)],
            vec![BasicBlock::new(
                vec![
                    Instruction::new().with_defs(vec![VReg(0)]),
                    Instruction::new().with_copies(vec![CopyOperand::new(VReg(0), VReg(1))]),
                    Instruction::new().with_uses(vec![VReg(1)]),
                ],
                vec![],
            )],
        );
        let allocation = allocate(&target, &function).unwrap();
        assert_eq!(
            allocation.location(VReg(0)),
            Some(Location::Register(PhysReg(0)))
        );
        assert_eq!(
            allocation.location(VReg(1)),
            Some(Location::Register(PhysReg(0)))
        );
        assert!(!allocation.intervals[0].overlaps(allocation.intervals[1]));
    }

    #[test]
    fn copy_preference_does_not_coalesce_overlapping_cfg_intervals() {
        let target = Target {
            units: vec![unit("R0"), unit("R1")],
            registers: vec![
                PhysicalRegister::new("R0", vec![RegUnit(0)]),
                PhysicalRegister::new("R1", vec![RegUnit(1)]),
            ],
            register_classes: vec![RegisterClass::new("general", vec![PhysReg(0), PhysReg(1)])],
            spill_classes: static_spills(),
        };
        let function = Function::new(
            vec![vreg("source", 0), vreg("destination", 0)],
            vec![BasicBlock::new(
                vec![
                    Instruction::new().with_defs(vec![VReg(0)]),
                    Instruction::new().with_copies(vec![CopyOperand::new(VReg(0), VReg(1))]),
                    Instruction::new().with_uses(vec![VReg(0), VReg(1)]),
                ],
                vec![],
            )],
        );
        let allocation = allocate(&target, &function).unwrap();
        assert_ne!(allocation.location(VReg(0)), allocation.location(VReg(1)));
    }

    #[test]
    fn instruction_clobber_blocks_values_live_across_calls() {
        let target = Target {
            units: vec![unit("caller"), unit("saved")],
            registers: vec![
                PhysicalRegister::new("caller", vec![RegUnit(0)]),
                PhysicalRegister::new("saved", vec![RegUnit(1)]),
            ],
            register_classes: vec![RegisterClass::new("general", vec![PhysReg(0), PhysReg(1)])],
            spill_classes: static_spills(),
        };
        let function = Function::new(
            vec![vreg("across-call", 0)],
            vec![BasicBlock::new(
                vec![
                    Instruction::new().with_defs(vec![VReg(0)]),
                    Instruction::new().with_clobbers(vec![PhysReg(0)]),
                    Instruction::new().with_uses(vec![VReg(0)]),
                ],
                vec![],
            )],
        );
        let allocation = allocate(&target, &function).unwrap();
        assert_eq!(
            allocation.location(VReg(0)),
            Some(Location::Register(PhysReg(1)))
        );
    }

    #[test]
    fn clobber_catches_a_value_live_in_at_the_first_instruction() {
        let target = Target {
            units: vec![unit("caller"), unit("saved")],
            registers: vec![
                PhysicalRegister::new("caller", vec![RegUnit(0)]),
                PhysicalRegister::new("saved", vec![RegUnit(1)]),
            ],
            register_classes: vec![RegisterClass::new("general", vec![PhysReg(0), PhysReg(1)])],
            spill_classes: static_spills(),
        };
        let function = Function::new(
            vec![vreg("live-in", 0)],
            vec![BasicBlock::new(
                vec![
                    Instruction::new()
                        .with_uses(vec![VReg(0)])
                        .with_clobbers(vec![PhysReg(0)]),
                    Instruction::new().with_uses(vec![VReg(0)]),
                ],
                vec![],
            )],
        );

        let allocation = allocate(&target, &function).unwrap();
        assert_eq!(
            allocation.location(VReg(0)),
            Some(Location::Register(PhysReg(1)))
        );
    }

    #[test]
    fn clobber_catches_a_value_live_out_at_a_loop_backedge() {
        let target = Target {
            units: vec![unit("caller"), unit("saved")],
            registers: vec![
                PhysicalRegister::new("caller", vec![RegUnit(0)]),
                PhysicalRegister::new("saved", vec![RegUnit(1)]),
            ],
            register_classes: vec![RegisterClass::new("general", vec![PhysReg(0), PhysReg(1)])],
            spill_classes: static_spills(),
        };
        let function = Function::new(
            vec![vreg("loop-carried", 0)],
            vec![
                BasicBlock::new(
                    vec![Instruction::new().with_uses(vec![VReg(0)])],
                    vec![BlockId(1)],
                ),
                BasicBlock::new(
                    vec![Instruction::new().with_clobbers(vec![PhysReg(0)])],
                    vec![BlockId(0)],
                ),
            ],
        );

        let allocation = allocate(&target, &function).unwrap();
        assert!(allocation.liveness.live_out[1].contains(&VReg(0)));
        assert_eq!(
            allocation.location(VReg(0)),
            Some(Location::Register(PhysReg(1)))
        );
    }

    #[test]
    fn clobber_conflicts_with_a_compound_use_and_def() {
        let target = Target {
            units: vec![unit("caller"), unit("saved")],
            registers: vec![
                PhysicalRegister::new("caller", vec![RegUnit(0)]),
                PhysicalRegister::new("saved", vec![RegUnit(1)]),
            ],
            register_classes: vec![RegisterClass::new("general", vec![PhysReg(0), PhysReg(1)])],
            spill_classes: static_spills(),
        };
        let function = Function::new(
            vec![vreg("updated", 0)],
            vec![BasicBlock::new(
                vec![
                    Instruction::new().with_defs(vec![VReg(0)]),
                    Instruction::new()
                        .with_uses(vec![VReg(0)])
                        .with_defs(vec![VReg(0)])
                        .with_clobbers(vec![PhysReg(0)]),
                    Instruction::new().with_uses(vec![VReg(0)]),
                ],
                vec![],
            )],
        );

        let allocation = allocate(&target, &function).unwrap();
        assert_eq!(
            allocation.location(VReg(0)),
            Some(Location::Register(PhysReg(1)))
        );
    }

    #[test]
    fn clobber_allows_a_fixed_result_redefined_by_the_same_instruction() {
        let target = Target {
            units: vec![unit("R")],
            registers: vec![PhysicalRegister::new("R", vec![RegUnit(0)])],
            register_classes: vec![RegisterClass::new("general", vec![PhysReg(0)])],
            spill_classes: static_spills(),
        };
        let fixed = FixedOperand::new(VReg(0), PhysReg(0));
        let function = Function::new(
            vec![vreg("updated", 0)],
            vec![BasicBlock::new(
                vec![
                    Instruction::new().with_defs(vec![VReg(0)]),
                    Instruction::new()
                        .with_fixed_defs(vec![fixed])
                        .with_clobbers(vec![PhysReg(0)]),
                    Instruction::new().with_uses(vec![VReg(0)]),
                ],
                vec![],
            )],
        );
        let allocation = allocate(&target, &function).unwrap();
        assert_eq!(
            allocation.location(VReg(0)),
            Some(Location::Register(PhysReg(0)))
        );
    }

    #[test]
    fn malformed_liveness_returns_a_diagnostic() {
        let function = Function::new(
            vec![vreg("value", 0)],
            vec![BasicBlock::new(vec![], vec![])],
        );
        let liveness = Liveness {
            live_in: vec![BTreeSet::from([VReg(9)])],
            live_out: vec![BTreeSet::new()],
        };
        let errors = build_live_intervals(&function, &liveness).unwrap_err();
        assert_eq!(errors[0].code, DiagnosticCode::InvalidOperand);
    }

    #[test]
    fn malformed_configuration_returns_diagnostics_without_panicking() {
        let target = Target {
            units: vec![unit("u")],
            registers: vec![PhysicalRegister::new("bad", vec![RegUnit(9)])],
            register_classes: vec![RegisterClass::new("empty", vec![])],
            spill_classes: vec![SpillClass::new("bad-spill", Some(1), 0).with_base_alignment(3)],
        };
        let function = Function::new(
            vec![VirtualRegister::new("bad", 0, 3, RegClass(4))],
            vec![BasicBlock::new(
                vec![
                    Instruction::new()
                        .with_uses(vec![VReg(7)])
                        .with_clobbers(vec![PhysReg(8)]),
                ],
                vec![BlockId(3)],
            )],
        );
        let errors = allocate(&target, &function).unwrap_err();
        assert!(errors.len() >= 7, "{errors:#?}");
        assert!(
            errors
                .iter()
                .any(|error| error.code == DiagnosticCode::InvalidTarget)
        );
        assert!(
            errors
                .iter()
                .any(|error| error.code == DiagnosticCode::InvalidFunction)
        );
        assert!(
            errors
                .iter()
                .any(|error| error.code == DiagnosticCode::InvalidOperand)
        );
    }
