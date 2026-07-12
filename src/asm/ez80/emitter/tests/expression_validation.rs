use super::*;

#[test]
fn rejects_unknown_struct_fields() {
    let cases = [
        (
            r#"
                struct Entity { x: u8 }
                global player: Entity = Entity { x: 1 }
                fn main() {
                    let y: u8 = player.y
                    test.pass()
                }
                "#,
            "struct `Entity` has no field `y`",
        ),
        (
            r#"
                struct Entity { x: u8 }
                global player: Entity = Entity { x: 1 }
                fn main() {
                    player.y = 2
                    test.pass()
                }
                "#,
            "struct `Entity` has no field `y`",
        ),
        (
            r#"
                struct Entity { x: u8 }
                global player: Entity = Entity { x: 1 }
                fn main() {
                    let p: ptr<u8> = &player.y
                    test.pass()
                }
                "#,
            "struct `Entity` has no field `y`",
        ),
        (
            r#"
                struct Entity { x: u8 }
                global player: Entity = Entity { x: 1, y: 2 }
                fn main() { test.pass() }
                "#,
            "struct `Entity` has no field `y`",
        ),
        (
            r#"
                struct Inner { x: u8 }
                struct Outer { inner: Inner }
                global outer: Outer = Outer { inner: Inner { x: 1 } }
                fn main() {
                    let value: u8 = outer.inner.y
                    test.pass()
                }
                "#,
            "struct `Inner` has no field `y`",
        ),
    ];

    for (source, expected) in cases {
        let program = parse_program(Path::new("game.ezra"), source).unwrap();
        let error = emit_ez80_assembly(&program).unwrap_err();

        assert_eq!(error.message, expected);
    }
}

#[test]
fn rejects_non_bool_logical_operands_and_conditions() {
    let cases = [
        (
            r#"
                fn main() {
                    let flag: bool = 1 && true
                    test.pass()
                }
                "#,
            "logical operand must be bool",
        ),
        (
            r#"
                const FLAG: bool = 1 || false
                fn main() { test.pass() }
                "#,
            "logical operand must be bool",
        ),
        (
            r#"
                fn main() {
                    let flag: bool = !1
                    test.pass()
                }
                "#,
            "logical operand must be bool",
        ),
        (
            r#"
                fn main() {
                    if 1 {
                        test.pass()
                    }
                }
                "#,
            "if condition must be bool",
        ),
        (
            r#"
                fn main() {
                    while 1 {
                        break
                    }
                    test.pass()
                }
                "#,
            "while condition must be bool",
        ),
    ];

    for (source, expected) in cases {
        let program = parse_program(Path::new("game.ezra"), source).unwrap();
        let error = emit_ez80_assembly(&program).unwrap_err();

        assert_eq!(error.message, expected);
    }
}

