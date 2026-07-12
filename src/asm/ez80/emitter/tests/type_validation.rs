use super::*;

#[test]
fn rejects_forbidden_integer_widths() {
    let source = r#"
            global score: u32 = 0
            fn main() { test.pass() }
        "#;
    let program = parse_program(Path::new("game.ezra"), source).unwrap();
    let error = emit_ez80_assembly(&program).unwrap_err();

    assert_eq!(
        error.message,
        "type `u32` is not supported; use explicit u8/u16/u24 or i8/i16/i24"
    );
}

#[test]
fn rejects_unknown_types() {
    let cases = [
        r#"
            global value: Missing = 0
            fn main() { test.pass() }
            "#,
        r#"
            fn takes_missing(value: Missing) {}
            fn main() { test.pass() }
            "#,
        r#"
            fn returns_missing() -> Missing {
                return 0
            }
            fn main() { test.pass() }
            "#,
        r#"
            alias MissingAlias = Missing
            fn main() { test.pass() }
            "#,
        r#"
            alias MissingPtr = ptr<Missing>
            fn main() { test.pass() }
            "#,
    ];

    for source in cases {
        let program = parse_program(Path::new("game.ezra"), source).unwrap();
        let error = emit_ez80_assembly(&program).unwrap_err();

        assert_eq!(error.message, "unknown type `Missing`");
    }
}

#[test]
fn rejects_constant_values_outside_declared_type_range() {
    let cases = [
        (
            r#"
                const NEG: u8 = -1
                fn main() { test.pass() }
                "#,
            "value -1 is outside u8 range",
        ),
        (
            r#"
                const WIDE: i8 = 128
                fn main() { test.pass() }
                "#,
            "value 128 is outside i8 range",
        ),
        (
            r#"
                const WIDE: i8 = 128i8
                fn main() { test.pass() }
                "#,
            "value 128 is outside i8 range",
        ),
        (
            r#"
                alias tiny = i8
                const WIDE: tiny = -129
                fn main() { test.pass() }
                "#,
            "value -129 is outside i8 range",
        ),
        (
            r#"
                const BAD: bool = 2
                fn main() { test.pass() }
                "#,
            "value 2 is outside bool range",
        ),
        (
            r#"
                global WIDE: u16 = cast<u16>(300u8)
                fn main() { test.pass() }
                "#,
            "value 300 is outside u8 range",
        ),
        (
            r#"
                fn takes_word(value: u16) {}
                fn main() {
                    takes_word(cast<u16>(300u8))
                    test.pass()
                }
                "#,
            "value 300 is outside u8 range",
        ),
        (
            r#"
                fn bad() -> u16 {
                    return cast<u16>(300u8)
                }
                fn main() { test.pass() }
                "#,
            "value 300 is outside u8 range",
        ),
    ];

    for (source, expected) in cases {
        let program = parse_program(Path::new("game.ezra"), source).unwrap();
        let error = emit_ez80_assembly(&program).unwrap_err();

        assert_eq!(error.message, expected);
    }
}

#[test]
fn rejects_embed_alignment_outside_address_space() {
    let source = r#"
            embed sprite: bytes = bytes [0xAA] align 0x100000000
            fn main() { test.pass() }
        "#;
    let program = parse_program(Path::new("game.ezra"), source).unwrap();
    let error = emit_ez80_assembly(&program).unwrap_err();

    assert_eq!(
        error.message,
        "embed `sprite` alignment 4294967296 exceeds 24-bit address space"
    );
}

#[test]
fn rejects_non_integer_embed_alignment() {
    let cases = [
        r#"
            embed sprite: bytes = bytes [0xAA] align true
            fn main() { test.pass() }
            "#,
        r#"
            const ALIGN: bool = true
            embed sprite: bytes = bytes [0xAA] align ALIGN
            fn main() { test.pass() }
            "#,
        r#"
            embed sprite: bytes = bytes [0xAA] align (1 == 1)
            fn main() { test.pass() }
            "#,
    ];

    for source in cases {
        let program = parse_program(Path::new("game.ezra"), source).unwrap();
        let error = emit_ez80_assembly(&program).unwrap_err();

        assert_eq!(
            error.message,
            "embed `sprite` alignment must be an integer constant"
        );
    }
}

