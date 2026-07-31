use super::*;

#[test]
fn m6809_backend_runs_through_test_runner() {
    let assembly = r#"
        lda #$48
        sta $FFF0
        lda #$69
        sta $FFF0
        lda #$00
        sta $FFF1
        lda #$01
        sta $FFF2
    "#;
    let bytes = assemble_subset_at(CpuFamily::M6809, assembly, 0x0200).unwrap();
    let runner = TestRunner::default();
    let run = runner
        .run(
            &TestImage {
                cpu_family: CpuFamily::M6809,
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
fn m6809_backend_reports_timeout() {
    let bytes = assemble_subset_at(CpuFamily::M6809, "start:\n    bra start\n", 0x0200).unwrap();
    let runner = TestRunner::default();
    let run = runner
        .run(
            &TestImage {
                cpu_family: CpuFamily::M6809,
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
