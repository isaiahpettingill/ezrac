use super::*;

#[test]
fn emits_target_specific_constant_shifts_and_reduces_power_of_two_multiply() {
    let source = r#"
            fn left_byte(value: u8) -> u8 {
                return value << 3
            }

            fn logical_byte(value: u8) -> u8 {
                return value >> 2
            }

            fn arithmetic_byte(value: i8) -> i8 {
                return value >> 2
            }

            fn left_word(value: u16) -> u16 {
                return value << 2
            }

            fn times_eight(value: u16) -> u16 {
                return value * 8
            }

            fn main() {
                test.assert_eq_u8(left_byte(3), 24, 1)
                test.assert_eq_u8(logical_byte(0x80), 0x20, 2)
                test.assert_eq_u8(cast<u8>(arithmetic_byte(-8)), 0xFE, 3)
                test.assert_eq_u16(left_word(0x1234), 0x48D0, 4)
                test.assert_eq_u16(times_eight(7), 56, 5)
                test.pass()
            }
        "#;
    let program = parse_program(Path::new("game.ezra"), source).unwrap();
    let asm = emit_ez80_assembly(&program).unwrap();
    let run = run_assembly_test(&asm, 12_000).unwrap();
    let body = |name: &str| {
        asm.split(&format!("_{name}:"))
            .nth(1)
            .unwrap()
            .split("    ret")
            .next()
            .unwrap()
    };

    let left_byte = body("left_byte");
    assert_eq!(left_byte.matches("    add a, a").count(), 3, "{left_byte}");
    assert!(!left_byte.contains("shift_loop"), "{left_byte}");

    let logical_byte = body("logical_byte");
    assert_eq!(
        logical_byte.matches("    srl a").count(),
        2,
        "{logical_byte}"
    );
    assert!(!logical_byte.contains("shift_loop"), "{logical_byte}");

    let arithmetic_byte = body("arithmetic_byte");
    assert_eq!(
        arithmetic_byte.matches("    sra a").count(),
        2,
        "{arithmetic_byte}"
    );
    assert!(!arithmetic_byte.contains("shift_loop"), "{arithmetic_byte}");

    let left_word = body("left_word");
    assert_eq!(left_word.matches("    add a, a").count(), 2, "{left_word}");
    assert_eq!(left_word.matches("    rl a").count(), 2, "{left_word}");
    assert!(!left_word.contains("shift_loop"), "{left_word}");

    let times_eight = body("times_eight");
    assert_eq!(
        times_eight.matches("    add a, a").count(),
        3,
        "{times_eight}"
    );
    assert_eq!(times_eight.matches("    rl a").count(), 3, "{times_eight}");
    assert!(!times_eight.contains("__ezra_mul"), "{times_eight}");
    assert!(!times_eight.contains("shift_loop"), "{times_eight}");

    assert!(run.halted, "{asm}");
    assert_eq!(run.result_code, 0, "{asm}");
}

#[test]
fn chooses_costed_shift_add_for_constant_multiplication() {
    let source = r#"
            fn times_three(value: u8) -> u8 {
                return value * 3
            }

            fn assign_three(mut_value: u8) -> u8 {
                mut_value *= 3
                return mut_value
            }

            fn times_three_word(value: u16) -> u16 {
                return value * 3
            }

            fn times_three_wide(value: u24) -> u24 {
                return value * 3
            }

            fn times_negative(value: i8) -> i8 {
                return value * -3
            }

            fn main() {
                test.assert_eq_u8(times_three(17), 51, 1)
                test.assert_eq_u8(assign_three(17), 51, 2)
                test.assert_eq_u16(times_three_word(0x1234), 0x369C, 3)
                test.assert_eq_u24(times_three_wide(0x010203), 0x030609, 4)
                test.assert_eq_u8(cast<u8>(times_negative(-5)), 15, 5)
                test.pass()
            }
        "#;
    let program = parse_program(Path::new("game.ezra"), source).unwrap();
    let asm = emit_ez80_assembly(&program).unwrap();
    let run = run_assembly_test(&asm, 4_000).unwrap();
    let body = |name: &str| {
        asm.split(&format!("_{name}:"))
            .nth(1)
            .unwrap()
            .split("    ret")
            .next()
            .unwrap()
    };
    let byte_body = body("times_three");
    let assignment_body = body("assign_three");
    let word_body = body("times_three_word");
    let wide_body = body("times_three_wide");
    let negative_body = body("times_negative");

    assert!(byte_body.contains("    add a, a"), "{byte_body}");
    assert!(byte_body.contains("    add a, b"), "{byte_body}");
    assert!(!byte_body.contains("__ezra_mul_u8"), "{byte_body}");
    assert!(
        assignment_body.contains("    add a, a"),
        "{assignment_body}"
    );
    assert!(
        !assignment_body.contains("__ezra_mul_u8"),
        "{assignment_body}"
    );
    assert!(word_body.contains("    add hl, hl"), "{word_body}");
    assert!(word_body.contains("    add hl, de"), "{word_body}");
    assert!(!word_body.contains("__ezra_mul_u16"), "{word_body}");
    assert!(wide_body.contains("    add hl, hl"), "{wide_body}");
    assert!(wide_body.contains("    add hl, de"), "{wide_body}");
    assert!(!wide_body.contains("__ezra_mul_u24"), "{wide_body}");
    assert!(negative_body.contains("    mlt bc"), "{negative_body}");
    assert!(!negative_body.contains("__ezra_mul_u8"), "{negative_body}");
    assert!(run.halted, "{asm}");
    assert_eq!(run.result_code, 0, "{asm}");
}

