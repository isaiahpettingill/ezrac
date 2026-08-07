use super::*;

#[test]
fn emits_and_runs_scalar_bit_intrinsics_at_each_width() {
    let source = r#"
            fn main() {
                let r8: u8 = ezra.bits.rotate_left(0x81u8, 1)
                test.assert_eq_u8(r8, 0x03, 1)
                test.assert_eq_u8(ezra.bits.rotate_right(r8, 2), 0xC0, 2)
                test.assert_eq_u16(ezra.bits.rotate_right(0x0001u16, 1), 0x8000, 16)
                test.assert_eq_u24(ezra.bits.rotate_right(0x000001u24, 1), 0x800000, 17)
                test.assert_eq_u8(ezra.bits.test(0x80u8, 7), true, 3)
                test.assert_eq_u8(ezra.bits.set(0x01u8, 7), 0x81, 4)
                test.assert_eq_u8(ezra.bits.clear(0xFFu8, 3), 0xF7, 5)
                test.assert_eq_u8(ezra.bits.toggle(0x55u8, 0), 0x54, 6)
                test.assert_eq_u8(ezra.bits.extract(0xF0u8, 4, 4), 0x0F, 7)
                test.assert_eq_u8(ezra.bits.insert(0x0Fu8, 0x02u8, 2, 3), 0x0B, 8)
                test.assert_eq_u16(ezra.bits.byte_swap(0x1234u16), 0x3412, 9)
                test.assert_eq_u24(ezra.bits.byte_swap(0x123456u24), 0x563412, 10)
                test.assert_eq_u8(ezra.bits.reverse(0x02u8), 0x40, 11)
                test.assert_eq_u16(ezra.bits.reverse(0x0001u16), 0x8000, 12)
                test.assert_eq_u8(ezra.bits.count_ones(0xB5u8), 5, 13)
                test.assert_eq_u8(ezra.bits.leading_zeros(0x0010u16), 11, 14)
                test.assert_eq_u8(ezra.bits.trailing_zeros(0x010000u24), 16, 15)
                test.pass()
            }
        "#;
    let program = parse_program(Path::new("intrinsics_bits.ezra"), source).unwrap();
    let asm = emit_ez80_assembly(&program).unwrap();
    let run = run_assembly_test(&asm, 100_000).unwrap();

    assert!(run.halted, "{asm}");
    assert_eq!(run.result_code, 0, "{asm}");
}

#[test]
fn emits_and_runs_scalar_integer_intrinsics() {
    let source = r#"
            fn main() {
                test.assert_eq_u16(ezra.int.widening_mul(200u8, 3u8), 600, 1)
                test.assert_eq_u16(ezra.int.mul_high(0x1234u16, 0x0010u16), 0x0001, 2)
                test.assert_eq_u8(ezra.int.saturating_add(250u8, 10u8), 0xFF, 3)
                test.assert_eq_u8(ezra.int.saturating_sub(2u8, 10u8), 0, 4)
                test.assert_eq_u8(ezra.int.saturating_add(120i8, 20i8), 0x7F, 5)
                test.assert_eq_u8(cast<u8>(ezra.int.saturating_sub((-120i8), 20i8)), 0x80, 6)
                test.pass()
            }
        "#;
    let program = parse_program(Path::new("intrinsics_int.ezra"), source).unwrap();
    let asm = emit_ez80_assembly(&program).unwrap();
    let run = run_assembly_test(&asm, 200_000).unwrap();

    assert!(run.halted, "{asm}");
    assert_eq!(run.result_code, 0, "{asm}");
}