#[test]
fn rejects_non_pointer_mmio_types() {
    let cases = [
        (
            r#"
                volatile mmio STATUS: u8 = 0x080000
                fn main() { test.pass() }
                "#,
            "mmio `STATUS` type `u8` must be a pointer type",
        ),
        (
            r#"
                alias byte = u8
                volatile mmio STATUS: byte = 0x080000
                fn main() { test.pass() }
                "#,
            "mmio `STATUS` type `byte` must be a pointer type",
        ),
    ];

    for (source, expected) in cases {
        let program = parse_program(Path::new("game.ezra"), source).unwrap();
        let error = emit_ez80_assembly(&program).unwrap_err();

        assert_eq!(error.message, expected);
    }
}

#[test]
fn rejects_non_integer_mmio_addresses() {
    let cases = [
        r#"
            volatile mmio STATUS: ptr<u8> = true
            fn main() { test.pass() }
            "#,
        r#"
            const STATUS_ADDR: bool = true
            volatile mmio STATUS: ptr<u8> = STATUS_ADDR
            fn main() { test.pass() }
            "#,
        r#"
            volatile mmio STATUS: ptr<u8> = 1 == 1
            fn main() { test.pass() }
            "#,
    ];

    for source in cases {
        let program = parse_program(Path::new("game.ezra"), source).unwrap();
        let error = emit_ez80_assembly(&program).unwrap_err();

        assert_eq!(
            error.message,
            "mmio `STATUS` address must be an integer constant"
        );
    }
}

#[test]
fn validates_port_declaration_types() {
    let ok = r#"
            alias byte = u8
            port DEBUG: byte = 0x0C

            fn main() {
                out DEBUG, 65
                test.pass()
            }
        "#;
    let program = parse_program(Path::new("game.ezra"), ok).unwrap();
    let asm = emit_ez80_assembly(&program).unwrap();
    let run = run_assembly_test(&asm, 4_000).unwrap();

    assert!(run.halted, "{asm}");
    assert_eq!(run.result_code, 0, "{asm}");

    let cases = [
        (
            r#"
                port WIDE: u16 = 0x01
                fn main() { test.pass() }
                "#,
            "port `WIDE` type `u16` must be u8",
        ),
        (
            r#"
                alias word = u16
                port BAD: word = 0x01
                fn main() { test.pass() }
                "#,
            "port `BAD` type `word` must be u8",
        ),
        (
            r#"
                port FLAG: u8 = true
                fn main() { test.pass() }
                "#,
            "port `FLAG` value must be an integer constant",
        ),
        (
            r#"
                const FLAG_VALUE: bool = true
                port FLAG: u8 = FLAG_VALUE
                fn main() { test.pass() }
                "#,
            "port `FLAG` value must be an integer constant",
        ),
        (
            r#"
                port FLAG: u8 = 1 == 1
                fn main() { test.pass() }
                "#,
            "port `FLAG` value must be an integer constant",
        ),
    ];

    for (source, expected) in cases {
        let program = parse_program(Path::new("game.ezra"), source).unwrap();
        let error = emit_ez80_assembly(&program).unwrap_err();

        assert_eq!(error.message, expected);
    }
}

#[test]
fn rejects_signed_unsigned_arithmetic_mix_without_cast() {
    let cases = [
        r#"
            fn main() {
                let signed: i8 = 1
                let unsigned: u8 = 2
                let mixed: i8 = signed + unsigned
                test.pass()
            }
            "#,
        r#"
            const SIGNED: i16 = -1
            const UNSIGNED: u16 = 2
            fn main() {
                let mixed: i16 = SIGNED + UNSIGNED
                test.pass()
            }
            "#,
        r#"
            fn main() {
                let mixed: i8 = 1i8 + 2u8
                test.pass()
            }
            "#,
    ];

    for source in cases {
        let program = parse_program(Path::new("game.ezra"), source).unwrap();
        let error = emit_ez80_assembly(&program).unwrap_err();

        assert_eq!(error.message, "signed/unsigned mix without cast");
    }
}