#[test]
fn rejects_non_integer_shift_operands_and_counts() {
    let cases = [
        (
            r#"
                fn main() {
                    let value: u8 = true << 1
                    test.pass()
                }
                "#,
            "shift operand must be an integer",
        ),
        (
            r#"
                global byte: u8 = 1
                fn main() {
                    let ptr: ptr<u8> = &byte
                    let value: u24 = ptr >> 1
                    test.pass()
                }
                "#,
            "shift operand must be an integer",
        ),
        (
            r#"
                fn main() {
                    let value: u8 = 1 << false
                    test.pass()
                }
                "#,
            "shift count must be an integer",
        ),
        (
            r#"
                global byte: u8 = 1
                fn main() {
                    let ptr: ptr<u8> = &byte
                    let value: u8 = 1 << ptr
                    test.pass()
                }
                "#,
            "shift count must be an integer",
        ),
        (
            r#"
                fn shift(value: u8, count: i8) -> u8 {
                    return value << count
                }
                fn main() {
                    let value: u8 = shift(1, 1)
                    test.pass()
                }
                "#,
            "runtime shift count must be u8",
        ),
        (
            r#"
                fn shift(value: u16, count: u16) -> u16 {
                    return value >> count
                }
                fn main() {
                    let value: u16 = shift(0x1234, 1)
                    test.pass()
                }
                "#,
            "runtime shift count must be u8",
        ),
        (
            r#"
                const BAD: u8 = true << 1
                fn main() { test.pass() }
                "#,
            "shift operand must be an integer",
        ),
        (
            r#"
                const BAD: u8 = 1 << false
                fn main() { test.pass() }
                "#,
            "shift count must be an integer",
        ),
        (
            r#"
                const BAD: u8 = 1 << -1i8
                fn main() { test.pass() }
                "#,
            "shift count -1 is outside supported range 0..=255",
        ),
        (
            r#"
                fn main() {
                    let value: u8 = 1
                    value <<= false
                    test.pass()
                }
                "#,
            "shift count must be an integer",
        ),
        (
            r#"
                fn shift(value: u24, count: u16) -> u24 {
                    value <<= count
                    return value
                }
                fn main() {
                    let value: u24 = shift(1, 1)
                    test.pass()
                }
                "#,
            "runtime shift count must be u8",
        ),
        (
            r#"
                global byte: u8 = 1
                fn main() {
                    let value: u16 = 1
                    let ptr: ptr<u8> = &byte
                    value >>= ptr
                    test.pass()
                }
                "#,
            "shift count must be an integer",
        ),
    ];

    for (source, expected) in cases {
        let program = parse_program(Path::new("game.ezra"), source).unwrap();
        let error = emit_ez80_assembly(&program).unwrap_err();

        assert_eq!(error.message, expected);
    }
}

#[test]
fn rejects_break_and_continue_outside_loops() {
    let cases = [
        (
            r#"
                fn main() {
                    break
                }
                "#,
            "`break` outside loop",
        ),
        (
            r#"
                fn main() {
                    continue
                }
                "#,
            "`continue` outside loop",
        ),
    ];

    for (source, expected) in cases {
        let program = parse_program(Path::new("game.ezra"), source).unwrap();
        let error = emit_ez80_assembly(&program).unwrap_err();

        assert_eq!(error.message, expected);
    }
}

