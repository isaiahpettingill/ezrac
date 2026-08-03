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
