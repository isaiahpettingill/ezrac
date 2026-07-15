use super::*;

#[test]
fn lr35902_program_runs_in_game_boy_mode_with_memory_test_abi() {
    let assembly = r#"
        ld a, 4Fh
        ld (0FF80h), a
        ld a, 4Bh
        ld (0FF80h), a
        xor a
        ld (0FF81h), a
        ld a, 1
        ld (0FF82h), a
    "#;
    let run = run_assembly_test_with_cpu_options_at(
        CpuFamily::Lr35902,
        assembly,
        &TestRunOptions {
            instruction_budget: 100,
            initial_ports: Vec::new(),
            initial_memory: Vec::new(),
            stack_top: 0xFFFE,
        },
        0x0150,
    )
    .unwrap();

    assert!(run.halted, "{run:?}");
    assert_eq!(run.result_code, 0);
    assert_eq!(run.debug_output, b"OK");
    assert_eq!(run.failure, None);
}
