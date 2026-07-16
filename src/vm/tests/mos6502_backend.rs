use super::*;

#[test]
fn mos6502_nmos_runs_through_test_runner() {
    let assembly = r#"
        lda #$48
        sta $FF0C
        lda #$69
        sta $FF0C
        lda #$00
        sta $FF0D
        lda #$01
        sta $FF0E
    "#;
    let bytes = assemble_subset_at(CpuFamily::Mos6502, assembly, 0x0200).unwrap();
    let runner = TestRunner::new(vec![Box::new(Mos6502Emulator)]);
    let run = runner
        .run(
            &TestImage {
                cpu_family: CpuFamily::Mos6502,
                base_addr: 0x0200,
                bytes,
            },
            &TestRunOptions {
                instruction_budget: 100,
                initial_ports: Vec::new(),
                initial_memory: Vec::new(),
                stack_top: 0x01FF,
            },
        )
        .unwrap();

    assert!(run.halted);
    assert_eq!(run.result_code, 0);
    assert_eq!(run.debug_output, b"Hi");
    assert_eq!(run.failure, None);
}

#[test]
fn mos6502_nmos_runs_reference_c64_16_bit_addition() {
    // Adapted from digitsensitive/c64's c64-users-guide/addition_16bit.asm.
    let run = run_assembly_test_with_cpu_options_at(
        CpuFamily::Mos6502,
        r#"
            cld
            clc
            lda $10
            adc $12
            sta $14
            lda $11
            adc $13
            sta $15
            lda $14
            cmp #$04
            bne fail
            lda $15
            cmp #$01
            bne fail
        pass:
            lda #$00
            sta $FF0D
            lda #$01
            sta $FF0E
        fail:
            lda #$01
            sta $FF0D
            lda #$01
            sta $FF0E
        "#,
        &TestRunOptions {
            instruction_budget: 100,
            initial_ports: Vec::new(),
            initial_memory: vec![(0x10, 0xFF), (0x11, 0x00), (0x12, 0x05), (0x13, 0x00)],
            stack_top: 0x01FF,
        },
        0x0200,
    )
    .unwrap();

    assert!(run.halted, "{run:?}");
    assert_eq!(run.result_code, 0);
    assert_eq!(run.failure, None);
}

#[test]
fn mos6502_nmos_reports_timeout() {
    let bytes = assemble_subset_at(CpuFamily::Mos6502, "loop:\n    jmp loop\n", 0x0200).unwrap();
    let runner = TestRunner::new(vec![Box::new(Mos6502Emulator)]);
    let run = runner
        .run(
            &TestImage {
                cpu_family: CpuFamily::Mos6502,
                base_addr: 0x0200,
                bytes,
            },
            &TestRunOptions {
                instruction_budget: 3,
                initial_ports: Vec::new(),
                initial_memory: Vec::new(),
                stack_top: 0x01FF,
            },
        )
        .unwrap();

    assert!(!run.halted);
    assert_eq!(run.instructions, 3);
    assert_eq!(run.failure, Some(TestRunFailure::Timeout));
}

#[test]
fn mos6502_nmos_works_via_default_runner() {
    let assembly = r#"
        lda #$48
        sta $FF0C
        lda #$69
        sta $FF0C
        lda #$00
        sta $FF0D
        lda #$01
        sta $FF0E
    "#;
    let run = run_assembly_test_with_cpu_options_at(
        CpuFamily::Mos6502,
        assembly,
        &TestRunOptions {
            instruction_budget: 100,
            initial_ports: Vec::new(),
            initial_memory: Vec::new(),
            stack_top: 0x01FF,
        },
        0x0200,
    )
    .unwrap();

    assert!(run.halted);
    assert_eq!(run.result_code, 0);
    assert_eq!(run.debug_output, b"Hi");
    assert_eq!(run.failure, None);
}

#[test]
fn mos6502_cmos65c02_runs_through_test_runner() {
    let assembly = r#"
        lda #$48
        sta $FF0C
        lda #$69
        sta $FF0C
        lda #$00
        sta $FF0D
        lda #$01
        sta $FF0E
    "#;
    let bytes = assemble_subset_at(CpuFamily::Cmos65C02, assembly, 0x0200).unwrap();
    let runner = TestRunner::new(vec![Box::new(Mos6502Emulator)]);
    let run = runner
        .run(
            &TestImage {
                cpu_family: CpuFamily::Cmos65C02,
                base_addr: 0x0200,
                bytes,
            },
            &TestRunOptions {
                instruction_budget: 100,
                initial_ports: Vec::new(),
                initial_memory: Vec::new(),
                stack_top: 0x01FF,
            },
        )
        .unwrap();

    assert!(run.halted);
    assert_eq!(run.result_code, 0);
    assert_eq!(run.debug_output, b"Hi");
    assert_eq!(run.failure, None);
}

#[test]
fn mos6502_ricoh2a03_runs_through_test_runner() {
    let assembly = r#"
        lda #$48
        sta $FF0C
        lda #$69
        sta $FF0C
        lda #$00
        sta $FF0D
        lda #$01
        sta $FF0E
    "#;
    let bytes = assemble_subset_at(CpuFamily::Ricoh2A03, assembly, 0x0200).unwrap();
    let runner = TestRunner::new(vec![Box::new(Mos6502Emulator)]);
    let run = runner
        .run(
            &TestImage {
                cpu_family: CpuFamily::Ricoh2A03,
                base_addr: 0x0200,
                bytes,
            },
            &TestRunOptions {
                instruction_budget: 100,
                initial_ports: Vec::new(),
                initial_memory: Vec::new(),
                stack_top: 0x01FF,
            },
        )
        .unwrap();

    assert!(run.halted);
    assert_eq!(run.result_code, 0);
    assert_eq!(run.debug_output, b"Hi");
    assert_eq!(run.failure, None);
}
