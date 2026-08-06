use super::*;
use crate::{
    asm::AssemblyOptions,
    parser::parse_program,
    target::CpuFamily,
    vm::{
        TestImage, TestRunOptions, TestRunner, assemble_subset_at, assemble_subset_with_symbols_at,
    },
};
use std::path::Path;
fn emit(source: &str) -> String {
    let program = parse_program(Path::new("dcpu.ezra"), source).unwrap();
    let assembly = emit_dcpu_assembly_with_options(
        &program,
        AssemblyOptions {
            cpu: CpuFamily::Dcpu,
            ram_base: crate::target::Address24::new(0x8000),
            rodata_base: crate::target::Address24::new(0x9000),
            asset_base: crate::target::Address24::new(0xa000),
            ..AssemblyOptions::default()
        },
    )
    .unwrap();
    assemble_subset_with_symbols_at(CpuFamily::Dcpu.into(), &assembly, 0)
        .unwrap_or_else(|error| panic!("{error}\n{assembly}"));
    assembly
}
#[test]
fn functions_control_flow_and_frames_assemble() {
    let assembly = emit(
        "fn add(a: u16, b: u16) -> u16 { let n: u16 = a + b; return n } fn main() { let n: u16 = 0; while n < 3 { n += add(n, 1) } }",
    );
    assert!(assembly.contains("_add:"));
    assert!(assembly.contains("__ezra_while"));
}
#[test]
fn globals_arrays_structs_pointers_strings_and_mmio_assemble() {
    let assembly = emit(
        "struct Pair { left: u16 right: u16 } global values: [u16; 2] = [1, 2] global pair: Pair = Pair { left: 3, right: 4 } mmio SCREEN: u16 = 0x8000 fn main() { values[1] = 7; let equal: bool = values[0] == values[1]; let unequal: bool = values[0] != values[1]; pair.right += 1; let p: ptr<u16> = &values[0]; *p = 9; let s: ptr<u8> = \"ok\"; SCREEN = cast<u16>(*s) }",
    );
    assert!(assembly.contains("__ezra_global_values:"));
    assert!(assembly.contains("__ezra_string_"));
    assert!(assembly.contains("    ife b, a"));
    assert!(assembly.contains("    ifn b, a"));
    assert!(!assembly.contains("    and a, 0x00ff"));
}
#[test]
fn lowers_masked_conditions_without_materializing_booleans() {
    let assembly = emit(
        r#"
            fn masked_conditions(word: u16, byte: u8) -> u16 {
                if (word & 0x0300u16) == 0 {
                    let clear: u16 = word
                }
                if (word & 0x0300u16) != 0 {
                    let any_set: u16 = word
                }
                if (word & 0x0300u16) == 0x0300u16 {
                    let all_set: u16 = word
                }
                if (word & 0x0300u16) != 0x0300u16 {
                    let not_all_set: u16 = word
                }
                while (byte & 0x20u8) == 0 {
                    break
                }
                while (byte & 0x20u8) != 0 {
                    break
                }
                return 0
            }
            fn main() {
                masked_conditions(0x0300u16, 0x20u8)
            }
            "#,
    );
    assert!(
        assembly.contains("    ifb a, 0x0300\n    set pc, __ezra_if_else_"),
        "{assembly}"
    );
    assert!(
        assembly.contains("    ifc a, 0x0300\n    set pc, __ezra_if_else_"),
        "{assembly}"
    );
    assert!(
        assembly.contains("    and a, 0x0300\n    ifn a, 0x0300\n    set pc, __ezra_if_else_"),
        "{assembly}"
    );
    assert!(
        assembly.contains("    and a, 0x0300\n    ife a, 0x0300\n    set pc, __ezra_if_else_"),
        "{assembly}"
    );
    assert!(
        assembly.contains("    ifb a, 0x0020\n    set pc, __ezra_while_end_"),
        "{assembly}"
    );
    assert!(
        assembly.contains("    ifc a, 0x0020\n    set pc, __ezra_while_end_"),
        "{assembly}"
    );
    assert!(!assembly.contains("__ezra_true_"), "{assembly}");
}

#[test]
fn keeps_mmio_mask_tests_to_one_word_read() {
    let assembly = emit(
        "volatile mmio STATUS: u16 = 0x9000 fn main() { if (STATUS & 0x0004u16) != 0 { STATUS = 1 } }",
    );
    assert_eq!(
        assembly.matches("    set a, [0x9000]").count(),
        1,
        "{assembly}"
    );
    assert!(
        assembly.contains("    set a, [0x9000]\n    ifc a, 0x0004"),
        "{assembly}"
    );
}

