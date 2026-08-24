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
fn lowers_global_and_local_typed_function_pointers() {
    let program = parse_program(
        Path::new("m6800_function_pointer.ezra"),
        "global callback: ptr<fn(u8, u8)u8> = &add global answer: u8 = 0 fn add(left: u8, right: u8) -> u8 { return left + right } fn main() { let local: ptr<fn(u8, u8)u8> = &add; answer = callback(20, 22); answer = local(20, 22) }",
    )
    .unwrap();
    let assembly = emit_m6800_assembly_with_options(&program, m6800_options()).unwrap();
    assert_eq!(assembly.matches("    jsr _add").count(), 2, "{assembly}");
    assert!(assembly.contains("__ezra_fn_ptr_add:"), "{assembly}");
    assert!(assembly.contains("    jmp _add"), "{assembly}");
    assemble_subset_with_symbols_at(AssemblerCpu::M6800, &assembly, 0)
        .unwrap_or_else(|error| panic!("{error}\n{assembly}"));
}

#[cfg(feature = "m6809")]
#[test]
fn lowers_m6809_global_and_local_typed_function_pointers() {
    let program = parse_program(
        Path::new("m6809_function_pointer.ezra"),
        "global callback: ptr<fn(u8, u8)u8> = &add global answer: u8 = 0 fn add(left: u8, right: u8) -> u8 { return left + right } fn main() { let local: ptr<fn(u8, u8)u8> = &add; answer = callback(20, 22); answer = local(20, 22) }",
    )
    .unwrap();
    let assembly = emit_m6809_assembly_with_options(&program, m6809_options()).unwrap();
    assert_eq!(assembly.matches("    jsr 0,x").count(), 2, "{assembly}");
    assert!(assembly.contains("__ezra_fn_ptr_add:"), "{assembly}");
    assert!(assembly.contains("    jmp _add"), "{assembly}");
    assemble_subset_with_symbols_at(AssemblerCpu::M6809, &assembly, 0)
        .unwrap_or_else(|error| panic!("{error}\n{assembly}"));
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

fn local_initializer_operand(assembly: &str, value: u8) -> String {
    let marker = format!("    ldaa #{value:02X}h\n");
    let rest = assembly
        .split_once(&marker)
        .unwrap_or_else(|| panic!("missing local initializer {value:02X}\n{assembly}"))
        .1;
    rest.lines()
        .find_map(|line| line.trim().strip_prefix("staa "))
        .map(str::to_owned)
        .unwrap_or_else(|| panic!("missing local store {value:02X}\n{assembly}"))
}

fn assert_reuses_frame_local(assembly: &str, cpu: AssemblerCpu) {
    let first = local_initializer_operand(assembly, 0x11);
    let second = local_initializer_operand(assembly, 0x22);
    assert_eq!(first, second, "{assembly}");
    assert!(first.ends_with(",x") || first.ends_with(",u"), "{assembly}");
    assemble_subset_with_symbols_at(cpu, assembly, 0)
        .unwrap_or_else(|error| panic!("{error}\n{assembly}"));
}

#[test]
fn colors_nonoverlapping_m6800_locals_into_one_frame_byte() {
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
    let mut options = m6800_options();
    options.optimization = crate::optimization::OptimizationOptions::new(0).unwrap();
    let assembly = emit_m6800_assembly_with_options(&program, options).unwrap();
    assert_reuses_frame_local(&assembly, AssemblerCpu::M6800);
}

#[cfg(feature = "m6809")]
#[test]
fn colors_nonoverlapping_m6809_locals_into_one_frame_byte() {
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
    let mut options = m6809_options();
    options.optimization = crate::optimization::OptimizationOptions::new(0).unwrap();
    let assembly = emit_m6809_assembly_with_options(&program, options).unwrap();
    assert_reuses_frame_local(&assembly, AssemblerCpu::M6809);
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
    assert!(assembly.contains("adda 1,x"), "{assembly}");
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
fn initializes_const_char_arrays_and_reads_them_through_runtime_indexes() {
    let result = run_main_result(
        r#"
            const LETTERS: [u8; 4] = ['E', 'Z', 'R', 'A']
            const UNUSED: [u8; 2] = ['x', 'y']

            fn read(index: u8) -> u8 {
                return LETTERS[index]
            }

            fn main() -> u8 {
                return read(1)
            }
        "#,
    );
    assert_eq!(result, b'Z');
}

#[cfg(feature = "m6809")]
#[test]
fn initializes_and_indexes_const_char_arrays_on_m6809() {
    let program = parse_program(
        Path::new("m6809_const_array.ezra"),
        r#"
            const LETTERS: [u8; 4] = ['E', 'Z', 'R', 'A']
            const UNUSED: [u8; 2] = ['x', 'y']
            fn read(index: u8) -> u8 { return LETTERS[index] }
            fn main() { let value: u8 = read(1) }
        "#,
    )
    .unwrap();
    let assembly = emit_m6809_assembly_with_options(&program, m6809_options()).unwrap();
    assert!(assembly.contains("    ldaa #45h"), "{assembly}");
    assert!(assembly.contains("    ldaa #5Ah"), "{assembly}");
    assert!(assembly.contains("    ldaa 0,x"), "{assembly}");
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
                instruction_budget: 2_000,
                initial_ports: Vec::new(),
                initial_memory: Vec::new(),
                stack_top: 0x7fff,
            },
        )
        .unwrap();
    assert!(run.halted, "run={run:?}\n{assembly}");
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
    assert!(!assembly.contains("__ezra_r1"), "{assembly}");
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

#[test]
fn executes_two_result_calls_with_a_and_b_abi() {
    assert_eq!(
        run_main_result(
            r#"
                global first_seen: u8 = 0
                global second_seen: bool = false

                fn main() -> u8 {
                    let first: u8, second: bool = pair(41)
                    first_seen = first
                    second_seen = second
                    return first_seen * 2 + cast<u8>(second_seen)
                }

                fn pair(value: u8) -> u8, bool {
                    return value + 1, value == 41
                }
            "#,
        ),
        85
    );
}

#[test]
fn rejects_two_result_calls_in_single_result_context() {
    let program = parse_program(
        Path::new("m6800-two-result.ezra"),
        "fn pair() -> u8, bool { return 1, true } fn main() { let value: u8 = pair() }",
    )
    .unwrap();
    let error = emit_m6800_assembly_with_options(&program, m6800_options()).unwrap_err();
    assert!(
        error
            .message
            .contains("two-result call `pair` may only be used"),
        "{error}"
    );
}

#[test]
fn lowers_m6800_catalog_bit_integer_and_memory_intrinsics() {
    let program = parse_program(
        Path::new("m6800_intrinsics.ezra"),
        r#"
            global source: u8 = 1
            global destination: u8 = 0
            fn main() {
                let rotated: u8 = bits.rotate_left(0x81u8, 1u8)
                let tested: bool = bits.test(0x20u8, 5u8)
                let set: u8 = bits.set(1u8, 3u8)
                let clear: u8 = bits.clear(0xFFu8, 3u8)
                let toggled: u8 = bits.toggle(0u8, 2u8)
                let extracted: u8 = bits.extract(0xD8u8, 3u8, 3u8)
                let inserted: u8 = bits.insert(0u8, 7u8, 2u8, 3u8)
                let reversed: u8 = bits.reverse(0x12u8)
                let ones: u8 = bits.count_ones(0xF0u8)
                let leading: u8 = bits.leading_zeros(0x10u8)
                let trailing: u8 = bits.trailing_zeros(0x10u8)
                let high: u8 = int.mul_high(0x12u8, 0x10u8)
                let quotient: u8, remainder: u8 = int.divmod(100u8, 7u8)
                let sum: u8, carry: bool = int.add_carry(0xFFu8, 1u8, false)
                let low: u8, product_high: u8 = int.full_mul(0x12u8, 0x10u8)
                let saturated: u8 = int.saturating_add(250u8, 20u8)
                let compared: i8 = mem.compare(&source, &destination, 1u24)
                mem.poke8(&destination, 0x2Au8)
                let loaded: u8 = mem.peek8(&destination)
                mem.copy_nonoverlapping(&destination, &source, 1u24)
                mem.move(&destination, &destination, 1u24)
                mem.fill(&destination, 0xAAu8, 1u24)
            }
        "#,
    )
    .unwrap();
    let mut options = m6800_options();
    options.optimization = crate::optimization::OptimizationOptions::new(0).unwrap();
    options
        .optimization
        .disable(crate::optimization::OptimizationPass::DeadCodeElimination);
    let assembly = emit_m6800_assembly_with_options(&program, options).unwrap();
    for instruction in [
        "    asla",
        "    bita #20h",
        "    oraa #08h",
        "    anda #F7h",
        "    eora #04h",
    ] {
        assert!(
            assembly.contains(instruction),
            "missing {instruction}:\n{assembly}"
        );
    }
    assert!(
        !assembly.contains("    mul\n"),
        "M6800 must use the software multiply helper"
    );
    assemble_subset_with_symbols_at(AssemblerCpu::M6800, &assembly, 0)
        .unwrap_or_else(|error| panic!("{error}\n{assembly}"));
}

#[cfg(feature = "m6809")]
#[test]
fn lowers_m6809_native_full_multiply_and_shared_intrinsics() {
    let program = parse_program(
        Path::new("m6809_intrinsics.ezra"),
        r#"
            global source: u8 = 3
            fn main() {
                let low: u8, high: u8 = int.full_mul(0x12u8, 0x10u8)
                let high_only: u8 = int.mul_high(0x12u8, 0x10u8)
                let quotient: u8, remainder: u8 = int.divmod(100u8, 7u8)
                let sum: u8, carry: bool = int.add_carry(0xFFu8, 1u8, false)
                mem.fill(&source, 0u8, 1u24)
            }
        "#,
    )
    .unwrap();
    let assembly = emit_m6809_assembly_with_options(&program, m6809_options()).unwrap();
    assert!(assembly.contains("    mul\n"), "{assembly}");
    assert!(assembly.contains("__ezra_intrinsic_div_loop"), "{assembly}");
    assert!(
        assembly.contains("__ezra_intrinsic_fill_loop"),
        "{assembly}"
    );
    assemble_subset_with_symbols_at(AssemblerCpu::M6809, &assembly, 0)
        .unwrap_or_else(|error| panic!("{error}\n{assembly}"));
}

#[test]
fn rejects_m6800_bit_indices_outside_u8() {
    let program = parse_program(
        Path::new("m6800_bad_intrinsic.ezra"),
        "fn main() { let value: u8 = bits.test(1u8, 8u8) }",
    )
    .unwrap();
    let error = emit_m6800_assembly_with_options(&program, m6800_options()).unwrap_err();
    assert!(error.message.contains("within the input width"), "{error}");
}

#[test]
fn rejects_two_result_functions_that_can_fall_through() {
    let program = parse_program(
        Path::new("m6800-two-result-fallthrough.ezra"),
        "fn pair(value: bool) -> u8, bool { if value { return 1, true } } fn main() { let first: u8, second: bool = pair(true) }",
    )
    .unwrap();
    let error = emit_m6800_assembly_with_options(&program, m6800_options()).unwrap_err();
    assert!(
        error
            .message
            .contains("missing two return values in function `pair`"),
        "{error}"
    );
}

#[test]
fn rejects_mixed_signedness_and_mismatched_intrinsic_integer_types() {
    let cases = [
        (
            "let value: u8 = int.saturating_add(1u8, 1i8)",
            "same exact integer type",
        ),
        (
            "let value: u8 = int.saturating_sub(1u8, 1i8)",
            "same exact integer type",
        ),
        (
            "let value: u8 = int.saturating_add(1u8, 1u16)",
            "same exact integer type",
        ),
        (
            "let value: u8 = int.mul_high(1u8, 1i8)",
            "same exact integer type",
        ),
        (
            "let first: u8, second: u8 = int.full_mul(1u8, 1i8)",
            "same exact integer type",
        ),
        (
            "let first: u8, second: u8 = int.divmod(1u8, 1i8)",
            "same exact integer type",
        ),
        (
            "let first: u8, second: bool = int.add_carry(1u8, 1i8, false)",
            "same exact integer type",
        ),
        (
            "let first: u8, second: bool = int.sub_borrow(1u8, 1i8, false)",
            "same exact integer type",
        ),
        (
            "let value: u8 = int.widening_mul(1u8, 1i8)",
            "matching signedness",
        ),
    ];

    for (statement, expected) in cases {
        let program = parse_program(
            Path::new("m6800_intrinsic_type_mismatch.ezra"),
            &format!("fn main() {{ {statement} }}"),
        )
        .unwrap();
        let error = emit_m6800_assembly_with_options(&program, m6800_options()).unwrap_err();
        assert!(
            error.message.contains(expected),
            "expected `{expected}` in `{error}` for `{statement}`"
        );
    }
}

#[test]
fn rejects_intrinsic_constant_indices_and_ranges_outside_u8_bounds() {
    let cases = [
        (
            "fn main() { let value: u8 = bits.test(1u8, 8u8) }",
            "within the input width",
        ),
        (
            "fn main() { let value: u8 = bits.test(1u8, -1) }",
            "within the input width",
        ),
        (
            "fn main() { let value: u8 = bits.extract(1u8, 7u8, 2u8) }",
            "inside the input width",
        ),
        (
            "const WIDTH: u8 = 0u8\nfn main() { let value: u8 = bits.extract(1u8, 0u8, WIDTH) }",
            "inside the input width",
        ),
    ];

    for (source, expected) in cases {
        let program =
            parse_program(Path::new("m6800_intrinsic_constant_bounds.ezra"), source).unwrap();
        let error = emit_m6800_assembly_with_options(&program, m6800_options()).unwrap_err();
        assert!(
            error.message.contains(expected),
            "expected `{expected}` in `{error}` for `{source}`"
        );
    }
}
