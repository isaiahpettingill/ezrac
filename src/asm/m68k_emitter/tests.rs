use super::*;
use crate::{
    asm::AssemblyOptions,
    parser::parse_program,
    target::{AssemblerCpu, CpuFamily},
    vm::assemble_subset_with_symbols_at,
};
use std::path::Path;

#[cfg(feature = "test-runner")]
fn emit_and_run(source: &str, instruction_budget: u64) -> (String, crate::vm::TestRun) {
    use crate::vm::{TestImage, TestRunOptions, TestRunner, assemble_subset_at};

    let program = parse_program(Path::new("test.ezra"), source).unwrap();
    let options = AssemblyOptions {
        cpu: CpuFamily::M68k,
        ..AssemblyOptions::default()
    };
    let base_addr = options.code_base.get();
    let stack_top = options.stack_top.get();
    let assembly = emit_m68k_assembly_with_options(&program, options).unwrap();
    assemble_subset_at(CpuFamily::M68k, &assembly, base_addr).unwrap();
    let main = &assembly[assembly
        .find("_main:\n")
        .expect("missing M68k main function")..];
    let bytes = assemble_subset_at(CpuFamily::M68k, main, base_addr).unwrap();
    let run = TestRunner::default()
        .run(
            &TestImage {
                cpu_family: CpuFamily::M68k,
                base_addr,
                bytes,
            },
            &TestRunOptions {
                instruction_budget,
                initial_ports: Vec::new(),
                initial_memory: Vec::new(),
                stack_top,
            },
        )
        .unwrap();
    (assembly, run)
}

#[cfg(feature = "test-runner")]
#[test]
fn nested_binary_right_expressions_preserve_outer_operands() {
    let (assembly, run) = emit_and_run(
        r#"
            volatile mmio DEBUG: u8 = 0xFFFFF0
            volatile mmio HALT: u8 = 0xFFFFF2
            fn main() {
                let a: u32 = 10
                let b: u32 = 7
                let c: u32 = 6
                let d: u32 = 3
                DEBUG = cast<u8>(a + (b * (c - d)))
                DEBUG = cast<u8>(a - (b + (c * d)))
                HALT = 1
            }
        "#,
        2_000,
    );
    assert!(run.halted, "{run:?}\n{assembly}");
    assert_eq!(run.debug_output, [31, 0xF1], "{assembly}");
    let pushes = assembly.matches("    move.l d0,-(sp)").count();
    assert!(pushes >= 6, "{assembly}");
    assert_eq!(pushes, assembly.matches("    move.l (sp)+,d0").count());
}

#[cfg(feature = "test-runner")]
#[test]
fn signed_wide_division_uses_xor_for_quotient_and_dividend_for_remainder() {
    for (expression, expected) in [
        ("left / right", 0xFE),
        ("left / negative_right", 2),
        ("positive / negative_right", 0xFE),
        ("left % right", 0xFD),
        ("left % negative_right", 0xFD),
        ("positive % negative_right", 3),
    ] {
        let source = format!(
            r#"
                volatile mmio DEBUG: u8 = 0xFFFFF0
                volatile mmio HALT: u8 = 0xFFFFF2
                global left: i32 = 0
                global positive: i32 = 0
                global right: i32 = 0
                global negative_right: i32 = 0
                fn main() {{
                    left = -13
                    positive = 13
                    right = 5
                    negative_right = -5
                    DEBUG = cast<u8>({expression})
                    HALT = 1
                }}
            "#
        );
        let (assembly, run) = emit_and_run(&source, 2_000);
        assert!(run.halted, "{run:?}\n{assembly}");
        assert_eq!(run.debug_output, [expected], "{assembly}");
    }
}

#[cfg(feature = "test-runner")]
#[test]
fn division_by_zero_returns_zero_for_native_and_wide_results() {
    let (assembly, run) = emit_and_run(
        r#"
            volatile mmio DEBUG: u8 = 0xFFFFF0
            volatile mmio HALT: u8 = 0xFFFFF2
            global value16: u16 = 12345
            global zero16: u16 = 0
            global value32: i32 = -1234567
            global zero32: i32 = 0
            fn main() {
                value16 = 12345
                zero16 = 0
                value32 = -1234567
                zero32 = 0
                DEBUG = cast<u8>(value16 / zero16)
                DEBUG = cast<u8>(value16 % zero16)
                DEBUG = cast<u8>(value32 / zero32)
                DEBUG = cast<u8>(value32 % zero32)
                HALT = 1
            }
        "#,
        3_000,
    );
    assert!(run.halted, "{run:?}\n{assembly}");
    assert_eq!(run.debug_output, [0, 0, 0, 0], "{assembly}");
    assert!(assembly.contains("__ezra_div_u16_zero"), "{assembly}");
    assert!(assembly.contains("__ezra_div_u32_zero"), "{assembly}");
}