#[test]
fn lowers_single_bit_expression_masks() {
    let source = r#"
            fn source16() -> u16 {
                return 0x1200
            }

            fn set8(value: u8) -> u8 {
                return value | 0x20
            }

            fn clear8(value: u8) -> u8 {
                return value & 0xDF
            }

            fn toggle8(value: u8) -> u8 {
                return value ^ 0x20
            }

            fn set16() -> u16 {
                return source16() | 0x0200u16
            }

            fn clear16(value: u16) -> u16 {
                return value & 0xFDFFu16
            }

            fn toggle16(value: u16) -> u16 {
                return value ^ 0x0200u16
            }

            fn set24(value: u24) -> u24 {
                return value | 0x020000u24
            }

            fn clear24(value: u24) -> u24 {
                return value & 0xFDFFFFu24
            }

            fn toggle24(value: u24) -> u24 {
                return value ^ 0x020000u24
            }

            fn main() {
                test.assert_eq_u8(set8(1), 0x21, 1)
                test.assert_eq_u8(clear8(0xFF), 0xDF, 2)
                test.assert_eq_u8(toggle8(1), 0x21, 3)
                test.assert_eq_u16(set16(), 0x1200, 4)
                test.assert_eq_u16(clear16(0xFFFF), 0xFDFF, 5)
                test.assert_eq_u16(toggle16(0xFFFF), 0xFDFF, 6)
                test.assert_eq_u24(set24(0x010203), 0x030203, 7)
                test.assert_eq_u24(clear24(0xFFFFFF), 0xFDFFFF, 8)
                test.assert_eq_u24(toggle24(0xFFFFFF), 0xFDFFFF, 9)
                test.pass()
            }
        "#;
    let program = parse_program(Path::new("game.ezra"), source).unwrap();
    let asm = emit_ez80_assembly(&program).unwrap();
    let run = run_assembly_test(&asm, 12_000).unwrap();
    let body = |name: &str| {
        asm.split(&format!("_{name}:"))
            .nth(1)
            .unwrap()
            .split("    ret")
            .next()
            .unwrap()
    };

    assert!(body("set8").contains("    set 5, a"), "{asm}");
    assert!(body("clear8").contains("    res 5, a"), "{asm}");
    assert!(body("toggle8").contains("    xor 20h"), "{asm}");
    assert!(body("set16").contains("    set 1, a"), "{asm}");
    assert!(body("clear16").contains("    res 1, a"), "{asm}");
    assert!(body("toggle16").contains("    xor 02h"), "{asm}");
    assert!(body("set24").contains("    set 1, a"), "{asm}");
    assert!(body("clear24").contains("    res 1, a"), "{asm}");
    assert!(body("toggle24").contains("    xor 02h"), "{asm}");
    assert_eq!(body("set16").matches("call _source16").count(), 1, "{asm}");
    assert!(run.halted, "{asm}");
    assert_eq!(run.result_code, 0, "{asm}");

    let intel_source = r#"
            fn set(value: u8) -> u8 {
                return value | 0x20
            }

            fn clear(value: u8) -> u8 {
                return value & 0xDF
            }

            fn toggle(value: u8) -> u8 {
                return value ^ 0x20
            }

            fn main() {
                let set_value: u8 = set(0)
                let clear_value: u8 = clear(0)
                let toggle_value: u8 = toggle(0)
            }
        "#;
    let intel_program = parse_program(Path::new("intel.ezra"), intel_source).unwrap();
    let intel_asm = emit_ez80_assembly_with_options(
        &intel_program,
        AssemblyOptions {
            cpu: CpuFamily::I8080,
            ram_base: Address24::new(0x2000),
            ..AssemblyOptions::default()
        },
    )
    .unwrap();

    assert!(!intel_asm.contains("\n    set "), "{intel_asm}");
    assert!(!intel_asm.contains("\n    res "), "{intel_asm}");
    assert!(intel_asm.contains("    ora c"), "{intel_asm}");
    assert!(intel_asm.contains("    ana c"), "{intel_asm}");
    assert!(intel_asm.contains("    xri 20h"), "{intel_asm}");
}

#[test]
fn lowers_u16_immediate_bitwise_operations_by_byte() {
    let source = r#"
            fn keep_low(value: u16) -> u16 {
                return value & 0x00FFu16
            }

            fn set_high(value: u16) -> u16 {
                return value | 0xFF00u16
            }

            fn toggle_low(value: u16) -> u16 {
                return value ^ 0x00FFu16
            }

            fn main() {
                test.assert_eq_u16(keep_low(0x1234), 0x0034, 1)
                test.assert_eq_u16(set_high(0x1234), 0xFF34, 2)
                test.assert_eq_u16(toggle_low(0x1234), 0x12CB, 3)
                test.pass()
            }
        "#;
    let program = parse_program(Path::new("game.ezra"), source).unwrap();
    let asm = emit_ez80_assembly(&program).unwrap();
    let run = run_assembly_test(&asm, 6_000).unwrap();
    let body = |name: &str| {
        asm.split(&format!("_{name}:"))
            .nth(1)
            .unwrap()
            .split("    ret")
            .next()
            .unwrap()
    };

    let keep_low = body("keep_low");
    assert!(keep_low.contains("    ld h, 00h"), "{keep_low}");
    assert!(!keep_low.contains("    and FFh"), "{keep_low}");

    let set_high = body("set_high");
    assert!(set_high.contains("    ld h, FFh"), "{set_high}");
    assert!(!set_high.contains("    or 00h"), "{set_high}");

    let toggle_low = body("toggle_low");
    assert!(toggle_low.contains("    ld a, l"), "{toggle_low}");
    assert!(toggle_low.contains("    xor FFh"), "{toggle_low}");
    assert!(toggle_low.contains("    ld l, a"), "{toggle_low}");

    for name in ["keep_low", "set_high", "toggle_low"] {
        let function = body(name);
        assert!(!function.contains("    push hl"), "{function}");
        assert!(!function.contains("    pop bc"), "{function}");
    }
    assert!(run.halted, "{asm}");
    assert_eq!(run.result_code, 0, "{asm}");
}

#[test]
fn lowers_single_bit_compound_assignments_for_local_scalars() {
    let source = r#"
        global global_value: u8 = 0

        fn local_u8() -> u8 {
            let value: u8 = 0xFFu8
            value |= 0x20u8
            value &= 0xDFu8
            value ^= 0x20u8
            return value
        }

        fn local_u16() -> u16 {
            let value: u16 = 0xFFFFu16
            value |= 0x0200u16
            value &= 0xFDFFu16
            return value
        }

        fn local_u24() -> u24 {
            let value: u24 = 0xFFFFFFu24
            value |= 0x020000u24
            value &= 0xFDFFFFu24
            return value
        }

        fn global_mask() -> u8 {
            global_value |= 0x20u8
            global_value &= 0xDFu8
            return global_value
        }

        fn pointed_mask() -> u8 {
            let value: u8 = 0xFFu8
            let pointer: ptr<u8> = &value
            *pointer |= 0x20u8
            *pointer &= 0xDFu8
            return *pointer
        }

        fn main() {
            test.assert_eq_u8(local_u8(), 0xFFu8, 1)
            test.assert_eq_u16(local_u16(), 0xFDFFu16, 2)
            test.assert_eq_u24(local_u24(), 0xFDFFFFu24, 3)
            test.assert_eq_u8(global_mask(), 0u8, 4)
            test.assert_eq_u8(pointed_mask(), 0xDFu8, 5)
            test.pass()
        }
    "#;
    let program = parse_program(Path::new("game.ezra"), source).unwrap();
    let asm = emit_ez80_assembly(&program).unwrap();
    let run = run_assembly_test(&asm, 12_000).unwrap();
    let body = |name: &str| {
        asm.split(&format!("_{name}:"))
            .nth(1)
            .unwrap()
            .split("    ret")
            .next()
            .unwrap()
    };

    assert!(body("local_u8").contains("    set 5, (hl)"), "{asm}");
    assert!(body("local_u8").contains("    res 5, (hl)"), "{asm}");
    assert!(body("local_u8").contains("    xor b"), "{asm}");
    assert!(body("local_u16").contains("    set 1, (hl)"), "{asm}");
    assert!(body("local_u16").contains("    res 1, (hl)"), "{asm}");
    assert!(body("local_u24").contains("    set 1, (hl)"), "{asm}");
    assert!(body("local_u24").contains("    res 1, (hl)"), "{asm}");
    assert!(!body("global_mask").contains(", (hl)"), "{asm}");
    assert!(!body("pointed_mask").contains("    set "), "{asm}");
    assert!(!body("pointed_mask").contains("    res "), "{asm}");
    assert!(run.halted, "{asm}");
    assert_eq!(run.result_code, 0, "{asm}");

    let intel_source = r#"
        fn main() {
            let value: u8 = 0xFFu8
            value |= 0x20u8
            value &= 0xDFu8
        }
    "#;
    let intel_program = parse_program(Path::new("intel.ezra"), intel_source).unwrap();
    for cpu in [CpuFamily::I8080, CpuFamily::I8085] {
        let intel_asm = emit_ez80_assembly_with_options(
            &intel_program,
            AssemblyOptions {
                cpu,
                ram_base: Address24::new(0x2000),
                ..AssemblyOptions::default()
            },
        )
        .unwrap();

        assert!(!intel_asm.contains("\n    set "), "{intel_asm}");
        assert!(!intel_asm.contains("\n    res "), "{intel_asm}");
        assert!(intel_asm.contains("    ora b"), "{intel_asm}");
        assert!(intel_asm.contains("    ana b"), "{intel_asm}");
    }
}

