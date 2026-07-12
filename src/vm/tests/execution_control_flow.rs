use super::*;

#[test]
fn test_runner_accepts_backends_for_other_architectures() {
    struct StubEmulator;

    impl EmulatorBackend for StubEmulator {
        fn supports(&self, cpu_family: CpuFamily) -> bool {
            cpu_family == CpuFamily::M68k
        }

        fn run(&self, image: &TestImage, _options: &TestRunOptions) -> Result<TestRun, Diagnostic> {
            assert_eq!(image.bytes, [0x4E, 0x75]);
            Ok(TestRun {
                halted: true,
                result_code: 0,
                instructions: 1,
                debug_output: Vec::new(),
                ports: [0; 256],
                failure: None,
            })
        }
    }

    let runner = TestRunner::new(vec![Box::new(StubEmulator)]);
    let run = runner
        .run(
            &TestImage {
                cpu_family: CpuFamily::M68k,
                base_addr: 0,
                bytes: vec![0x4E, 0x75],
            },
            &TestRunOptions {
                instruction_budget: 1,
                initial_ports: Vec::new(),
                initial_memory: Vec::new(),
                stack_top: 0,
            },
        )
        .unwrap();

    assert!(run.halted);
}

#[test]
fn reports_timeout_when_program_does_not_halt() {
    let run = run_assembly_test("spin:\n    jp spin\n", 3).unwrap();

    assert!(!run.halted);
    assert_eq!(run.instructions, 3);
    assert_eq!(run.failure, Some(TestRunFailure::Timeout));
}

#[test]
fn runs_current_address_jump_on_ez80_vm() {
    let run = run_assembly_test("jp $\n", 3).unwrap();

    assert!(!run.halted);
    assert_eq!(run.instructions, 3);
    assert_eq!(run.failure, Some(TestRunFailure::Timeout));
}

#[test]
fn reports_execution_outside_mapped_memory() {
    let run = run_assembly_test("jp 020000h\n", 10).unwrap();

    assert!(!run.halted);
    assert_eq!(run.instructions, 1);
    assert_eq!(
        run.failure,
        Some(TestRunFailure::ExecutionOutsideMappedMemory { pc: 0x020000 })
    );
}

#[test]
fn initializes_stack_pointer_to_default_stack_top() {
    let asm = r#"
            call leaves_return_address
            ld a, (0EFFFFDh)
            out0 (0Dh), a
            ld a, 01h
            out0 (0Eh), a
        leaves_return_address:
            ret
        "#;
    let run = run_assembly_test(asm, 100).unwrap();

    assert!(run.halted);
    assert_eq!(run.result_code, 0x04);
}

#[test]
fn run_options_set_initial_stack_top() {
    let asm = r#"
            call leaves_return_address
            ld a, (0402FDh)
            out0 (0Dh), a
            ld a, 01h
            out0 (0Eh), a
        leaves_return_address:
            ret
        "#;
    let run = run_assembly_test_with_options(
        asm,
        &TestRunOptions {
            instruction_budget: 100,
            initial_ports: Vec::new(),
            initial_memory: Vec::new(),
            stack_top: 0x040300,
        },
    )
    .unwrap();

    assert!(run.halted);
    assert_eq!(run.result_code, 0x04);
}

#[test]
fn rejects_stack_top_outside_address_space() {
    let error = run_assembly_test_with_options(
        "ret\n",
        &TestRunOptions {
            instruction_budget: 100,
            initial_ports: Vec::new(),
            initial_memory: Vec::new(),
            stack_top: 0x01_000000,
        },
    )
    .unwrap_err();

    assert_eq!(
        error.message,
        "test stack top 0x1000000 is outside the 24-bit address space"
    );
}

#[test]
fn reports_stack_overflow_into_non_stack_memory() {
    let asm = r#"
            ld sp, 030400h
            ld hl, 012345h
            push hl
            ld a, 01h
            out0 (0Eh), a
        "#;
    let run = run_assembly_test_with_options(
        asm,
        &TestRunOptions {
            instruction_budget: 100,
            initial_ports: Vec::new(),
            initial_memory: Vec::new(),
            stack_top: 0x040400,
        },
    )
    .unwrap();

    assert!(!run.halted);
    assert_eq!(
        run.failure,
        Some(TestRunFailure::StackOverflow { sp: 0x0303FD })
    );
}

#[test]
fn runs_relative_jump_loop_on_ez80_vm() {
    let asm = r#"
            ld a, 03h
            ld b, a
        loop:
            dec b
            jr z, done
            jr loop
        done:
            xor a
            out0 (0Dh), a
            ld a, 01h
            out0 (0Eh), a
        "#;
    let run = run_assembly_test(asm, 100).unwrap();

    assert!(run.halted);
    assert_eq!(run.result_code, 0);
}

#[test]
fn runs_djnz_loop_on_ez80_vm() {
    let asm = r#"
            ld a, 03h
            ld b, a
        loop:
            djnz loop
            xor a
            out0 (0Dh), a
            ld a, 01h
            out0 (0Eh), a
        "#;
    let run = run_assembly_test(asm, 100).unwrap();

    assert!(run.halted);
    assert_eq!(run.result_code, 0);
}

#[test]
fn runs_non_maskable_interrupt_return_on_ez80_vm() {
    let asm = r#"
            call raw_return
            ld a, 00h
            out0 (0Dh), a
            ld a, 01h
            out0 (0Eh), a

        raw_return:
            retn
        "#;
    let run = run_assembly_test(asm, 100).unwrap();

    assert!(run.halted);
    assert_eq!(run.result_code, 0);
}

