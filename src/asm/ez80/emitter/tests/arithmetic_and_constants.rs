use super::*;

#[test]
fn emits_and_runs_u16_storage_and_return() {
    let source = r#"
            global total: u16 = 0x0100

            fn add_base(v: u16) -> u16 {
                return v + 0x0023
            }

            fn main() {
                let x: u16 = add_base(total)
                x += 0x0010
                test.assert_eq_u16(x, 0x0133, 5)
                test.pass()
            }
        "#;
    let program = parse_program(Path::new("game.ezra"), source).unwrap();
    let asm = emit_ez80_assembly(&program).unwrap();
    let run = run_assembly_test(&asm, 2_000).unwrap();

    assert!(run.halted, "{asm}");
    assert_eq!(run.result_code, 0, "{asm}");
}

#[test]
fn emits_byte_accurate_u16_store_without_clobbering_next_variable() {
    let source = r#"
            fn main() {
                let wide: u16 = 0x1234
                let guard: u8 = 0x7A
                wide += 1
                test.assert_eq_u16(wide, 0x1235, 6)
                test.assert_eq_u8(guard, 0x7A, 7)
                test.pass()
            }
        "#;
    let program = parse_program(Path::new("game.ezra"), source).unwrap();
    let asm = emit_ez80_assembly(&program).unwrap();
    let run = run_assembly_test(&asm, 2_000).unwrap();

    assert!(run.halted, "{asm}");
    assert_eq!(run.result_code, 0, "{asm}");
}

#[test]
fn emits_and_runs_u24_storage_and_return() {
    let source = r#"
            global base: u24 = 0x010000

            fn bump(v: u24) -> u24 {
                return v + 0x000123
            }

            fn main() {
                let x: u24 = bump(base)
                x += 0x000010
                test.assert_eq_u24(x, 0x010133, 8)
                test.pass()
            }
        "#;
    let program = parse_program(Path::new("game.ezra"), source).unwrap();
    let asm = emit_ez80_assembly(&program).unwrap();
    let run = run_assembly_test(&asm, 2_000).unwrap();

    assert!(run.halted, "{asm}");
    assert_eq!(run.result_code, 0, "{asm}");
}

#[test]
fn emits_and_runs_wide_sub_and_bitwise_ops() {
    let expected_u16 = (((0x12F0u16 - 0x0010) & 0x0FF0) | 0x1000) ^ 0x00F0;
    let expected_u24 =
        ((((0x010123u32 - 0x000020) & 0x01FFFF) | 0x020000) ^ 0x000003) & 0x00FF_FFFF;
    let source = format!(
        r#"
            fn main() {{
                let a: u16 = 0x12F0 - 0x0010
                a &= 0x0FF0
                a |= 0x1000
                a ^= 0x00F0
                test.assert_eq_u16(a, 0x{expected_u16:04X}, 10)

                let b: u24 = 0x010123 - 0x000020
                b &= 0x01FFFF
                b |= 0x020000
                b ^= 0x000003
                test.assert_eq_u24(b, 0x{expected_u24:06X}, 11)
                test.pass()
            }}
        "#
    );
    let program = parse_program(Path::new("game.ezra"), &source).unwrap();
    let asm = emit_ez80_assembly(&program).unwrap();
    let run = run_assembly_test(&asm, 4_000).unwrap();

    assert!(run.halted, "{asm}");
    assert_eq!(run.result_code, 0, "{asm}");
}

#[test]
fn emits_and_runs_dynamic_unary_ops() {
    let expected_u8_neg = 0u8.wrapping_sub(5);
    let expected_u8_not = !0x5Au8;
    let expected_u16_neg = 0u16.wrapping_sub(0x0023);
    let expected_u16_not = !0x120Fu16;
    let expected_u24_neg = (0u32.wrapping_sub(0x000123)) & 0x00FF_FFFF;
    let expected_u24_not = (!0x010203u32) & 0x00FF_FFFF;
    let source = format!(
        r#"
            fn main() {{
                let a: u8 = 5
                let b: u8 = 0x5A
                test.assert_eq_u8(-a, 0x{expected_u8_neg:02X}, 1)
                test.assert_eq_u8(~b, 0x{expected_u8_not:02X}, 2)
                test.assert_eq_u8(!(a == 0), 1, 3)
                test.assert_eq_u8(!(a != 0), 0, 4)

                let c: u16 = 0x0023
                let d: u16 = 0x120F
                test.assert_eq_u16(-c, 0x{expected_u16_neg:04X}, 5)
                test.assert_eq_u16(~d, 0x{expected_u16_not:04X}, 6)

                let e: u24 = 0x000123
                let f: u24 = 0x010203
                test.assert_eq_u24(-e, 0x{expected_u24_neg:06X}, 7)
                test.assert_eq_u24(~f, 0x{expected_u24_not:06X}, 8)
                test.pass()
            }}
            "#
    );
    let program = parse_program(Path::new("game.ezra"), &source).unwrap();
    let asm = emit_ez80_assembly(&program).unwrap();
    let run = run_assembly_test(&asm, 6_000).unwrap();

    assert!(run.halted, "{asm}");
    assert_eq!(run.result_code, 0, "{asm}");
}

#[test]
fn emits_and_runs_short_circuit_logical_ops() {
    let source = r#"
            alias flag = bool
            global calls: u8 = 0

            fn bump(value: bool) -> bool {
                calls += 1
                return value
            }

            fn main() {
                calls = 0
                let and_skip: bool = false && bump(true)
                test.assert_eq_u8(and_skip, false, 1)
                test.assert_eq_u8(calls, 0, 2)

                let or_skip: flag = true || bump(false)
                test.assert_eq_u8(or_skip, true, 3)
                test.assert_eq_u8(calls, 0, 4)

                let and_run: bool = true && bump(true)
                test.assert_eq_u8(and_run, true, 5)
                test.assert_eq_u8(calls, 1, 6)

                let or_run: bool = false || bump(true)
                test.assert_eq_u8(or_run, true, 7)
                test.assert_eq_u8(calls, 2, 8)

                test.pass()
            }
        "#;
    let program = parse_program(Path::new("game.ezra"), source).unwrap();
    let asm = emit_ez80_assembly(&program).unwrap();
    let run = run_assembly_test(&asm, 8_000).unwrap();

    assert!(run.halted, "{asm}");
    assert_eq!(run.result_code, 0, "{asm}");
}