#[test]
fn r800_integer_intrinsics_use_native_full_multiply_results() {
    let source = r#"
            fn main() {
                test.assert_eq_u16(ezra.int.widening_mul(0xFFu8, 0xFFu8), 0xFE01, 1)
                test.assert_eq_u8(ezra.int.mul_high(0xFFu8, 0xFFu8), 0xFE, 2)
                test.assert_eq_u16(ezra.int.mul_high(0xFFFFu16, 0xFFFFu16), 0xFFFE, 3)
                let low8: u8, high8: u8 = ezra.int.full_mul(0xF0u8, 0x10u8)
                test.assert_eq_u8(low8, 0, 4)
                test.assert_eq_u8(high8, 0x0F, 5)
                let low16: u16, high16: u16 = ezra.int.full_mul(0xFFFFu16, 0xFFFFu16)
                test.assert_eq_u16(low16, 1, 6)
                test.assert_eq_u16(high16, 0xFFFE, 7)
                test.assert_eq_u16(cast<u16>(ezra.int.widening_mul((-2i8), 3i8)), 0xFFFA, 8)
                test.pass()
            }
        "#;
    let program = parse_program(Path::new("r800-intrinsics.ezra"), source).unwrap();
    let asm = emit_ez80_assembly_with_options(
        &program,
        AssemblyOptions {
            cpu: CpuFamily::R800,
            default_sdk_symbols: true,
            ram_base: Address24::new(0x2000),
            stack_top: Address24::new(0xF000),
            ..AssemblyOptions::default()
        },
    )
    .unwrap();

    assert!(asm.matches("    mulub a, c").count() >= 4, "{asm}");
    assert!(asm.matches("    muluw hl, bc").count() >= 2, "{asm}");
    crate::vm::assemble_subset_with_symbols_at(AssemblerCpu::R800, &asm, 0x0100)
        .unwrap_or_else(|error| panic!("R800 intrinsic code did not assemble: {error}\n{asm}"));
    let run = crate::vm::run_assembly_test_with_cpu_options_at(
        CpuFamily::R800,
        &asm,
        &TestRunOptions {
            instruction_budget: 100_000,
            initial_ports: Vec::new(),
            initial_memory: Vec::new(),
            stack_top: 0xF000,
        },
        0x0100,
    )
    .unwrap();
    assert!(run.halted, "{run:?}\n{asm}");
    assert_eq!(run.result_code, 0, "{run:?}\n{asm}");
}

#[test]
fn emits_and_runs_two_result_intrinsics() {
    let source = r#"
            global bytes: [u8; 4] = [1, 2, 3, 4]
            fn forwarded_divmod(value: u8) -> u8, u8 {
                return ezra.int.divmod(value, 3u8)
            }

            fn main() {
                let q: u8, r: u8 = ezra.int.divmod(7u8, 3u8)
                let sum: u8, carry: bool = ezra.int.add_carry(0xFFu8, 1u8, false)
                let diff: u8, borrow: bool = ezra.int.sub_borrow(0u8, 1u8, false)
                let low: u8, high: u8 = ezra.int.full_mul(3u8, 4u8)
                let found: ptr<u8>, ok: bool = ezra.mem.find_byte(&bytes[0], 4u24, 2u8)
                test.assert_eq_u8(q, 2, 1)
                test.assert_eq_u8(r, 1, 2)
                test.assert_eq_u8(sum, 0, 3)
                test.assert_eq_u8(carry, true, 4)
                test.assert_eq_u8(diff, 0xFF, 5)
                test.assert_eq_u8(borrow, true, 6)
                test.assert_eq_u8(low, 12, 7)
                test.assert_eq_u8(high, 0, 8)
                test.assert_eq_u8(ok, true, 9)
                test.assert_eq_u24(cast<u24>(found), cast<u24>(&bytes[1]), 10)
                let forwarded_q: u8, forwarded_r: u8 = forwarded_divmod(8u8)
                test.assert_eq_u8(forwarded_q, 2, 11)
                test.assert_eq_u8(forwarded_r, 2, 12)
                test.pass()
            }
        "#;
    let program = parse_program(Path::new("intrinsics_two_result.ezra"), source).unwrap();
    let asm = emit_ez80_assembly(&program).unwrap();
    let run = run_assembly_test(&asm, 200_000).unwrap();
    assert!(run.halted, "{asm}");
    assert_eq!(run.result_code, 0, "{asm}");
}