#[cfg(feature = "test-runner")]
#[test]
fn wide_division_has_bounded_runtime_for_large_values() {
    for expression in ["dividend / divisor", "dividend % divisor"] {
        let source = format!(
            r#"
                volatile mmio DEBUG: u8 = 0xFFFFF0
                volatile mmio HALT: u8 = 0xFFFFF2
                global dividend: u32 = 0
                global one: u32 = 0
                global divisor: u32 = 0
                fn main() {{
                    dividend = 0x7FFFFF80u32
                    one = 1
                    divisor = 0x00808080u32
                    DEBUG = cast<u8>({expression})
                    HALT = 1
                }}
            "#
        );
        let (assembly, run) = emit_and_run(&source, 2_000);
        assert!(run.halted, "{run:?}\n{assembly}");
        assert_eq!(run.debug_output.len(), 1, "{expression}\n{assembly}");
        assert!(assembly.contains("    moveq #32,d4"), "{assembly}");
        assert!(assembly.contains("    subq.w #1,d4"), "{assembly}");
        assert!(
            assembly.contains("    bne __ezra_div_u32_loop"),
            "{assembly}"
        );
    }
}

#[test]
fn lowers_global_and_local_typed_function_pointers() {
    let program = parse_program(
        Path::new("m68k_function_pointer.ezra"),
        "global callback: ptr<fn(u8, u8)u8> = &add global answer: u8 = 0 fn add(left: u8, right: u8) -> u8 { return left + right } fn main() { let local: ptr<fn(u8, u8)u8> = &add; answer = callback(20, 22); answer = local(20, 22) }",
    )
    .unwrap();
    let assembly = emit_m68k_assembly_with_options(
        &program,
        AssemblyOptions {
            cpu: CpuFamily::M68k,
            ..AssemblyOptions::default()
        },
    )
    .unwrap();
    assert_eq!(assembly.matches("    jsr (a0)").count(), 2, "{assembly}");
    assert!(assembly.contains("__ezra_fn_ptr_add:"), "{assembly}");
    assert!(assembly.contains("    jsr (_add).l"), "{assembly}");
    assemble_subset_with_symbols_at(AssemblerCpu::M68k, &assembly, 0x010040)
        .unwrap_or_else(|error| panic!("{error}\n{assembly}"));
}

#[test]
fn emits_and_assembles_scalar_control_flow() {
    let program = parse_program(
        Path::new("test.ezra"),
        r#"
            global counter: u16 = 1
            fn add(value: u16) -> u16 { return value + counter }
            fn main() { let result: u16 = add(2); if result == 3 { counter += result } }
        "#,
    )
    .unwrap();
    let assembly = emit_m68k_assembly_with_options(
        &program,
        AssemblyOptions {
            cpu: CpuFamily::M68k,
            ..AssemblyOptions::default()
        },
    )
    .unwrap();
    assert!(assembly.contains("move.l #$F00000,sp"), "{assembly}");
    assert!(assembly.contains("jsr (_add).l"), "{assembly}");
    assemble_subset_with_symbols_at(AssemblerCpu::M68k, &assembly, 0x010040).unwrap();
}

#[test]
fn emits_and_assembles_pointer_aggregate_and_string_operations() {
    let program = parse_program(
        Path::new("test.ezra"),
        r#"
            struct Pair { left: u8 right: u8 }
            global bytes: [u8; 3] = [1, 2, 3]
            global pair: Pair = Pair { left: 4, right: 5 }
            fn main() {
                let pointer: ptr<u8> = &bytes[0]
                *pointer = 9
                let value: u8 = bytes[0] ^ pair.left
                let shifted: u8 = value << 1
                let text: ptr<u8> = "ok"
                let first: u8 = *text
                bytes[1] = shifted ^ first
            }
        "#,
    )
    .unwrap();
    let assembly = emit_m68k_assembly_with_options(
        &program,
        AssemblyOptions {
            cpu: CpuFamily::M68k,
            ..AssemblyOptions::default()
        },
    )
    .unwrap();
    assert!(assembly.contains("eor.b d2,d0"), "{assembly}");
    assert!(assembly.contains("lsl.b #1,d0"), "{assembly}");
    assemble_subset_with_symbols_at(AssemblerCpu::M68k, &assembly, 0x010040).unwrap();
}

#[test]
fn lowers_one_bit_mask_branches_to_btst() {
    let program = parse_program(
        Path::new("test.ezra"),
        r#"
            fn bit_tests(byte: u8, word: u16) {
                if (byte & 0x20u8) == 0 {
                    let clear: u8 = byte
                }
                if (byte & 0x20u8) != 0 {
                    let set: u8 = byte
                }
                while (word & 0x0200u16) == 0 { break }
                while (word & 0x0200u16) != 0 { break }
            }
            fn main() {
                let byte: u8 = 0x20u8
                let word: u16 = 0
                bit_tests(byte, word)
            }
        "#,
    )
    .unwrap();
    let assembly = emit_m68k_assembly_with_options(
        &program,
        AssemblyOptions {
            cpu: CpuFamily::M68k,
            ..AssemblyOptions::default()
        },
    )
    .unwrap();

    assert_eq!(assembly.matches("    btst #5,d0").count(), 2, "{assembly}");
    assert_eq!(assembly.matches("    btst #9,d0").count(), 2, "{assembly}");
    assert!(!assembly.contains("    and.b d2,d0"), "{assembly}");
    assert!(!assembly.contains("    and.w d2,d0"), "{assembly}");
    for branch in [
        "    btst #5,d0\n    bne __ezra_if_end_",
        "    btst #5,d0\n    beq __ezra_if_end_",
        "    btst #9,d0\n    bne __ezra_while_end_",
        "    btst #9,d0\n    beq __ezra_while_end_",
    ] {
        assert!(assembly.contains(branch), "missing {branch}:\n{assembly}");
    }
}