#[test]
fn emits_and_runs_constant_shift_ops() {
    let expected_u8_assign = 0x12u8.wrapping_shl(2) >> 1;
    let expected_u8_expr = 0x81u8 >> 3;
    let expected_u16_expr = 0x1234u16.wrapping_shl(3) >> 2;
    let expected_u16_assign = 0x00F0u16.wrapping_shl(4) >> 3;
    let expected_u24_expr = ((0x010203u32 << 4) & 0x00FF_FFFF) >> 3;
    let expected_u24_assign = ((0x000F00u32 << 5) & 0x00FF_FFFF) >> 2;
    let expected_i16_const = ((-0x1234i16) >> 3) as u16;
    let expected_i24_const = ((-0x012345i32) >> 5) & 0x00FF_FFFF;
    let source = format!(
        r#"
            const SIGNED_WORD_SHIFT: i16 = (-0x1234i16) >> 3
            const SIGNED_WIDE_SHIFT: i24 = (-0x012345i24) >> 5
            const SIGNED_BYTE_BIG_SHIFT: i8 = (-1i8) >> 64
            const SIGNED_WORD_BIG_SHIFT: i16 = (-1i16) >> 64
            const SIGNED_WIDE_BIG_SHIFT: i24 = (-1i24) >> 64

            fn main() {{
                let a: u8 = 0x12
                a <<= 2
                a >>= 1
                test.assert_eq_u8(a, 0x{expected_u8_assign:02X}, 1)
                test.assert_eq_u8(0x81 >> 3, 0x{expected_u8_expr:02X}, 2)

                let b: u16 = 0x1234
                let c: u16 = (b << 3) >> 2
                test.assert_eq_u16(c, 0x{expected_u16_expr:04X}, 3)
                let d: u16 = 0x00F0
                d <<= 4
                d >>= 3
                test.assert_eq_u16(d, 0x{expected_u16_assign:04X}, 4)

                let e: u24 = 0x010203
                let f: u24 = (e << 4) >> 3
                test.assert_eq_u24(f, 0x{expected_u24_expr:06X}, 5)
                let g: u24 = 0x000F00
                g <<= 5
                g >>= 2
                test.assert_eq_u24(g, 0x{expected_u24_assign:06X}, 6)
                test.assert_eq_u16(cast<u16>(SIGNED_WORD_SHIFT), 0x{expected_i16_const:04X}, 7)
                test.assert_eq_u24(cast<u24>(SIGNED_WIDE_SHIFT), 0x{expected_i24_const:06X}, 8)
                test.assert_eq_u8(cast<u8>(SIGNED_BYTE_BIG_SHIFT), 0xFF, 9)
                test.assert_eq_u16(cast<u16>(SIGNED_WORD_BIG_SHIFT), 0xFFFF, 10)
                test.assert_eq_u24(cast<u24>(SIGNED_WIDE_BIG_SHIFT), 0xFFFFFF, 11)
                test.pass()
            }}
            "#
    );
    let program = parse_program(Path::new("game.ezra"), &source).unwrap();
    let asm = emit_ez80_assembly(&program).unwrap();
    let run = run_assembly_test(&asm, 10_000).unwrap();

    assert!(run.halted, "{asm}");
    assert_eq!(run.result_code, 0, "{asm}");
}

#[test]
fn emits_and_runs_runtime_shift_counts() {
    let source = r#"
            const BYTE_COUNT: u8 = 3
            const WORD_COUNT: u16 = 4
            const SIGNED_CONST_COUNT: i8 = 1
            const WIDE_LEFT: u24 = 0x010203u24 << BYTE_COUNT
            const WIDE_RIGHT: u24 = WIDE_LEFT >> WORD_COUNT
            const SIGNED_RIGHT: i16 = (-0x1234i16) >> BYTE_COUNT

            fn shl8(value: u8, count: u8) -> u8 {
                return value << count
            }

            fn shr8(value: u8, count: u8) -> u8 {
                return value >> count
            }

            fn main() {
                let count: u8 = 3
                test.assert_eq_u8(shl8(0x12, count), 0x90, 1)
                test.assert_eq_u8(shr8(0x81, count), 0x10, 2)

                let word_count: u8 = 4
                let word: u16 = 0x1234 << word_count
                test.assert_eq_u16(word, 0x2340, 3)

                let word_shift: u8 = 3
                let word_assign: u16 = word
                word_assign >>= word_shift
                test.assert_eq_u16(word_assign, 0x0468, 4)

                let wide_count: u8 = 4
                let wide: u24 = 0x010203 << wide_count
                test.assert_eq_u24(wide, 0x102030, 5)

                let wide_assign: u24 = wide
                let wide_shift: u8 = 2
                wide_assign >>= wide_shift
                test.assert_eq_u24(wide_assign, 0x04080C, 6)

                let byte: u8 = 0x80
                let byte_shift: u8 = 8
                let zero: u8 = byte >> byte_shift
                test.assert_eq_u8(zero, 0, 7)

                test.assert_eq_u24(WIDE_LEFT, 0x081018, 8)
                test.assert_eq_u24(WIDE_RIGHT, 0x008101, 9)
                test.assert_eq_u16(cast<u16>(SIGNED_RIGHT), 0xFDB9, 10)

                test.assert_eq_u16(0x1234u16 >> SIGNED_CONST_COUNT, 0x091A, 11)
                test.pass()
            }
        "#;
    let program = parse_program(Path::new("game.ezra"), source).unwrap();
    let asm = emit_ez80_assembly(&program).unwrap();
    let run = run_assembly_test(&asm, 40_000).unwrap();

    assert!(asm.contains("    dec b"), "{asm}");
    assert!(run.halted, "{asm}");
    assert_eq!(run.result_code, 0, "{asm}");
}