#[test]
fn lowers_byte_aligned_wide_temporary_shifts() {
    let source = r#"
            fn shl16_8(value: u16) -> u16 {
                return value << 8
            }

            fn shr16_8(value: u16) -> u16 {
                return value >> 8
            }

            fn shl16_16(value: u16) -> u16 {
                return value << 16
            }

            fn shr_i16_8(value: i16) -> i16 {
                return value >> 8
            }

            fn shr_i16_16(value: i16) -> i16 {
                return value >> 16
            }

            fn assign_shl16_8(value: u16) -> u16 {
                let shifted: u16 = value
                shifted <<= 8
                return shifted
            }

            fn assign_shr16_8(value: u16) -> u16 {
                let shifted: u16 = value
                shifted >>= 8
                return shifted
            }

            fn shl24_16(value: u24) -> u24 {
                return value << 16
            }

            fn shr24_8(value: u24) -> u24 {
                return value >> 8
            }

            fn shr24_24(value: u24) -> u24 {
                return value >> 24
            }

            fn shr_i24_16(value: i24) -> i24 {
                return value >> 16
            }

            fn shr_i24_24(value: i24) -> i24 {
                return value >> 24
            }

            fn assign_shl24_8(value: u24) -> u24 {
                let shifted: u24 = value
                shifted <<= 8
                return shifted
            }

            fn assign_shr_i24_8(value: i24) -> i24 {
                let shifted: i24 = value
                shifted >>= 8
                return shifted
            }

            fn assign_shr_i24_16(value: i24) -> i24 {
                let shifted: i24 = value
                shifted >>= 16
                return shifted
            }

            fn main() {
                test.assert_eq_u16(shl16_8(0x1234), 0x3400, 1)
                test.assert_eq_u16(shr16_8(0x1234), 0x0012, 2)
                test.assert_eq_u16(shl16_16(0x1234), 0, 3)
                test.assert_eq_u16(cast<u16>(shr_i16_8(-0x1234)), 0xFFED, 4)
                test.assert_eq_u16(cast<u16>(shr_i16_16(-0x1234)), 0xFFFF, 5)
                test.assert_eq_u16(assign_shl16_8(0x1234), 0x3400, 6)
                test.assert_eq_u16(assign_shr16_8(0x1234), 0x0012, 7)
                test.assert_eq_u24(shl24_16(0x010203), 0x030000, 8)
                test.assert_eq_u24(shr24_8(0x010203), 0x000102, 9)
                test.assert_eq_u24(shr24_24(0x010203), 0, 10)
                test.assert_eq_u24(cast<u24>(shr_i24_16(-0x012345)), 0xFFFFFE, 11)
                test.assert_eq_u24(cast<u24>(shr_i24_24(-0x012345)), 0xFFFFFF, 12)
                test.assert_eq_u24(assign_shl24_8(0x010203), 0x020300, 13)
                test.assert_eq_u24(cast<u24>(assign_shr_i24_8(-0x012345)), 0xFFFEDC, 14)
                test.assert_eq_u24(cast<u24>(assign_shr_i24_16(-0x012345)), 0xFFFFFE, 15)
                test.pass()
            }
        "#;
    let program = parse_program(Path::new("game.ezra"), source).unwrap();
    let asm = emit_ez80_assembly(&program).unwrap();
    let run = run_assembly_test(&asm, 12_000).unwrap();
    let body = |name: &str| {
        asm.split(&format!("_{name}:"))
            .nth(1)
            .unwrap()
            .split("    ret")
            .next()
            .unwrap()
    };

    for name in [
        "shl16_8",
        "shl16_16",
        "assign_shl16_8",
        "shl24_16",
        "assign_shl24_8",
    ] {
        let function = body(name);
        assert!(!function.contains("    add a, a"), "{function}");
        assert!(!function.contains("    rl a"), "{function}");
    }
    for name in ["shr16_8", "assign_shr16_8", "shr24_8", "shr24_24"] {
        let function = body(name);
        assert!(!function.contains("    srl a"), "{function}");
    }
    for name in [
        "shr_i16_8",
        "shr_i16_16",
        "shr_i24_16",
        "shr_i24_24",
        "assign_shr_i24_8",
        "assign_shr_i24_16",
    ] {
        let function = body(name);
        assert!(!function.contains("    sra a"), "{function}");
    }

    for (name, expected_storage_bytes) in [
        ("assign_shl16_8", 6),
        ("assign_shr16_8", 6),
        ("assign_shl24_8", 6),
        ("assign_shr_i24_8", 6),
        ("assign_shr_i24_16", 6),
    ] {
        let function = body(name);
        let addresses = function
            .lines()
            .filter_map(|line| line.split_once('(')?.1.split_once("h)").map(|part| part.0))
            .collect::<std::collections::HashSet<_>>();
        assert_eq!(addresses.len(), expected_storage_bytes, "{function}");
    }

    assert!(run.halted, "{asm}");
    assert_eq!(run.result_code, 0, "{asm}");
}

#[test]
fn preserves_volatile_wide_accesses_for_byte_aligned_compound_shifts() {
    let source = r#"
            volatile mmio STATUS16: ptr<u16> = 0x040180
            volatile mmio STATUS24: ptr<i24> = 0x040190

            fn shift_status16() {
                *STATUS16 >>= 8
            }

            fn shift_status24() {
                *STATUS24 >>= 8
            }

            fn main() {
                shift_status16()
                shift_status24()
                test.assert_eq_u16(*STATUS16, 0x0012, 1)
                test.assert_eq_u24(cast<u24>(*STATUS24), 0xFF8034, 2)
                test.pass()
            }
        "#;
    let program = parse_program(Path::new("game.ezra"), source).unwrap();
    let asm = emit_ez80_assembly(&program).unwrap();
    let run = run_assembly_test_with_options(
        &asm,
        &TestRunOptions {
            instruction_budget: 8_000,
            initial_ports: Vec::new(),
            initial_memory: vec![
                (0x040180, 0x34),
                (0x040181, 0x12),
                (0x040190, 0x56),
                (0x040191, 0x34),
                (0x040192, 0x80),
            ],
            stack_top: EZRA_STACK_TOP.get(),
        },
    )
    .unwrap();
    let body = |name: &str| {
        asm.split(&format!("_{name}:"))
            .nth(1)
            .unwrap()
            .split("    ret")
            .next()
            .unwrap()
    };

    assert_eq!(
        body("shift_status16").matches("    ld a, (hl)").count(),
        2,
        "{asm}"
    );
    assert_eq!(
        body("shift_status16").matches("    ld (hl), a").count(),
        2,
        "{asm}"
    );
    assert_eq!(
        body("shift_status24").matches("    ld a, (hl)").count(),
        3,
        "{asm}"
    );
    assert_eq!(
        body("shift_status24").matches("    ld (hl), a").count(),
        3,
        "{asm}"
    );
    assert!(run.halted, "{asm}");
    assert_eq!(run.result_code, 0, "{asm}");
}