#[test]
fn uses_native_word_immediates_and_narrows_byte_masks() {
    let assembly = emit(
        r#"
            fn operations(word: u16, byte: u8) -> u16 {
                let masked: u16 = word & 0x00ffu16
                let sum: u16 = word + 1u16
                let narrow: u8 = byte & 0x0fu8
                return masked + sum + cast<u16>(narrow)
            }
            fn main() {
                let result: u16 = operations(0x1234u16, 0x2fu8)
            }
            "#,
    );
    assert!(assembly.contains("    and a, 0x00ff"), "{assembly}");
    assert!(assembly.contains("    add a, 0x0001"), "{assembly}");
    assert!(assembly.contains("    and a, 0x000f"), "{assembly}");
    assert!(
        !assembly.contains("    set push, a\n    set a, 0x0001\n    set b, pop\n    add b, a"),
        "{assembly}"
    );
}

#[test]
fn keeps_straight_scalar_locals_in_registers_without_a_local_frame() {
    let assembly = emit(
        "volatile mmio INPUT: u16 = 0x8000 volatile mmio OUTPUT: u16 = 0x8001 fn main() { let first: u16 = INPUT; let second: u16 = INPUT; first += second; OUTPUT = first; OUTPUT = second }",
    );

    assert!(!assembly.contains("    sub sp,"), "{assembly}");
    assert!(assembly.contains("    set c, a"), "{assembly}");
    assert!(assembly.contains("    set x, a"), "{assembly}");
    assert!(assembly.contains("    set a, c"), "{assembly}");
    assert!(assembly.contains("    set a, x"), "{assembly}");
}

#[test]
fn spills_values_live_across_calls() {
    let assembly = emit(
        "volatile mmio INPUT: u16 = 0x8000 volatile mmio OUTPUT: u16 = 0x8001 fn touch(value: u16) -> u16 { return value } fn main() { let live: u16 = INPUT; touch(0); OUTPUT = live }",
    );

    assert!(
        assembly.contains("_main:\n    set push, j\n    set j, sp\n    sub sp, 1"),
        "{assembly}"
    );
    assert!(assembly.contains("    set [j + -1], a"), "{assembly}");
    assert!(assembly.contains("    set a, [j + -1]"), "{assembly}");
}

#[test]
fn spills_values_live_across_inline_asm() {
    let assembly = emit(
        "volatile mmio INPUT: u16 = 0x8000 volatile mmio OUTPUT: u16 = 0x8001 fn main() { let live: u16 = INPUT; asm volatile(clobber memory) { \"set c, 0\" \"set x, 0\" \"set y, 0\" \"set z, 0\" } OUTPUT = live }",
    );

    assert!(
        assembly.contains("_main:\n    set push, j\n    set j, sp\n    sub sp, 1"),
        "{assembly}"
    );
    assert!(assembly.contains("    set [j + -1], a"), "{assembly}");
    assert!(assembly.contains("    set a, [j + -1]"), "{assembly}");
}

#[test]
fn address_taken_and_aggregate_locals_use_frame_slots() {
    let assembly = emit(
        "fn main() -> u16 { let scalar: u16 = 5; let pointer: ptr<u16> = &scalar; let values: [u16; 2] = [2, 3]; return *pointer + values[1] }",
    );

    assert!(
        assembly.contains("_main:\n    set push, j\n    set j, sp\n    sub sp, 3"),
        "{assembly}"
    );
    assert!(assembly.contains("    set [j + -1], a"), "{assembly}");
    assert!(assembly.contains("    set [j + -3 + 0], a"), "{assembly}");
    assert!(assembly.contains("    set [j + -3 + 1], a"), "{assembly}");
}

#[test]
fn reuses_non_overlapping_aggregate_spill_slots() {
    let assembly = emit(
        "global sink: u16 = 0 fn main() { let first: [u16; 2] = [1, 2]; sink = first[0]; let second: [u16; 2] = [3, 4]; sink = second[1] }",
    );

    assert!(
        assembly.contains("_main:\n    set push, j\n    set j, sp\n    sub sp, 2"),
        "{assembly}"
    );
    assert!(!assembly.contains("    sub sp, 4"), "{assembly}");
    assert_eq!(
        assembly.matches("    set [j + -2 + 0], a").count(),
        2,
        "{assembly}"
    );
}