#[test]
fn emits_and_runs_large_literal_shift_counts() {
    let source = r#"
            fn main() {
                test.assert_eq_u8(0x80 >> 25, 0, 1)
                test.assert_eq_u8(0x01 << 25, 0, 2)
                test.assert_eq_u16(0x8000 >> 25, 0, 3)
                test.assert_eq_u16(0x0001 << 25, 0, 4)
                test.assert_eq_u24(0x800000 >> 25, 0, 5)
                test.assert_eq_u24(0x000001 << 25, 0, 6)
                test.assert_eq_u8((-1i8) >> 25, 0xFF, 7)
                test.assert_eq_u16((-1i16) >> 25, 0xFFFF, 8)
                test.assert_eq_u24((-1i24) >> 25, 0xFFFFFF, 9)
                test.assert_eq_u8((-1i8) >> 64, 0xFF, 10)
                test.assert_eq_u16((-1i16) >> 64, 0xFFFF, 11)
                test.assert_eq_u24((-1i24) >> 64, 0xFFFFFF, 12)
                test.pass()
            }
        "#;
    let program = parse_program(Path::new("game.ezra"), source).unwrap();
    let asm = emit_ez80_assembly(&program).unwrap();
    let run = run_assembly_test(&asm, 20_000).unwrap();

    assert!(run.halted, "{asm}");
    assert_eq!(run.result_code, 0, "{asm}");
}

#[test]
fn emits_and_runs_signed_right_shifts() {
    let expected_i8_expr = ((-4i8) >> 1) as u8;
    let expected_i8_assign = ((-5i8) >> 2) as u8;
    let expected_i16_expr = ((-0x1234i16) >> 3) as u16;
    let expected_i16_assign = ((-0x2345i16) >> 4) as u16;
    let expected_i24_expr = ((-0x012345i32) >> 5) & 0x00FF_FFFF;
    let expected_i24_assign = ((-0x023456i32) >> 6) & 0x00FF_FFFF;
    let source = format!(
        r#"
            fn shr8(value: i8, count: u8) -> i8 {{
                return value >> count
            }}

            fn shr16(value: i16, count: u8) -> i16 {{
                return value >> count
            }}

            fn shr24(value: i24, count: u8) -> i24 {{
                return value >> count
            }}

            fn main() {{
                let one: u8 = 1
                test.assert_eq_u8(shr8(-4, one), 0x{expected_i8_expr:02X}, 1)

                let byte: i8 = -5
                let two: u8 = 2
                byte >>= two
                test.assert_eq_u8(byte, 0x{expected_i8_assign:02X}, 2)

                let three: u8 = 3
                test.assert_eq_u16(shr16(-0x1234, three), 0x{expected_i16_expr:04X}, 3)

                let word: i16 = -0x2345
                let four: u8 = 4
                word >>= four
                test.assert_eq_u16(word, 0x{expected_i16_assign:04X}, 4)

                let five: u8 = 5
                test.assert_eq_u24(shr24(-0x012345, five), 0x{expected_i24_expr:06X}, 5)

                let wide: i24 = -0x023456
                let six: u8 = 6
                wide >>= six
                test.assert_eq_u24(wide, 0x{expected_i24_assign:06X}, 6)
                test.pass()
            }}
            "#
    );
    let program = parse_program(Path::new("game.ezra"), &source).unwrap();
    let asm = emit_ez80_assembly(&program).unwrap();
    let run = run_assembly_test(&asm, 40_000).unwrap();

    assert!(asm.contains("    sra a"), "{asm}");
    assert!(run.halted, "{asm}");
    assert_eq!(run.result_code, 0, "{asm}");
}

#[test]
fn emits_and_runs_defined_u8_division_and_modulo() {
    let expected_div = 23u8 / 5;
    let expected_mod = 23u8 % 5;
    let expected_div_zero = 0u8;
    let expected_mod_zero = 0u8;
    let expected_const_div_zero = 0u8;
    let expected_const_mod_zero = 0u8;
    let source = format!(
        r#"
            const CONST_DIV_ZERO: u8 = 10 / 0
            const CONST_MOD_ZERO: u8 = 10 % 0

            fn div(v: u8, by: u8) -> u8 {{
                return v / by
            }}

            fn rem(v: u8, by: u8) -> u8 {{
                return v % by
            }}

            fn main() {{
                let a: u8 = div(23, 5)
                let b: u8 = rem(23, 5)
                let c: u8 = div(23, 0)
                let d: u8 = rem(23, 0)
                test.assert_eq_u8(a, {expected_div}, 1)
                test.assert_eq_u8(b, {expected_mod}, 2)
                test.assert_eq_u8(c, {expected_div_zero}, 3)
                test.assert_eq_u8(d, {expected_mod_zero}, 4)
                test.assert_eq_u8(CONST_DIV_ZERO, {expected_const_div_zero}, 5)
                test.assert_eq_u8(CONST_MOD_ZERO, {expected_const_mod_zero}, 6)
                test.pass()
            }}
            "#
    );
    let program = parse_program(Path::new("game.ezra"), &source).unwrap();
    let asm = emit_ez80_assembly(&program).unwrap();
    let run = run_assembly_test(&asm, 4_000).unwrap();

    assert!(asm.contains("    call __ezra_div_u8"), "{asm}");
    assert!(asm.contains("    call __ezra_mod_u8"), "{asm}");
    assert!(run.halted, "{asm}");
    assert_eq!(run.result_code, 0, "{asm}");
}

