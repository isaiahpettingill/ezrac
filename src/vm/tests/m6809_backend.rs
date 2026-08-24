use std::path::Path;

use super::*;
use crate::{
    asm::{AssemblyOptions, emit_m6809_assembly_with_options},
    parser::parse_program,
    target::{Address24, CpuFamily},
};

fn m6809_source_options() -> AssemblyOptions {
    AssemblyOptions {
        cpu: CpuFamily::M6809,
        ram_base: Address24::new(0xA000),
        rodata_base: Address24::new(0x8000),
        asset_base: Address24::new(0xC000),
        ..AssemblyOptions::default()
    }
}

fn run_m6809_source(source: &str) -> u8 {
    let program = parse_program(Path::new("m6809_vm_backend.ezra"), source).unwrap();
    let assembly = emit_m6809_assembly_with_options(&program, m6809_source_options()).unwrap();
    let assembly = assembly.replace(
        "__ezra_exit:\n    bra __ezra_exit\n",
        "__ezra_exit:\n    staa >FFF1h\n    ldaa #01h\n    staa >FFF2h\n",
    );
    let bytes = assemble_subset_at(CpuFamily::M6809, &assembly, 0).unwrap();
    let run = TestRunner::default()
        .run(
            &TestImage {
                cpu_family: CpuFamily::M6809,
                base_addr: 0,
                bytes,
            },
            &TestRunOptions {
                instruction_budget: 10_000,
                initial_ports: Vec::new(),
                initial_memory: Vec::new(),
                stack_top: 0x7FFF,
            },
        )
        .unwrap();
    assert!(run.halted, "run={run:?}\n{assembly}");
    run.result_code
}

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
fn m6809_backend_supports_direct_and_mutual_recursion() {
    assert_eq!(
        run_m6809_source(
            r#"
                fn countdown(value: u8) -> u8 {
                    if value == 0 { return 0 }
                    return countdown(value - 1) + 1
                }
                fn main() -> u8 { return countdown(6) }
            "#,
        ),
        6
    );
    assert_eq!(
        run_m6809_source(
            r#"
                fn even(value: u8) -> bool {
                    if value == 0 { return true }
                    return odd(value - 1)
                }
                fn odd(value: u8) -> bool {
                    if value == 0 { return false }
                    return even(value - 1)
                }
                fn main() -> u8 { return cast<u8>(even(8)) }
            "#,
        ),
        1
    );
}

#[test]
fn m6809_backend_supports_callback_reentry() {
    assert_eq!(
        run_m6809_source(
            r#"
                fn apply(callback: ptr<fn(u8)u8>, value: u8) -> u8 {
                    if value == 0 { return 0 }
                    return callback(value - 1) + 1
                }
                fn reenter(value: u8) -> u8 {
                    if value == 0 { return 0 }
                    return apply(&reenter, value - 1) + 1
                }
                fn main() -> u8 {
                    let callback: ptr<fn(u8)u8> = &reenter
                    return apply(callback, 4)
                }
            "#,
        ),
        4
    );
}

#[test]
fn m6809_backend_raw_subroutine_returns() {
    let bytes = assemble_subset_at(
        CpuFamily::M6809,
        "    jsr sub\n    staa >FFF1h\n    ldaa #01h\n    staa >FFF2h\nsub:\n    ldaa #2Ah\n    rts\n",
        0,
    )
    .unwrap();
    let run = TestRunner::default()
        .run(
            &TestImage {
                cpu_family: CpuFamily::M6809,
                base_addr: 0,
                bytes,
            },
            &TestRunOptions {
                instruction_budget: 100,
                initial_ports: Vec::new(),
                initial_memory: Vec::new(),
                stack_top: 0x7FFF,
            },
        )
        .unwrap();
    assert!(run.halted, "{run:?}");
    assert_eq!(run.result_code, 0x2A);
}

#[test]
fn m6809_backend_raw_frame_reservation_returns() {
    let bytes = assemble_subset_at(
        CpuFamily::M6809,
        "    pshs u\n    leas -36,s\n    tfr s,u\n    leas 36,s\n    jsr sub\n    staa >FFF1h\n    ldaa #01h\n    staa >FFF2h\nsub:\n    ldaa #2Ah\n    rts\n",
        0,
    )
    .unwrap();
    let run = TestRunner::default()
        .run(
            &TestImage {
                cpu_family: CpuFamily::M6809,
                base_addr: 0,
                bytes,
            },
            &TestRunOptions {
                instruction_budget: 100,
                initial_ports: Vec::new(),
                initial_memory: Vec::new(),
                stack_top: 0x7FFF,
            },
        )
        .unwrap();
    assert!(run.halted, "{run:?}");
    assert_eq!(run.result_code, 0x2A);
}

#[test]
fn m6809_backend_runs_a_frame_return() {
    assert_eq!(run_m6809_source("fn main() -> u8 { return 42 }"), 42);
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
