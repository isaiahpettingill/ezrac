use super::*;

#[test]
fn pic18_backend_runs_assembled_programs_with_test_host_io() {
    let run = run_assembly_test_with_cpu_options_at(
        CpuFamily::Pic18,
        "movlw 2Ah\nmovwf 71h, a\nmovlw 1\nmovwf 70h, a\nmovlw 7\nmovwf 72h, a\nmovlw 1\nmovwf 73h, a\n",
        &TestRunOptions {
            instruction_budget: 32,
            initial_ports: Vec::new(),
            initial_memory: Vec::new(),
            stack_top: 0x0DFF,
        },
        0,
    )
    .unwrap();

    assert!(run.halted);
    assert_eq!(run.result_code, 7);
    assert_eq!(run.debug_output, b"*");
    assert_eq!(run.failure, None);
}