#[test]
fn emits_and_runs_signed_runtime_division_and_modulo() {
    let expected_i8_div = ((-3i8) / 2) as u8;
    let expected_i8_mod = ((-3i8) % 2) as u8;
    let expected_i16_div = ((-300i16) / 7) as u16;
    let expected_i16_mod = ((-300i16) % 7) as u16;
    let expected_i24_div = ((-0x012345i32) / 17) & 0x00FF_FFFF;
    let expected_i24_mod = ((-0x012345i32) % 17) & 0x00FF_FFFF;
    let expected_i8_neg_divisor_div = (7i8 / -3) as u8;
    let expected_i8_neg_divisor_mod = (7i8 % -3) as u8;
    let expected_i8_both_negative_div = (-7i8 / -3) as u8;
    let expected_i8_both_negative_mod = (-7i8 % -3) as u8;
    let expected_i16_neg_divisor_div = (300i16 / -7) as u16;
    let expected_i16_neg_divisor_mod = (300i16 % -7) as u16;
    let expected_i24_both_negative_div = ((-0x012345i32) / -17) & 0x00FF_FFFF;
    let expected_i24_both_negative_mod = ((-0x012345i32) % -17) & 0x00FF_FFFF;
    let expected_i8_overflow_div = i8::MIN as u8;
    let expected_i16_overflow_div = i16::MIN as u16;
    let expected_i24_overflow_div = 0x800000u32;
    let expected_signed_div_zero = 0;
    let expected_signed_mod_zero = 0;
    let source = format!(
        r#"
            alias subpx = i24
            const CONST_I16_DIV_ZERO: i16 = -300 / 0
            const CONST_I16_MOD_ZERO: i16 = -300 % 0
            const CONST_I24_DIV_ZERO: subpx = -0x012345 / 0
            const CONST_I24_MOD_ZERO: subpx = -0x012345 % 0

            fn div8(a: i8, b: i8) -> i8 {{
                return a / b
            }}

            fn mod8(a: i8, b: i8) -> i8 {{
                return a % b
            }}

            fn div16(a: i16, b: i16) -> i16 {{
                return a / b
            }}

            fn mod16(a: i16, b: i16) -> i16 {{
                return a % b
            }}

            fn div24(a: subpx, b: subpx) -> subpx {{
                return a / b
            }}

            fn mod24(a: subpx, b: subpx) -> subpx {{
                return a % b
            }}

            fn main() {{
                let a: i8 = -3
                let b: i8 = 2
                test.assert_eq_u8(div8(a, b), 0x{expected_i8_div:02X}, 1)
                test.assert_eq_u8(mod8(a, b), 0x{expected_i8_mod:02X}, 2)
                test.assert_eq_u8(div8(a, 0), 0, 3)
                test.assert_eq_u8(mod8(a, 0), 0, 4)
                test.assert_eq_u8(div8(-128, -1), 0x{expected_i8_overflow_div:02X}, 5)
                test.assert_eq_u8(mod8(-128, -1), 0, 6)
                test.assert_eq_u8(div8(7, -3), 0x{expected_i8_neg_divisor_div:02X}, 15)
                test.assert_eq_u8(mod8(7, -3), 0x{expected_i8_neg_divisor_mod:02X}, 16)
                test.assert_eq_u8(div8(-7, -3), 0x{expected_i8_both_negative_div:02X}, 17)
                test.assert_eq_u8(mod8(-7, -3), 0x{expected_i8_both_negative_mod:02X}, 18)

                let c: i16 = -300
                let d: i16 = 7
                test.assert_eq_u16(div16(c, d), 0x{expected_i16_div:04X}, 7)
                test.assert_eq_u16(mod16(c, d), 0x{expected_i16_mod:04X}, 8)
                test.assert_eq_u16(div16(-32768, -1), 0x{expected_i16_overflow_div:04X}, 9)
                test.assert_eq_u16(mod16(-32768, -1), 0, 10)
                test.assert_eq_u16(div16(300, -7), 0x{expected_i16_neg_divisor_div:04X}, 19)
                test.assert_eq_u16(mod16(300, -7), 0x{expected_i16_neg_divisor_mod:04X}, 20)
                test.assert_eq_u16(div16(c, 0), {expected_signed_div_zero}, 23)
                test.assert_eq_u16(mod16(c, 0), {expected_signed_mod_zero}, 24)

                let e: subpx = -0x012345
                let f: subpx = 17
                test.assert_eq_u24(div24(e, f), 0x{expected_i24_div:06X}, 11)
                test.assert_eq_u24(mod24(e, f), 0x{expected_i24_mod:06X}, 12)
                test.assert_eq_u24(div24(-0x800000, -1), 0x{expected_i24_overflow_div:06X}, 13)
                test.assert_eq_u24(mod24(-0x800000, -1), 0, 14)
                test.assert_eq_u24(div24(-0x012345, -17), 0x{expected_i24_both_negative_div:06X}, 21)
                test.assert_eq_u24(mod24(-0x012345, -17), 0x{expected_i24_both_negative_mod:06X}, 22)
                test.assert_eq_u24(div24(e, 0), {expected_signed_div_zero}, 25)
                test.assert_eq_u24(mod24(e, 0), {expected_signed_mod_zero}, 26)
                test.assert_eq_u16(CONST_I16_DIV_ZERO, {expected_signed_div_zero}, 27)
                test.assert_eq_u16(CONST_I16_MOD_ZERO, {expected_signed_mod_zero}, 28)
                test.assert_eq_u24(CONST_I24_DIV_ZERO, {expected_signed_div_zero}, 29)
                test.assert_eq_u24(CONST_I24_MOD_ZERO, {expected_signed_mod_zero}, 30)
                test.pass()
            }}
            "#
    );
    let program = parse_program(Path::new("game.ezra"), &source).unwrap();
    let asm = emit_ez80_assembly(&program).unwrap();
    let run = run_assembly_test(&asm, 1_000_000).unwrap();

    assert!(asm.contains("    call __ezra_div_i24"), "{asm}");
    assert!(asm.contains("    call __ezra_mod_i24"), "{asm}");
    assert!(asm.contains("__ezra_div_i24:"), "{asm}");
    assert!(asm.contains("__ezra_mod_i24:"), "{asm}");
    assert!(run.halted, "{asm}");
    assert_eq!(run.result_code, 0, "{asm}");
}

