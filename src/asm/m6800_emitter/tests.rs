
use std::path::Path;

use super::emit_m6800_assembly_with_options;
#[cfg(feature = "m6809")]
use super::emit_m6809_assembly_with_options;
use crate::{
    asm::AssemblyOptions,
    parser::parse_program,
    regalloc::PhysReg,
    target::{Address24, AssemblerCpu, CpuFamily},
    vm::{
        TestImage, TestRunOptions, TestRunner, assemble_subset_at, assemble_subset_with_symbols_at,
    },
};

fn m6800_options() -> AssemblyOptions {
    AssemblyOptions {
        cpu: CpuFamily::M6800,
        ram_base: Address24::new(0xA000),
        rodata_base: Address24::new(0x8000),
        asset_base: Address24::new(0xC000),
        ..AssemblyOptions::default()
    }
}

#[cfg(feature = "m6809")]
fn m6809_options() -> AssemblyOptions {
    AssemblyOptions {
        cpu: CpuFamily::M6809,
        ram_base: Address24::new(0xA000),
        rodata_base: Address24::new(0x8000),
        asset_base: Address24::new(0xC000),
        ..AssemblyOptions::default()
    }
}

#[test]
fn local_target_models_accumulators_and_uses_memory_only_locals() {
    let m6800 = super::m6800_local_target(CpuFamily::M6800);
    assert_eq!(
        m6800.register_classes[super::M6800_BYTE_CLASS.0]
            .registers
            .len(),
        2
    );
    assert!(
        m6800.register_classes[super::M6800_LOCAL_CLASS.0]
            .registers
            .is_empty()
    );
    assert!(!m6800.registers_alias(PhysReg(0), PhysReg(1)));

    let m6809 = super::m6800_local_target(CpuFamily::M6809);
    assert_eq!(m6809.registers[2].name, "d");
    assert!(m6809.registers_alias(PhysReg(0), PhysReg(2)));
    assert!(m6809.registers_alias(PhysReg(1), PhysReg(2)));
}

fn local_initializer_address(assembly: &str, value: u8) -> String {
    let marker = format!("    ldaa #{value:02X}h\n    staa >");
    let rest = assembly
        .split_once(&marker)
        .unwrap_or_else(|| panic!("missing local initializer {value:02X}\n{assembly}"))
        .1;
    rest[..4].to_owned()
}

fn assert_reuses_memory_local(assembly: &str, cpu: AssemblerCpu) {
    assert_eq!(
        local_initializer_address(assembly, 0x11),
        local_initializer_address(assembly, 0x22),
        "{assembly}"
    );
    assemble_subset_with_symbols_at(cpu, assembly, 0)
        .unwrap_or_else(|error| panic!("{error}\n{assembly}"));
}

#[test]
fn colors_nonoverlapping_m6800_locals_into_one_static_byte() {
    let program = parse_program(
        Path::new("m6800_spill_reuse.ezra"),
        r#"
                global sink: u8 = 0
                fn main() {
                    let first: u8 = 0x11
                    sink = first
                    let second: u8 = 0x22
                    sink = second
                }
            "#,
    )
    .unwrap();
    let assembly = emit_m6800_assembly_with_options(&program, m6800_options()).unwrap();
    assert_reuses_memory_local(&assembly, AssemblerCpu::M6800);
}

#[cfg(feature = "m6809")]
#[test]
fn colors_nonoverlapping_m6809_locals_into_one_static_byte() {
    let program = parse_program(
        Path::new("m6809_spill_reuse.ezra"),
        r#"
                global sink: u8 = 0
                fn main() {
                    let first: u8 = 0x11
                    sink = first
                    let second: u8 = 0x22
                    sink = second
                }
            "#,
    )
    .unwrap();
    let assembly = emit_m6809_assembly_with_options(&program, m6809_options()).unwrap();
    assert_reuses_memory_local(&assembly, AssemblerCpu::M6809);
}

