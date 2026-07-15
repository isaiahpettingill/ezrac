use super::*;

const DEBUG_OUTPUT: u32 = 0xFFFFF0;
const RESULT_CODE: u32 = 0xFFFFF1;
const HALT: u32 = 0xFFFFF2;

fn options(instruction_budget: u64) -> TestRunOptions {
    TestRunOptions {
        instruction_budget,
        initial_ports: Vec::new(),
        initial_memory: Vec::new(),
        stack_top: 0xFF0000,
    }
}

#[test]
fn m68k_backend_runs_through_default_test_runner_and_observes_mmio_test_abi() {
    let assembly = format!(
        r#"
            move.b #$48, {DEBUG_OUTPUT:#08X}.l
            move.b #$69, {DEBUG_OUTPUT:#08X}.l
            move.b #$2A, {RESULT_CODE:#08X}.l
            move.b #$01, {HALT:#08X}.l
        "#
    );
    let bytes = assemble_subset_at(CpuFamily::M68k, &assembly, 0x001000).unwrap();
    let run = TestRunner::default()
        .run(
            &TestImage {
                cpu_family: CpuFamily::M68k,
                base_addr: 0x001000,
                bytes,
            },
            &options(100),
        )
        .unwrap();

    assert!(run.halted, "{run:?}");
    assert_eq!(run.result_code, 0x2A);
    assert_eq!(run.debug_output, b"Hi");
    assert_eq!(run.failure, None);
}

#[test]
fn m68k_backend_reports_timeout() {
    let bytes = assemble_subset_at(CpuFamily::M68k, "loop:\n    bra loop\n", 0x001000).unwrap();
    let run = TestRunner::default()
        .run(
            &TestImage {
                cpu_family: CpuFamily::M68k,
                base_addr: 0x001000,
                bytes,
            },
            &options(3),
        )
        .unwrap();

    assert!(!run.halted);
    assert_eq!(run.instructions, 3);
    assert_eq!(run.failure, Some(TestRunFailure::Timeout));
}