#[test]
fn emits_and_runs_memory_intrinsics_with_endian_and_overlap_semantics() {
    let source = r#"
            global source: [u8; 8] = [1, 2, 3, 4, 5, 6, 7, 8]
            global target: [u8; 8] = [0, 0, 0, 0, 0, 0, 0, 0]

            fn main() {
                ezra.mem.copy_nonoverlapping(&target[0], &source[0], 4u24)
                test.assert_eq_u8(target[0], 1, 1)
                test.assert_eq_u8(target[3], 4, 2)
                ezra.mem.fill(&target[4], 0xAAu8, 4u24)
                test.assert_eq_u8(target[7], 0xAA, 3)

                ezra.mem.store_le16(&target[0], 0x1234u16)
                test.assert_eq_u16(ezra.mem.load_le16(&target[0]), 0x1234, 4)
                ezra.mem.store_be16(&target[2], 0x5678u16)
                test.assert_eq_u16(ezra.mem.load_be16(&target[2]), 0x5678, 5)
                ezra.mem.store_le24(&target[4], 0x123456u24)
                test.assert_eq_u24(ezra.mem.load_le24(&target[4]), 0x123456, 6)
                ezra.mem.store_be24(&target[1], 0x0ABCDEu24)
                test.assert_eq_u24(ezra.mem.load_be24(&target[1]), 0x0ABCDE, 7)

                test.assert_eq_u8(ezra.mem.compare(&target[0], &target[0], 8u24), 0, 8)
                test.assert_eq_u8(ezra.mem.compare(&source[0], &target[0], 1u24), 0xFF, 9)
                test.assert_eq_u8(ezra.mem.peek8(&target[0]), 0x34, 10)
                ezra.mem.poke8(&target[0], 0x99u8)
                test.assert_eq_u8(ezra.mem.peek8(&target[0]), 0x99, 11)

                ezra.mem.move(&target[1], &target[0], 5u24)
                test.assert_eq_u8(target[1], 0x99, 12)
                test.assert_eq_u8(target[5], 0x56, 13)

                test.pass()
            }
        "#;
    let program = parse_program(Path::new("intrinsics_memory.ezra"), source).unwrap();
    let asm = emit_ez80_assembly(&program).unwrap();
    let run = run_assembly_test(&asm, 200_000).unwrap();

    assert!(run.halted, "{asm}");
    assert_eq!(run.result_code, 0, "{asm}");
}

#[test]
fn intrinsic_aliases_constants_and_unsupported_errors_are_explicit() {
    let alias_source = r#"
            global source: [u8; 2] = [7, 8]
            global target: [u8; 2] = [0, 0]
            fn main() {
                mem.memcpy(&target[0], &source[0], 2)
                mem.memset(&target[1], 0xAA, 1)
                test.assert_eq_u8(target[1], 0xAA, 1)
                test.pass()
            }
        "#;
    let program = parse_program(Path::new("intrinsics_aliases.ezra"), alias_source).unwrap();
    let asm = emit_ez80_assembly(&program).unwrap();
    let run = run_assembly_test(&asm, 50_000).unwrap();
    assert!(run.halted, "{asm}");
    assert_eq!(run.result_code, 0, "{asm}");

    let cases = [
        (
            r#"
                fn run(index: u8) -> u8 {
                    return ezra.bits.set(0x01u8, index)
                }
                fn main() {
                    let value: u8 = run(1)
                    test.pass()
                }
            "#,
            "intrinsic `ezra.bits.set` argument 2 must be a compile-time constant bit index",
        ),
        (
            r#"
                fn main() {
                    let value: u8 = ezra.bits.set(0x01u8, 8)
                    test.pass()
                }
            "#,
            "intrinsic `ezra.bits.set` argument 2 has value 8; the bit index must be within the input width",
        ),
        (
            r#"
                fn main() {
                    let value: u16 = ezra.int.widening_mul(2u16, 3u16)
                    test.pass()
                }
            "#,
            "intrinsic `ezra.int.widening_mul` result type `Named(\"u32\")` is unsupported by the eZ80 emitter: type `u32` is not supported; use explicit u8/u16/u24 or i8/i16/i24",
        ),
    ];
    for (source, expected) in cases {
        let program = parse_program(Path::new("intrinsics_error.ezra"), source).unwrap();
        let error = emit_ez80_assembly(&program).unwrap_err();
        assert_eq!(error.message, expected);
    }
}

#[test]
fn nonvolatile_intrinsics_reject_volatile_mmio_but_peek_poke_preserve_access() {
    let source = r#"
            volatile mmio REG: ptr<u8> = 0x9000
            fn main() {
                let value: u8 = ezra.mem.load_le16(REG)
                test.pass()
            }
        "#;
    let program = parse_program(Path::new("intrinsics_volatile.ezra"), source).unwrap();
    let error = emit_ez80_assembly(&program).unwrap_err();
    assert_eq!(
        error.message,
        "intrinsic `ezra.mem.load_le16` cannot access volatile memory"
    );
}