#[test]
fn emits_and_runs_variable_shifts_with_safe_byte_decomposition() {
    let source = r#"
            fn shl8(value: u8, count: u8) -> u8 {
                return value << count
            }

            fn shr8(value: u8, count: u8) -> u8 {
                return value >> count
            }

            fn shl16(value: u16, count: u8) -> u16 {
                return value << count
            }

            fn shr16(value: u16, count: u8) -> u16 {
                return value >> count
            }

            fn shr_i16(value: i16, count: u8) -> i16 {
                return value >> count
            }

            fn shl24(value: u24, count: u8) -> u24 {
                return value << count
            }

            fn shr24(value: u24, count: u8) -> u24 {
                return value >> count
            }

            fn shr_i24(value: i24, count: u8) -> i24 {
                return value >> count
            }

            fn main() {
                test.assert_eq_u8(shl8(0x81, 0), 0x81, 1)
                test.assert_eq_u8(shl8(0x81, 7), 0x80, 2)
                test.assert_eq_u8(shl8(0x81, 8), 0, 3)
                test.assert_eq_u8(shl8(1, 255), 0, 4)
                test.assert_eq_u8(shr8(0x81, 7), 1, 5)
                test.assert_eq_u8(shr8(0x81, 8), 0, 6)
                test.assert_eq_u16(shl16(0x1234, 8), 0x3400, 7)
                test.assert_eq_u16(shl16(0x1234, 9), 0x6800, 8)
                test.assert_eq_u16(shl16(0x1234, 16), 0, 9)
                test.assert_eq_u16(shr16(0x1234, 8), 0x12, 10)
                test.assert_eq_u16(shr16(0x1234, 9), 0x09, 11)
                test.assert_eq_u16(shr16(0x1234, 16), 0, 12)
                test.assert_eq_u16(cast<u16>(shr_i16(-0x1234, 8)), 0xFFED, 13)
                test.assert_eq_u16(cast<u16>(shr_i16(-0x1234, 16)), 0xFFFF, 14)
                test.assert_eq_u24(shl24(0x010203, 8), 0x020300, 15)
                test.assert_eq_u24(shr24(0x010203, 8), 0x000102, 16)
                test.assert_eq_u24(shr24(0x010203, 24), 0, 17)
                test.assert_eq_u24(cast<u24>(shr_i24(-0x012345, 8)), 0xFFFEDC, 18)
                test.assert_eq_u24(cast<u24>(shr_i24(-0x012345, 24)), 0xFFFFFF, 19)
                test.pass()
            }
        "#;
    let program = parse_program(Path::new("game.ezra"), source).unwrap();
    let asm = emit_ez80_assembly(&program).unwrap();
    let run = run_assembly_test(&asm, 20_000).unwrap();
    let body = |name: &str| {
        asm.split(&format!("_{name}:"))
            .nth(1)
            .unwrap()
            .split("    ret")
            .next()
            .unwrap()
    };

    let shl8 = body("shl8");
    assert!(shl8.contains("    cp 08h"), "{shl8}");
    assert!(shl8.contains("shift_bit_loop"), "{shl8}");
    assert!(!shl8.contains("shift_loop"), "{shl8}");
    let shl16 = body("shl16");
    assert!(shl16.contains("    cp 10h"), "{shl16}");
    assert!(shl16.contains("shift_byte_loop"), "{shl16}");
    assert!(shl16.contains("    srl a"), "{shl16}");
    let shr_i24 = body("shr_i24");
    assert!(shr_i24.contains("    cp 18h"), "{shr_i24}");
    assert!(shr_i24.contains("    sbc a, a"), "{shr_i24}");

    let z80_source = r#"
            fn shift(value: u16, count: u8) -> u16 {
                return value << count
            }

            fn main() {
                let value: u16 = shift(0x1234, 1)
                test.assert_eq_u16(value, 0x2468, 1)
                test.pass()
            }
        "#;
    let z80_program = parse_program(Path::new("z80.ezra"), z80_source).unwrap();
    let z80_asm = emit_ez80_assembly_with_options(
        &z80_program,
        AssemblyOptions {
            cpu: CpuFamily::Z80,
            ram_base: Address24::new(0x2000),
            ..AssemblyOptions::default()
        },
    )
    .unwrap();
    let z80_body = z80_asm.split("_shift:").nth(1).unwrap();
    assert!(z80_body.contains("shift_byte_loop"), "{z80_asm}");
    assert!(z80_body.contains("    srl a"), "{z80_asm}");

    assert!(run.halted, "{asm}");
    assert_eq!(run.result_code, 0, "{asm}");
}

#[test]
fn hoists_pure_loop_invariants_before_the_loop() {
    let source = r#"
            fn sum_scaled(base: u8, count: u8) -> u8 {
                let index: u8 = 0
                let total: u8 = 0
                while index < count {
                    let scaled: u8 = base + 3
                    total += scaled
                    index += 1
                }
                return total
            }

            fn main() {
                test.assert_eq_u8(sum_scaled(4, 3), 21, 1)
                test.pass()
            }
        "#;
    let program = parse_program(Path::new("game.ezra"), source).unwrap();
    let asm = emit_ez80_assembly_with_debug_comments(&program, true).unwrap();
    let run = run_assembly_test(&asm, 8_000).unwrap();
    let preheader = asm.find("let __tbir_licm_").unwrap();
    let loop_start = asm.find("source: while").unwrap();
    let replacement = asm.find("let scaled: u8 = __tbir_licm_").unwrap();

    assert!(preheader < loop_start, "{asm}");
    assert!(replacement > loop_start, "{asm}");
    assert!(run.halted, "{asm}");
    assert_eq!(run.result_code, 0, "{asm}");
}

#[test]
fn hoists_nonvolatile_global_reads_before_the_loop() {
    let source = r#"
            global factor: u8 = 7

            fn sum_factor(count: u8) -> u8 {
                let index: u8 = 0
                let total: u8 = 0
                while index < count {
                    let sampled: u8 = factor
                    total += sampled
                    index += 1
                }
                return total
            }

            fn main() {
                test.assert_eq_u8(sum_factor(3), 21, 1)
                test.pass()
            }
        "#;
    let program = parse_program(Path::new("game.ezra"), source).unwrap();
    let asm = emit_ez80_assembly_with_debug_comments(&program, true).unwrap();
    let run = run_assembly_test(&asm, 8_000).unwrap();
    let preheader = asm.find("let __tbir_mem_licm_").unwrap();
    let loop_start = asm.find("source: while").unwrap();
    let replacement = asm.find("let sampled: u8 = __tbir_mem_licm_").unwrap();

    assert!(preheader < loop_start, "{asm}");
    assert!(replacement > loop_start, "{asm}");
    assert!(run.halted, "{asm}");
    assert_eq!(run.result_code, 0, "{asm}");
}