#[test]
fn emits_and_runs_arithmetic_with_fitting_untyped_literals() {
    let source = r#"
            const BASE: u16 = 0x0100
            const SUM: u16 = BASE + 2

            fn main() {
                let word: u16 = 0x0100
                let plus: u16 = word + 2
                let minus: u16 = 0x0105 - word
                let signed: i16 = -3
                let bumped: i16 = signed + 2
                let zero: i16 = bumped + 1
                test.assert_eq_u24(cast<u24>(SUM), 0x000102, 1)
                test.assert_eq_u24(cast<u24>(plus), 0x000102, 2)
                test.assert_eq_u24(cast<u24>(minus), 0x000005, 3)
                test.assert_eq_u24(cast<u24>(zero), 0, 4)
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
fn rejects_arithmetic_width_mismatch_without_cast() {
    let cases = [
        (
            r#"
                fn main() {
                    let byte: u8 = 1
                    let word: u16 = 2
                    let mixed: u16 = byte + word
                    test.pass()
                }
                "#,
            "arithmetic operands must have same width without cast",
        ),
        (
            r#"
                const BYTE: u8 = 1
                const WORD: u16 = 2
                const MIXED: u16 = BYTE + WORD
                fn main() { test.pass() }
                "#,
            "arithmetic operands must have same width without cast",
        ),
        (
            r#"
                fn main() {
                    let byte: u8 = 1
                    let mixed: u16 = byte + 300
                    test.pass()
                }
                "#,
            "value 300 is outside u8 range",
        ),
        (
            r#"
                const BYTE: u8 = 1
                const MIXED: u16 = BYTE + 300
                fn main() { test.pass() }
                "#,
            "value 300 is outside u8 range",
        ),
    ];

    for (source, expected) in cases {
        let program = parse_program(Path::new("game.ezra"), source).unwrap();
        let error = emit_ez80_assembly(&program).unwrap_err();

        assert_eq!(error.message, expected);
    }
}

#[test]
fn rejects_invalid_comparison_operand_types() {
    let cases = [
        (
            r#"
                fn main() {
                    let signed: i8 = 1
                    let unsigned: u8 = 1
                    let same: bool = signed == unsigned
                    test.pass()
                }
                "#,
            "signed/unsigned mix without cast",
        ),
        (
            r#"
                fn main() {
                    let byte: u8 = 1
                    let word: u16 = 1
                    let same: bool = byte == word
                    test.pass()
                }
                "#,
            "comparison operands must have same width without cast",
        ),
        (
            r#"
                fn main() {
                    let left: bool = false
                    let right: bool = true
                    let ordered: bool = left < right
                    test.pass()
                }
                "#,
            "type mismatch",
        ),
        (
            r#"
                global byte: u8 = 0
                global word: u16 = 0
                fn main() {
                    let bp: ptr<u8> = &byte
                    let wp: ptr<u16> = &word
                    let same: bool = bp == wp
                    test.pass()
                }
                "#,
            "type mismatch",
        ),
        (
            r#"
                global left: u8 = 0
                global right: u8 = 0
                fn main() {
                    let lp: ptr<u8> = &left
                    let rp: ptr<u8> = &right
                    let ordered: bool = lp < rp
                    test.pass()
                }
                "#,
            "pointer comparisons support only == and !=",
        ),
        (
            r#"
                struct Pair {
                    left: u8
                    right: u8
                }

                global left: Pair = Pair { left: 1, right: 2 }
                global right: Pair = Pair { left: 1, right: 2 }

                fn main() {
                    let same: bool = left == right
                    test.pass()
                }
                "#,
            "struct `Pair` cannot be used as a scalar value",
        ),
        (
            r#"
                struct Pair {
                    left: u8
                    right: u8
                }
                alias AliasPair = Pair

                global left: AliasPair = Pair { left: 1, right: 2 }
                global right: AliasPair = Pair { left: 1, right: 2 }

                fn main() {
                    let same: bool = left == right
                    test.pass()
                }
                "#,
            "struct `Pair` cannot be used as a scalar value",
        ),
        (
            r#"
                const BYTE: u8 = 1
                const WORD: u16 = 1
                const SAME: bool = BYTE == WORD
                fn main() { test.pass() }
                "#,
            "comparison operands must have same width without cast",
        ),
        (
            r#"
                fn main() {
                    let byte: u8 = 1
                    let same: bool = byte == 300
                    test.pass()
                }
                "#,
            "value 300 is outside u8 range",
        ),
        (
            r#"
                const BYTE: u8 = 1
                const SAME: bool = BYTE == 300
                fn main() { test.pass() }
                "#,
            "value 300 is outside u8 range",
        ),
    ];

    for (source, expected) in cases {
        let program = parse_program(Path::new("game.ezra"), source).unwrap();
        let error = emit_ez80_assembly(&program).unwrap_err();

        assert_eq!(error.message, expected);
    }
}

#[test]
fn rejects_invalid_const_expression_operand_types() {
    let cases = [
        (
            r#"
                const SIGNED: i16 = -1
                const UNSIGNED: u16 = 2
                const MIXED: i16 = SIGNED + UNSIGNED
                fn main() { test.pass() }
                "#,
            "signed/unsigned mix without cast",
        ),
        (
            r#"
                const FLAG: bool = true
                const VALUE: u8 = 1
                const BAD: u8 = FLAG + VALUE
                fn main() { test.pass() }
                "#,
            "type mismatch",
        ),
    ];

    for (source, expected) in cases {
        let program = parse_program(Path::new("game.ezra"), source).unwrap();
        let error = emit_ez80_assembly(&program).unwrap_err();

        assert_eq!(error.message, expected);
    }
}

#[test]
fn rejects_invalid_array_index_types() {
    let cases = [
        (
            r#"
                global bytes: [u8; 2] = [1, 2]
                fn main() {
                    let flag: bool = true
                    let value: u8 = bytes[flag]
                    test.pass()
                }
                "#,
            "array index type `bool` is not supported; use u8, u16, or u24",
        ),
        (
            r#"
                global bytes: [u8; 2] = [1, 2]
                fn main() {
                    let idx: i8 = 1
                    bytes[idx] = 7
                    test.pass()
                }
                "#,
            "array index type `i8` is not supported; use u8, u16, or u24",
        ),
        (
            r#"
                global bytes: [u8; 2] = [1, 2]
                fn main() {
                    let ptr: ptr<u8> = &bytes[0]
                    let p: ptr<u8> = &bytes[ptr]
                    test.pass()
                }
                "#,
            "array index type `ptr<u8>` is not supported; use u8, u16, or u24",
        ),
        (
            r#"
                global bytes: [u8; 2] = [1, 2]
                fn main() {
                    let value: u8 = bytes[-1]
                    test.pass()
                }
                "#,
            "array index value -1 is outside u24 range",
        ),
        (
            r#"
                global bytes: [u8; 2] = [1, 2]
                fn main() {
                    let value: u8 = bytes[2]
                    test.pass()
                }
                "#,
            "array index 2 is out of bounds for `bytes` length 2",
        ),
        (
            r#"
                global bytes: [u8; 2] = [1, 2]
                fn main() {
                    bytes[2] = 7
                    test.pass()
                }
                "#,
            "array index 2 is out of bounds for `bytes` length 2",
        ),
        (
            r#"
                global bytes: [u8; 2] = [1, 2]
                fn main() {
                    let p: ptr<u8> = &bytes[2]
                    test.pass()
                }
                "#,
            "array index 2 is out of bounds for `bytes` length 2",
        ),
        (
            r#"
                const IDX: u8 = 2
                global bytes: [u8; 2] = [1, 2]
                fn main() {
                    let value: u8 = bytes[IDX]
                    test.pass()
                }
                "#,
            "array index 2 is out of bounds for `bytes` length 2",
        ),
        (
            r#"
                global grid: [[u8; 2]; 2] = [[1, 2], [3, 4]]
                fn main() {
                    let row: u8 = 0
                    let value: u8 = grid[row][2]
                    test.pass()
                }
                "#,
            "array index 2 is out of bounds for `grid[row][2]` length 2",
        ),
    ];

    for (source, expected) in cases {
        let program = parse_program(Path::new("game.ezra"), source).unwrap();
        let error = emit_ez80_assembly(&program).unwrap_err();

        assert_eq!(error.message, expected);
    }
}

#[test]
fn rejects_array_lengths_outside_address_space() {
    let cases = [
        (
            r#"
                global bytes: [u8; 0x1000000] = []
                fn main() { test.pass() }
                "#,
            "array length 16777216 exceeds 24-bit address space",
        ),
        (
            r#"
                const LEN: u24 = 0xFFFFFF
                global bytes: [u8; LEN + 1] = []
                fn main() { test.pass() }
                "#,
            "array length 16777216 exceeds 24-bit address space",
        ),
        (
            r#"
                fn main() {
                    let bytes: [u8; 0x1000000] = []
                    test.pass()
                }
                "#,
            "array length 16777216 exceeds 24-bit address space",
        ),
    ];

    for (source, expected) in cases {
        let program = parse_program(Path::new("game.ezra"), source).unwrap();
        let error = emit_ez80_assembly(&program).unwrap_err();

        assert_eq!(error.message, expected);
    }
}

#[test]
fn rejects_assignment_width_changes_without_cast() {
    let cases = [
        (
            r#"
                fn main() {
                    let small: u8 = 1
                    let wide: u16 = small
                    test.pass()
                }
                "#,
            "widening without cast",
        ),
        (
            r#"
                fn main() {
                    let wide: u16 = 0x1234
                    let small: u8 = wide
                    test.pass()
                }
                "#,
            "narrowing without cast",
        ),
        (
            r#"
                fn value() -> u8 {
                    let wide: u16 = 1
                    return wide
                }
                fn main() { test.pass() }
                "#,
            "narrowing without cast",
        ),
        (
            r#"
                fn main() {
                    let wide: u16 = 1
                    let small: u8 = 0
                    small = wide
                    test.pass()
                }
                "#,
            "narrowing without cast",
        ),
    ];

    for (source, expected) in cases {
        let program = parse_program(Path::new("game.ezra"), source).unwrap();
        let error = emit_ez80_assembly(&program).unwrap_err();

        assert_eq!(error.message, expected);
    }
}

#[test]
fn rejects_indirect_assignment_type_changes_without_cast() {
    let cases = [
        (
            r#"
                global bytes: [u8; 2] = [0, 0]
                fn main() {
                    let wide: u16 = 0x1234
                    bytes[0] = wide
                    test.pass()
                }
                "#,
            "narrowing without cast",
        ),
        (
            r#"
                global words: [u16; 2] = [0, 0]
                fn main() {
                    let small: u8 = 1
                    let index: u8 = 1
                    words[index] = small
                    test.pass()
                }
                "#,
            "widening without cast",
        ),
        (
            r#"
                global signed: [i8; 1] = [0]
                fn main() {
                    let unsigned: u8 = 1
                    signed[0] = unsigned
                    test.pass()
                }
                "#,
            "signed/unsigned mix without cast",
        ),
        (
            r#"
                global byte: u8 = 0
                fn main() {
                    let p: ptr<u8> = &byte;
                    let wide: u16 = 1;
                    *p = wide;
                    test.pass()
                }
                "#,
            "narrowing without cast",
        ),
    ];

    for (source, expected) in cases {
        let program = parse_program(Path::new("game.ezra"), source).unwrap();
        let error = emit_ez80_assembly(&program).unwrap_err();

        assert_eq!(error.message, expected);
    }
}

#[test]
fn rejects_initializer_type_changes_without_cast() {
    let cases = [
        (
            r#"
                global words: [u16; 2] = [1, 2]
                fn main() {
                    let small: u8 = 1
                    let values: [u16; 2] = [small, 2]
                    test.pass()
                }
                "#,
            "widening without cast",
        ),
        (
            r#"
                global values: [i8; 1] = [0]
                fn main() {
                    let unsigned: u8 = 1
                    let local: [i8; 1] = [unsigned]
                    test.pass()
                }
                "#,
            "signed/unsigned mix without cast",
        ),
        (
            r#"
                struct Pair { value: u8 }
                fn main() {
                    let wide: u16 = 1
                    let pair: Pair = Pair { value: wide }
                    test.pass()
                }
                "#,
            "narrowing without cast",
        ),
        (
            r#"
                struct Pair { value: i8 }
                fn main() {
                    let unsigned: u8 = 1
                    let pair: Pair = Pair { value: unsigned }
                    test.pass()
                }
                "#,
            "signed/unsigned mix without cast",
        ),
    ];

    for (source, expected) in cases {
        let program = parse_program(Path::new("game.ezra"), source).unwrap();
        let error = emit_ez80_assembly(&program).unwrap_err();

        assert_eq!(error.message, expected);
    }
}

#[test]
fn rejects_call_argument_type_changes_without_cast() {
    let cases = [
        (
            r#"
                fn takes_wide(value: u16) {}
                fn main() {
                    let small: u8 = 1
                    takes_wide(small)
                    test.pass()
                }
                "#,
            "widening without cast",
        ),
        (
            r#"
                fn takes_small(value: u8) {}
                fn main() {
                    let wide: u16 = 0x1234
                    takes_small(wide)
                    test.pass()
                }
                "#,
            "narrowing without cast",
        ),
        (
            r#"
                fn takes_unsigned(value: u8) {}
                fn main() {
                    let signed: i8 = 1
                    takes_unsigned(signed)
                    test.pass()
                }
                "#,
            "signed/unsigned mix without cast",
        ),
    ];

    for (source, expected) in cases {
        let program = parse_program(Path::new("game.ezra"), source).unwrap();
        let error = emit_ez80_assembly(&program).unwrap_err();

        assert_eq!(error.message, expected);
    }
}

#[test]
fn rejects_bool_integer_mismatch() {
    let cases = [
        r#"
            fn main() {
                let value: u8 = true
                test.pass()
            }
            "#,
        r#"
            fn main() {
                let flag: bool = true
                let value: u8 = 1
                let mixed: u8 = flag + value
                test.pass()
            }
            "#,
        r#"
            fn takes_array(values: ptr<[u8; 2]>) {}
            fn main() {
                let values: [u8; 2] = [1, 2]
                takes_array(values)
                test.pass()
            }
            "#,
    ];

    for source in cases {
        let program = parse_program(Path::new("game.ezra"), source).unwrap();
        let error = emit_ez80_assembly(&program).unwrap_err();

        assert_eq!(error.message, "type mismatch");
    }
}

#[test]
fn rejects_non_integer_unary_operands() {
    let cases = [
        (
            r#"
                fn main() {
                    let value: bool = ~true
                    test.pass()
                }
                "#,
            "unary operand must be an integer",
        ),
        (
            r#"
                fn main() {
                    let value: bool = -false
                    test.pass()
                }
                "#,
            "unary operand must be an integer",
        ),
        (
            r#"
                const BAD: bool = ~true
                fn main() { test.pass() }
                "#,
            "unary operand must be an integer",
        ),
        (
            r#"
                global byte: u8 = 1
                fn main() {
                    let ptr: ptr<u8> = &byte
                    let value: u24 = ~ptr
                    test.pass()
                }
                "#,
            "unary operand must be an integer",
        ),
        (
            r#"
                fn main() {
                    let raw: ptr24 = cast<ptr24>(0x040000)
                    let value: ptr24 = -raw
                    test.pass()
                }
                "#,
            "unary operand must be an integer",
        ),
    ];

    for (source, expected) in cases {
        let program = parse_program(Path::new("game.ezra"), source).unwrap();
        let error = emit_ez80_assembly(&program).unwrap_err();

        assert_eq!(error.message, expected);
    }
}
