use super::*;

#[test]
fn m6800_backend_runs_through_test_runner() {
    let assembly = r#"
        ldaa #$48
        staa $FFF0
        ldaa #$69
        staa $FFF0
        ldaa #$00
        staa $FFF1
        ldaa #$01
        staa $FFF2
    "#;
    let bytes = assemble_subset_at(CpuFamily::M6800, assembly, 0x0200).unwrap();
    let runner = TestRunner::default();
    let run = runner
        .run(
            &TestImage {
                cpu_family: CpuFamily::M6800,
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
fn m6800_backend_reports_timeout() {
    let bytes = assemble_subset_at(CpuFamily::M6800, "start:\n    bra start\n", 0x0200).unwrap();
    let runner = TestRunner::default();
    let run = runner
        .run(
            &TestImage {
                cpu_family: CpuFamily::M6800,
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