#[test]
fn pointer_writes_block_global_read_hoisting() {
    let source = r#"
            global value: u8 = 1

            fn sample_three() -> u8 {
                let pointer: ptr<u8> = &value
                let index: u8 = 0
                let total: u8 = 0
                while index < 3 {
                    let sampled: u8 = value
                    if index == 1 {
                        *pointer = 5
                    }
                    total += sampled
                    index += 1
                }
                return total
            }

            fn main() {
                test.assert_eq_u8(sample_three(), 7, 1)
                test.pass()
            }
        "#;
    let program = parse_program(Path::new("game.ezra"), source).unwrap();
    let asm = emit_ez80_assembly_with_debug_comments(&program, true).unwrap();
    let run = run_assembly_test(&asm, 10_000).unwrap();

    assert!(!asm.contains("let __tbir_mem_licm_"), "{asm}");
    assert!(run.halted, "{asm}");
    assert_eq!(run.result_code, 0, "{asm}");
}

#[test]
fn keeps_port_reads_inside_loops() {
    let source = r#"
            port INPUT: u8 = 0x20

            fn main() {
                let count: u8 = 0
                let total: u8 = 0
                while count < 3 {
                    let sample: u8 = in INPUT
                    total += sample
                    count += 1
                }
                test.assert_eq_u8(total, 0, 1)
                test.pass()
            }
        "#;
    let program = parse_program(Path::new("game.ezra"), source).unwrap();
    let asm = emit_ez80_assembly_with_debug_comments(&program, true).unwrap();
    let run = run_assembly_test(&asm, 8_000).unwrap();
    let loop_start = asm.find("source: while").unwrap();
    let port_read = asm.find("let sample: u8 = in INPUT").unwrap();

    assert!(port_read > loop_start, "{asm}");
    assert!(!asm.contains("let __tbir_licm_"), "{asm}");
    assert!(run.halted, "{asm}");
    assert_eq!(run.result_code, 0, "{asm}");
}

#[test]
fn emits_and_runs_recursive_function_calls() {
    let source = r#"
            fn sum_to(value: u8) -> u8 {
                if value == 0 {
                    return 0
                }
                let current: u8 = value
                return current + sum_to(value - 1)
            }

            fn even(value: u8) -> bool {
                if value == 0 {
                    return true
                }
                return odd(value - 1)
            }

            fn odd(value: u8) -> bool {
                if value == 0 {
                    return false
                }
                return even(value - 1)
            }

            fn main() {
                test.assert_eq_u8(sum_to(4), 10, 1)
                test.assert_eq_u8(even(6), true, 2)
                test.assert_eq_u8(odd(6), false, 3)
                test.pass()
            }
        "#;
    let program = parse_program(Path::new("game.ezra"), source).unwrap();
    let asm = emit_ez80_assembly(&program).unwrap();
    let run = run_assembly_test(&asm, 80_000).unwrap();

    assert!(asm.contains("call _sum_to"), "{asm}");
    assert!(asm.contains("call _odd"), "{asm}");
    assert!(run.halted, "{asm}");
    assert_eq!(run.result_code, 0, "{asm}");
}

#[test]
fn preserves_pointer_out_arguments_across_recursive_calls() {
    let source = r#"
            fn write_through_chain(output: ptr<u8>, depth: u8) {
                let local: u8 = 0
                if depth == 0 {
                    *output = 7
                    return
                }
                write_through_chain(&local, depth - 1)
                *output = local
            }

            fn main() {
                let result: u8 = 0
                write_through_chain(&result, 3)
                test.assert_eq_u8(result, 7, 1)
                test.pass()
            }
        "#;
    let program = parse_program(Path::new("game.ezra"), source).unwrap();
    let asm = emit_ez80_assembly(&program).unwrap();
    let recursive_call = asm
        .split("_write_through_chain:")
        .nth(1)
        .and_then(|body| body.split("_main:").next())
        .expect("write_through_chain function");

    assert!(
        recursive_call.contains("call _write_through_chain"),
        "{recursive_call}"
    );
    // The 14-byte caller frame excludes the one-byte `local` out-argument.
    // Direct comparison branching also avoids a scratch boolean local.
    assert_eq!(
        recursive_call.matches("    dec sp").count(),
        13,
        "{recursive_call}"
    );
    assert_eq!(
        recursive_call.matches("    inc sp").count(),
        13,
        "{recursive_call}"
    );
}

#[test]
fn emits_and_runs_recursive_function_with_stack_arguments() {
    let source = r#"
            fn stepped(value: u8, base: u8, filler: u8, step: u8) -> u8 {
                if value == 0 {
                    return base
                }
                let saved_step: u8 = step
                return saved_step + stepped(value - 1, base, filler, step)
            }

            fn main() {
                test.assert_eq_u8(stepped(3, 2, 7, 4), 14, 1)
                test.pass()
            }
        "#;
    let program = parse_program(Path::new("game.ezra"), source).unwrap();
    let asm = emit_ez80_assembly(&program).unwrap();
    let run = run_assembly_test(&asm, 12_000).unwrap();

    assert!(run.halted, "{asm}");
    assert_eq!(run.result_code, 0, "{asm}");
}

#[test]
fn omits_unused_functions_regardless_of_visibility() {
    let source = r#"
            fn used(value: u8) -> u8 {
                return value + 1
            }

            fn unused_private(value: u8) -> u8 {
                return value + 2
            }

            pub fn exported(value: u8) -> u8 {
                return value + 3
            }

            pub inline fn exported_inline(value: u8) -> u8 {
                return value + 4
            }

            fn main() {
                test.assert_eq_u8(used(4), 5, 1)
                test.pass()
            }
        "#;
    let program = parse_program(Path::new("game.ezra"), source).unwrap();
    let asm = emit_ez80_assembly(&program).unwrap();
    let run = run_assembly_test(&asm, 4_000).unwrap();

    assert!(run.halted, "{asm}");
    assert_eq!(run.result_code, 0, "{asm}");
    assert!(asm.contains("_used:"));
    assert!(!asm.contains("_exported:"));
    assert!(!asm.contains("_exported_inline:"));
    assert!(!asm.contains("_unused_private:"));
}

#[test]
fn validates_calls_in_unused_private_functions_before_omitting_them() {
    let source = r#"
            fn unused_private() {
                missing()
            }

            fn main() {
                test.pass()
            }
        "#;
    let program = parse_program(Path::new("game.ezra"), source).unwrap();
    let error = emit_ez80_assembly(&program).unwrap_err();

    assert_eq!(error.message, "unknown function `missing`");
}