#[test]
fn lowers_multi_bit_mask_branches_without_memory_btst() {
    let program = parse_program(
        Path::new("test.ezra"),
        r#"
            fn bit_tests(byte: u8, word: u16) {
                if (byte & 0x30u8) == 0 {
                    let clear: u8 = byte
                }
                if (byte & 0x30u8) != 0 {
                    let any_set: u8 = byte
                }
                if (word & 0x0300u16) == 0x0300u16 {
                    let all_set: u16 = word
                }
                if (word & 0x0300u16) != 0x0300u16 {
                    let not_all_set: u16 = word
                }
            }
            fn main() {
                bit_tests(0x30u8, 0x0300u16)
            }
        "#,
    )
    .unwrap();
    let assembly = emit_m68k_assembly_with_options(
        &program,
        AssemblyOptions {
            cpu: CpuFamily::M68k,
            ..AssemblyOptions::default()
        },
    )
    .unwrap();

    assert_eq!(
        assembly.matches("    andi.b #$30,d0").count(),
        2,
        "{assembly}"
    );
    assert_eq!(
        assembly.matches("    andi.w #$300,d0").count(),
        2,
        "{assembly}"
    );
    assert_eq!(
        assembly.matches("    cmpi.w #$300,d0").count(),
        2,
        "{assembly}"
    );
    assert!(!assembly.contains("    btst #"), "{assembly}");
    for branch in [
        "    andi.b #$30,d0\n    bne __ezra_if_end_",
        "    andi.b #$30,d0\n    beq __ezra_if_end_",
        "    cmpi.w #$300,d0\n    bne __ezra_if_end_",
        "    cmpi.w #$300,d0\n    beq __ezra_if_end_",
    ] {
        assert!(assembly.contains(branch), "missing {branch}:\n{assembly}");
    }
    assemble_subset_with_symbols_at(AssemblerCpu::M68k, &assembly, 0x010040)
        .unwrap_or_else(|error| panic!("{error}\n{assembly}"));
}

#[test]
fn lowers_local_single_bit_compound_assignments() {
    let program = parse_program(
        Path::new("test.ezra"),
        r#"
            struct Pair { value: u8 }
            global global_value: u8 = 0
            global bytes: [u8; 1] = [0]
            global pair: Pair = Pair { value: 0 }

            fn bit_ops(input8: u8, input16: u16, input24: u24, input32: u32) {
                let byte: u8 = input8
                let word: u16 = input16
                let triple: u24 = input24
                let long: u32 = input32
                byte |= 0x20u8
                byte &= 0xDFu8
                byte ^= 0x20u8
                word |= 0x0200u16
                word &= 0xFDFFu16
                word ^= 0x0200u16
                triple |= 0x020000u24
                triple &= 0xFDFFFFu24
                triple ^= 0x020000u24
                long |= 0x80000000u32
                long &= 0x7FFFFFFFu32
                long ^= 0x80000000u32

                global_value |= 0x20u8
                bytes[0] |= 0x20u8
                pair.value |= 0x20u8
                let pointer: ptr<u8> = &byte
                *pointer |= 0x20u8
            }

            fn main() {
                bit_ops(0, 0, 0, 0)
            }
        "#,
    )
    .unwrap();
    let assembly = emit_m68k_assembly_with_options(
        &program,
        AssemblyOptions {
            cpu: CpuFamily::M68k,
            ..AssemblyOptions::default()
        },
    )
    .unwrap();

    for instruction in [
        "    bset #5,$",
        "    bclr #5,$",
        "    bchg #5,$",
        "    bset #1,$",
        "    bclr #1,$",
        "    bchg #1,$",
        "    bset #7,$",
        "    bclr #7,$",
        "    bchg #7,$",
    ] {
        assert!(
            assembly.contains(instruction),
            "missing {instruction}:\n{assembly}"
        );
    }
    assert_eq!(assembly.matches("    bset #5,$").count(), 1, "{assembly}");
    assert_eq!(assembly.matches("    bclr #5,$").count(), 1, "{assembly}");
    assert_eq!(assembly.matches("    bchg #5,$").count(), 1, "{assembly}");
    assemble_subset_with_symbols_at(AssemblerCpu::M68k, &assembly, 0x010040)
        .unwrap_or_else(|error| panic!("{error}\n{assembly}"));
}

#[test]
fn selects_legal_immediate_constant_shifts() {
    let program = parse_program(
        Path::new("test.ezra"),
        r#"
            global sink: u16 = 0
            global signed_sink: i16 = 0
            fn main() {
                let value: u16 = 3
                sink = value << 3
                sink = sink >> 8
                let signed: i16 = -16
                signed_sink = signed >> 2
                sink = value << 0
                sink = value << 9
                sink = value * 4
            }
        "#,
    )
    .unwrap();
    let assembly = emit_m68k_assembly_with_options(
        &program,
        AssemblyOptions {
            cpu: CpuFamily::M68k,
            ..AssemblyOptions::default()
        },
    )
    .unwrap();
    assert!(assembly.contains("lsl.w #3,d0"), "{assembly}");
    assert!(assembly.contains("lsr.w #8,d0"), "{assembly}");
    assert!(assembly.contains("asr.w #2,d0"), "{assembly}");
    assert!(
        assembly.contains("lsl.w #8,d0\n    lsl.w #1,d0"),
        "{assembly}"
    );
    assert!(assembly.contains("lsl.w #2,d0"), "{assembly}");
    assert!(!assembly.contains("lsl.w #0,d0"), "{assembly}");
    assemble_subset_with_symbols_at(AssemblerCpu::M68k, &assembly, 0x010040).unwrap();
}