#[test]
fn compare_carry_jump_sequence_matches_emitter_assumption() {
    let asm = r#"
            ld sp, 0F00000h
            ld a, 00h
            ld b, a
            ld a, 04h
            ld c, a
            ld a, b
            cp c
            jp c, yes
            ld a, 09h
            out0 (0Dh), a
            ld a, 01h
            out0 (0Eh), a
        yes:
            ld a, 00h
            out0 (0Dh), a
            ld a, 01h
            out0 (0Eh), a
        "#;
    let run = run_assembly_test(asm, 100).unwrap();

    assert!(run.halted);
    assert_eq!(run.result_code, 0);
}

#[test]
fn runs_sign_conditional_absolute_jumps_on_ez80_vm() {
    let asm = r#"
            ld a, 80h
            or a
            jp m, negative
            ld a, 10h
            out0 (0Dh), a
            ld a, 01h
            out0 (0Eh), a

        negative:
            ld a, 00h
            or a
            jp p, positive
            ld a, 11h
            out0 (0Dh), a
            ld a, 01h
            out0 (0Eh), a

        positive:
            ld a, 00h
            out0 (0Dh), a
            ld a, 01h
            out0 (0Eh), a
        "#;
    let run = run_assembly_test(asm, 100).unwrap();

    assert!(run.halted);
    assert_eq!(run.result_code, 0);
}

#[test]
fn runs_conditional_calls_on_ez80_vm() {
    let asm = r#"
            xor a
            call z, mark_taken
            call nz, fail
            out0 (0Dh), a
            ld a, 01h
            out0 (0Eh), a

        mark_taken:
            ld a, 00h
            ret

        fail:
            ld a, 20h
            out0 (0Dh), a
            ld a, 01h
            out0 (0Eh), a
        "#;
    let run = run_assembly_test(asm, 100).unwrap();

    assert!(run.halted);
    assert_eq!(run.result_code, 0);
}

#[test]
fn run_options_seed_input_ports() {
    let asm = r#"
            in0 a, (01h)
            out0 (0Dh), a
            ld a, 01h
            out0 (0Eh), a
        "#;
    let run = run_assembly_test_with_options(
        asm,
        &TestRunOptions {
            instruction_budget: 100,
            initial_ports: vec![(0x01, 0x2A)],
            initial_memory: Vec::new(),
            stack_top: EZRA_STACK_TOP.get(),
        },
    )
    .unwrap();

    assert!(run.halted);
    assert_eq!(run.result_code, 0x2A);
}

#[test]
fn run_options_seed_memory() {
    let asm = r#"
            ld a, (040123h)
            out0 (0Dh), a
            ld a, 01h
            out0 (0Eh), a
        "#;
    let run = run_assembly_test_with_options(
        asm,
        &TestRunOptions {
            instruction_budget: 100,
            initial_ports: Vec::new(),
            initial_memory: vec![(0x040123, 0x6C)],
            stack_top: EZRA_STACK_TOP.get(),
        },
    )
    .unwrap();

    assert!(run.halted);
    assert_eq!(run.result_code, 0x6C);
}

#[test]
fn rejects_initial_memory_outside_address_space() {
    let error = run_assembly_test_with_options(
        "ret\n",
        &TestRunOptions {
            instruction_budget: 100,
            initial_ports: Vec::new(),
            initial_memory: vec![(0x01_000000, 0x6C)],
            stack_top: EZRA_STACK_TOP.get(),
        },
    )
    .unwrap_err();

    assert_eq!(
        error.message,
        "test memory address 0x1000000 is outside the 24-bit address space"
    );
}

#[test]
fn rejects_test_program_that_exceeds_address_space() {
    let error = run_assembly_test_with_options_at(
        "nop\nnop\n",
        &TestRunOptions {
            instruction_budget: 100,
            initial_ports: Vec::new(),
            initial_memory: Vec::new(),
            stack_top: EZRA_STACK_TOP.get(),
        },
        0xFF_FFFF,
    )
    .unwrap_err();

    assert_eq!(
        error.message,
        "assembly instruction at 0x1000000 with length 0x1 exceeds the 24-bit address space"
    );
}

#[test]
fn runs_conditional_returns_on_ez80_vm() {
    let asm = r#"
            ld a, 01h
            or a
            call check_nz

            ld b, a
            cp b
            call check_z

            ld a, 01h
            or a
            call check_nc

            ld b, 01h
            ld a, 00h
            cp b
            call check_c

            ld a, 00h
            out0 (0Dh), a
            ld a, 01h
            out0 (0Eh), a

        check_nz:
            ret nz
            ld a, 10h
            out0 (0Dh), a
            ld a, 01h
            out0 (0Eh), a

        check_z:
            ret z
            ld a, 11h
            out0 (0Dh), a
            ld a, 01h
            out0 (0Eh), a

        check_nc:
            ret nc
            ld a, 12h
            out0 (0Dh), a
            ld a, 01h
            out0 (0Eh), a

        check_c:
            ret c
            ld a, 13h
            out0 (0Dh), a
            ld a, 01h
            out0 (0Eh), a
        "#;
    let bytes = assemble_ez80_subset_at(asm, EZRA_LOAD_ADDR.get()).unwrap();

    assert!(bytes.contains(&0xC0));
    assert!(bytes.contains(&0xC8));
    assert!(bytes.contains(&0xD0));
    assert!(bytes.contains(&0xD8));

    let run = run_assembly_test(asm, 200).unwrap();

    assert!(run.halted);
    assert_eq!(run.result_code, 0);
}