#[test]
fn emits_and_runs_signed_constant_returns() {
    let expected_i16 = (-300i16) as u16;
    let expected_i24 = (-0x012345i32) & 0x00FF_FFFF;
    let source = format!(
        r#"
            alias subpx = i24
            const NEG16: i16 = -300
            const NEG24: subpx = -0x012345

            fn neg16() -> i16 {{
                return NEG16
            }}

            fn neg24() -> subpx {{
                return NEG24
            }}

            fn main() {{
                test.assert_eq_u16(neg16(), 0x{expected_i16:04X}, 1)
                test.assert_eq_u24(neg24(), 0x{expected_i24:06X}, 2)
                test.pass()
            }}
            "#
    );
    let program = parse_program(Path::new("game.ezra"), &source).unwrap();
    let asm = emit_ez80_assembly(&program).unwrap();
    let run = run_assembly_test(&asm, 4_000).unwrap();

    assert!(run.halted, "{asm}");
    assert_eq!(run.result_code, 0, "{asm}");
}

#[test]
fn emits_and_runs_forward_scalar_constants() {
    let source = r#"
            const DOUBLE_WIDTH: u16 = SCREEN_W * 2
            const BYTE_COUNT: u8 = BASE + EXTRA
            const MASKED: u8 = (FLAGS & ENABLED) | READY
            const CASTED: u24 = cast<u24>(DOUBLE_WIDTH) + 1

            const SCREEN_W: u16 = 160
            const BASE: u8 = 3
            const EXTRA: u8 = 4
            const FLAGS: u8 = 0b1010
            const ENABLED: u8 = 0b0110
            const READY: u8 = 0b0001

            fn main() {
                test.assert_eq_u16(DOUBLE_WIDTH, 320, 1)
                test.assert_eq_u8(BYTE_COUNT, 7, 2)
                test.assert_eq_u8(MASKED, 3, 3)
                test.assert_eq_u24(CASTED, 321, 4)
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
fn rejects_circular_constant_references() {
    let source = r#"
            const A: u8 = B + 1
            const B: u8 = A + 1
            fn main() {
                test.pass()
            }
        "#;
    let program = parse_program(Path::new("game.ezra"), source).unwrap();
    let error = emit_ez80_assembly(&program).unwrap_err();

    assert_eq!(error.message, "circular constant reference involving `A`");
}

#[test]
fn emits_and_runs_signed_arithmetic_with_untyped_literals() {
    let expected_i8 = (-3i8).wrapping_add(1) as u8;
    let expected_i16 = (-300i16).wrapping_add(1) as u16;
    let source = format!(
        r#"
            fn main() {{
                let a: i8 = -3
                test.assert_eq_u8(a + 1, 0x{expected_i8:02X}, 1)

                let b: i16 = -300
                test.assert_eq_u16(b + 1, 0x{expected_i16:04X}, 2)
                test.pass()
            }}
            "#
    );
    let program = parse_program(Path::new("game.ezra"), &source).unwrap();
    let asm = emit_ez80_assembly(&program).unwrap();
    let run = run_assembly_test(&asm, 4_000).unwrap();

    assert!(run.halted, "{asm}");
    assert_eq!(run.result_code, 0, "{asm}");
}

#[test]
fn emits_and_runs_typed_integer_literal_suffixes() {
    let expected_i8 = (-3i8) as u8;
    let expected_i16 = (-300i16) as u16;
    let expected_i24 = (-0x012345i32) & 0x00FF_FFFF;
    let source = format!(
        r#"
            const BYTE: u8 = 123u8
            const NEG8: i8 = -3i8
            const WORD: u16 = 12345u16
            const NEG16: i16 = -300i16
            const LONG: u24 = 0x123456u24
            const NEG24: i24 = -0x012345i24

            fn main() {{
                let byte: u8 = 7u8
                let signed: i8 = -3i8
                let word: u16 = 0x2345u16
                let wide: u24 = 0x345678u24
                test.assert_eq_u8(BYTE, 123, 1)
                test.assert_eq_u8(cast<u8>(NEG8), 0x{expected_i8:02X}, 2)
                test.assert_eq_u16(WORD, 12345, 3)
                test.assert_eq_u16(cast<u16>(NEG16), 0x{expected_i16:04X}, 4)
                test.assert_eq_u24(LONG, 0x123456, 5)
                test.assert_eq_u24(cast<u24>(NEG24), 0x{expected_i24:06X}, 6)
                test.assert_eq_u8(byte, 7, 7)
                test.assert_eq_u8(cast<u8>(signed), 0x{expected_i8:02X}, 8)
                test.assert_eq_u16(word, 0x2345, 9)
                test.assert_eq_u24(wide, 0x345678, 10)
                test.pass()
            }}
            "#
    );
    let program = parse_program(Path::new("game.ezra"), &source).unwrap();
    let asm = emit_ez80_assembly(&program).unwrap();
    let run = run_assembly_test(&asm, 8_000).unwrap();

    assert!(run.halted, "{asm}");
    assert_eq!(run.result_code, 0, "{asm}");
}

#[test]
fn emits_and_runs_signed_i24_multiply_runtime_helper() {
    let expected = ((-0x123i32) * 0x45) & 0x00FF_FFFF;
    let source = format!(
        r#"
            alias subpx = i24

            fn mul24(a: subpx, b: subpx) -> subpx {{
                return a * b
            }}

            fn main() {{
                let a: subpx = -0x123
                let b: subpx = 0x45
                test.assert_eq_u24(mul24(a, b), 0x{expected:06X}, 1)
                test.pass()
            }}
            "#
    );
    let program = parse_program(Path::new("game.ezra"), &source).unwrap();
    let asm = emit_ez80_assembly(&program).unwrap();
    let run = run_assembly_test(&asm, 80_000).unwrap();

    assert!(asm.contains("    call __ezra_mul_i24"), "{asm}");
    assert!(asm.contains("__ezra_mul_i24:"), "{asm}");
    assert!(run.halted, "{asm}");
    assert_eq!(run.result_code, 0, "{asm}");
}

#[test]
fn emits_and_runs_explicit_integer_casts() {
    let source = r#"
            const SMALL: u8 = 0x12
            const WIDE: u16 = cast<u16>(SMALL) + 0x0100

            fn low_byte(v: u16) -> u8 {
                return cast<u8>(v)
            }

            fn widen(v: u8) -> u16 {
                return cast<u16>(v)
            }

            fn widen_signed_byte(v: i8) -> i16 {
                return cast<i16>(v)
            }

            fn widen_signed_word(v: i16) -> i24 {
                return cast<i24>(v)
            }

            fn widen_signed_byte_to_u24(v: i8) -> u24 {
                return cast<u24>(v)
            }

            fn bool_from_u8(v: u8) -> bool {
                return cast<bool>(v)
            }

            fn bool_from_i8(v: i8) -> bool {
                return cast<bool>(v)
            }

            fn bool_from_u16(v: u16) -> bool {
                return cast<bool>(v)
            }

            fn bool_from_u24(v: u24) -> bool {
                return cast<bool>(v)
            }

            fn bool_from_ptr(v: ptr<u8>) -> bool {
                return cast<bool>(v)
            }

            fn main() {
                let wide: u16 = cast<u16>(0x12)
                let narrow: u8 = cast<u8>(0x1234)
                let local_true: bool = cast<bool>(2)
                let local_false: bool = cast<bool>(0)
                let local_ptr_true: bool = cast<bool>(cast<ptr<u8>>(0x040123))
                let local_ptr_false: bool = cast<bool>(cast<ptr<u8>>(0u24))
                let assigned: u8 = 0
                assigned = cast<u8>(0x01FE)
                test.assert_eq_u16(wide, 0x0012, 1)
                test.assert_eq_u8(narrow, 0x34, 2)
                test.assert_eq_u8(assigned, 0xFE, 3)
                test.assert_eq_u8(low_byte(0xABCD), 0xCD, 4)
                test.assert_eq_u16(widen(0x7A), 0x007A, 5)
                test.assert_eq_u16(WIDE, 0x0112, 6)
                test.assert_eq_u16(widen_signed_byte(-3), 0xFFFD, 7)
                test.assert_eq_u24(widen_signed_word(-300), 0xFFFED4, 8)
                test.assert_eq_u24(widen_signed_byte_to_u24(-3), 0xFFFFFD, 9)
                test.assert_eq_u8(local_true, true, 10)
                test.assert_eq_u8(local_false, false, 11)
                test.assert_eq_u8(bool_from_u8(2), true, 12)
                test.assert_eq_u8(bool_from_u8(0), false, 13)
                test.assert_eq_u8(bool_from_i8(-3), true, 14)
                test.assert_eq_u8(bool_from_u16(0x0100), true, 15)
                test.assert_eq_u8(bool_from_u16(0), false, 16)
                test.assert_eq_u8(bool_from_u24(0x010000), true, 17)
                test.assert_eq_u8(bool_from_u24(0), false, 18)
                test.assert_eq_u8(local_ptr_true, true, 19)
                test.assert_eq_u8(local_ptr_false, false, 20)
                test.assert_eq_u8(bool_from_ptr(cast<ptr<u8>>(0x040123)), true, 21)
                test.assert_eq_u8(bool_from_ptr(cast<ptr<u8>>(0u24)), false, 22)
                test.pass()
            }
        "#;
    let program = parse_program(Path::new("game.ezra"), source).unwrap();
    let asm = emit_ez80_assembly(&program).unwrap();
    let run = run_assembly_test(&asm, 8_000).unwrap();

    assert!(run.halted, "{asm}");
    assert_eq!(run.result_code, 0, "{asm}");
}

#[test]
fn emits_and_runs_constant_cast_semantics() {
    let source = r#"
            alias byte = u8

            const NARROW: u8 = cast<u8>(0x1234)
            const WIDE: u16 = cast<u16>(0x12)
            const BIT_PATTERN: u8 = cast<u8>(-1)
            const ALIAS_NARROW: byte = cast<byte>(0x01AB)
            const TRUE_VALUE: bool = cast<bool>(2)
            const FALSE_VALUE: bool = cast<bool>(0)
            const RAW: u24 = cast<u24>(cast<ptr<u8>>(0x040123))
            const PTR_TRUE: bool = cast<bool>(cast<ptr<u8>>(0x040123))
            const PTR_FALSE: bool = cast<bool>(cast<ptr<u8>>(0u24))

            fn main() {
                test.assert_eq_u8(NARROW, 0x34, 1)
                test.assert_eq_u16(WIDE, 0x0012, 2)
                test.assert_eq_u8(BIT_PATTERN, 0xFF, 3)
                test.assert_eq_u8(ALIAS_NARROW, 0xAB, 4)
                test.assert_eq_u8(TRUE_VALUE, true, 5)
                test.assert_eq_u8(FALSE_VALUE, false, 6)
                test.assert_eq_u24(RAW, 0x040123, 7)
                test.assert_eq_u8(PTR_TRUE, true, 8)
                test.assert_eq_u8(PTR_FALSE, false, 9)
                test.pass()
            }
        "#;
    let program = parse_program(Path::new("game.ezra"), source).unwrap();
    let asm = emit_ez80_assembly(&program).unwrap();
    let run = run_assembly_test(&asm, 6_000).unwrap();

    assert!(run.halted, "{asm}");
    assert_eq!(run.result_code, 0, "{asm}");
}

#[test]
fn emits_and_runs_wrapping_constant_arithmetic() {
    let source = format!(
        r#"
            const U8_WRAP: u8 = 255 + 1
            const I8_WRAP: i8 = 127 + 1
            const U16_WRAP: u16 = 0xFFFF + 2
            const I16_WRAP: i16 = 32767 + 1
            const U8_NOT: u8 = ~0
            const U8_SHIFT: u8 = 1 << 8
            const HOST_ADD_WRAP: u24 = 9223372036854775807 + 1
            const HOST_SUB_WRAP: u24 = (-9223372036854775807 - 1) - 1
            const HOST_MUL_WRAP: u24 = 9223372036854775807 * 3
            const HOST_NEG_WRAP: u24 = -(-9223372036854775807 - 1)

            fn main() {{
                test.assert_eq_u8(U8_WRAP, 0x{:02X}, 1)
                test.assert_eq_u8(cast<u8>(I8_WRAP), 0x{:02X}, 2)
                test.assert_eq_u16(U16_WRAP, 0x{:04X}, 3)
                test.assert_eq_u16(cast<u16>(I16_WRAP), 0x{:04X}, 4)
                test.assert_eq_u8(U8_NOT, 0x{:02X}, 5)
                test.assert_eq_u8(U8_SHIFT, 0x{:02X}, 6)
                test.assert_eq_u24(HOST_ADD_WRAP, 0x{:06X}, 7)
                test.assert_eq_u24(HOST_SUB_WRAP, 0x{:06X}, 8)
                test.assert_eq_u24(HOST_MUL_WRAP, 0x{:06X}, 9)
                test.assert_eq_u24(HOST_NEG_WRAP, 0x{:06X}, 10)
                test.pass()
            }}
            "#,
        255u8.wrapping_add(1),
        127i8.wrapping_add(1) as u8,
        0xFFFFu16.wrapping_add(2),
        32767i16.wrapping_add(1) as u16,
        !0u8,
        1u16.wrapping_shl(8) as u8,
        (i64::MAX.wrapping_add(1) as u64 & 0x00FF_FFFF) as u32,
        (i64::MIN.wrapping_sub(1) as u64 & 0x00FF_FFFF) as u32,
        (i64::MAX.wrapping_mul(3) as u64 & 0x00FF_FFFF) as u32,
        (i64::MIN.wrapping_neg() as u64 & 0x00FF_FFFF) as u32,
    );
    let program = parse_program(Path::new("game.ezra"), &source).unwrap();
    let asm = emit_ez80_assembly(&program).unwrap();
    let run = run_assembly_test(&asm, 6_000).unwrap();

    assert!(run.halted, "{asm}");
    assert_eq!(run.result_code, 0, "{asm}");
}

#[test]
fn emits_and_runs_pointer_constants() {
    let source = r#"
            const TI_VRAM: ptr<u8> = 0x040180
            const TI_VRAM_RAW: u24 = cast<u24>(TI_VRAM)
            const AGON_VDP_BUFFER: ptr<u16> = 0x040190

            fn main() {
                *(TI_VRAM) = 0x42;
                test.assert_eq_u8(*TI_VRAM, 0x42, 1);

                let ti_next: ptr<u8> = TI_VRAM + 1;
                *(ti_next) = 0x43;
                test.assert_eq_u8(*(TI_VRAM + 1), 0x43, 2);
                test.assert_eq_u24(TI_VRAM_RAW, 0x040180, 3);

                let agon_next: ptr<u16> = AGON_VDP_BUFFER + 1;
                *(agon_next) = 0x1234;
                test.assert_eq_u16(*(AGON_VDP_BUFFER + 1), 0x1234, 4);
                test.assert_eq_u24(cast<u24>(agon_next), cast<u24>(AGON_VDP_BUFFER) + 2, 5);
                test.pass();
            }
        "#;
    let program = parse_program(Path::new("game.ezra"), source).unwrap();
    let asm = emit_ez80_assembly(&program).unwrap();
    let run = run_assembly_test(&asm, 8_000).unwrap();

    assert!(run.halted, "{asm}");
    assert_eq!(run.result_code, 0, "{asm}");
}

#[test]
fn emits_and_runs_null_pointer_constants() {
    let source = r#"
            const NULL_BYTE: ptr<u8> = 0
            const NULL_WORD: ptr<u16> = cast<ptr<u16>>(0u24)
            const NULL_RAW: ptr24 = cast<ptr24>(NULL_BYTE)

            fn is_null(p: ptr<u8>) -> bool {
                return p == NULL_BYTE
            }

            fn main() {
                let local_null: ptr<u8> = cast<ptr<u8>>(0u24)
                test.assert_eq_u24(cast<u24>(NULL_BYTE), 0, 1)
                test.assert_eq_u24(cast<u24>(NULL_WORD), 0, 2)
                test.assert_eq_u24(cast<u24>(local_null), 0, 3)
                test.assert_eq_u24(cast<u24>(cast<ptr<u8>>(NULL_RAW)), 0, 4)
                test.assert_eq_u8(is_null(local_null), true, 5)
                test.assert_eq_u8(local_null != cast<ptr<u8>>(0u24), false, 6)
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
fn emits_and_runs_same_type_pointer_comparisons() {
    let source = r#"
            global left: u8 = 1
            global right: u8 = 2
            global words: [u16; 2] = [0x0102, 0x0304]

            fn same_byte(a: ptr<u8>, b: ptr<u8>) -> bool {
                return a == b
            }

            fn different_word(a: ptr<u16>, b: ptr<u16>) -> bool {
                return a != b
            }

            fn main() {
                let left_ptr: ptr<u8> = &left
                let also_left: ptr<u8> = &left
                let right_ptr: ptr<u8> = &right
                let first_word: ptr<u16> = &words[0]
                let second_word: ptr<u16> = &words[1]

                test.assert_eq_u8(same_byte(left_ptr, also_left), true, 1)
                test.assert_eq_u8(same_byte(left_ptr, right_ptr), false, 2)
                test.assert_eq_u8(different_word(first_word, second_word), true, 3)
                test.assert_eq_u8(different_word(first_word, first_word), false, 4)
                test.pass()
            }
        "#;
    let program = parse_program(Path::new("game.ezra"), source).unwrap();
    let asm = emit_ez80_assembly(&program).unwrap();
    let run = run_assembly_test(&asm, 8_000).unwrap();

    assert!(run.halted, "{asm}");
    assert_eq!(run.result_code, 0, "{asm}");
}

#[test]
fn emits_and_runs_pointer_u24_cast_round_trip() {
    let source = r#"
            global byte: u8 = 0

            fn main() {
                let p: ptr<u8> = &byte
                let raw: u24 = cast<u24>(p)
                let q: ptr<u8> = cast<ptr<u8>>(raw)
                mem.poke8(q, 0x6D)
                test.assert_eq_u8(byte, 0x6D, 1)
                test.assert_eq_u24(raw, cast<u24>(&byte), 2)
                test.pass()
            }
        "#;
    let program = parse_program(Path::new("game.ezra"), source).unwrap();
    let asm = emit_ez80_assembly(&program).unwrap();
    let run = run_assembly_test(&asm, 8_000).unwrap();

    assert!(run.halted, "{asm}");
    assert_eq!(run.result_code, 0, "{asm}");
}

#[test]
fn emits_and_runs_runtime_multiplication() {
    let expected_u8 = 17u8.wrapping_mul(15);
    let expected_u16 = 0x0123u16.wrapping_mul(0x0021);
    let expected_u16_wrap = 0xFFFFu16.wrapping_mul(0xFFFF);
    let expected_u24 = (0x000123u32 * 0x000045) & 0x00FF_FFFF;
    let expected_wrap = (0x00FF00u32 * 0x000101) & 0x00FF_FFFF;
    let source = format!(
        r#"
            struct Accum {{
                wide: u16
                long: u24
            }}

            fn mul8(a: u8, b: u8) -> u8 {{
                return a * b
            }}

            fn mul16(a: u16, b: u16) -> u16 {{
                return a * b
            }}

            fn mul24(a: u24, b: u24) -> u24 {{
                return a * b
            }}

            fn main() {{
                let a: u8 = mul8(17, 15)
                test.assert_eq_u8(a, 0x{expected_u8:02X}, 1)

                let b: u16 = mul16(0x0123, 0x0021)
                test.assert_eq_u16(b, 0x{expected_u16:04X}, 2)

                let b_wrap: u16 = mul16(0xFFFF, 0xFFFF)
                test.assert_eq_u16(b_wrap, 0x{expected_u16_wrap:04X}, 7)

                let c: u24 = mul24(0x000123, 0x000045)
                test.assert_eq_u24(c, 0x{expected_u24:06X}, 3)

                let d: u24 = mul24(0x00FF00, 0x000101)
                test.assert_eq_u24(d, 0x{expected_wrap:06X}, 4)

                let accum: Accum = Accum {{ wide: 3, long: 5 }}
                accum.wide = accum.wide * 7
                accum.long = accum.long * 9
                test.assert_eq_u16(accum.wide, 21, 5)
                test.assert_eq_u24(accum.long, 45, 6)
                test.pass()
            }}
            "#
    );
    let program = parse_program(Path::new("game.ezra"), &source).unwrap();
    let asm = emit_ez80_assembly(&program).unwrap();
    let run = run_assembly_test(&asm, 120_000).unwrap();

    assert!(asm.contains("    call __ezra_mul_u8"), "{asm}");
    assert!(
        asm.contains("__ezra_mul_u8:\n    ld b, a\n    mlt bc\n    ld a, c\n    ret"),
        "{asm}"
    );
    assert!(asm.contains("    call __ezra_mul_u16"), "{asm}");
    assert!(
        asm.contains("__ezra_mul_u16:\n    ld d, h\n    ld e, l\n    ld h, c\n    mlt hl"),
        "{asm}"
    );
    assert!(asm.contains("    call __ezra_mul_u24"), "{asm}");
    assert!(asm.contains("__ezra_mul_u24:"), "{asm}");
    assert!(run.halted, "{asm}");
    assert_eq!(run.result_code, 0, "{asm}");
}

#[test]
fn emits_and_runs_wide_runtime_division_and_modulo() {
    let expected_u16_div = 1000u16 / 17;
    let expected_u16_mod = 1000u16 % 17;
    let expected_u24_div = 0x000123u32 / 5;
    let expected_u24_mod = 0x000123u32 % 5;
    let source = format!(
        r#"
            fn div16(a: u16, b: u16) -> u16 {{
                return a / b
            }}

            fn mod16(a: u16, b: u16) -> u16 {{
                return a % b
            }}

            fn div24(a: u24, b: u24) -> u24 {{
                return a / b
            }}

            fn mod24(a: u24, b: u24) -> u24 {{
                return a % b
            }}

            fn main() {{
                test.assert_eq_u16(div16(1000, 17), {expected_u16_div}, 1)
                test.assert_eq_u16(mod16(1000, 17), {expected_u16_mod}, 2)
                test.assert_eq_u16(div16(1000, 0), 0, 3)
                test.assert_eq_u16(mod16(1000, 0), 0, 4)

                test.assert_eq_u24(div24(0x000123, 5), 0x{expected_u24_div:06X}, 5)
                test.assert_eq_u24(mod24(0x000123, 5), 0x{expected_u24_mod:06X}, 6)
                test.assert_eq_u24(div24(0x000123, 0), 0, 7)
                test.assert_eq_u24(mod24(0x000123, 0), 0, 8)
                test.pass()
            }}
            "#
    );
    let program = parse_program(Path::new("game.ezra"), &source).unwrap();
    let asm = emit_ez80_assembly(&program).unwrap();
    let run = run_assembly_test(&asm, 80_000).unwrap();

    assert!(asm.contains("    call __ezra_div_u16"), "{asm}");
    assert!(asm.contains("    call __ezra_mod_u16"), "{asm}");
    assert!(asm.contains("    call __ezra_div_u24"), "{asm}");
    assert!(asm.contains("    call __ezra_mod_u24"), "{asm}");
    assert!(asm.contains("__ezra_div_u24:"), "{asm}");
    assert!(asm.contains("__ezra_mod_u24:"), "{asm}");
    assert!(run.halted, "{asm}");
    assert_eq!(run.result_code, 0, "{asm}");
}

#[test]
fn constant_division_uses_truncating_semantics() {
    assert_eq!(trunc_div_or_zero(7, 3), 2);
    assert_eq!(trunc_mod_or_zero(7, 3), 1);
    assert_eq!(trunc_div_or_zero(-7, 3), -2);
    assert_eq!(trunc_mod_or_zero(-7, 3), -1);
    assert_eq!(trunc_div_or_zero(7, -3), -2);
    assert_eq!(trunc_mod_or_zero(7, -3), 1);
    assert_eq!(trunc_div_or_zero(-3, 2), -1);
    assert_eq!(trunc_div_or_zero(7, 0), 0);
    assert_eq!(trunc_mod_or_zero(7, 0), 0);
}