#[test]
fn selects_native_rotates_swaps_and_extensions() {
    let program = parse_program(
        Path::new("test.ezra"),
        r#"
            global sink16: u16 = 0
            global sink32: u32 = 0
            global signed_sink: i32 = 0

            fn rotate16(value: u16) -> u16 {
                return (value << 4) | (value >> 12)
            }
            fn rotate32_right8(value: u32) -> u32 {
                return (value >> 8) | (value << 24)
            }
            fn rotate32_half(value: u32) -> u32 {
                return (value << 16) | (value >> 16)
            }
            fn swap32(value: u32) -> u32 {
                return ((value & 0x000000FFu32) << 24)
                    | ((value & 0x0000FF00u32) << 8)
                    | ((value & 0x00FF0000u32) >> 8)
                    | ((value & 0xFF000000u32) >> 24)
            }
            fn extend8(value: i8) -> i32 { return cast<i32>(value) }
            fn extend16(value: i16) -> i32 { return cast<i32>(value) }

            fn main() {
                sink16 = rotate16(0x1234)
                sink32 = rotate32_right8(0x12345678u32)
                sink32 = rotate32_half(sink32)
                sink32 = swap32(sink32)
                signed_sink = extend8(-1)
                signed_sink = extend16(-2)
            }
        "#,
    )
    .unwrap();
    let assembly = emit_m68k_assembly_with_options(
        &program,
        AssemblyOptions {
            cpu: CpuFamily::M68k,
            ..AssemblyOptions::default()
        },
    )
    .unwrap();

    assert!(assembly.contains("    rol.w #4,d0"), "{assembly}");
    assert!(assembly.contains("    ror.l #8,d0"), "{assembly}");
    assert!(assembly.contains("_rotate32_half:"), "{assembly}");
    assert!(assembly.contains("    swap d0"), "{assembly}");
    assert!(
        assembly.contains("    ror.w #8,d0\n    swap d0\n    ror.w #8,d0"),
        "{assembly}"
    );
    assert!(
        assembly.contains("    ext.w d0\n    ext.l d0"),
        "{assembly}"
    );
    assert!(assembly.contains("    ext.l d0"), "{assembly}");
    assemble_subset_with_symbols_at(AssemblerCpu::M68k, &assembly, 0x010040)
        .unwrap_or_else(|error| panic!("{error}\n{assembly}"));
}

#[test]
fn selects_native_multiply_for_eight_and_sixteen_bit_values() {
    let program = parse_program(
        Path::new("test.ezra"),
        r#"
            global u8_sink: u8 = 0
            global i8_sink: i8 = 0
            global u16_sink: u16 = 0
            global i16_sink: i16 = 0
            fn main() {
                let u8_left: u8 = 200
                let u8_right: u8 = 3
                u8_sink = u8_left * u8_right
                let i8_left: i8 = -100
                let i8_right: i8 = 3
                i8_sink = i8_left * i8_right
                let u16_left: u16 = 40000
                let u16_right: u16 = 3
                u16_sink = u16_left * u16_right
                let i16_left: i16 = -20000
                let i16_right: i16 = 3
                i16_sink = i16_left * i16_right
                u16_sink = u16_left * 4
            }
        "#,
    )
    .unwrap();
    let assembly = emit_m68k_assembly_with_options(
        &program,
        AssemblyOptions {
            cpu: CpuFamily::M68k,
            ..AssemblyOptions::default()
        },
    )
    .unwrap();
    assert!(assembly.contains("andi.w #$FF,d0"), "{assembly}");
    assert!(assembly.contains("andi.w #$FF,d2"), "{assembly}");
    assert!(assembly.contains("ext.w d0"), "{assembly}");
    assert!(assembly.contains("ext.w d2"), "{assembly}");
    assert!(assembly.contains("mulu.w d2,d0"), "{assembly}");
    assert!(assembly.contains("muls.w d2,d0"), "{assembly}");
    assert!(assembly.contains("lsl.w #2,d0"), "{assembly}");
    assemble_subset_with_symbols_at(AssemblerCpu::M68k, &assembly, 0x010040).unwrap();
}

#[test]
fn emits_and_assembles_u24_arithmetic() {
    let program = parse_program(
        Path::new("test.ezra"),
        r#"
            global left: u24 = 0x010001
            global right: u24 = 3
            global product: u24 = 0
            global quotient: u24 = 0
            global remainder: u24 = 0
            global signed_left: i24 = -9
            global signed_right: i24 = 2
            global signed_quotient: i24 = 0
            fn main() {
                product = left * right
                quotient = product / right
                remainder = product % right
                signed_quotient = signed_left / signed_right
            }
        "#,
    )
    .unwrap();
    let assembly = emit_m68k_assembly_with_options(
        &program,
        AssemblyOptions {
            cpu: CpuFamily::M68k,
            ..AssemblyOptions::default()
        },
    )
    .unwrap();
    assert!(assembly.contains("__ezra_mul_u24_loop"), "{assembly}");
    assert!(assembly.contains("__ezra_div_u24_loop"), "{assembly}");
    assemble_subset_with_symbols_at(AssemblerCpu::M68k, &assembly, 0x010040).unwrap();
}