#[test]
fn emits_assemblable_scalar_globals_locals_and_control_flow() {
    let program = parse_program(
        Path::new("m6800_test.ezra"),
        r#"
                global counter: u8 = 1
                global enabled: bool = true

                fn main() {
                    let value: u8 = counter + 2
                    if enabled && value == 3 { counter = value }
                    while counter < 5 { counter += 1 }
                }
            "#,
    )
    .unwrap();
    let assembly = emit_m6800_assembly_with_options(&program, m6800_options()).unwrap();

    assert!(assembly.contains("_main:"), "{assembly}");
    assert!(assembly.contains("adda >__ezra_r1"), "{assembly}");
    assemble_subset_with_symbols_at(AssemblerCpu::M6800, &assembly, 0)
        .unwrap_or_else(|error| panic!("{error}\n{assembly}"));
}

#[cfg(feature = "m6809")]
#[test]
fn m6809_source_uses_tbir_strength_reduction_and_assembles() {
    let program = parse_program(
            Path::new("m6809_tbir_test.ezra"),
            "fn shift(value: u8) -> u8 { return value * 8 } fn scale(value: u8) -> u8 { return value * 3 } fn main() { let shifted: u8 = shift(1) let scaled: u8 = scale(3) }",
        )
        .unwrap();
    let assembly = emit_m6809_assembly_with_options(&program, m6809_options()).unwrap();
    assert!(assembly.contains("    asla\n"), "{assembly}");
    assert!(assembly.contains("    mul\n"), "{assembly}");
    assert!(assembly.contains("target: Motorola M6809"), "{assembly}");
    assemble_subset_with_symbols_at(AssemblerCpu::M6809, &assembly, 0)
        .unwrap_or_else(|error| panic!("{error}\n{assembly}"));
}

#[test]
fn emits_original_m6800_constant_shift_instructions_and_strength_reduces_multiply() {
    let program = parse_program(
        Path::new("m6800_shift_test.ezra"),
        r#"
                fn left(value: u8) -> u8 { return value << 2 }
                fn unsigned_right(value: u8) -> u8 { return value >> 3 }
                fn signed_right(value: i8) -> i8 { return value >> 2 }
                fn times_eight(value: u8) -> u8 { return value * 8 }
                fn main() {
                    let left_value: u8 = left(1)
                    let unsigned_value: u8 = unsigned_right(1)
                    let signed_value: i8 = signed_right(1i8)
                    let times_eight_value: u8 = times_eight(1)
                }
            "#,
    )
    .unwrap();
    let assembly = emit_m6800_assembly_with_options(&program, m6800_options()).unwrap();

    assert_eq!(assembly.matches("    asla\n").count(), 5, "{assembly}");
    assert_eq!(assembly.matches("    lsra\n").count(), 3, "{assembly}");
    assert_eq!(assembly.matches("    asra\n").count(), 2, "{assembly}");
    assert!(!assembly.contains("    mul"), "{assembly}");
    assemble_subset_with_symbols_at(AssemblerCpu::M6800, &assembly, 0)
        .unwrap_or_else(|error| panic!("{error}\n{assembly}"));
}

fn run_main_result(source: &str) -> u8 {
    let program = parse_program(Path::new("m6800_shift_runtime.ezra"), source).unwrap();
    let assembly = emit_m6800_assembly_with_options(&program, m6800_options()).unwrap();
    let assembly = assembly.replace(
        "__ezra_exit:\n    bra __ezra_exit\n",
        "__ezra_exit:\n    staa >FFF1h\n    ldaa #01h\n    staa >FFF2h\n",
    );
    let bytes = assemble_subset_at(CpuFamily::M6800, &assembly, 0).unwrap();
    let run = TestRunner::default()
        .run(
            &TestImage {
                cpu_family: CpuFamily::M6800,
                base_addr: 0,
                bytes,
            },
            &TestRunOptions {
                instruction_budget: 200,
                initial_ports: Vec::new(),
                initial_memory: Vec::new(),
                stack_top: 0x01ff,
            },
        )
        .unwrap();
    assert!(run.halted, "{assembly}");
    run.result_code
}