#[test]
fn allocated_arithmetic_and_control_flow_execute_on_dcpu() {
    let assembly = emit(
        r#"
            fn bump(value: u16) -> u16 { return value + 1 }
            fn main() -> u16 {
                let total: u16 = 2
                let index: u16 = 0
                while index < 4 {
                    total += bump(index)
                    index += 1
                }
                if total == 12 { return total }
                return 1
            }
            "#,
    );
    let assembly = assembly.replace(
        "__ezra_exit:\n    set pc, __ezra_exit\n",
        "__ezra_exit:\n    set [0xfff2], a\n    set [0xfff3], 1\n",
    );
    let bytes = assemble_subset_at(CpuFamily::Dcpu, &assembly, 0)
        .unwrap_or_else(|error| panic!("{error}\n{assembly}"));
    let run = TestRunner::default()
        .run(
            &TestImage {
                cpu_family: CpuFamily::Dcpu,
                base_addr: 0,
                bytes,
            },
            &TestRunOptions {
                instruction_budget: 2_000,
                initial_ports: Vec::new(),
                initial_memory: Vec::new(),
                stack_top: 0x1_fffe,
            },
        )
        .unwrap();

    assert!(run.halted, "{run:?}\n{assembly}");
    assert_eq!(run.result_code, 12, "{assembly}");
    assert_eq!(run.failure, None, "{assembly}");
}

#[test]
fn memory_fill_executes_with_destination_and_zero_count() {
    let assembly = emit(
        r#"
            volatile mmio DEBUG_SEQUENCE: u16 = 0xfff0
            volatile mmio DEBUG_VALUE: u8 = 0xfff1
            volatile mmio RESULT: u8 = 0xfff2
            volatile mmio HALT: u8 = 0xfff3
            global words: [u8; 4] = [0x11u8, 0x22u8, 0x33u8, 0x44u8]

            fn main() {
                mem.fill(cast<ptr<u8>>(&words[0]), 0x5au8, 3u24)
                DEBUG_VALUE = words[0]
                DEBUG_SEQUENCE = 1
                DEBUG_VALUE = words[1]
                DEBUG_SEQUENCE = 2
                DEBUG_VALUE = words[2]
                DEBUG_SEQUENCE = 3

                mem.fill(cast<ptr<u8>>(&words[3]), 0xa5u8, 0u24)
                DEBUG_VALUE = words[3]
                DEBUG_SEQUENCE = 4
                RESULT = 0
                HALT = 1
            }
            "#,
    );
    let bytes = assemble_subset_at(CpuFamily::Dcpu, &assembly, 0)
        .unwrap_or_else(|error| panic!("{error}\n{assembly}"));
    let run = TestRunner::default()
        .run(
            &TestImage {
                cpu_family: CpuFamily::Dcpu,
                base_addr: 0,
                bytes,
            },
            &TestRunOptions {
                instruction_budget: 1_000,
                initial_ports: Vec::new(),
                initial_memory: Vec::new(),
                stack_top: 0x1_fffe,
            },
        )
        .unwrap();

    assert!(run.halted, "{run:?}\n{assembly}");
    assert_eq!(run.debug_output, [0x5a, 0x5a, 0x5a, 0x44], "{assembly}");
    assert_eq!(run.result_code, 0, "{assembly}");
    assert_eq!(run.failure, None, "{assembly}");
}

#[test]
fn sdk_style_inline_operands_assemble() {
    let assembly = emit(
        "inline fn command(value: u16) { asm volatile(in value: u16 as reg16, clobber memory) { \"set a, 0\" \"set b, {value}\" \"hwi 2\" } } fn main() { command(60) }",
    );
    assert!(assembly.contains("hwi 2"));
}

#[test]
fn void_calls_remain_invalid_in_value_contexts() {
    let program = parse_program(
        Path::new("dcpu.ezra"),
        "fn command() {} fn main() { let value: u16 = command() }",
    )
    .unwrap();
    let error = emit_dcpu_assembly_with_options(
        &program,
        AssemblyOptions {
            cpu: CpuFamily::Dcpu,
            ram_base: crate::target::Address24::new(0x8000),
            ..AssemblyOptions::default()
        },
    )
    .unwrap_err();
    assert_eq!(error.message, "DCPU function `command` has no value return");
}

#[test]
fn ports_remain_rejected() {
    let program = parse_program(Path::new("dcpu.ezra"), "port P: u8 = 1 fn main() {}").unwrap();
    let error = emit_dcpu_assembly_with_options(
        &program,
        AssemblyOptions {
            cpu: CpuFamily::Dcpu,
            ram_base: crate::target::Address24::new(0x8000),
            ..AssemblyOptions::default()
        },
    )
    .unwrap_err();
    assert!(error.message.contains("port"));
}