#[test]
fn emits_and_assembles_u32_and_i32_arithmetic() {
    let program = parse_program(
        Path::new("test.ezra"),
        r#"
            global unsigned_sink: u32 = 0
            global signed_sink: i32 = 0
            global xor_left: u32 = 0x12345678
            global xor_right: u32 = 3
            global comparison_sink: bool = false
            fn main() {
                let left: u32 = 0x12345678
                let right: u32 = 3
                unsigned_sink = ((left + right) - 1) & 0xFFFFFFFF
                unsigned_sink = unsigned_sink | (xor_left ^ xor_right)
                unsigned_sink = unsigned_sink << right
                unsigned_sink = unsigned_sink >> 2
                unsigned_sink = left * right
                unsigned_sink = unsigned_sink / right
                unsigned_sink = unsigned_sink % right
                comparison_sink = left < right
                let signed_left: i32 = -9
                let signed_right: i32 = 2
                signed_sink = signed_left >> 1
                signed_sink = signed_left * signed_right
                signed_sink = signed_left / signed_right
                comparison_sink = signed_left < signed_right
            }
        "#,
    )
    .unwrap();
    let assembly = emit_m68k_assembly_with_options(
        &program,
        AssemblyOptions {
            cpu: CpuFamily::M68k,
            ..AssemblyOptions::default()
        },
    )
    .unwrap();
    for instruction in [
        "move.l",
        "add.l d2,d0",
        "sub.l d2,d0",
        "and.l d2,d0",
        "or.l d2,d0",
        "eor.l d2,d0",
        "lsl.l #3,d0",
        "lsr.l #2,d0",
        "asr.l #1,d0",
        "cmp.l d2,d0",
    ] {
        assert!(
            assembly.contains(instruction),
            "missing {instruction}:\n{assembly}"
        );
    }
    assert!(assembly.contains("move.l #$FFFFFFFF,d0"), "{assembly}");
    assert!(assembly.contains("__ezra_mul_u32_loop"), "{assembly}");
    assert!(assembly.contains("__ezra_div_u32_loop"), "{assembly}");
    assert!(!assembly.contains("andi.l #$FFFFFF,d0"), "{assembly}");
    assemble_subset_with_symbols_at(AssemblerCpu::M68k, &assembly, 0x010040).unwrap();
}

#[test]
fn emits_and_assembles_memory_helpers() {
    let program = parse_program(
        Path::new("test.ezra"),
        r#"
            global bytes: [u8; 4] = [0, 0, 0, 0]
            fn main() {
                let pointer: ptr<u8> = &bytes[0]
                mem.poke8(pointer, 0x2A)
                let value: u8 = mem.peek8(pointer)
                mem.memset(pointer, 0, 4)
                mem.memcpy(pointer, pointer, 4)
            }
        "#,
    )
    .unwrap();
    let assembly = emit_m68k_assembly_with_options(
        &program,
        AssemblyOptions {
            cpu: CpuFamily::M68k,
            ..AssemblyOptions::default()
        },
    )
    .unwrap();
    assert!(
        assembly.contains("__ezra_intrinsic_copy_forward_"),
        "{assembly}"
    );
    assert!(
        assembly.contains("__ezra_intrinsic_fill_loop"),
        "{assembly}"
    );
    assemble_subset_with_symbols_at(AssemblerCpu::M68k, &assembly, 0x010040).unwrap();
}

#[test]
fn explicit_inline_calls_preserve_argument_and_unsafe_call_semantics() {
    let program = parse_program(
        Path::new("test.ezra"),
        r#"
            fn first() -> u8 { return 1 }
            fn second() -> u16 { return 2 }
            @inline fn bump(value: u16) -> u16 { return value + 1 }
            @inline fn pair(left: u8, right: u16) -> u16 {
                return bump(cast<u16>(left) + right)
            }
            @inline fn yes() -> bool { return true }
            fn main(flag: bool) {
                let value: u16 = pair(first(), second())
                let short: bool = flag && yes()
                while yes() { return }
            }
        "#,
    )
    .unwrap();
    let assembly = emit_m68k_assembly_with_options(
        &program,
        AssemblyOptions {
            cpu: CpuFamily::M68k,
            ..AssemblyOptions::default()
        },
    )
    .unwrap();

    assert_eq!(assembly.matches("jsr (_first).l").count(), 1, "{assembly}");
    assert_eq!(assembly.matches("jsr (_second).l").count(), 1, "{assembly}");
    assert!(
        assembly.find("jsr (_first).l").unwrap() < assembly.find("jsr (_second).l").unwrap(),
        "{assembly}"
    );
    assert!(!assembly.contains("jsr (_pair).l"), "{assembly}");
    assert!(!assembly.contains("jsr (_bump).l"), "{assembly}");
    assert_eq!(assembly.matches("jsr (_yes).l").count(), 2, "{assembly}");
    assemble_subset_with_symbols_at(AssemblerCpu::M68k, &assembly, 0x010040).unwrap();
}