#[test]
fn omits_unreachable_statements_after_terminators() {
    let source = r#"
            fn choose(flag: bool) -> u8 {
                if flag {
                    return 1
                } else {
                    return 2
                }
                test.fail(7)
                return 3
            }

            fn main() {
                test.assert_eq_u8(choose(true), 1, 1)
                test.assert_eq_u8(choose(false), 2, 2)
                test.pass()
            }
        "#;
    let program = parse_program(Path::new("game.ezra"), source).unwrap();
    let asm = emit_ez80_assembly_with_debug_comments(&program, true).unwrap();
    let run = run_assembly_test(&asm, 4_000).unwrap();

    assert!(!asm.contains("; source: test.fail(7)"), "{asm}");
    assert!(!asm.contains("; source: return 3"), "{asm}");
    assert!(run.halted, "{asm}");
    assert_eq!(run.result_code, 0, "{asm}");
}

#[test]
fn omits_unreachable_statements_after_nonbreaking_loop() {
    let source = r#"
            fn exit_loop() {
                loop {
                    return
                }
                test.fail(7)
            }

            fn main() {
                exit_loop()
                test.fail(8)
            }
        "#;
    let program = parse_program(Path::new("game.ezra"), source).unwrap();
    let asm = emit_ez80_assembly_with_debug_comments(&program, true).unwrap();
    let run = run_assembly_test(&asm, 4_000).unwrap();

    assert!(!asm.contains("; source: test.fail(7)"), "{asm}");
    assert!(run.halted, "{asm}");
    assert_eq!(run.result_code, 8, "{asm}");
}

#[test]
fn validates_unreachable_statements_before_omitting_them() {
    let source = r#"
            fn done() {
                return;
                let value: u8 = 0x100
            }

            fn main() {
                done()
                test.pass()
            }
        "#;
    let program = parse_program(Path::new("game.ezra"), source).unwrap();
    let error = emit_ez80_assembly(&program).unwrap_err();

    assert_eq!(error.message, "value 256 is outside u8 range");
}

#[test]
fn omits_constant_dead_if_and_while_branches() {
    let source = r#"
            const RUN_COLD: bool = false

            fn cold() {
                test.fail(9)
            }

            fn choose() -> u8 {
                if RUN_COLD {
                    cold()
                    return 9
                } else {
                    return 4
                }
            }

            fn main() {
                while false {
                    test.fail(7)
                }
                test.assert_eq_u8(choose(), 4, 1)
                test.pass()
            }
        "#;
    let program = parse_program(Path::new("game.ezra"), source).unwrap();
    let asm = emit_ez80_assembly_with_debug_comments(&program, true).unwrap();
    let run = run_assembly_test(&asm, 4_000).unwrap();

    assert!(!asm.contains("_cold:"), "{asm}");
    assert!(!asm.contains("; source: cold()"), "{asm}");
    assert!(!asm.contains("; source: test.fail(7)"), "{asm}");
    assert!(!asm.contains("; source: return 9"), "{asm}");
    assert!(run.halted, "{asm}");
    assert_eq!(run.result_code, 0, "{asm}");
}

#[test]
fn omits_constant_true_while_condition_checks() {
    let source = r#"
            const KEEP_RUNNING: bool = true

            fn main() {
                let count: u8 = 0
                while KEEP_RUNNING {
                    count += 1
                    if count == 3 {
                        break
                    }
                }
                test.assert_eq_u8(count, 3, 1)
                test.pass()
            }
        "#;
    let program = parse_program(Path::new("game.ezra"), source).unwrap();
    let asm = emit_ez80_assembly_with_debug_comments(&program, true).unwrap();
    let run = run_assembly_test(&asm, 4_000).unwrap();
    let while_body = asm
        .split("; source: while KEEP_RUNNING")
        .nth(1)
        .and_then(|tail| tail.split("; source: test.assert_eq_u8").next())
        .unwrap();

    assert!(!while_body.contains("    jp z, .L_endwhile"), "{asm}");
    assert!(while_body.contains("    jp .L_while"), "{asm}");
    assert!(run.halted, "{asm}");
    assert_eq!(run.result_code, 0, "{asm}");
}

#[test]
fn omits_unreachable_statements_after_const_true_while_return() {
    let source = r#"
            const KEEP_RUNNING: bool = true

            fn done() {
                while KEEP_RUNNING {
                    return
                }
                test.fail(7)
            }

            fn choose() -> u8 {
                if KEEP_RUNNING {
                    return 5
                }
                test.fail(8)
                return 9
            }

            fn main() {
                done()
                test.assert_eq_u8(choose(), 5, 1)
                test.pass()
            }
        "#;
    let program = parse_program(Path::new("game.ezra"), source).unwrap();
    let asm = emit_ez80_assembly_with_debug_comments(&program, true).unwrap();
    let run = run_assembly_test(&asm, 4_000).unwrap();

    assert!(!asm.contains("; source: test.fail(7)"), "{asm}");
    assert!(!asm.contains("; source: test.fail(8)"), "{asm}");
    assert!(!asm.contains("; source: return 9"), "{asm}");
    assert!(run.halted, "{asm}");
    assert_eq!(run.result_code, 0, "{asm}");
}

#[test]
fn validates_constant_dead_branches_before_omitting_them() {
    let source = r#"
            fn main() {
                if false {
                    let value: u8 = 0x100
                }
                test.pass()
            }
        "#;
    let program = parse_program(Path::new("game.ezra"), source).unwrap();
    let error = emit_ez80_assembly(&program).unwrap_err();

    assert_eq!(error.message, "value 256 is outside u8 range");
}

#[test]
fn omits_private_functions_only_called_from_unreachable_statements() {
    let source = r#"
            fn unreachable_private() {
                test.fail(7)
            }

            fn done() {
                return;
                unreachable_private()
            }

            fn main() {
                done()
                test.pass()
            }
        "#;
    let program = parse_program(Path::new("game.ezra"), source).unwrap();
    let asm = emit_ez80_assembly_with_debug_comments(&program, true).unwrap();
    let run = run_assembly_test(&asm, 4_000).unwrap();

    assert!(!asm.contains("_unreachable_private:"), "{asm}");
    assert!(!asm.contains("; source: unreachable_private()"), "{asm}");
    assert!(run.halted, "{asm}");
    assert_eq!(run.result_code, 0, "{asm}");
}

#[test]
fn propagates_local_scalar_constants_until_assignment() {
    let source = r#"
            fn copied() -> u8 {
                let base: u8 = 4
                let derived: u8 = base + 3
                return derived
            }

            fn assigned() -> u8 {
                let value: u8 = 4
                value = value + 1
                return value
            }

            fn main() {
                test.assert_eq_u8(copied(), 7, 1)
                test.assert_eq_u8(assigned(), 5, 2)
                test.pass()
            }
        "#;
    let program = parse_program(Path::new("game.ezra"), source).unwrap();
    let asm = emit_ez80_assembly(&program).unwrap();
    let run = run_assembly_test(&asm, 4_000).unwrap();
    let copied = asm
        .split("_copied:")
        .nth(1)
        .and_then(|tail| tail.split("_assigned:").next())
        .unwrap();
    let assigned = asm
        .split("_assigned:")
        .nth(1)
        .and_then(|tail| tail.split("section .header").next())
        .unwrap();

    assert!(copied.contains("    ld a, 07h"), "{asm}");
    assert!(assigned.contains("    ld a, (040"), "{asm}");
    assert!(run.halted, "{asm}");
    assert_eq!(run.result_code, 0, "{asm}");
}

