use super::*;

fn options(instruction_budget: u64) -> TestRunOptions {
    TestRunOptions {
        instruction_budget,
        initial_ports: Vec::new(),
        initial_memory: Vec::new(),
        stack_top: 0x1_FFFE,
    }
}

#[test]
fn dcpu_backend_runs_word_memory_test_abi() {
    let bytes = assemble_subset_at(
        CpuFamily::Dcpu,
        r#"
            set a, 0x48
            set [0xfff1], a
            set [0xfff0], 1
            set a, 0x69
            set [0xfff1], a
            set [0xfff0], 2
            set [0xfff2], 7
            set [0xfff3], 1
        "#,
        0,
    )
    .unwrap();

    let run = TestRunner::default()
        .run(
            &TestImage {
                cpu_family: CpuFamily::Dcpu,
                base_addr: 0,
                bytes,
            },
            &options(100),
        )
        .unwrap();

    assert!(run.halted);
    assert_eq!(run.result_code, 7);
    assert_eq!(run.debug_output, b"Hi");
    assert_eq!(run.failure, None);
}

#[test]
fn dcpu_backend_loads_initial_memory_bytes_as_little_endian_words() {
    let bytes = assemble_subset_at(
        CpuFamily::Dcpu,
        r#"
            set a, [0x100]
            ifn a, 0x4241
            set [0xfff2], 1
            set [0xfff3], 1
        "#,
        0,
    )
    .unwrap();
    let mut options = options(100);
    options.initial_memory = vec![(0x200, 0x41), (0x201, 0x42)];

    let run = TestRunner::default()
        .run(
            &TestImage {
                cpu_family: CpuFamily::Dcpu,
                base_addr: 0,
                bytes,
            },
            &options,
        )
        .unwrap();

    assert!(run.halted);
    assert_eq!(run.result_code, 0);
}

#[test]
fn dcpu_backend_reports_timeout() {
    let bytes = assemble_subset_at(CpuFamily::Dcpu, "loop:\n    set pc, loop\n", 0).unwrap();
    let run = TestRunner::default()
        .run(
            &TestImage {
                cpu_family: CpuFamily::Dcpu,
                base_addr: 0,
                bytes,
            },
            &options(3),
        )
        .unwrap();

    assert!(!run.halted);
    assert_eq!(run.instructions, 3);
    assert_eq!(run.failure, Some(TestRunFailure::Timeout));
}

#[test]
fn dcpu_backend_rejects_unaligned_or_out_of_range_byte_addresses() {
    let runner = TestRunner::default();
    let options = options(1);

    let error = runner
        .run(
            &TestImage {
                cpu_family: CpuFamily::Dcpu,
                base_addr: 1,
                bytes: vec![0; 2],
            },
            &options,
        )
        .unwrap_err();
    assert!(error.message.contains("aligned byte address"));

    let error = runner
        .run(
            &TestImage {
                cpu_family: CpuFamily::Dcpu,
                base_addr: 0,
                bytes: vec![0],
            },
            &options,
        )
        .unwrap_err();
    assert!(error.message.contains("even number of bytes"));

    let error = runner
        .run(
            &TestImage {
                cpu_family: CpuFamily::Dcpu,
                base_addr: 0x1_FFFE,
                bytes: vec![0; 4],
            },
            &options,
        )
        .unwrap_err();
    assert!(error.message.contains("exceeds the DCPU"));
}