#[test]
fn does_not_restore_direct_pointer_out_arguments() {
    let program = parse_program(
        Path::new("test.ezra"),
        r#"
            fn write(output: ptr<u8>) { *output = 7 }
            fn main() {
                let local: u8 = 0
                write(&local)
            }
        "#,
    )
    .unwrap();
    let assembly = emit_m68k_assembly_with_options(
        &program,
        AssemblyOptions {
            cpu: CpuFamily::M68k,
            ..AssemblyOptions::default()
        },
    )
    .unwrap();

    assert!(assembly.contains("jsr (_write).l"), "{assembly}");
    assert!(!assembly.contains("-(sp)"), "{assembly}");
    assemble_subset_with_symbols_at(AssemblerCpu::M68k, &assembly, 0x010040).unwrap();
}

#[test]
fn lowers_catalog_bits_integer_and_endian_intrinsics_with_native_m68k_ops() {
    let program = parse_program(
        Path::new("m68k_intrinsics.ezra"),
        r#"
            global source: [u8; 4] = [1, 2, 3, 4]
            global destination: [u8; 4] = [0, 0, 0, 0]
            fn main() {
                let rotated: u16 = bits.rotate_left(0x1234u16, 3u8)
                let tested: bool = bits.test(0x20u8, 5u8)
                let set: u8 = bits.set(1u8, 3u8)
                let clear: u8 = bits.clear(0xFFu8, 3u8)
                let toggled: u8 = bits.toggle(0u8, 2u8)
                let extracted: u8 = bits.extract(0xD8u8, 3u8, 3u8)
                let inserted: u8 = bits.insert(0u8, 7u8, 2u8, 3u8)
                let swapped: u24 = bits.byte_swap(0x112233u24)
                let reversed: u8 = bits.reverse(0x12u8)
                let ones: u8 = bits.count_ones(0xF0u8)
                let leading: u8 = bits.leading_zeros(0x10u8)
                let trailing: u8 = bits.trailing_zeros(0x10u8)
                let product: u16 = int.widening_mul(0x12u8, 0x10u8)
                let high: u16 = int.mul_high(0x1234u16, 2u16)
                let saturated: u8 = int.saturating_add(250u8, 20u8)
                let little: u16 = mem.load_le16(&source[0])
                let big: u24 = mem.load_be24(&source[0])
                mem.store_le24(&destination[0], 0x010203u24)
                mem.copy_nonoverlapping(&destination[0], &source[0], 4u24)
                mem.move(&destination[1], &destination[0], 3u24)
                mem.fill(&destination[0], 0xAAu8, 4u24)
            }
        "#,
    )
    .unwrap();
    let assembly = emit_m68k_assembly_with_options(
        &program,
        AssemblyOptions {
            cpu: CpuFamily::M68k,
            ..AssemblyOptions::default()
        },
    )
    .unwrap();
    for instruction in [
        "rol.w d2,d0",
        "btst #5,d0",
        "bset #3,d0",
        "bclr #3,d0",
        "bchg #2,d0",
        "mulu.w d2,d0",
        "__ezra_intrinsic_copy_backward",
        "__ezra_intrinsic_fill_loop",
    ] {
        assert!(
            assembly.contains(instruction),
            "missing {instruction}:\n{assembly}"
        );
    }
    assert!(assembly.contains("move.b 0(a0),d0"), "{assembly}");
    assert!(assembly.contains("move.b d1,0(a0)"), "{assembly}");
    assemble_subset_with_symbols_at(AssemblerCpu::M68k, &assembly, 0x010040).unwrap();
}

#[test]
fn lowers_m68k_paired_intrinsics_and_preserves_divide_by_zero_branch() {
    let program = parse_program(
        Path::new("m68k_paired_intrinsics.ezra"),
        r#"
            global source: [u8; 3] = [4, 7, 9]
            fn main() {
                let quotient: u16, remainder: u16 = int.divmod(100u16, 7u16)
                let sum: u16, carry: bool = int.add_carry(0xFFFFu16, 1u16, false)
                let difference: u16, borrow: bool = int.sub_borrow(0u16, 1u16, false)
                let low: u16, high: u16 = int.full_mul(0x1234u16, 2u16)
                let address: ptr<u8>, found: bool = mem.find_byte(&source[0], 3u24, 7u8)
            }
        "#,
    )
    .unwrap();
    let assembly = emit_m68k_assembly_with_options(
        &program,
        AssemblyOptions {
            cpu: CpuFamily::M68k,
            ..AssemblyOptions::default()
        },
    )
    .unwrap();
    assert!(assembly.contains("divu.w d2,d0"), "{assembly}");
    assert!(assembly.contains("__ezra_intrinsic_div_zero"), "{assembly}");
    assert!(assembly.contains("add.w d2,d0"), "{assembly}");
    assert!(assembly.contains("sub.w d2,d0"), "{assembly}");
    assert!(assembly.contains("mulu.w d2,d0"), "{assembly}");
    assert!(
        assembly.contains("__ezra_intrinsic_find_loop"),
        "{assembly}"
    );
    assemble_subset_with_symbols_at(AssemblerCpu::M68k, &assembly, 0x010040).unwrap();
}

#[test]
fn rejects_m68k_bit_indices_outside_the_value_width() {
    let program = parse_program(
        Path::new("m68k_bad_intrinsic.ezra"),
        "fn main() { let value: u8 = bits.test(1u8, 8u8) }",
    )
    .unwrap();
    let error = emit_m68k_assembly_with_options(
        &program,
        AssemblyOptions {
            cpu: CpuFamily::M68k,
            ..AssemblyOptions::default()
        },
    )
    .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("bit index must be within the input width"),
        "{error}"
    );
}