#[test]
fn propagates_local_pointer_constants_until_assignment() {
    let source = r#"
            global byte: u8 = 0

            fn copied_ptr() -> u24 {
                let base: ptr<u8> = &byte
                let copied: ptr<u8> = base
                return cast<u24>(copied)
            }

            fn copied_raw() -> u24 {
                let raw: ptr = cast<ptr>(&byte)
                return cast<u24>(raw)
            }

            fn assigned_ptr() -> u24 {
                let value: ptr<u8> = &byte
                value = value + 1
                return cast<u24>(value)
            }

            fn main() {
                test.assert_eq_u24(copied_ptr(), cast<u24>(&byte), 1)
                test.assert_eq_u24(copied_raw(), cast<u24>(&byte), 2)
                test.assert_eq_u24(assigned_ptr(), cast<u24>(&byte) + 1, 3)
                test.pass()
            }
        "#;
    let program = parse_program(Path::new("game.ezra"), source).unwrap();
    let asm = emit_ez80_assembly(&program).unwrap();
    let run = run_assembly_test(&asm, 6_000).unwrap();
    let copied_ptr = asm
        .split("_copied_ptr:")
        .nth(1)
        .and_then(|tail| tail.split("_copied_raw:").next())
        .unwrap();
    let copied_raw = asm
        .split("_copied_raw:")
        .nth(1)
        .and_then(|tail| tail.split("_assigned_ptr:").next())
        .unwrap();
    let assigned_ptr = asm
        .split("_assigned_ptr:")
        .nth(1)
        .and_then(|tail| tail.split("section .header").next())
        .unwrap();

    assert!(copied_ptr.contains("    ld hl, 040"), "{asm}");
    assert!(copied_ptr.contains("    ret"), "{asm}");
    assert!(copied_raw.contains("    ld hl, 040"), "{asm}");
    assert!(copied_raw.contains("    ret"), "{asm}");
    assert!(
        assigned_ptr.contains("    ld hl, 040") || assigned_ptr.contains("    ld hl, (040"),
        "{asm}"
    );
    assert!(run.halted, "{asm}");
    assert_eq!(run.result_code, 0, "{asm}");
}

#[test]
fn emits_direct_z80_pointer_loop_fast_paths() {
    let source = r#"
        global s: u8 = 0
        global p: u8 = 0

        fn main() {
            let text: ptr<u8> = "SP$"
            let cursor: ptr<u8> = text
            let count_s: ptr<u8> = &s
            let count_p: ptr<u8> = &p
            loop {
                let letter: u8 = *cursor
                if letter == 'S' {
                    *count_s += 1
                } else if letter == 'P' {
                    *count_p += 1
                } else if letter == '$' {
                    break
                }
                cursor = cursor + 1
            }
            test.assert_eq_u8(s, 1, 1)
            test.assert_eq_u8(p, 1, 2)
            test.pass()
        }
    "#;
    let program = parse_program(Path::new("game.ezra"), source).unwrap();
    let options = AssemblyOptions {
        cpu: CpuFamily::Z80,
        ram_base: Address24::new(0x2000),
        rodata_base: Address24::new(0x3000),
        section_bases: vec![(".rodata".to_owned(), Address24::new(0x3000))],
        ..AssemblyOptions::default()
    };
    let asm = emit_ez80_assembly_with_options(&program, options).unwrap();
    let ez80_asm = emit_ez80_assembly(&program).unwrap();
    let run = run_assembly_test(&ez80_asm, 4_000).unwrap();

    assert!(asm.contains("    cp 53h\n    jp nz, .L_else"), "{asm}");
    assert!(asm.contains("    cp 50h\n    jp nz, .L_else"), "{asm}");
    assert!(
        asm.contains("    ld a, (hl)\n    inc a\n    ld (hl), a"),
        "{asm}"
    );
    assert!(asm.contains("    inc hl"), "{asm}");
    assert!(!asm.contains(".L_cmp_true"), "{asm}");
    assert!(run.halted, "{ez80_asm}");
    assert_eq!(run.result_code, 0, "{ez80_asm}");
}

#[test]
fn emits_and_runs_pointer_ordering_comparisons() {
    let source = r#"
        global bytes: [u8; 2] = [0, 0]

        fn main() {
            let first: ptr<u8> = &bytes[0]
            let second: ptr<u8> = &bytes[1]
            test.assert_eq_u8(first < second, true, 1)
            test.assert_eq_u8(first <= second, true, 2)
            test.assert_eq_u8(second > first, true, 3)
            test.assert_eq_u8(second >= first, true, 4)
            test.pass()
        }
    "#;
    let program = parse_program(Path::new("game.ezra"), source).unwrap();
    let asm = emit_ez80_assembly(&program).unwrap();
    let run = run_assembly_test(&asm, 4_000).unwrap();

    assert!(run.halted, "{asm}");
    assert_eq!(run.result_code, 0, "{asm}");
}

#[test]
fn emits_and_runs_u24_header_allocator() {
    let source = r#"
        const HEAP_START: u24 = 0x0D0000
        const HEAP_END: u24 = 0x0E0000
        const HEADER_SIZE: u24 = 3

        fn null_ptr() -> ptr<u8> {
            return cast<ptr<u8>>(0u24)
        }

        fn alloc(request_size: u24) -> ptr<u8> {
            if request_size == 0 {
                return null_ptr()
            }
            let total_needed: u24 = (request_size + HEADER_SIZE + 1u24) & ~1u24
            let heap_end: ptr<u8> = cast<ptr<u8>>(HEAP_END)
            let current: ptr<u8> = cast<ptr<u8>>(HEAP_START)
            while current < heap_end {
                let current_header: ptr<u24> = cast<ptr<u24>>(current)
                let tagged_size: u24 = *current_header
                if tagged_size == 0 {
                    break
                }
                let block_size: u24 = tagged_size & ~1u24
                if (tagged_size & 1u24) == 1u24 && block_size >= total_needed {
                    *current_header = block_size
                    return current + HEADER_SIZE
                }
                current = current + block_size
            }
            if current + total_needed > heap_end {
                return null_ptr()
            }
            let header: ptr<u24> = cast<ptr<u24>>(current)
            let next: ptr<u8> = current + total_needed
            *header = total_needed
            if next + HEADER_SIZE <= heap_end {
                *(cast<ptr<u24>>(next)) = 0
            }
            return current + HEADER_SIZE
        }

        fn release(pointer: ptr<u8>) {
            if pointer == null_ptr() {
                return
            }
            let header: ptr<u24> = cast<ptr<u24>>(pointer - HEADER_SIZE)
            *header = *header | 1u24
        }

        fn main() {
            let heap: ptr<u24> = cast<ptr<u24>>(HEAP_START)
            *heap = 0
            let first: ptr<u8> = alloc(10u24)
            let second: ptr<u8> = alloc(5u24)
            *first = 0x42
            *second = 0x99
            test.assert_eq_u8(*first, 0x42, 1)
            test.assert_eq_u8(*second, 0x99, 2)
            test.assert_eq_u24(cast<u24>(second), cast<u24>(first) + 14u24, 3)
            release(first)
            let reused: ptr<u8> = alloc(8u24)
            test.assert_eq_u24(cast<u24>(reused), cast<u24>(first), 4)
            test.assert_eq_u8(*reused, 0x42, 5)
            test.pass()
        }
    "#;
    let program = parse_program(Path::new("game.ezra"), source).unwrap();
    let asm = emit_ez80_assembly(&program).unwrap();
    let run = run_assembly_test(&asm, 80_000).unwrap();

    assert!(run.halted, "{asm}");
    assert_eq!(run.result_code, 0, "{asm}");
}