#[test]
fn rejects_invalid_out_value_types() {
    let cases = [
        (
            r#"
                port DEBUG: u8 = 0x0C
                fn main() {
                    let wide: u16 = 0x1234
                    out DEBUG, wide
                    test.pass()
                }
                "#,
            "narrowing without cast",
        ),
        (
            r#"
                port DEBUG: u8 = 0x0C
                fn main() {
                    let signed: i8 = 1
                    out DEBUG, signed
                    test.pass()
                }
                "#,
            "signed/unsigned mix without cast",
        ),
        (
            r#"
                port DEBUG: u8 = 0x0C
                fn main() {
                    out DEBUG, true
                    test.pass()
                }
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
fn rejects_invalid_mem_builtin_argument_types() {
    let cases = [
        (
            r#"
                fn main() {
                    let value: u8 = mem.peek8(0x040000)
                    test.pass()
                }
                "#,
            "type mismatch",
        ),
        (
            r#"
                fn main() {
                    mem.poke8(cast<ptr<u8>>(0x040000), 0x0100)
                    test.pass()
                }
                "#,
            "value 256 is outside u8 range",
        ),
        (
            r#"
                global src: [u8; 1] = [1]
                global dst: [u8; 1] = [0]
                fn main() {
                    mem.memcpy(&dst[0], &src[0], true)
                    test.pass()
                }
                "#,
            "type mismatch",
        ),
        (
            r#"
                global dst: [u8; 1] = [0]
                fn main() {
                    mem.memset(0x040000, 0, 1)
                    test.pass()
                }
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
fn rejects_invalid_debug_builtin_argument_types() {
    let cases = [
        (
            r#"
                fn main() {
                    let wide: u16 = 0x1234
                    debug.char(wide)
                    test.pass()
                }
                "#,
            "narrowing without cast",
        ),
        (
            r#"
                fn main() {
                    debug.str(0x040000)
                    test.pass()
                }
                "#,
            "type mismatch",
        ),
        (
            r#"
                fn main() {
                    let byte: u8 = 0x12
                    debug.hex_u16(byte)
                    test.pass()
                }
                "#,
            "widening without cast",
        ),
        (
            r#"
                fn main() {
                    let signed: i8 = -1
                    debug.hex_u8(signed)
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
fn rejects_invalid_test_builtin_argument_types() {
    let cases = [
        (
            r#"
                fn main() {
                    test.fail(0x0100)
                }
                "#,
            "value 256 is outside u8 range",
        ),
        (
            r#"
                fn main() {
                    let wide: u16 = 0x0012
                    test.assert_eq_u8(wide, 0x12, 1)
                    test.pass()
                }
                "#,
            "narrowing without cast",
        ),
        (
            r#"
                fn main() {
                    let byte: u8 = 0x12
                    test.assert_eq_u16(byte, 0x0012, 1)
                    test.pass()
                }
                "#,
            "widening without cast",
        ),
        (
            r#"
                fn main() {
                    let pointer: ptr<u8> = cast<ptr<u8>>(0x040000)
                    test.assert_eq_u24(pointer, 0x040000, 1)
                    test.pass()
                }
                "#,
            "type mismatch",
        ),
        (
            r#"
                fn main() {
                    test.assert_eq_u8(true, true, 0x0100)
                    test.pass()
                }
                "#,
            "value 256 is outside u8 range",
        ),
    ];

    for (source, expected) in cases {
        let program = parse_program(Path::new("game.ezra"), source).unwrap();
        let error = emit_ez80_assembly(&program).unwrap_err();

        assert_eq!(error.message, expected);
    }
}

#[test]
fn rejects_invalid_pointer_casts() {
    let cases = [
        (
            r#"
                fn main() {
                    let raw: u16 = 0x1234
                    let p: ptr<u8> = cast<ptr<u8>>(raw)
                    test.pass()
                }
                "#,
            "integer-to-pointer casts require u24 or ptr24",
        ),
        (
            r#"
                global byte: u8 = 0
                fn main() {
                    let p: ptr<u8> = &byte
                    let raw: u16 = cast<u16>(p)
                    test.pass()
                }
                "#,
            "pointer-to-integer casts produce u24 or ptr24",
        ),
    ];

    for (source, expected) in cases {
        let program = parse_program(Path::new("game.ezra"), source).unwrap();
        let error = emit_ez80_assembly(&program).unwrap_err();

        assert_eq!(error.message, expected);
    }
}

#[test]
fn emits_and_runs_ptr24_pointer_casts() {
    let source = r#"
            volatile mmio SCRATCH: ptr24 = 0x040180

            fn read_raw(raw: ptr24) -> u8 {
                let p: ptr<u8> = cast<ptr<u8>>(raw)
                return *p
            }

            fn main() {
                let p: ptr<u8> = cast<ptr<u8>>(SCRATCH);
                *(p) = 0x5A;
                let raw: ptr24 = cast<ptr24>(p);
                test.assert_eq_u8(read_raw(raw), 0x5A, 1);
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
fn rejects_invalid_constant_pointer_casts() {
    let cases = [
        (
            r#"
                const VRAM_BASE: ptr<u8> = 0x040180
                const RAW: u16 = cast<u16>(VRAM_BASE)

                fn main() {
                    test.pass()
                }
                "#,
            "pointer-to-integer casts produce u24 or ptr24",
        ),
        (
            r#"
                const VRAM_BASE: ptr<u8> = cast<ptr<u8>>(0x1234)

                fn main() {
                    test.pass()
                }
                "#,
            "integer-to-pointer casts require u24 or ptr24",
        ),
    ];

    for (source, expected) in cases {
        let program = parse_program(Path::new("game.ezra"), source).unwrap();
        let error = emit_ez80_assembly(&program).unwrap_err();

        assert_eq!(error.message, expected);
    }
}

#[test]
fn emits_and_runs_constant_ptr24_pointer_casts() {
    let source = r#"
            const RAW: ptr24 = cast<ptr24>(cast<ptr<u8>>(0x040190))
            const BYTE_PTR: ptr<u8> = cast<ptr<u8>>(RAW)

            fn main() {
                *BYTE_PTR = 0x6B;
                test.assert_eq_u8(*BYTE_PTR, 0x6B, 1);
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
fn emits_and_runs_constant_storage_addresses() {
    let source = r#"
            struct Cell {
                value: u8
                next: u16
            }

            struct Packet {
                cells: [Cell; 2]
            }

            global byte: u8 = 0
            global bytes: [u8; 3] = [0, 0, 0]
            global cell: Cell = Cell { value: 0, next: 0 }
            global packet: Packet = Packet {
                cells: [
                    Cell { value: 0, next: 0 },
                    Cell { value: 0, next: 0 }
                ]
            }

            const BYTE: ptr<u8> = &byte
            const SECOND: ptr<u8> = &bytes[1]
            const CELL_NEXT: ptr<u16> = &cell.next
            const PACKET_NEXT: ptr<u16> = &packet.cells[1].next
            const RAW_THIRD: ptr24 = cast<ptr24>(&bytes[2])

            fn main() {
                let byte_ptr: ptr<u8> = BYTE;
                let second_ptr: ptr<u8> = SECOND;
                let cell_next_ptr: ptr<u16> = CELL_NEXT;
                let packet_next_ptr: ptr<u16> = PACKET_NEXT;
                *(byte_ptr) = 0x11;
                *(second_ptr) = 0x22;
                *(cell_next_ptr) = 0x3344;
                *(packet_next_ptr) = 0x5566;
                let third: ptr<u8> = cast<ptr<u8>>(RAW_THIRD);
                *(third) = 0x77;

                test.assert_eq_u8(byte, 0x11, 1)
                test.assert_eq_u8(bytes[1], 0x22, 2)
                test.assert_eq_u16(cell.next, 0x3344, 3)
                test.assert_eq_u16(packet.cells[1].next, 0x5566, 4)
                test.assert_eq_u8(bytes[2], 0x77, 5)
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
fn emits_and_runs_forward_constant_storage_addresses() {
    let source = r#"
            struct Pair {
                left: u8
                right: u16
            }

            const SECOND: ptr<u8> = &bytes[1]
            const PAIR_RIGHT: ptr<u16> = &pair.right
            const RAW_THIRD: ptr24 = cast<ptr24>(&bytes[2])

            const MARKER_ALIGN: u8 = 4
            embed marker: bytes = bytes [0xAA, 0xBB] align MARKER_ALIGN
            global prefix: u8 = 0
            global bytes: [u8; 3] = [0, 0, 0]
            global pair: Pair = Pair { left: 0, right: 0 }

            fn main() {
                let second: ptr<u8> = SECOND;
                let pair_right: ptr<u16> = PAIR_RIGHT;
                *(second) = 0x44;
                *(pair_right) = 0x5678;
                let third: ptr<u8> = cast<ptr<u8>>(RAW_THIRD);
                *(third) = 0x99;

                test.assert_eq_u24(cast<u24>(marker.ptr), EZRA_ASSET_BASE, 1)
                test.assert_eq_u24(cast<u24>(&bytes[0]), cast<u24>(&prefix) + 1, 2)
                test.assert_eq_u8(bytes[1], 0x44, 3)
                test.assert_eq_u8(bytes[2], 0x99, 4)
                test.assert_eq_u16(pair.right, 0x5678, 5)
                test.assert_eq_u24(cast<u24>(marker.end), EZRA_ASSET_BASE + 2, 6)
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
fn emits_and_runs_forward_constants_in_storage_layout() {
    let source = r#"
            global prefix: u8 = 0
            global bytes: [u8; LEN + EXTRA] = [0x11, 0x22, 0x33, 0x44]
            embed marker: bytes = bytes [0xAA] align ALIGN

            const LEN: u8 = BASE + 1
            const EXTRA: u8 = 1
            const BASE: u8 = 2
            const ALIGN: u8 = 8

            fn main() {
                test.assert_eq_u8(bytes[0], 0x11, 1)
                test.assert_eq_u8(bytes[3], 0x44, 2)
                test.assert_eq_u24(cast<u24>(marker.ptr) & 7, 0, 3)
                test.assert_eq_u24(cast<u24>(&bytes[0]), cast<u24>(&prefix) + 1, 4)
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
fn emits_and_runs_forward_constants_in_hardware_and_alias_declarations() {
    let source = r#"
            alias Row = [u8; ROW_LEN]

            port INPUT: u8 = PORT_BASE + 1
            port OUTPUT: u8 = PORT_BASE + 2
            volatile mmio SCRATCH: ptr<u8> = MMIO_BASE + 0x20
            embed header: bytes = bytes [FILL, FILL + 1] align ALIGN
            embed blank: bytes = repeat(FILL, REPEAT_COUNT)
            global row: Row = [0x11, 0x22, 0x33]

            const ROW_LEN: u8 = 3
            const PORT_BASE: u8 = 0x20
            const MMIO_BASE: u24 = 0x040100
            const FILL: u8 = 0x44
            const ALIGN: u8 = 8
            const REPEAT_COUNT: u8 = 2

            fn main() {
                let value: u8 = in INPUT
                out OUTPUT, value + 1
                mem.poke8(SCRATCH, cast<u8>(header.len + blank.len))
                row[2] = value

                test.assert_eq_u8(value, 0x5A, 1)
                test.assert_eq_u8(mem.peek8(SCRATCH), 4, 2)
                test.assert_eq_u8(row[2], 0x5A, 3)
                test.assert_eq_u8(*(header.ptr + 0), 0x44, 4)
                test.assert_eq_u8(*(header.ptr + 1), 0x45, 5)
                test.assert_eq_u8(*(blank.ptr + 1), 0x44, 6)
                test.assert_eq_u24(cast<u24>(header.ptr) & 7, 0, 7)
                test.pass()
            }
        "#;
    let program = parse_program(Path::new("game.ezra"), source).unwrap();
    let asm = emit_ez80_assembly(&program).unwrap();
    let run = run_assembly_test_with_options(
        &asm,
        &TestRunOptions {
            instruction_budget: 20_000,
            initial_ports: vec![(0x21, 0x5A)],
            initial_memory: Vec::new(),
            stack_top: EZRA_STACK_TOP.get(),
        },
    )
    .unwrap();

    assert!(asm.contains("in0 a, (21h)"), "{asm}");
    assert!(asm.contains("out0 (22h), a"), "{asm}");
    assert!(run.halted, "{asm}");
    assert_eq!(run.result_code, 0, "{asm}");
    assert_eq!(run.ports[0x22], 0x5B, "{asm}");
}

#[test]
fn rejects_invalid_pointer_arithmetic() {
    let cases = [
        (
            r#"
                global left: u8 = 0
                global right: u8 = 0
                fn main() {
                    let lp: ptr<u8> = &left
                    let rp: ptr<u8> = &right
                    let bad: ptr<u8> = lp + rp
                    test.pass()
                }
                "#,
            "pointer arithmetic requires exactly one pointer operand",
        ),
        (
            r#"
                global byte: u8 = 0
                fn main() {
                    let p: ptr<u8> = &byte
                    let bad: ptr<u8> = p - p
                    test.pass()
                }
                "#,
            "pointer subtraction between two pointers is not supported",
        ),
        (
            r#"
                global byte: u8 = 0
                fn main() {
                    let p: ptr<u8> = &byte
                    let bad: ptr<u8> = 1 - p
                    test.pass()
                }
                "#,
            "cannot subtract a pointer from a non-pointer value",
        ),
        (
            r#"
                global byte: u8 = 0
                fn main() {
                    let p: ptr<u8> = &byte
                    let flag: bool = true
                    let bad: ptr<u8> = p + flag
                    test.pass()
                }
                "#,
            "pointer arithmetic offset must be an integer",
        ),
        (
            r#"
                global byte: u8 = 0
                fn main() {
                    let p: ptr<u8> = &byte
                    let bad: u24 = p & 0x00FFFF
                    test.pass()
                }
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