#[test]
fn rejects_wider_endian_intrinsics_on_known_volatile_memory() {
    let program = parse_program(
        Path::new("m68k_volatile_intrinsic.ezra"),
        r#"
            volatile mmio REGISTER: u8 = 0xFFFF00
            fn main() {
                let value: u16 = mem.load_be16(&REGISTER)
            }
        "#,
    )
    .unwrap();
    let error = emit_m68k_assembly_with_options(
        &program,
        AssemblyOptions {
            cpu: CpuFamily::M68k,
            ..AssemblyOptions::default()
        },
    )
    .unwrap_err();
    assert!(
        error.to_string().contains("ordinary nonvolatile memory"),
        "{error}"
    );
}

#[test]
fn local_target_reserves_only_a2_through_a6_for_pointers() {
    let target = m68k_local_target();
    assert_eq!(
        target
            .registers
            .iter()
            .map(|register| register.name.as_str())
            .collect::<Vec<_>>(),
        vec!["a2", "a3", "a4", "a5", "a6"]
    );
    assert_eq!(
        target.register_classes[M68K_POINTER_CLASS.0].registers,
        (0..5).map(PhysReg).collect::<Vec<_>>()
    );
    assert!(
        target.register_classes[M68K_MEMORY_CLASS.0]
            .registers
            .is_empty()
    );
    for left in 0..5 {
        for right in 0..5 {
            assert_eq!(
                target.registers_alias(PhysReg(left), PhysReg(right)),
                left == right
            );
        }
    }
}

fn emit_test_program(source: &str) -> String {
    let program = parse_program(Path::new("m68k_regalloc_test.ezra"), source).unwrap();
    emit_m68k_assembly_with_options(
        &program,
        AssemblyOptions {
            cpu: CpuFamily::M68k,
            ..AssemblyOptions::default()
        },
    )
    .unwrap()
}

#[test]
fn places_source_comments_inline_without_debug_anchors() {
    let assembly = emit_test_program(
        "// run the program\nfn main() {\n    // initialize value\n    let value: u8 = 1\n    value += 1 // increment value\n}",
    );

    assert!(assembly.contains("_main:\n; run the program"), "{assembly}");
    assert!(assembly.contains("; initialize value"), "{assembly}");
    assert!(assembly.contains("; increment value"), "{assembly}");
    assert!(!assembly.contains("; source:"), "{assembly}");
    assert!(!assembly.contains(";   initialize value"), "{assembly}");
    assert!(!assembly.contains(";   increment value"), "{assembly}");
}

fn main_assembly(assembly: &str) -> &str {
    assembly
        .split_once("_main:\n")
        .map(|(_, main)| main)
        .expect("missing main function")
}

#[test]
fn keeps_pointer_locals_in_address_registers() {
    let assembly = emit_test_program(
        r#"
                global byte: u8 = 0
                fn main() {
                    let pointer: ptr<u8> = &byte
                    *pointer = 7
                }
            "#,
    );
    let main = main_assembly(&assembly);
    assert!(main.contains("    move.l d0,a2\n"), "{assembly}");
    assert!(main.contains("    move.l a2,d0\n"), "{assembly}");
    assert!(!main.contains("move.b d0,a2"), "{assembly}");
    assert!(!main.contains("move.w d0,a2"), "{assembly}");
    assemble_subset_with_symbols_at(AssemblerCpu::M68k, &assembly, 0x010040).unwrap();
}

#[cfg(feature = "test-runner")]
#[test]
fn pointer_register_locals_execute_correctly() {
    let (assembly, run) = emit_and_run(
        r#"
                volatile mmio DEBUG: u8 = 0xFFFFF0
                volatile mmio HALT: u8 = 0xFFFFF2
                global byte: u8 = 0
                fn main() {
                    let pointer: ptr<u8> = &byte
                    *pointer = 7
                    DEBUG = byte
                    HALT = 1
                }
            "#,
        200,
    );
    assert!(run.halted, "{run:?}\n{assembly}");
    assert_eq!(run.debug_output, [7], "{assembly}");
}

fn local_byte_initializer_address(assembly: &str, value: u8) -> String {
    let marker = format!("    move.b #${value:X},d0\n    move.b d0,$");
    let rest = assembly
        .split_once(&marker)
        .unwrap_or_else(|| panic!("missing local initializer {value:X}\n{assembly}"))
        .1;
    rest[..6].to_owned()
}

#[test]
fn colors_nonoverlapping_memory_locals_into_one_static_byte() {
    let assembly = emit_test_program(
        r#"
                global sink: u8 = 0
                fn main() {
                    let first: u8 = 0x11
                    sink = first
                    let second: u8 = 0x22
                    sink = second
                }
            "#,
    );
    assert_eq!(
        local_byte_initializer_address(&assembly, 0x11),
        local_byte_initializer_address(&assembly, 0x22),
        "{assembly}"
    );
    assemble_subset_with_symbols_at(AssemblerCpu::M68k, &assembly, 0x010040).unwrap();
}

#[test]
fn spills_pointer_locals_across_calls() {
    let assembly = emit_test_program(
        r#"
                global byte: u8 = 0
                fn touch() {}
                fn main() {
                    let pointer: ptr<u8> = &byte
                    touch()
                    *pointer = 7
                }
            "#,
    );
    let main = main_assembly(&assembly);
    assert!(main.contains("    jsr (_touch).l\n"), "{assembly}");
    for register in M68K_ADDRESS_REGISTERS {
        assert!(!main.contains(&format!("a{register}")), "{assembly}");
    }
    assert!(main.matches("move.b").count() >= 6, "{assembly}");
    assemble_subset_with_symbols_at(AssemblerCpu::M68k, &assembly, 0x010040).unwrap();
}