#[test]
fn emits_wide_branches_low_bit_masks_and_small_pointer_offsets() {
    let source = r#"
        global bytes: [u8; 8] = []
        global tagged: u24 = 3u24

        fn main() {
            let cursor: ptr<u8> = &bytes[0]
            let aligned: u24 = tagged & ~1u24
            if aligned >= 2u24 {
                if (tagged & 1u24) == 1u24 {
                    let next: ptr<u8> = cursor + 3u24
                    test.assert_eq_u24(cast<u24>(next), cast<u24>(cursor) + 3u24, 1)
                    test.pass()
                }
            }
            test.fail(2)
        }
    "#;
    let program = parse_program(Path::new("game.ezra"), source).unwrap();
    let asm = emit_ez80_assembly(&program).unwrap();
    let run = run_assembly_test(&asm, 4_000).unwrap();

    assert!(asm.contains("    res 0, a"), "{asm}");
    assert!(asm.contains("    bit 0, a"), "{asm}");
    assert!(asm.contains("    inc hl\n    inc hl\n    inc hl"), "{asm}");
    assert!(run.halted, "{asm}");
    assert_eq!(run.result_code, 0, "{asm}");
}

#[test]
fn branches_directly_on_single_bit_masks() {
    let source = r#"
        global wide: u24 = 0x31u24

        fn byte_has_high_bit(byte: u8) {
            if (byte & 0x80u8) == 0u8 {
                test.fail(1)
            }
            if (byte & 0x30u8) == 0u8 {
                test.fail(2)
            }
        }

        fn main() {
            byte_has_high_bit(0xB0u8)
            if (wide & 0x20u24) != 0u24 {
                if (wide & 1u24) == 1u24 {
                    if (wide & 0x30u24) == 0x30u24 {
                        test.pass()
                    }
                }
            }
            test.fail(3)
        }
    "#;
    let program = parse_program(Path::new("game.ezra"), source).unwrap();
    let asm = emit_ez80_assembly(&program).unwrap();
    let run = run_assembly_test(&asm, 4_000).unwrap();

    assert!(asm.contains("    bit 7, a"), "{asm}");
    assert!(asm.contains("    bit 5, a"), "{asm}");
    assert!(asm.contains("    bit 0, a"), "{asm}");
    assert!(asm.contains("    and 30h"), "{asm}");
    assert!(asm.contains("    cp 30h"), "{asm}");
    assert!(run.halted, "{asm}");
    assert_eq!(run.result_code, 0, "{asm}");
}

#[test]
fn branches_directly_on_single_byte_wide_masks() {
    let source = r#"
        fn source16() -> u16 {
            return 0xA55Au16
        }


        fn equals_zero() -> u8 {
            if (source16() & 0xA500u16) == 0u16 {
                return 1
            }
            return 0
        }

        fn not_equals_zero() -> u8 {
            if (source16() & 0xA500u16) != 0u16 {
                return 1
            }
            return 0
        }

        fn equals_mask() -> u8 {
            if (source16() & 0xA500u16) == 0xA500u16 {
                return 1
            }
            return 0
        }


        fn main() {
            test.assert_eq_u8(equals_zero(), 0, 1)
            test.assert_eq_u8(not_equals_zero(), 1, 2)
            test.assert_eq_u8(equals_mask(), 1, 3)
            test.pass()
        }
    "#;
    let program = parse_program(Path::new("game.ezra"), source).unwrap();
    let asm = emit_ez80_assembly(&program).unwrap();
    let run = run_assembly_test(&asm, 8_000).unwrap();
    let body = |name: &str| {
        asm.split(&format!("_{name}:"))
            .nth(1)
            .unwrap()
            .split("    ret")
            .next()
            .unwrap()
    };

    for name in ["equals_zero", "not_equals_zero", "equals_mask"] {
        let function = body(name);
        assert_eq!(function.matches("call _source16").count(), 1, "{function}");
        assert!(function.contains("    and A5h"), "{function}");
    }
    assert!(body("equals_mask").contains("    cp A5h"), "{asm}");
    assert!(run.halted, "{asm}");
    assert_eq!(run.result_code, 0, "{asm}");

    let z80_asm = emit_ez80_assembly_with_options(
        &program,
        AssemblyOptions {
            cpu: CpuFamily::Z80,
            ..AssemblyOptions::default()
        },
    )
    .unwrap();
    assert!(z80_asm.contains("    and A5h"), "{z80_asm}");
    assert!(z80_asm.contains("    cp A5h"), "{z80_asm}");

    let u24_source = r#"
        fn source24() -> u24 {
            return 0x01A55Au24
        }

        fn not_equals_mask() -> u8 {
            if (source24() & 0x010000u24) != 0x010000u24 {
                return 1
            }
            return 0
        }

        fn main() {
            test.assert_eq_u8(not_equals_mask(), 0, 1)
            test.pass()
        }
    "#;
    let u24_program = parse_program(Path::new("u24.ezra"), u24_source).unwrap();
    let u24_asm = emit_ez80_assembly(&u24_program).unwrap();
    let u24_run = run_assembly_test(&u24_asm, 4_000).unwrap();
    let not_equals_mask = u24_asm
        .split("_not_equals_mask:")
        .nth(1)
        .unwrap()
        .split("    ret")
        .next()
        .unwrap();

    assert_eq!(
        not_equals_mask.matches("call _source24").count(),
        1,
        "{not_equals_mask}"
    );
    assert!(
        not_equals_mask.contains("    bit 0, a"),
        "{not_equals_mask}"
    );
    assert!(u24_run.halted, "{u24_asm}");
    assert_eq!(u24_run.result_code, 0, "{u24_asm}");

    let intel_asm = emit_ez80_assembly_with_options(
        &program,
        AssemblyOptions {
            cpu: CpuFamily::I8080,
            ram_base: Address24::new(0x2000),
            ..AssemblyOptions::default()
        },
    )
    .unwrap();

    assert!(intel_asm.contains("    ani A5h"), "{intel_asm}");
    assert!(intel_asm.contains("    cpi A5h"), "{intel_asm}");
    assert!(!intel_asm.contains("\n    bit "), "{intel_asm}");
}

#[test]
fn rejects_local_shadowing() {
    let source = r#"
            global score: u8 = 0

            fn bump(value: u8) {
                let value: u8 = 1
            }

            fn main() {
                let score: u8 = 1
                test.pass()
            }
        "#;
    let program = parse_program(Path::new("game.ezra"), source).unwrap();
    let error = emit_ez80_assembly(&program).unwrap_err();

    assert_eq!(error.message, "local `score` shadows an existing name");
}
