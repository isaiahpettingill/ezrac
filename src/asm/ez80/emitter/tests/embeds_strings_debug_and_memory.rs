use super::*;

#[test]
fn emits_and_runs_inline_embedded_bytes() {
    let source = r#"
            embed palette: bytes = bytes [0x11, 0x22, 0x33] section .rodata align 16
            embed title_text: bytes = text("HI")
            embed title_cstr: bytes = cstr("OK")
            embed blank: bytes = repeat(0x7E, 4)

            global palette_ptr: ptr<u8> = palette.ptr

            fn main() {
                test.assert_eq_u24(palette.len, 3, 1);
                test.assert_eq_u8(*palette_ptr, 0x11, 2);
                test.assert_eq_u8(*(palette.ptr + 1), 0x22, 3);
                test.assert_eq_u8(*(palette.end - 1), 0x33, 4);
                test.assert_eq_u24(cast<ptr24>(palette.ptr), EZRA_RODATA_BASE, 14);

                test.assert_eq_u24(title_text.len, 2, 5);
                test.assert_eq_u8(*(title_text.ptr + 0), 'H', 6);
                test.assert_eq_u8(*(title_text.ptr + 1), 'I', 7);
                test.assert_eq_u24(cast<ptr24>(title_text.ptr), EZRA_ASSET_BASE, 15);

                test.assert_eq_u24(title_cstr.len, 3, 8);
                test.assert_eq_u8(*(title_cstr.ptr + 0), 'O', 9);
                test.assert_eq_u8(*(title_cstr.ptr + 1), 'K', 10);
                test.assert_eq_u8(*(title_cstr.ptr + 2), 0, 11);
                test.assert_eq_u24(cast<ptr24>(title_cstr.ptr), EZRA_ASSET_BASE + 2, 16);

                test.assert_eq_u24(blank.len, 4, 12);
                test.assert_eq_u8(*(blank.ptr + 3), 0x7E, 13);
                test.assert_eq_u24(cast<ptr24>(blank.ptr), EZRA_ASSET_BASE + 5, 17);
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
fn emits_and_runs_custom_section_embedded_bytes_at_section_base() {
    let source = r#"
            embed banked: bytes = bytes [0xA1, 0xA2] section .bank1 align 256

            fn main() {
                test.assert_eq_u24(cast<ptr24>(banked.ptr), 0x120000, 1)
                test.assert_eq_u8(*(banked.ptr + 0), 0xA1, 2)
                test.assert_eq_u8(*(banked.ptr + 1), 0xA2, 3)
                test.pass()
            }
        "#;
    let program = parse_program(Path::new("game.ezra"), source).unwrap();
    let asm = emit_ez80_assembly_with_options(
        &program,
        AssemblyOptions {
            section_bases: vec![(".bank1".to_owned(), Address24::new(0x12_0000))],
            ..AssemblyOptions::default()
        },
    )
    .unwrap();
    let run = run_assembly_test(&asm, 12_000).unwrap();

    assert!(run.halted, "{asm}");
    assert_eq!(run.result_code, 0, "{asm}");
}

#[test]
fn rejects_writes_to_read_only_embedded_bytes() {
    let cases = [
        (
            r#"
                embed sprite: bytes = bytes [0x11, 0x22]

                fn main() {
                    *(sprite.ptr) = 0x33
                    test.pass()
                }
                "#,
            "embedded object `sprite` is read-only",
        ),
        (
            r#"
                embed sprite: bytes = bytes [0x11, 0x22]

                fn main() {
                    *(sprite.ptr + 1) = 0x33
                    test.pass()
                }
                "#,
            "embedded object `sprite` is read-only",
        ),
        (
            r#"
                embed sprite: bytes = bytes [0x11, 0x22]

                fn main() {
                    let p: ptr<u8> = sprite.ptr;
                    *(p) = 0x33
                    test.pass()
                }
                "#,
            "embedded object `sprite` is read-only",
        ),
        (
            r#"
                embed sprite: bytes = bytes [0x11, 0x22]

                fn main() {
                    let p: ptr<u16> = cast<ptr<u16>>(sprite.ptr + 1);
                    *(p) = 0x3344
                    test.pass()
                }
                "#,
            "embedded object `sprite` is read-only",
        ),
        (
            r#"
                embed sprite: bytes = bytes [0x11, 0x22, 0x33, 0x44]

                fn main() {
                    let p: ptr<u16> = cast<ptr<u16>>(sprite.ptr);
                    let q: ptr<u16> = p + 1;
                    *(q) = 0x5566
                    test.pass()
                }
                "#,
            "embedded object `sprite` is read-only",
        ),
        (
            r#"
                embed sprite: bytes = bytes [0x11, 0x22]
                global sprite_alias: ptr<u8> = sprite.ptr

                fn main() {
                    *(sprite_alias + 1) = 0x33
                    test.pass()
                }
                "#,
            "embedded object `sprite` is read-only",
        ),
        (
            r#"
                embed sprite: bytes = bytes [0x11, 0x22]

                fn main() {
                    let offset: u8 = 1
                    let p: ptr<u8> = sprite.ptr;
                    let q: ptr<u8> = p + offset;
                    *(q) = 0x33
                    test.pass()
                }
                "#,
            "embedded object `sprite` is read-only",
        ),
    ];

    for (source, expected) in cases {
        let program = parse_program(Path::new("game.ezra"), source).unwrap();
        let error = emit_ez80_assembly(&program).unwrap_err();

        assert_eq!(error.message, expected);
    }
}

#[test]
fn allows_reassigned_embedded_pointer_alias_to_mutable_memory() {
    let source = r#"
            embed sprite: bytes = bytes [0x11, 0x22]

            fn main() {
                let p: ptr<u8> = sprite.ptr;
                p = cast<ptr<u8>>(0x040120);
                *(p) = 0x33
                test.assert_eq_u8(*(cast<ptr<u8>>(0x040120)), 0x33, 1)
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
fn emits_and_runs_file_embedded_bytes() {
    let root = std::env::temp_dir().join(format!(
        "ezra_file_embed_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let assets = root.join("assets");
    std::fs::create_dir_all(&assets).unwrap();
    std::fs::write(assets.join("blob.bin"), [0xA5, 0x5A, 0xC3]).unwrap();
    let source_path = root.join("game.ezra");
    let source = r#"
            embed blob: bytes = file("assets/blob.bin") align 4

            fn main() {
                test.assert_eq_u24(blob.len, 3, 1);
                test.assert_eq_u8(*(blob.ptr + 0), 0xA5, 2);
                test.assert_eq_u8(*(blob.ptr + 1), 0x5A, 3);
                test.assert_eq_u8(*(blob.end - 1), 0xC3, 4);
                test.pass()
            }
        "#;
    let program = parse_program(&source_path, source).unwrap();
    let asm = emit_ez80_assembly(&program).unwrap();
    let run = run_assembly_test(&asm, 12_000).unwrap();

    let _ = std::fs::remove_dir_all(&root);
    assert!(run.halted, "{asm}");
    assert_eq!(run.result_code, 0, "{asm}");
}

#[test]
fn file_embeds_fall_back_to_project_root() {
    let relative_dir = Path::new("target").join(format!(
        "ezra_project_root_file_embed_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&relative_dir).unwrap();
    std::fs::write(relative_dir.join("blob.bin"), [0xDE, 0xAD]).unwrap();
    let source_root = std::env::temp_dir().join(format!(
        "ezra_project_root_source_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let source_path = source_root.join("nested/game.ezra");
    let embed_path = format!("{}/blob.bin", relative_dir.display()).replace('\\', "/");
    let source = format!(
        r#"
            embed blob: bytes = file("{embed_path}")

            fn main() {{
                test.assert_eq_u24(blob.len, 2, 1)
                test.assert_eq_u8(*(blob.ptr + 0), 0xDE, 2)
                test.assert_eq_u8(*(blob.ptr + 1), 0xAD, 3)
                test.pass()
            }}
            "#
    );
    let program = parse_program(&source_path, &source).unwrap();
    let asm = emit_ez80_assembly(&program).unwrap();
    let run = run_assembly_test(&asm, 8_000).unwrap();

    let _ = std::fs::remove_dir_all(&relative_dir);
    let _ = std::fs::remove_dir_all(&source_root);
    assert!(run.halted, "{asm}");
    assert_eq!(run.result_code, 0, "{asm}");
}

#[test]
fn reports_missing_embedded_files() {
    let root = std::env::temp_dir().join(format!(
        "ezra_missing_file_embed_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let source_path = root.join("game.ezra");
    let source = r#"
            embed blob: bytes = file("assets/missing.bin")
            fn main() { test.pass() }
        "#;
    let program = parse_program(&source_path, source).unwrap();
    let error = emit_ez80_assembly(&program).unwrap_err();

    assert_eq!(
        error.message,
        format!(
            "embedded file `{}` not found",
            root.join("assets/missing.bin").display()
        )
    );
}

#[test]
fn emits_and_runs_zero_terminated_string_literals() {
    let source = r#"
            global title: ptr<u8> = "EZ"

            fn same(a: ptr<u8>, b: ptr<u8>) -> bool {
                return a == b
            }

            fn main() {
                let text: ptr<u8> = "OK";
                test.assert_eq_u8(*text, 'O', 1);
                test.assert_eq_u8(*(text + 1), 'K', 2);
                test.assert_eq_u8(*(text + 2), 0, 3);
                test.assert_eq_u8(*title, 'E', 4);
                test.assert_eq_u8(*(title + 1), 'Z', 5);
                test.assert_eq_u8(*(title + 2), 0, 6);
                test.assert_eq_u8(same("OK", "OK"), true, 7);
                test.assert_eq_u24(cast<ptr24>(title), EZRA_RODATA_BASE, 8);
                test.assert_eq_u24(cast<ptr24>(text), EZRA_RODATA_BASE + 3, 9);
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
fn emits_and_runs_character_literal_escapes() {
    let source = r#"
            fn main() {
                test.assert_eq_u8('\n', 10, 1)
                test.assert_eq_u8('\0', 0, 2)
                test.assert_eq_u8('\t', 9, 3)
                test.assert_eq_u8('\'', 39, 4)
                test.assert_eq_u8('\\', 92, 5)
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
fn emits_and_runs_const_string_literal_pointers() {
    let source = r#"
            const TITLE: ptr<u8> = "EZ"
            global title_copy: ptr<u8> = TITLE

            fn same(a: ptr<u8>, b: ptr<u8>) -> bool {
                return a == b
            }

            fn main() {
                test.assert_eq_u8(*TITLE, 'E', 1)
                test.assert_eq_u8(*(TITLE + 1), 'Z', 2)
                test.assert_eq_u8(*(TITLE + 2), 0, 3)
                test.assert_eq_u8(*title_copy, 'E', 4)
                test.assert_eq_u8(same(TITLE, "EZ"), true, 5)
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
fn rejects_writes_to_read_only_string_literals() {
    let cases = [
        r#"
                fn main() {
                    *("OK") = 'N'
                    test.pass()
                }
            "#,
        r#"
                fn main() {
                    let text: ptr<u8> = "OK";
                    *(text + 1) = 'X'
                    test.pass()
                }
            "#,
        r#"
                const TITLE: ptr<u8> = "EZ";

                fn main() {
                    *(TITLE) = 'N'
                    test.pass()
                }
            "#,
        r#"
                global title_copy: ptr<u8> = "EZ";

                fn main() {
                    *(title_copy + 1) = 'X'
                    test.pass()
                }
            "#,
    ];

    for source in cases {
        let program = parse_program(Path::new("game.ezra"), source).unwrap();
        let error = emit_ez80_assembly(&program).unwrap_err();

        assert_eq!(error.message, "string literal is read-only");
    }
}

#[test]
fn allows_reassigned_global_readonly_pointer_aliases_to_mutable_memory() {
    let source = r#"
            embed sprite: bytes = bytes [0x11, 0x22]
            global p: ptr<u8> = sprite.ptr
            global text: ptr<u8> = "OK"

            fn main() {
                p = cast<ptr<u8>>(0x040120);
                text = cast<ptr<u8>>(0x040121);
                *(p) = 0x33;
                *(text) = 0x44
                test.assert_eq_u8(*(cast<ptr<u8>>(0x040120)), 0x33, 1)
                test.assert_eq_u8(*(cast<ptr<u8>>(0x040121)), 0x44, 2)
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
fn emits_and_runs_debug_str_builtin() {
    let source = r#"
            global title: ptr<u8> = "EZ"

            fn main() {
                debug.str("OK")
                debug.char(' ')
                ezra.debug.str(title)
                test.pass()
            }
        "#;
    let program = parse_program(Path::new("game.ezra"), source).unwrap();
    let asm = emit_ez80_assembly(&program).unwrap();
    let run = run_assembly_test(&asm, 4_000).unwrap();

    assert!(asm.contains("out0 (0Ch), a"), "{asm}");
    assert!(run.halted, "{asm}");
    assert_eq!(run.result_code, 0, "{asm}");
    assert_eq!(run.debug_output, b"OK EZ", "{asm}");
}

#[test]
fn emits_and_runs_debug_hex_builtins() {
    let source = r#"
            fn main() {
                let byte: u8 = 0xAF;
                let word: u16 = 0x1234;
                let addr: u24 = 0x00BEEF;
                debug.hex_u8(byte)
                debug.char(' ')
                ezra.debug.hex_u16(word)
                debug.char(' ')
                debug.hex_u24(addr)
                test.pass()
            }
        "#;
    let program = parse_program(Path::new("game.ezra"), source).unwrap();
    let asm = emit_ez80_assembly(&program).unwrap();
    let run = run_assembly_test(&asm, 8_000).unwrap();

    assert!(asm.contains("srl a"), "{asm}");
    assert!(run.halted, "{asm}");
    assert_eq!(run.result_code, 0, "{asm}");
    assert_eq!(run.debug_output, b"AF 1234 00BEEF", "{asm}");
}

#[test]
fn emits_and_runs_generic_mmio_peek_poke_examples() {
    let source = r#"
            volatile mmio SCRATCH: ptr<u8> = 0x040120
            volatile mmio TI_LCD_BUFFER: ptr<u8> = 0x080000
            volatile mmio AGON_VDP_BUFFER: ptr<u8> = 0x0C0000

            fn ti_write(value: u8) {
                *(TI_LCD_BUFFER) = value;
            }

            fn agon_write(value: u8) {
                *(AGON_VDP_BUFFER) = value;
            }

            fn main() {
                let ptr: ptr<u8> = cast<ptr<u8>>(0x040121);
                *(SCRATCH) = 0x5A;
                *ptr = *SCRATCH + 1;
                ti_write(*ptr);
                agon_write(0xC3);
                test.assert_eq_u8(*SCRATCH, 0x5A, 1);
                test.assert_eq_u8(*ptr, 0x5B, 2);
                test.assert_eq_u8(*TI_LCD_BUFFER, 0x5B, 3);
                test.assert_eq_u8(*AGON_VDP_BUFFER, 0xC3, 4);
                test.pass()
            }
        "#;
    let program = parse_program(Path::new("game.ezra"), source).unwrap();
    let asm = emit_ez80_assembly(&program).unwrap();
    let run = run_assembly_test(&asm, 4_000).unwrap();

    assert!(asm.contains("ld a, (hl)"), "{asm}");
    assert!(asm.contains("ld (hl), a"), "{asm}");
    assert!(run.halted, "{asm}");
    assert_eq!(run.result_code, 0, "{asm}");
}

#[test]
fn emits_full_width_discarded_volatile_mmio_loads() {
    let source = r#"
            volatile mmio STATUS16: ptr<u16> = 0x040180
            volatile mmio STATUS24: ptr<u24> = 0x040190

            fn main() {
                *STATUS16;
                *STATUS24;
                test.pass()
            }
        "#;
    let program = parse_program(Path::new("game.ezra"), source).unwrap();
    let asm = emit_ez80_assembly_with_debug_comments(&program, true).unwrap();
    let run = run_assembly_test(&asm, 4_000).unwrap();
    let status16 = asm
        .split("; source: *STATUS16")
        .nth(1)
        .and_then(|tail| tail.split("; source: *STATUS24").next())
        .unwrap();
    let status24 = asm
        .split("; source: *STATUS24")
        .nth(1)
        .and_then(|tail| tail.split("; source: test.pass()").next())
        .unwrap();

    assert!(status16.contains("    inc hl"), "{asm}");
    assert_eq!(status24.matches("    inc hl").count(), 2, "{asm}");
    assert!(run.halted, "{asm}");
    assert_eq!(run.result_code, 0, "{asm}");
}

#[test]
fn emits_and_runs_wide_comparisons() {
    let source = r#"
            fn main() {
                let a: u16 = 0x0100
                let b: u16 = 0x0200
                test.assert_eq_u8(a < b, 1, 1)
                test.assert_eq_u8(b > a, 1, 2)
                test.assert_eq_u8(a >= b, 0, 3)
                test.assert_eq_u8(a != b, 1, 4)

                let c: u24 = 0x010000
                let d: u24 = 0x010000
                let e: u24 = 0x020000
                test.assert_eq_u8(c == d, 1, 5)
                test.assert_eq_u8(c <= d, 1, 6)
                test.assert_eq_u8(e <= c, 0, 7)

                let count: u8 = 0
                while c < e {
                    c += 0x008000
                    count += 1
                }
                if c >= e {
                    count += 1
                }
                test.assert_eq_u8(count, 3, 8)
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
fn emits_and_runs_comparisons_with_fitting_untyped_literals() {
    let source = r#"
            const SMALL_NEG: i8 = -2
            const CONST_SIGNED_LT: bool = SMALL_NEG < -1
            const CONST_UNSIGNED_EQ: bool = 5 == 5

            fn main() {
                let a: i8 = -2
                test.assert_eq_u8(a < -1, 1, 1)
                test.assert_eq_u8(a >= -2, 1, 2)

                let b: i16 = -300
                test.assert_eq_u8(b < -1, 1, 3)
                test.assert_eq_u8(-301 <= b, 1, 4)

                let c: i24 = -0x012345
                test.assert_eq_u8(c < -1, 1, 5)
                test.assert_eq_u8(-0x012345 == c, 1, 6)

                let d: u8 = 7
                test.assert_eq_u8(d == 7, 1, 7)
                test.assert_eq_u8(7 <= d, 1, 8)

                test.assert_eq_u8(CONST_SIGNED_LT, 1, 9)
                test.assert_eq_u8(CONST_UNSIGNED_EQ, 1, 10)
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
fn emits_and_runs_signed_comparisons() {
    let source = r#"
            alias subpx = i24

            fn main() {
                let a: i8 = -1
                let b: i8 = 1
                let c: i8 = -2
                test.assert_eq_u8(a < b, 1, 1)
                test.assert_eq_u8(b > a, 1, 2)
                test.assert_eq_u8(c < a, 1, 3)
                test.assert_eq_u8(a >= c, 1, 4)

                let d: i16 = -300
                let e: i16 = 7
                let f: i16 = -301
                test.assert_eq_u8(d < e, 1, 5)
                test.assert_eq_u8(e <= d, 0, 6)
                test.assert_eq_u8(f <= d, 1, 7)
                test.assert_eq_u8(d != f, 1, 8)

                let g: subpx = -0x010000
                let h: subpx = 0x000100
                let i: subpx = -0x020000
                test.assert_eq_u8(g < h, 1, 9)
                test.assert_eq_u8(h >= g, 1, 10)
                test.assert_eq_u8(i < g, 1, 11)
                test.assert_eq_u8(g == g, 1, 12)

                let min: subpx = -0x800000
                let max: subpx = 0x7FFFFF
                test.assert_eq_u8(min < max, 1, 13)
                test.assert_eq_u8(max > min, 1, 14)
                test.pass()
            }
        "#;
    let program = parse_program(Path::new("game.ezra"), source).unwrap();
    let asm = emit_ez80_assembly(&program).unwrap();
    let run = run_assembly_test(&asm, 6_000).unwrap();

    assert!(run.halted, "{asm}");
    assert_eq!(run.result_code, 0, "{asm}");
}