#[test]
fn executes_constant_shifts_and_multiply_strength_reduction() {
    assert_eq!(
        run_main_result("global value: u8 = 0x23 fn main() -> u8 { return value << 3 }"),
        0x18
    );
    assert_eq!(
        run_main_result("global value: u8 = 0x90 fn main() -> u8 { return value >> 3 }"),
        0x12
    );
    assert_eq!(
        run_main_result("global value: i8 = -64 fn main() -> i8 { return value >> 3 }"),
        0xf8
    );
    assert_eq!(
        run_main_result("global value: u8 = 0x23 fn main() -> u8 { return value * 8 }"),
        0x18
    );
}

#[test]
fn explicit_inline_assembles_typed_arguments_nested_helpers_and_safe_fallbacks() {
    let source = r#"
            global sequence: u8 = 0
            global first_seen: u8 = 0
            global second_seen: u8 = 0

            fn next() -> u8 {
                sequence += 1
                return sequence
            }

            inline fn record_pair(first: u8, second: u8) {
                first_seen = first
                second_seen = second
            }

            inline fn nested_pair(first: u8, second: u8) {
                record_pair(first, second)
            }

            inline fn ready() -> bool {
                sequence += 1
                return sequence < 5
            }

            fn main() -> u8 {
                nested_pair(next(), next())
                let short_circuit: bool = false && ready()
                while ready() {}
                return second_seen
            }
        "#;
    let program = parse_program(Path::new("m6800_inline_test.ezra"), source).unwrap();
    let assembly = emit_m6800_assembly_with_options(&program, m6800_options()).unwrap();

    assemble_subset_with_symbols_at(AssemblerCpu::M6800, &assembly, 0)
        .unwrap_or_else(|error| panic!("{error}\n{assembly}"));
    assert_eq!(assembly.matches("jsr _next").count(), 2, "{assembly}");
    assert!(!assembly.contains("jsr _record_pair"), "{assembly}");
    assert!(!assembly.contains("jsr _nested_pair"), "{assembly}");
    assert!(!assembly.contains("_record_pair:"), "{assembly}");
    assert!(!assembly.contains("_nested_pair:"), "{assembly}");
    assert!(assembly.contains("_ready:"), "{assembly}");
    assert_eq!(assembly.matches("jsr _ready").count(), 1, "{assembly}");
}

#[test]
fn lowers_single_bit_mask_branches_to_bita_without_skipping_evaluation() {
    let program = parse_program(
        Path::new("m6800_bit_test.ezra"),
        r#"
                global calls: u8 = 0

                fn next() -> u8 {
                    calls += 1
                    return calls
                }

                fn main() -> u8 {
                    if (next() & 1) == 0 { calls += 2 }
                    while (next() & 2) != 0 { break }
                    return calls
                }
            "#,
    )
    .unwrap();
    let assembly = emit_m6800_assembly_with_options(&program, m6800_options()).unwrap();

    assert_eq!(assembly.matches("    bita #01h\n").count(), 1, "{assembly}");
    assert_eq!(assembly.matches("    bita #02h\n").count(), 1, "{assembly}");
    assert_eq!(assembly.matches("jsr _next").count(), 2, "{assembly}");
    assert!(!assembly.contains("anda >__ezra_r1"), "{assembly}");
    assemble_subset_with_symbols_at(AssemblerCpu::M6800, &assembly, 0)
        .unwrap_or_else(|error| panic!("{error}\n{assembly}"));
}

#[test]
fn rejects_wide_scalar_storage() {
    let program = parse_program(
        Path::new("m6800_test.ezra"),
        "fn main() -> u16 { let wide: u16 = 1 return wide }",
    )
    .unwrap();
    let error = emit_m6800_assembly_with_options(&program, m6800_options()).unwrap_err();
    assert!(error.message.contains("8-bit integer and bool"));
}
