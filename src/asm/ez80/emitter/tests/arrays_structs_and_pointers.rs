use super::*;

#[test]
fn emits_and_runs_static_arrays() {
    let source = r#"
            global palette: [u8; 4] = [1, 2, 3]
            global words: [u16; 3] = [0x0100, 0x0200]

            fn main() {
                test.assert_eq_u8(palette[0], 1, 1)
                test.assert_eq_u8(palette[3], 0, 2)
                palette[1] = 9
                test.assert_eq_u8(palette[1], 9, 3)

                let local: [u8; 3] = [4, 5, 6]
                local[2] = palette[1] + 1
                test.assert_eq_u8(local[2], 10, 4)

                test.assert_eq_u16(words[0], 0x0100, 5)
                test.assert_eq_u16(words[2], 0, 6)
                words[2] = 0x1234
                test.assert_eq_u16(words[2], 0x1234, 7)

                let p: ptr<u8> = &palette[1]
                mem.poke8(p, 0x44)
                test.assert_eq_u8(mem.peek8(p), 0x44, 8)
                test.assert_eq_u8(palette[1], 0x44, 9)
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
fn emits_and_runs_array_length_constant_expressions() {
    let source = r#"
            const BASE: u8 = 2
            const EXTRA: u8 = 1
            global bytes: [u8; BASE + EXTRA] = [7, 8, 9]

            fn main() {
                test.assert_eq_u8(bytes[2], 9, 1)
                bytes[BASE] = 11
                test.assert_eq_u8(bytes[2], 11, 2)

                let local: [u16; BASE * 2] = [1, 2, 3, 4]
                test.assert_eq_u16(local[3], 4, 3)
                local[EXTRA + 1] = 0x1234
                test.assert_eq_u16(local[2], 0x1234, 4)
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
fn emits_and_runs_large_aggregate_storage() {
    let source = r#"
            struct Big {
                padding: [u8; 300]
                tail: u8
            }

            global bytes: [u8; 300] = []
            global big: Big = Big { tail: 7 }

            fn main() {
                bytes[299] = 0xA5
                test.assert_eq_u8(bytes[299], 0xA5, 1)
                test.assert_eq_u8(big.tail, 7, 2)

                big.padding[299] = 0x5A
                test.assert_eq_u8(big.padding[299], 0x5A, 3)

                let local: [u8; 260] = []
                local[259] = 0xC3
                test.assert_eq_u8(local[259], 0xC3, 4)
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
fn emits_and_runs_arrays_of_structs() {
    let source = r#"
            struct Point {
                x: u8
                y: u16
            }

            global points: [Point; 3] = [
                Point { x: 1, y: 0x0203 },
                Point { x: 4, y: 0x0506 }
            ]

            fn main() {
                let raw: ptr<u8> = cast<ptr<u8>>(&points[0])
                test.assert_eq_u8(mem.peek8(raw + 0), 1, 1)
                test.assert_eq_u8(mem.peek8(raw + 1), 0x03, 2)
                test.assert_eq_u8(mem.peek8(raw + 2), 0x02, 3)
                test.assert_eq_u8(mem.peek8(raw + 3), 4, 4)
                test.assert_eq_u8(mem.peek8(raw + 4), 0x06, 5)
                test.assert_eq_u8(mem.peek8(raw + 5), 0x05, 6)
                test.assert_eq_u8(mem.peek8(raw + 6), 0, 7)
                test.assert_eq_u8(mem.peek8(raw + 7), 0, 8)
                test.assert_eq_u8(mem.peek8(raw + 8), 0, 9)

                let local: [Point; 2] = [Point { x: 7, y: 0x0809 }]
                let local_raw: ptr<u8> = cast<ptr<u8>>(&local[0])
                test.assert_eq_u8(mem.peek8(local_raw + 0), 7, 10)
                test.assert_eq_u8(mem.peek8(local_raw + 1), 0x09, 11)
                test.assert_eq_u8(mem.peek8(local_raw + 2), 0x08, 12)
                test.assert_eq_u8(mem.peek8(local_raw + 3), 0, 13)
                test.assert_eq_u8(mem.peek8(local_raw + 4), 0, 14)
                test.assert_eq_u8(mem.peek8(local_raw + 5), 0, 15)

                let i: u8 = 1
                let second: ptr<u8> = cast<ptr<u8>>(&points[i])
                test.assert_eq_u24(cast<u24>(second), cast<u24>(raw) + 3, 16)
                test.assert_eq_u8(mem.peek8(second + 0), 4, 17)
                test.assert_eq_u8(mem.peek8(second + 1), 0x06, 18)
                test.assert_eq_u8(mem.peek8(second + 2), 0x05, 19)

                points[2] = Point { x: 9, y: 0x0A0B }
                test.assert_eq_u8(mem.peek8(raw + 6), 9, 20)
                test.assert_eq_u8(mem.peek8(raw + 7), 0x0B, 21)
                test.assert_eq_u8(mem.peek8(raw + 8), 0x0A, 22)

                points[i] = Point { x: 0x0C, y: 0x0D0E }
                test.assert_eq_u8(mem.peek8(second + 0), 0x0C, 23)
                test.assert_eq_u8(mem.peek8(second + 1), 0x0E, 24)
                test.assert_eq_u8(mem.peek8(second + 2), 0x0D, 25)
                test.pass()
            }
        "#;
    let program = parse_program(Path::new("game.ezra"), source).unwrap();
    let asm = emit_ez80_assembly(&program).unwrap();
    let run = run_assembly_test(&asm, 10_000).unwrap();

    assert!(run.halted, "{asm}");
    assert_eq!(run.result_code, 0, "{asm}");
}

#[test]
fn emits_and_runs_nested_arrays() {
    let source = r#"
            global grid: [[u8; 3]; 3] = [
                [1, 2, 3],
                [4, 5, 6]
            ]

            fn main() {
                let raw: ptr<u8> = cast<ptr<u8>>(&grid[0])
                test.assert_eq_u8(mem.peek8(raw + 0), 1, 1)
                test.assert_eq_u8(mem.peek8(raw + 1), 2, 2)
                test.assert_eq_u8(mem.peek8(raw + 2), 3, 3)
                test.assert_eq_u8(mem.peek8(raw + 3), 4, 4)
                test.assert_eq_u8(mem.peek8(raw + 4), 5, 5)
                test.assert_eq_u8(mem.peek8(raw + 5), 6, 6)
                test.assert_eq_u8(mem.peek8(raw + 6), 0, 7)
                test.assert_eq_u8(mem.peek8(raw + 7), 0, 8)
                test.assert_eq_u8(mem.peek8(raw + 8), 0, 9)

                let row_index: u8 = 2
                let third: ptr<u8> = cast<ptr<u8>>(&grid[row_index])
                test.assert_eq_u24(cast<u24>(third), cast<u24>(raw) + 6, 10)

                grid[2] = [7, 8, 9]
                test.assert_eq_u8(mem.peek8(third + 0), 7, 11)
                test.assert_eq_u8(mem.peek8(third + 1), 8, 12)
                test.assert_eq_u8(mem.peek8(third + 2), 9, 13)

                grid[row_index] = [10, 11, 12]
                test.assert_eq_u8(mem.peek8(third + 0), 10, 14)
                test.assert_eq_u8(mem.peek8(third + 1), 11, 15)
                test.assert_eq_u8(mem.peek8(third + 2), 12, 16)

                let local: [[u16; 2]; 2] = [[0x0102, 0x0304]]
                let local_raw: ptr<u8> = cast<ptr<u8>>(&local[0])
                test.assert_eq_u8(mem.peek8(local_raw + 0), 0x02, 17)
                test.assert_eq_u8(mem.peek8(local_raw + 1), 0x01, 18)
                test.assert_eq_u8(mem.peek8(local_raw + 2), 0x04, 19)
                test.assert_eq_u8(mem.peek8(local_raw + 3), 0x03, 20)
                test.assert_eq_u8(mem.peek8(local_raw + 4), 0, 21)
                test.assert_eq_u8(mem.peek8(local_raw + 5), 0, 22)
                test.assert_eq_u8(mem.peek8(local_raw + 6), 0, 23)
                test.assert_eq_u8(mem.peek8(local_raw + 7), 0, 24)
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
fn emits_and_runs_chained_array_and_struct_accesses() {
    let source = r#"
            struct Point {
                x: u8
                y: u16
            }

            struct Packet {
                points: [Point; 2]
            }

            global grid: [[u8; 3]; 2] = [
                [1, 2, 3],
                [4, 5, 6]
            ]

            global packets: [Packet; 2] = [
                Packet {
                    points: [
                        Point { x: 7, y: 0x0809 },
                        Point { x: 10, y: 0x0B0C }
                    ]
                }
            ]

            fn main() {
                let row: u8 = 1
                let col: u8 = 2
                test.assert_eq_u8(grid[row][col], 6, 1)
                grid[row][col] = 0x44
                test.assert_eq_u8(grid[1][2], 0x44, 2)
                grid[row][col] += 1
                test.assert_eq_u8(grid[1][2], 0x45, 3)

                let packet_index: u8 = 0
                let point_index: u8 = 1
                test.assert_eq_u8(packets[packet_index].points[point_index].x, 10, 4)
                test.assert_eq_u16(packets[packet_index].points[point_index].y, 0x0B0C, 5)
                packets[packet_index].points[point_index].x = grid[row][col]
                test.assert_eq_u8(packets[0].points[1].x, 0x45, 6)
                packets[packet_index].points[point_index].y += 1
                test.assert_eq_u16(packets[0].points[1].y, 0x0B0D, 7)

                let x_ptr: ptr<u8> = &packets[packet_index].points[point_index].x
                test.assert_eq_u8(*x_ptr, 0x45, 8)
                test.pass()
            }
        "#;
    let program = parse_program(Path::new("game.ezra"), source).unwrap();
    let asm = emit_ez80_assembly(&program).unwrap();
    let run = run_assembly_test(&asm, 30_000).unwrap();

    assert!(run.halted, "{asm}");
    assert_eq!(run.result_code, 0, "{asm}");
}

#[test]
fn emits_and_runs_structs_with_array_fields() {
    let source = r#"
            struct Packet {
                tag: u8
                bytes: [u8; 3]
                words: [u16; 2]
            }

            global packet: Packet = Packet {
                tag: 0xAA,
                bytes: [1, 2, 3],
                words: [0x0405]
            }

            fn main() {
                let raw: ptr<u8> = cast<ptr<u8>>(&packet)
                test.assert_eq_u8(mem.peek8(raw + 0), 0xAA, 1)
                test.assert_eq_u8(mem.peek8(raw + 1), 1, 2)
                test.assert_eq_u8(mem.peek8(raw + 2), 2, 3)
                test.assert_eq_u8(mem.peek8(raw + 3), 3, 4)
                test.assert_eq_u8(mem.peek8(raw + 4), 0x05, 5)
                test.assert_eq_u8(mem.peek8(raw + 5), 0x04, 6)
                test.assert_eq_u8(mem.peek8(raw + 6), 0, 7)
                test.assert_eq_u8(mem.peek8(raw + 7), 0, 8)

                packet.bytes = [9, 8]
                let bytes: ptr<u8> = cast<ptr<u8>>(&packet.bytes)
                test.assert_eq_u8(mem.peek8(bytes + 0), 9, 9)
                test.assert_eq_u8(mem.peek8(bytes + 1), 8, 10)
                test.assert_eq_u8(mem.peek8(bytes + 2), 0, 11)

                let local: Packet = Packet { tag: 0x55 }
                let local_raw: ptr<u8> = cast<ptr<u8>>(&local)
                test.assert_eq_u8(mem.peek8(local_raw + 0), 0x55, 12)
                test.assert_eq_u8(mem.peek8(local_raw + 1), 0, 13)
                test.assert_eq_u8(mem.peek8(local_raw + 7), 0, 14)
                test.pass()
            }
        "#;
    let program = parse_program(Path::new("game.ezra"), source).unwrap();
    let asm = emit_ez80_assembly(&program).unwrap();
    let run = run_assembly_test(&asm, 10_000).unwrap();

    assert!(run.halted, "{asm}");
    assert_eq!(run.result_code, 0, "{asm}");
}

#[test]
fn emits_and_runs_forward_constants_in_struct_array_fields() {
    let source = r#"
            struct Packet {
                tag: u8
                bytes: [u8; BYTE_LEN]
                words: [u16; WORD_LEN + EXTRA_WORDS]
            }

            global packet: Packet = Packet {
                tag: 0xAA,
                bytes: [1, 2, 3, 4],
                words: [0x0506, 0x0708, 0x090A]
            }

            const BYTE_LEN: u8 = BASE_LEN + 1
            const BASE_LEN: u8 = 3
            const WORD_LEN: u8 = 2
            const EXTRA_WORDS: u8 = 1

            fn main() {
                let raw: ptr<u8> = cast<ptr<u8>>(&packet)
                test.assert_eq_u8(mem.peek8(raw + 0), 0xAA, 1)
                test.assert_eq_u8(mem.peek8(raw + 4), 4, 2)
                test.assert_eq_u8(mem.peek8(raw + 5), 0x06, 3)
                test.assert_eq_u8(mem.peek8(raw + 10), 0x09, 4)
                packet.bytes[2] = 0x55
                packet.words[2] = 0x1234
                test.assert_eq_u8(packet.bytes[2], 0x55, 5)
                test.assert_eq_u16(packet.words[2], 0x1234, 6)
                test.pass()
            }
        "#;
    let program = parse_program(Path::new("game.ezra"), source).unwrap();
    let asm = emit_ez80_assembly(&program).unwrap();
    let run = run_assembly_test(&asm, 16_000).unwrap();

    assert!(run.halted, "{asm}");
    assert_eq!(run.result_code, 0, "{asm}");
}

#[test]
fn emits_and_runs_array_pointer_parameters() {
    let source = r#"
            global bytes: [u8; 3] = [0x11, 0x22, 0x33]

            fn first(values: ptr<[u8; 3]>) -> u8 {
                return mem.peek8(cast<ptr<u8>>(values))
            }

            fn second(values: ptr<[u8; 3]>) -> u8 {
                let raw: ptr<u8> = cast<ptr<u8>>(values)
                return mem.peek8(raw + 1)
            }

            fn main() {
                test.assert_eq_u8(first(&bytes), 0x11, 1)
                test.assert_eq_u8(second(&bytes), 0x22, 2)
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
fn rejects_array_pointer_decay() {
    let cases = [
        r#"
            global bytes: [u8; 2] = [1, 2]

            fn main() {
                let ptr: ptr<u8> = bytes
                test.pass()
            }
            "#,
        r#"
            global bytes: [u8; 2] = [1, 2]

            fn first(values: ptr<[u8; 2]>) -> u8 {
                let raw: ptr<u8> = cast<ptr<u8>>(values)
                return *raw
            }

            fn main() {
                test.assert_eq_u8(first(bytes), 1, 1)
                test.pass()
            }
            "#,
        r#"
            global bytes: [u8; 2] = [1, 2]
            global dst: [u8; 2] = [0, 0]

            fn main() {
                mem.memcpy(dst, bytes, 2)
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
fn rejects_arrays_used_as_scalar_values() {
    let cases = [
        r#"
            global bytes: [u8; 2] = [1, 2]

            fn main() {
                let bad: u24 = bytes + 1
                test.pass()
            }
            "#,
        r#"
            global bytes: [u8; 2] = [1, 2]

            fn main() {
                debug.hex_u24(bytes)
                test.pass()
            }
            "#,
        r#"
            global left: [u8; 2] = [1, 2]
            global right: [u8; 2] = [1, 2]

            fn main() {
                let same: bool = left == right
                test.pass()
            }
            "#,
    ];

    for source in cases {
        let program = parse_program(Path::new("game.ezra"), source).unwrap();
        let error = emit_ez80_assembly(&program).unwrap_err();

        assert_eq!(error.message, "array value cannot be used as a scalar");
    }
}

#[test]
fn emits_and_runs_array_pointer_arithmetic_scale() {
    let source = r#"
            struct Cell {
                x: u8
                y: u16
            }

            fn next_chunk(values: ptr<[u8; 3]>) -> ptr<[u8; 3]> {
                return values + 1
            }

            fn prev_chunk(values: ptr<[u8; 3]>) -> ptr<[u8; 3]> {
                return values - 1
            }

            fn next_cell(values: ptr<Cell>) -> ptr<Cell> {
                return values + 1
            }

            fn prev_cell(values: ptr<Cell>) -> ptr<Cell> {
                return values - 1
            }

            fn main() {
                let chunks: [[u8; 3]; 2] = [[1, 2, 3], [4, 5, 6]]
                let next: ptr<[u8; 3]> = next_chunk(&chunks[0])
                let prev: ptr<[u8; 3]> = prev_chunk(next)
                test.assert_eq_u24(cast<u24>(next), cast<u24>(&chunks[0]) + 3, 1)
                test.assert_eq_u24(cast<u24>(prev), cast<u24>(&chunks[0]), 2)

                let cells: [Cell; 2] = [
                    Cell { x: 1, y: 0x0203 },
                    Cell { x: 4, y: 0x0506 },
                ]
                let second: ptr<Cell> = next_cell(&cells[0])
                let first: ptr<Cell> = prev_cell(second)
                test.assert_eq_u24(cast<u24>(second), cast<u24>(&cells[0]) + 3, 3)
                test.assert_eq_u24(cast<u24>(first), cast<u24>(&cells[0]), 4)
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
fn emits_and_runs_runtime_array_indexes() {
    let source = r#"
            global bytes: [u8; 4] = [0, 0, 0, 0]
            global words: [u16; 3] = [0, 0, 0]
            global longs: [u24; 2] = [0, 0]

            fn main() {
                let i: u8 = 0
                while i < 4 {
                    bytes[i] = i + 1
                    i += 1
                }
                test.assert_eq_u8(bytes[0], 1, 1)
                test.assert_eq_u8(bytes[3], 4, 2)

                let j: u8 = 0
                while j < 3 {
                    words[j] = cast<u16>(j) + 0x0100
                    j += 1
                }
                test.assert_eq_u16(words[0], 0x0100, 3)
                test.assert_eq_u16(words[2], 0x0102, 4)

                let k: u8 = 0
                while k < 2 {
                    longs[k] = cast<u24>(k) + 0x010000
                    k += 1
                }
                test.assert_eq_u24(longs[0], 0x010000, 5)
                test.assert_eq_u24(longs[1], 0x010001, 6)

                let p: ptr<u8> = &bytes[i - 2]
                mem.poke8(p, 0x7E)
                test.assert_eq_u8(bytes[2], 0x7E, 7)
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
fn emits_and_runs_compound_indexed_assignments() {
    let source = r#"
            global bytes: [u8; 4] = [1, 2, 3, 4]
            global words: [u16; 3] = [0x0100, 0x0200, 0x0300]
            global longs: [u24; 2] = [0x010000, 0x020000]

            fn main() {
                bytes[1] += 5
                bytes[2] ^= 0x0F
                test.assert_eq_u8(bytes[1], 7, 1)
                test.assert_eq_u8(bytes[2], 12, 2)

                let i: u8 = 3
                bytes[i] -= 2
                test.assert_eq_u8(bytes[3], 2, 3)

                let j: u8 = 1
                words[j] += 0x0010
                words[j] <<= 1
                test.assert_eq_u16(words[1], 0x0420, 4)

                let k: u8 = 0
                longs[k] += 0x000123
                longs[k] &= 0x01FFFF
                test.assert_eq_u24(longs[0], 0x010123, 5)
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
fn emits_and_runs_pointer_dereferences() {
    let source = r#"
            global bytes: [u8; 4] = [0, 0, 0, 0]
            global words: [u16; 2] = [0, 0]
            global longs: [u24; 2] = [0, 0]

            fn second_byte() -> ptr<u8> {
                return &bytes[2]
            }

            fn main() {
                let p: ptr<u8> = &bytes[0];
                *p = 0x12;
                *(p + 1) = 0x34;
                test.assert_eq_u8(*p, 0x12, 1);
                test.assert_eq_u8(*(p + 1), 0x34, 2);
                *second_byte() = 0x56;
                test.assert_eq_u8(*second_byte(), 0x56, 7);

                let w: ptr<u16> = &words[1];
                *w = 0x5678;
                test.assert_eq_u16(words[1], 0x5678, 3);
                test.assert_eq_u16(*w, 0x5678, 4);

                let l: ptr<u24> = &longs[1];
                *l = 0x010203;
                test.assert_eq_u24(longs[1], 0x010203, 5);
                test.assert_eq_u24(*l, 0x010203, 6);
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
fn emits_and_runs_arithmetic_compound_assignments() {
    let expected_i8_mul = (-9i8).wrapping_mul(3) as u8;
    let expected_i8_div = ((-27i8) / 4) as u8;
    let expected_i8_mod = ((-6i8) % 4) as u8;
    let expected_i16_div = ((-300i16) / 7) as u16;
    let expected_i16_mod = ((-42i16) % 5) as u16;
    let expected_i24_div = ((-0x012345i32) / 17) & 0x00FF_FFFF;
    let expected_i24_mod = ((-0x012345i32) % 17) & 0x00FF_FFFF;
    let source = format!(
        r#"
            global bytes: [u8; 2] = [9, 10]
            global words: [u16; 1] = [300]

            fn main() {{
                let a: u8 = 7
                a *= 3
                test.assert_eq_u8(a, 21, 1)
                a /= 2
                test.assert_eq_u8(a, 10, 2)
                a %= 4
                test.assert_eq_u8(a, 2, 3)
                a /= 0
                test.assert_eq_u8(a, 0, 4)

                let w: u16 = 300
                w *= 3
                test.assert_eq_u16(w, 900, 5)
                w /= 7
                test.assert_eq_u16(w, 128, 6)
                w %= 5
                test.assert_eq_u16(w, 3, 7)

                let s8: i8 = -9
                s8 *= 3
                test.assert_eq_u8(s8, 0x{expected_i8_mul:02X}, 8)
                s8 /= 4
                test.assert_eq_u8(s8, 0x{expected_i8_div:02X}, 9)
                s8 %= 4
                test.assert_eq_u8(s8, 0x{expected_i8_mod:02X}, 10)
                s8 %= 0
                test.assert_eq_u8(s8, 0, 11)

                let s16: i16 = -300
                s16 /= 7
                test.assert_eq_u16(s16, 0x{expected_i16_div:04X}, 12)
                s16 %= 5
                test.assert_eq_u16(s16, 0x{expected_i16_mod:04X}, 13)

                let s24: i24 = -0x012345
                s24 /= 17
                test.assert_eq_u24(s24, 0x{expected_i24_div:06X}, 14)
                let r24: i24 = -0x012345
                r24 %= 17
                test.assert_eq_u24(r24, 0x{expected_i24_mod:06X}, 15)

                bytes[0] *= 2
                bytes[1] %= 6
                test.assert_eq_u8(bytes[0], 18, 16)
                test.assert_eq_u8(bytes[1], 4, 17)
                words[0] /= 0
                test.assert_eq_u16(words[0], 0, 18)
                test.pass()
            }}
            "#
    );
    let program = parse_program(Path::new("game.ezra"), &source).unwrap();
    let asm = emit_ez80_assembly(&program).unwrap();
    let run = run_assembly_test(&asm, 1_000_000).unwrap();

    assert!(asm.contains("    call __ezra_mul_u8"), "{asm}");
    assert!(asm.contains("    call __ezra_div_u8"), "{asm}");
    assert!(asm.contains("    call __ezra_mod_u8"), "{asm}");
    assert!(run.halted, "{asm}");
    assert_eq!(run.result_code, 0, "{asm}");
}

#[test]
fn emits_and_runs_aggregate_pointer_assignments() {
    let source = r#"
            struct Pair {
                left: u8
                right: u16
            }

            global bytes: [u8; 3] = [0, 0, 0]
            global pair: Pair = Pair { left: 0, right: 0 }

            fn main() {
                let byte_ptr: ptr<[u8; 3]> = &bytes;
                *(byte_ptr) = [4, 5, 6]
                test.assert_eq_u8(bytes[0], 4, 1)
                test.assert_eq_u8(bytes[2], 6, 2)

                let pair_ptr: ptr<Pair> = &pair;
                *(pair_ptr) = Pair { left: 7, right: 0x0809 }
                test.assert_eq_u8(pair.left, 7, 3)
                test.assert_eq_u16(pair.right, 0x0809, 4)
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
fn emits_and_runs_aggregate_pointer_reads() {
    let source = r#"
            struct Pair {
                left: u8
                right: u16
            }

            global bytes: [u8; 3] = [4, 5, 6]
            global pair: Pair = Pair { left: 7, right: 0x0809 }

            fn main() {
                let byte_ptr: ptr<[u8; 3]> = &bytes;
                let local_bytes: [u8; 3] = *(byte_ptr)
                test.assert_eq_u8(local_bytes[0], 4, 1)
                test.assert_eq_u8(local_bytes[2], 6, 2)

                let pair_ptr: ptr<Pair> = &pair;
                let local_pair: Pair = *(pair_ptr)
                test.assert_eq_u8(local_pair.left, 7, 3)
                test.assert_eq_u16(local_pair.right, 0x0809, 4)
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
fn emits_and_runs_stored_aggregate_copies() {
    let source = r#"
            struct Pair {
                left: u8
                right: u16
            }

            struct Packet {
                bytes: [u8; 3]
                pair: Pair
            }

            global source_bytes: [u8; 3] = [1, 2, 3]
            global target_bytes: [u8; 3] = [0, 0, 0]
            global source_pair: Pair = Pair { left: 4, right: 0x0506 }
            global target_pair: Pair = Pair { left: 0, right: 0 }
            global packet: Packet = Packet {
                bytes: [7, 8, 9],
                pair: Pair { left: 10, right: 0x0B0C }
            }

            fn main() {
                target_bytes = source_bytes
                test.assert_eq_u8(target_bytes[0], 1, 1)
                test.assert_eq_u8(target_bytes[2], 3, 2)

                target_pair = source_pair
                test.assert_eq_u8(target_pair.left, 4, 3)
                test.assert_eq_u16(target_pair.right, 0x0506, 4)

                let local_bytes: [u8; 3] = source_bytes
                test.assert_eq_u8(local_bytes[1], 2, 5)

                let local_pair: Pair = target_pair
                test.assert_eq_u8(local_pair.left, 4, 6)
                test.assert_eq_u16(local_pair.right, 0x0506, 7)

                packet.bytes = target_bytes
                test.assert_eq_u8(packet.bytes[0], 1, 8)
                test.assert_eq_u8(packet.bytes[2], 3, 9)

                packet.pair = source_pair
                test.assert_eq_u8(packet.pair.left, 4, 10)
                test.assert_eq_u16(packet.pair.right, 0x0506, 11)
                test.pass()
            }
        "#;
    let program = parse_program(Path::new("game.ezra"), source).unwrap();
    let asm = emit_ez80_assembly(&program).unwrap();
    let run = run_assembly_test(&asm, 16_000).unwrap();

    assert!(run.halted, "{asm}");
    assert_eq!(run.result_code, 0, "{asm}");
}

#[test]
fn emits_and_runs_copied_aggregate_global_initializers() {
    let source = r#"
            struct Pair {
                left: u8
                right: u16
            }

            struct Packet {
                bytes: [u8; 3]
                pair: Pair
            }

            global source_bytes: [u8; 3] = [1, 2, 3]
            global copied_bytes: [u8; 3] = source_bytes
            global source_pair: Pair = Pair { left: 4, right: 0x0506 }
            global copied_pair: Pair = source_pair
            global source_packet: Packet = Packet {
                bytes: [7, 8, 9],
                pair: Pair { left: 10, right: 0x0B0C }
            }
            global copied_packet: Packet = source_packet

            fn main() {
                test.assert_eq_u8(copied_bytes[0], 1, 1)
                test.assert_eq_u8(copied_bytes[2], 3, 2)
                test.assert_eq_u8(copied_pair.left, 4, 3)
                test.assert_eq_u16(copied_pair.right, 0x0506, 4)
                test.assert_eq_u8(copied_packet.bytes[0], 7, 5)
                test.assert_eq_u8(copied_packet.bytes[2], 9, 6)
                test.assert_eq_u8(copied_packet.pair.left, 10, 7)
                test.assert_eq_u16(copied_packet.pair.right, 0x0B0C, 8)
                test.pass()
            }
        "#;
    let program = parse_program(Path::new("game.ezra"), source).unwrap();
    let asm = emit_ez80_assembly(&program).unwrap();
    let run = run_assembly_test(&asm, 16_000).unwrap();

    assert!(run.halted, "{asm}");
    assert_eq!(run.result_code, 0, "{asm}");
}

#[test]
fn emits_and_runs_overlapping_stored_aggregate_copies() {
    let source = r#"
            struct Rows {
                first: [u8; 3]
                second: [u8; 3]
            }

            global rows: Rows = Rows {
                first: [1, 2, 3],
                second: [4, 5, 6]
            }
            global grid: [[u8; 3]; 2] = [
                [7, 8, 9],
                [10, 11, 12]
            ]

            fn main() {
                rows.second = rows.first
                test.assert_eq_u8(rows.second[0], 1, 1)
                test.assert_eq_u8(rows.second[2], 3, 2)

                rows.first = rows.first
                test.assert_eq_u8(rows.first[0], 1, 3)
                test.assert_eq_u8(rows.first[2], 3, 4)

                grid[1] = grid[0]
                test.assert_eq_u8(grid[1][0], 7, 5)
                test.assert_eq_u8(grid[1][2], 9, 6)
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
fn rejects_compound_assignment_to_aggregate_values() {
    let cases = [
        r#"
            global bytes: [u8; 2] = [1, 2]

            fn main() {
                bytes += [3, 4]
                test.pass()
            }
            "#,
        r#"
            struct Pair {
                left: u8
                right: u8
            }

            global pair: Pair = Pair { left: 1, right: 2 }

            fn main() {
                pair += Pair { left: 3, right: 4 }
                test.pass()
            }
            "#,
        r#"
            struct Packet {
                bytes: [u8; 2]
            }

            global packet: Packet = Packet { bytes: [1, 2] }

            fn main() {
                packet.bytes += [3, 4]
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
fn emits_and_runs_casted_indirect_assignments() {
    let source = r#"
            global bytes: [u8; 2] = [0, 0]
            global word: u16 = 0

            fn main() {
                let wide: u16 = 0x12FE
                bytes[1] = cast<u8>(wide)

                let p: ptr<u16> = &word;
                let small: u8 = 0x34;
                *p = cast<u16>(small);

                test.assert_eq_u8(bytes[1], 0xFE, 1)
                test.assert_eq_u16(word, 0x0034, 2)
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
fn emits_and_runs_compound_pointer_dereference_assignments() {
    let source = r#"
            global bytes: [u8; 4] = [10, 20, 30, 40]
            global words: [u16; 2] = [0x0100, 0x0200]
            global longs: [u24; 2] = [0x010000, 0x020000]

            fn main() {
                let b: ptr<u8> = &bytes[1];
                *b += 7;
                *(b + 1) &= 0x1F;
                test.assert_eq_u8(bytes[1], 27, 1)
                test.assert_eq_u8(bytes[2], 30, 2)

                let w: ptr<u16> = &words[0];
                *w += 0x0023;
                *(w + 1) >>= 1;
                test.assert_eq_u16(words[0], 0x0123, 3)
                test.assert_eq_u16(words[1], 0x0100, 4)

                let l: ptr<u24> = &longs[0];
                *l += 0x000123;
                *(l + 1) ^= 0x0000FF;
                test.assert_eq_u24(longs[0], 0x010123, 5)
                test.assert_eq_u24(longs[1], 0x0200FF, 6)
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
fn emits_and_runs_scaled_pointer_arithmetic() {
    let source = r#"
            global bytes: [u8; 4] = [0, 0, 0, 0]
            global words: [u16; 3] = [0, 0, 0]
            global longs: [u24; 3] = [0, 0, 0]

            fn main() {
                let b: ptr<u8> = &bytes[1];
                *(b + 2) = 0x7A;
                test.assert_eq_u8(bytes[3], 0x7A, 1);
                let back_byte: i8 = -1;
                *(b + back_byte) = 0x33;
                test.assert_eq_u8(bytes[0], 0x33, 5);
                test.assert_eq_u24(cast<u24>(b + back_byte), cast<u24>(&bytes[0]), 7);

                let w: ptr<u16> = &words[0];
                *(w + 2) = 0x4567;
                test.assert_eq_u16(words[2], 0x4567, 2);
                *(w + 2 - 1) = 0x1234;
                test.assert_eq_u16(words[1], 0x1234, 3);
                let back_word: i8 = -1;
                *(w + 2 + back_word) = 0x2345;
                test.assert_eq_u16(words[1], 0x2345, 6);
                test.assert_eq_u24(cast<u24>(w + 2 + back_word), cast<u24>(&words[1]), 8);

                let l: ptr<u24> = &longs[0];
                *(l + 2) = 0x010203;
                test.assert_eq_u24(longs[2], 0x010203, 4);
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
fn emits_and_runs_scaled_pointer_compound_assignments() {
    let source = r#"
            struct Cell {
                x: u8
                y: u16
            }

            global bytes: [u8; 4] = [0, 0, 0, 0]
            global words: [u16; 3] = [0, 0, 0]
            global cells: [Cell; 2] = [
                Cell { x: 1, y: 0x0203 },
                Cell { x: 4, y: 0x0506 },
            ]

            fn main() {
                let byte_ptr: ptr<u8> = &bytes[1]
                byte_ptr += 2
                *byte_ptr = 0x7A
                test.assert_eq_u8(bytes[3], 0x7A, 1)
                byte_ptr -= 3
                *byte_ptr = 0x33
                test.assert_eq_u8(bytes[0], 0x33, 2)

                let word_ptr: ptr<u16> = &words[0]
                word_ptr += 2
                *word_ptr = 0x4567
                test.assert_eq_u16(words[2], 0x4567, 3)
                word_ptr -= 1
                *word_ptr = 0x1234
                test.assert_eq_u16(words[1], 0x1234, 4)

                let cell_ptr: ptr<Cell> = &cells[0]
                cell_ptr += 1
                test.assert_eq_u8(mem.peek8(cast<ptr<u8>>(cell_ptr)), 4, 5)
                test.assert_eq_u8(mem.peek8(cast<ptr<u8>>(cell_ptr) + 1), 0x06, 6)
                test.assert_eq_u8(mem.peek8(cast<ptr<u8>>(cell_ptr) + 2), 0x05, 7)
                test.pass()
            }
        "#;
    let program = parse_program(Path::new("game.ezra"), source).unwrap();
    let asm = emit_ez80_assembly(&program).unwrap();
    let run = run_assembly_test(&asm, 16_000).unwrap();

    assert!(run.halted, "{asm}");
    assert_eq!(run.result_code, 0, "{asm}");
}

#[test]
fn rejects_invalid_pointer_compound_assignments() {
    let cases = [
        (
            r#"
            global byte: u8 = 0
            fn main() {
                let p: ptr<u8> = &byte
                p &= 0x040000
                test.pass()
            }
            "#,
            "type mismatch",
        ),
        (
            r#"
            global byte: u8 = 0
            fn main() {
                let p: ptr<u8> = &byte
                p <<= 1
                test.pass()
            }
            "#,
            "type mismatch",
        ),
        (
            r#"
            global byte: u8 = 0
            fn main() {
                let p: ptr<u8> = &byte
                p += true
                test.pass()
            }
            "#,
            "pointer arithmetic offset must be an integer",
        ),
    ];

    for (source, expected) in cases {
        let program = parse_program(Path::new("game.ezra"), source).unwrap();
        let error = emit_ez80_assembly(&program).unwrap_err();

        assert_eq!(error.message, expected);
    }
}

#[test]
fn emits_and_runs_struct_pointer_arithmetic_scale() {
    let source = r#"
            struct Cell {
                value: u24
                flags: u8
            }

            struct BigCell {
                padding: [u8; 300]
                value: u8
            }

            global cell: Cell = Cell { value: 0x010203, flags: 0x44 }
            global big: BigCell = BigCell { value: 0x99 }

            fn main() {
                let p: ptr<Cell> = &cell
                let q: ptr<Cell> = p + 2
                let r: ptr<Cell> = q - 1
                test.assert_eq_u24(cast<u24>(q), cast<u24>(p) + 8, 1)
                test.assert_eq_u24(cast<u24>(r), cast<u24>(p) + 4, 2)

                let big_p: ptr<BigCell> = &big
                let big_q: ptr<BigCell> = big_p + 1
                let big_r: ptr<BigCell> = big_q - 1
                test.assert_eq_u24(cast<u24>(big_q), cast<u24>(big_p) + 301, 3)
                test.assert_eq_u24(cast<u24>(big_r), cast<u24>(big_p), 4)
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
fn emits_and_runs_scalar_address_of() {
    let source = r#"
            global byte_value: u8 = 0
            global word_value: u16 = 0
            global long_value: u24 = 0
            global word_ptr: ptr<u16> = &word_value

            fn write_byte(ptr: ptr<u8>, value: u8) {
                *ptr = value
            }

            fn main() {
                let byte_ptr: ptr<u8> = &byte_value;
                write_byte(byte_ptr, 0x5A);
                test.assert_eq_u8(byte_value, 0x5A, 1);
                test.assert_eq_u8(*byte_ptr, 0x5A, 2);

                *word_ptr = 0x1234;
                test.assert_eq_u16(word_value, 0x1234, 3);

                let long_ptr: ptr<u24> = &long_value;
                *long_ptr = 0x010203;
                test.assert_eq_u24(long_value, 0x010203, 4);
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
fn emits_and_runs_basic_struct_fields() {
    let source = r#"
            struct Entity {
                x: u24
                y: u24
                sprite: u8
                flags: u8
            }

            global player: Entity = Entity {
                x: 0x010000,
                sprite: 3,
            }

            fn main() {
                test.assert_eq_u24(player.x, 0x010000, 1);
                test.assert_eq_u24(player.y, 0, 2);
                test.assert_eq_u8(player.sprite, 3, 3);
                test.assert_eq_u8(player.flags, 0, 4);

                player.y = player.x + 0x000123;
                player.sprite += 4;
                player.flags = 0x80;

                let local: Entity = Entity {
                    x: player.y,
                    y: 0x020000,
                    sprite: player.sprite,
                    flags: player.flags,
                };

                test.assert_eq_u24(player.y, 0x010123, 5);
                test.assert_eq_u8(player.sprite, 7, 6);
                test.assert_eq_u24(local.x, 0x010123, 7);
                test.assert_eq_u24(local.y, 0x020000, 8);
                test.assert_eq_u8(local.sprite, 7, 9);
                test.assert_eq_u8(local.flags, 0x80, 10);
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
fn emits_and_runs_struct_field_addresses() {
    let source = r#"
            struct Entity {
                x: u24
                sprite: u8
                hp: u16
            }

            global player: Entity = Entity {
                x: 0,
                sprite: 1,
                hp: 100,
            }

            fn write_u24(ptr: ptr<u24>, value: u24) {
                *ptr = value
            }

            fn main() {
                let x_ptr: ptr<u24> = &player.x;
                write_u24(x_ptr, 0x010203);
                test.assert_eq_u24(player.x, 0x010203, 1);
                test.assert_eq_u24(*x_ptr, 0x010203, 2);

                let sprite_ptr: ptr<u8> = &player.sprite;
                *sprite_ptr = 7;
                test.assert_eq_u8(player.sprite, 7, 3);

                let hp_ptr: ptr<u16> = &player.hp;
                *hp_ptr = *hp_ptr + 23;
                test.assert_eq_u16(player.hp, 123, 4);
                test.pass()
            }
        "#;
    let program = parse_program(Path::new("game.ezra"), source).unwrap();
    let asm = emit_ez80_assembly(&program).unwrap();
    let run = run_assembly_test(&asm, 8_000).unwrap();

    assert!(run.halted, "{asm}");
    assert_eq!(run.result_code, 0, "{asm}");
}