#[test]
fn interrupt_pointer_locals_keep_full_register_preservation() {
    let assembly = emit_test_program(
        r#"
                global byte: u8 = 0
                interrupt fn irq() {
                    let pointer: ptr<u8> = &byte
                    *pointer = 7
                }
                fn main() {}
            "#,
    );
    assert!(assembly.contains("    move.l a2,-(sp)\n"), "{assembly}");
    assert!(assembly.contains("    move.l (sp)+,a2\n"), "{assembly}");
    assert!(assembly.contains("    move.l d0,a2\n"), "{assembly}");
    assert!(assembly.contains("    rte\n"), "{assembly}");
    assemble_subset_with_symbols_at(AssemblerCpu::M68k, &assembly, 0x010040).unwrap();
}

#[test]
fn spills_pointer_locals_across_inline_asm() {
    let assembly = emit_test_program(
        r#"
                global byte: u8 = 0
                fn main() {
                    let pointer: ptr<u8> = &byte
                    asm volatile(in pointer: ptr<u8> as mem, clobber memory) { "nop" }
                    *pointer = 7
                }
            "#,
    );
    let main = main_assembly(&assembly);
    assert!(main.contains("    nop\n"), "{assembly}");
    for register in M68K_ADDRESS_REGISTERS {
        assert!(!main.contains(&format!("a{register}")), "{assembly}");
    }
    assemble_subset_with_symbols_at(AssemblerCpu::M68k, &assembly, 0x010040).unwrap();
}

#[test]
fn spills_address_taken_pointer_locals() {
    let assembly = emit_test_program(
        r#"
                global byte: u8 = 0
                global saved: u24 = 0
                fn main() {
                    let pointer: ptr<u8> = &byte
                    let address: u24 = cast<u24>(&pointer)
                    saved = address
                }
            "#,
    );
    let main = main_assembly(&assembly);
    for register in M68K_ADDRESS_REGISTERS {
        assert!(!main.contains(&format!("a{register}")), "{assembly}");
    }
    assert!(main.matches("move.b d0,$").count() >= 3, "{assembly}");
    assemble_subset_with_symbols_at(AssemblerCpu::M68k, &assembly, 0x010040).unwrap();
}

#[test]
fn preserves_static_locals_across_recursive_calls() {
    let program = parse_program(
        Path::new("test.ezra"),
        r#"
            fn count(value: u8) -> u8 {
                if value == 0 { return 0 }
                let next: u8 = value - 1
                return count(next) + 1
            }
            fn main() { let result: u8 = count(3) }
        "#,
    )
    .unwrap();
    let assembly = emit_m68k_assembly_with_options(
        &program,
        AssemblyOptions {
            cpu: CpuFamily::M68k,
            ..AssemblyOptions::default()
        },
    )
    .unwrap();
    assert!(assembly.contains("move.b $040"), "{assembly}");
    assert!(assembly.contains("-(sp)"), "{assembly}");
    assemble_subset_with_symbols_at(AssemblerCpu::M68k, &assembly, 0x010040).unwrap();
}

#[cfg(feature = "test-runner")]
#[test]
fn executes_two_result_calls_with_d0_and_d1_abi() {
    let (assembly, run) = emit_and_run(
        r#"
            volatile mmio DEBUG: u8 = 0xFFFFF0
            volatile mmio HALT: u8 = 0xFFFFF2
            fn main() {
                let first: u8, second: bool = pair(41)
                DEBUG = first
                DEBUG = cast<u8>(second)
                HALT = 1
            }
            fn pair(value: u8) -> u8, bool {
                return value + 1, value == 41
            }
        "#,
        2_000,
    );
    assert!(run.halted, "{run:?}\n{assembly}");
    assert_eq!(run.debug_output, [42, 1], "{assembly}");
    assert!(assembly.contains("    move.b d1,"), "{assembly}");
}

#[test]
fn rejects_two_result_calls_in_single_result_context() {
    let program = parse_program(
        Path::new("m68k-two-result.ezra"),
        "fn pair() -> u8, bool { return 1, true } fn main() { let value: u8 = pair() }",
    )
    .unwrap();
    let error = emit_m68k_assembly_with_options(
        &program,
        AssemblyOptions {
            cpu: CpuFamily::M68k,
            ..AssemblyOptions::default()
        },
    )
    .unwrap_err();
    assert!(
        error
            .message
            .contains("two-result call `pair` may only be used"),
        "{error}"
    );
}

#[test]
fn rejects_two_result_functions_that_can_fall_through() {
    let program = parse_program(
        Path::new("m68k-two-result-fallthrough.ezra"),
        "fn pair(value: bool) -> u8, bool { if value { return 1, true } } fn main() { let first: u8, second: bool = pair(true) }",
    )
    .unwrap();
    let error = emit_m68k_assembly_with_options(
        &program,
        AssemblyOptions {
            cpu: CpuFamily::M68k,
            ..AssemblyOptions::default()
        },
    )
    .unwrap_err();
    assert!(
        error
            .message
            .contains("missing two return values in function `pair`"),
        "{error}"
    );
}