#[test]
fn lowers_catalog_bit_and_integer_intrinsics() {
    let assembly = emit(
        r#"
            fn main() {
                let rotated: u16 = bits.rotate_left(0x1234u16, 4u8)
                let set_bit: u8 = bits.set(1u8, 3u8)
                let high: u16 = int.mul_high(0x1234u16, 2u16)
            }
        "#,
    );
    assert!(assembly.contains("    shl a, b"), "{assembly}");
    assert!(assembly.contains("    shr a, b"), "{assembly}");
    assert!(assembly.contains("    mul b, a"), "{assembly}");
}

#[test]
fn lowers_catalog_paired_and_memory_intrinsics() {
    let assembly = emit(
        r#"
            global words: [u8; 4] = [0x11u8, 0x22u8, 0x33u8, 0x44u8]
            fn main() {
                let low: u16, high: u16 = int.full_mul(0x1234u16, 2u16)
                let value: u16 = mem.load_be16(cast<ptr<u8>>(&words[0]))
                mem.fill(cast<ptr<u8>>(&words[0]), 0x55u8, 2u24)
            }
        "#,
    );
    assert!(assembly.contains("    mul b, a"), "{assembly}");
    assert!(assembly.contains("    set a, ex"), "{assembly}");
    assert!(assembly.contains("    set [b], a"), "{assembly}");
    assert!(assembly.contains("__ezra_mem_fill_loop"), "{assembly}");
}

#[test]
fn rejects_catalog_wide_results_and_volatile_block_access() {
    let wide = parse_program(
        Path::new("dcpu.ezra"),
        "fn main() { let value: u24 = mem.load_le24(cast<ptr<u8>>(0x8000)) }",
    )
    .unwrap();
    let error = emit_dcpu_assembly_with_options(
        &wide,
        AssemblyOptions {
            cpu: CpuFamily::Dcpu,
            ram_base: crate::target::Address24::new(0x8000),
            ..AssemblyOptions::default()
        },
    )
    .unwrap_err();
    assert_eq!(
        error.message,
        "DCPU does not support type `u24`; values must fit in one word"
    );

    let volatile = parse_program(
        Path::new("dcpu.ezra"),
        "volatile mmio DEVICE: ptr<u8> = 0x9000 fn main() { let value: u16 = mem.load_le16(DEVICE) }",
    )
    .unwrap();
    let error = emit_dcpu_assembly_with_options(
        &volatile,
        AssemblyOptions {
            cpu: CpuFamily::Dcpu,
            ram_base: crate::target::Address24::new(0x8000),
            ..AssemblyOptions::default()
        },
    )
    .unwrap_err();
    assert!(error.message.contains("volatile"), "{}", error.message);
}

#[test]
fn preserves_two_results_through_user_function_calls() {
    let assembly = emit(
        r#"
            fn pair(seed: u16) -> u16, u8 {
                return seed + 1, cast<u8>(seed + 3)
            }
            fn forward(seed: u16) -> u16, u8 {
                return pair(seed)
            }
            fn main() -> u16 {
                let first: u16, second: u8 = forward(40)
                return first + cast<u16>(second)
            }
            "#,
    );
    assert!(assembly.contains("    set ex, a"), "{assembly}");
    assert!(assembly.contains("    set a, ex"), "{assembly}");
    assert!(assembly.contains("    set b, ex"), "{assembly}");

    let assembly = assembly.replace(
        "__ezra_exit:\n    set pc, __ezra_exit\n",
        "__ezra_exit:\n    set [0xfff2], a\n    set [0xfff3], 1\n",
    );
    let bytes = assemble_subset_at(CpuFamily::Dcpu, &assembly, 0)
        .unwrap_or_else(|error| panic!("{error}\n{assembly}"));
    let run = TestRunner::default()
        .run(
            &TestImage {
                cpu_family: CpuFamily::Dcpu,
                base_addr: 0,
                bytes,
            },
            &TestRunOptions {
                instruction_budget: 1_000,
                initial_ports: Vec::new(),
                initial_memory: Vec::new(),
                stack_top: 0x1_fffe,
            },
        )
        .unwrap();

    assert!(run.halted, "{run:?}\n{assembly}");
    assert_eq!(run.result_code, 84, "{assembly}");
    assert_eq!(run.failure, None, "{assembly}");
}
