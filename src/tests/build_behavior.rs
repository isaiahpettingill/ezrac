use super::*;

#[test]
fn build_writes_artifacts() {
    let root = temp_root("build");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::create_dir_all(root.join("lib")).unwrap();
    let source_path = root.join("game.ezra");
    std::fs::write(
        root.join("lib/math.ezra"),
        "pub fn add_one(v: u8) -> u8 { return v + 1 }\n",
    )
    .unwrap();
    std::fs::write(
        root.join("lib/assets.ezra"),
        r#"
            pub const BASE: u8 = 2
            pub const LEN: u8 = BASE + 1
            pub const BYTE: u8 = 0x5A
            "#,
    )
    .unwrap();
    std::fs::write(
        &source_path,
        r#"
            import lib.math
            import lib.assets

            embed palette: bytes = bytes [0x11, 0x22]
            embed blob: bytes = repeat(assets.BYTE, assets.LEN)

            fn main() {
                let x: u8 = add_one(4)
                test.pass()
            }
            "#,
    )
    .unwrap();

    let outputs = build_source(source_path.to_str().unwrap()).unwrap();
    let asm = std::fs::read_to_string(&outputs.asm).unwrap();
    let map = std::fs::read_to_string(&outputs.map).unwrap();
    let bin = std::fs::read(&outputs.executable).unwrap();

    assert!(asm.contains("__ezra_start:"));
    assert!(asm.contains("_add_one:"));
    assert!(
        map.starts_with("section      start      end        size\n"),
        "{map}"
    );
    assert!(
        map.contains(".header      0x010000 0x01003F 0x000040"),
        "{map}"
    );
    assert!(map.contains(".text        0x010040"), "{map}");
    assert!(
        map.contains(".assets:palette 0x100000 0x100001 0x000002"),
        "{map}"
    );
    assert!(
        map.contains(".assets:blob 0x100100 0x100102 0x000003"),
        "{map}"
    );
    assert_eq!(
        outputs.executable.extension().and_then(|ext| ext.to_str()),
        Some("bin")
    );
    assert_eq!(&bin[0..5], &[0xF3, 0x31, 0x00, 0x00, 0xF0]);
    assert!(bin.len() > 5);

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn build_accepts_assembly_input_by_extension() {
    let root = temp_root("build_asm_extension");
    std::fs::create_dir_all(&root).unwrap();
    let source_path = root.join("exit.asm");
    std::fs::write(
        &source_path,
        r#"
            start:
                ld c, 00h
                call 0005h
            "#,
    )
    .unwrap();

    let outputs = build_source_with_command_options(&CommandOptions {
        path: source_path.to_string_lossy().into_owned(),
        debug_comments: false,
        default_sdk_symbols: true,
        layout_path: None,
        target: Some("cpm-2.2-z80".to_owned()),
    })
    .unwrap();

    let asm = std::fs::read_to_string(&outputs.asm).unwrap();
    let map = std::fs::read_to_string(&outputs.map).unwrap();
    let executable = std::fs::read(&outputs.executable).unwrap();

    assert!(asm.contains("start:"), "{asm}");
    assert!(map.contains(".text        0x000100"), "{map}");
    assert_eq!(
        outputs.executable.extension().and_then(|ext| ext.to_str()),
        Some("com")
    );
    assert_eq!(executable, [0x0E, 0x00, 0xCD, 0x05, 0x00]);

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn agon_mos_assembly_build_emits_mos_wrapper() {
    let root = temp_root("build_asm_agon_mos");
    std::fs::create_dir_all(&root).unwrap();
    let source_path = root.join("main.asm");
    std::fs::write(&source_path, "ret\n").unwrap();

    let outputs = build_source_with_build_options(&BuildCommandOptions {
        path: Some(source_path.to_string_lossy().into_owned()),
        debug_comments: false,
        default_sdk_symbols: true,
        input_kind: Some(InputKind::Assembly),
        assembler_cpu: None,
        layout_path: None,
        target: Some("agonlight-mos-ez80".to_owned()),
    })
    .unwrap();
    let bin = std::fs::read(outputs.executable).unwrap();

    assert_eq!(bin[0], 0xC3);
    assert_eq!(&bin[64..67], b"MOS");
    assert_eq!(bin[69], 0xC9);

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn agon_assembly_links_cross_section_symbols_and_preserves_sections() {
    let root = temp_root("build_asm_agon_sections");
    std::fs::create_dir_all(&root).unwrap();
    let source_path = root.join("main.asm");
    std::fs::write(
        &source_path,
        r#"section .header
                db 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0A5h
            section .text
            TEXT_DATA equ rodata_value
            start:
                ld hl, TEXT_DATA
                ld de, data_value
                ret
            section .rodata
            rodata_value:
                db 0AAh, 0BBh
            section .data
            data_value:
                db 0CCh, 0DDh
            section .bss
            bss_value:
                db 0EEh
            section .assets
            asset_value:
                db 0F0h
            "#,
    )
    .unwrap();

    let outputs = build_source_with_build_options(&BuildCommandOptions {
        path: Some(source_path.to_string_lossy().into_owned()),
        debug_comments: false,
        default_sdk_symbols: true,
        input_kind: Some(InputKind::Assembly),
        assembler_cpu: None,
        layout_path: None,
        target: Some("agonlight-mos-ez80".to_owned()),
    })
    .unwrap();
    let bin = std::fs::read(outputs.executable).unwrap();
    let map = std::fs::read_to_string(outputs.map).unwrap();

    assert_eq!(&bin[64..69], b"MOS\0\x01");
    assert_eq!(bin[10], 0xA5);
    assert_eq!(&bin[0x20_000..0x20_002], &[0xAA, 0xBB]);
    assert_eq!(&bin[0x30_000..0x30_002], &[0xCC, 0xDD]);
    assert_eq!(bin[0x30_010], 0xEE);
    assert_eq!(bin[0x80_000], 0xF0);
    assert!(map.contains("rodata_value 0x060000"), "{map}");
    assert!(map.contains("TEXT_DATA    0x060000"), "{map}");
    assert!(map.contains("data_value   0x070000"), "{map}");
    assert!(map.contains("bss_value    0x070010"), "{map}");
    assert_eq!(&bin[69..73], &[0x21, 0x00, 0x00, 0x06]);
    assert_eq!(&bin[73..77], &[0x11, 0x00, 0x00, 0x07]);

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn bare_z80_source_build_starts_at_zero_without_header() {
    let root = temp_root("bare_z80_source_bin");
    std::fs::create_dir_all(&root).unwrap();
    let source_path = root.join("main.ezra");
    std::fs::write(&source_path, "fn main() {}\n").unwrap();

    let outputs = build_source_with_build_options(&BuildCommandOptions {
        path: Some(source_path.to_string_lossy().into_owned()),
        debug_comments: false,
        default_sdk_symbols: true,
        input_kind: Some(InputKind::Ezra),
        assembler_cpu: None,
        layout_path: None,
        target: Some("bare-z80".to_owned()),
    })
    .unwrap();
    let map = std::fs::read_to_string(outputs.map).unwrap();
    let bin = std::fs::read(outputs.executable).unwrap();

    assert!(map.contains(".text        0x000000"), "{map}");
    assert!(!bin.starts_with(b"EZRA"), "{bin:02X?}");
    assert!(!bin.is_empty());

    let _ = std::fs::remove_dir_all(root);
}

#[cfg(feature = "mos6502")]
#[test]
fn generic_6502_source_build_writes_raw_binary() {
    let root = temp_root("generic_6502_source_bin");
    std::fs::create_dir_all(&root).unwrap();
    let source_path = root.join("main.ezra");
    std::fs::write(
        &source_path,
        r#"
            volatile mmio BORDER: ptr<u8> = 0xD020
            global frame: u16 = 0
            fn main() {
                frame += 1
                *(BORDER) = cast<u8>(frame)
            }
        "#,
    )
    .unwrap();

    let outputs = build_source_with_build_options(&BuildCommandOptions {
        path: Some(source_path.to_string_lossy().into_owned()),
        debug_comments: false,
        default_sdk_symbols: true,
        input_kind: Some(InputKind::Ezra),
        assembler_cpu: None,
        layout_path: None,
        target: Some("generic-6502-bare".to_owned()),
    })
    .unwrap();
    let assembly = std::fs::read_to_string(outputs.asm).unwrap();
    let map = std::fs::read_to_string(outputs.map).unwrap();
    let binary = std::fs::read(outputs.executable).unwrap();

    assert!(assembly.contains("; target: MOS 6502"), "{assembly}");
    assert!(map.contains(".text        0x000200"), "{map}");
    assert_eq!(&binary[..4], &[0xD8, 0xA2, 0xFF, 0x9A]);

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn bare_z80n_source_build_accepts_z80n_inline_asm() {
    let root = temp_root("bare_z80n_source_inline_asm");
    std::fs::create_dir_all(&root).unwrap();
    let source_path = root.join("main.ezra");
    std::fs::write(
        &source_path,
        r#"
                fn main() {
                    asm volatile {
                        "nextreg 12h,a"
                    }
                }
            "#,
    )
    .unwrap();

    let outputs = build_source_with_build_options(&BuildCommandOptions {
        path: Some(source_path.to_string_lossy().into_owned()),
        debug_comments: false,
        default_sdk_symbols: true,
        input_kind: Some(InputKind::Ezra),
        assembler_cpu: None,
        layout_path: None,
        target: Some("bare-z80n".to_owned()),
    })
    .unwrap();
    let asm = std::fs::read_to_string(outputs.asm).unwrap();
    let bin = std::fs::read(outputs.executable).unwrap();

    assert!(asm.contains("; target: z80n"), "{asm}");
    assert!(bin.windows(3).any(|bytes| bytes == [0xED, 0x92, 0x12]));

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn z80_source_build_rejects_z80n_inline_asm() {
    let root = temp_root("z80_rejects_z80n_inline_asm");
    std::fs::create_dir_all(&root).unwrap();
    let source_path = root.join("main.ezra");
    std::fs::write(
        &source_path,
        r#"
                fn main() {
                    asm volatile {
                        "nextreg 12h,a"
                    }
                }
            "#,
    )
    .unwrap();

    let error = build_source_with_build_options(&BuildCommandOptions {
        path: Some(source_path.to_string_lossy().into_owned()),
        debug_comments: false,
        default_sdk_symbols: true,
        input_kind: Some(InputKind::Ezra),
        assembler_cpu: None,
        layout_path: None,
        target: Some("bare-z80".to_owned()),
    })
    .unwrap_err();

    assert!(
        error.contains("test assembler does not support instruction `nextreg 12h,a`"),
        "{error}"
    );

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn bare_z180_source_build_accepts_z180_inline_asm() {
    let root = temp_root("bare_z180_source_inline_asm");
    std::fs::create_dir_all(&root).unwrap();
    let source_path = root.join("main.ezra");
    std::fs::write(
        &source_path,
        r#"
                fn main() {
                    asm volatile(clobber flags) {
                        "tst a"
                    }
                }
            "#,
    )
    .unwrap();

    let outputs = build_source_with_build_options(&BuildCommandOptions {
        path: Some(source_path.to_string_lossy().into_owned()),
        debug_comments: false,
        default_sdk_symbols: true,
        input_kind: Some(InputKind::Ezra),
        assembler_cpu: None,
        layout_path: None,
        target: Some("bare-z180".to_owned()),
    })
    .unwrap();
    let asm = std::fs::read_to_string(outputs.asm).unwrap();
    let bin = std::fs::read(outputs.executable).unwrap();

    assert!(asm.contains("; target: z180"), "{asm}");
    assert!(bin.windows(2).any(|bytes| bytes == [0xED, 0x3C]));

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn z80_family_source_builds_reject_z180_only_inline_asm() {
    for target in ["bare-z80", "bare-z80n"] {
        let root = temp_root(target);
        std::fs::create_dir_all(&root).unwrap();
        let source_path = root.join("main.ezra");
        std::fs::write(
            &source_path,
            r#"
                    fn main() {
                        asm volatile(clobber flags) {
                            "tst a"
                        }
                    }
                "#,
        )
        .unwrap();

        let error = build_source_with_build_options(&BuildCommandOptions {
            path: Some(source_path.to_string_lossy().into_owned()),
            debug_comments: false,
            default_sdk_symbols: true,
            input_kind: Some(InputKind::Ezra),
            assembler_cpu: None,
            layout_path: None,
            target: Some(target.to_owned()),
        })
        .unwrap_err();

        assert!(
            error.contains("test assembler does not support instruction `tst a`"),
            "{target}: {error}"
        );

        let _ = std::fs::remove_dir_all(root);
    }
}

#[test]
fn bare_i8080_source_build_emits_intel_assembly() {
    let root = temp_root("bare_i8080_source_build");
    std::fs::create_dir_all(&root).unwrap();
    let source_path = root.join("main.ezra");
    std::fs::write(&source_path, "fn main() { test.pass() }\n").unwrap();

    let outputs = build_source_with_build_options(&BuildCommandOptions {
        path: Some(source_path.to_string_lossy().into_owned()),
        debug_comments: false,
        default_sdk_symbols: true,
        input_kind: Some(InputKind::Ezra),
        assembler_cpu: None,
        layout_path: None,
        target: Some("bare-i8080".to_owned()),
    })
    .unwrap();
    let asm = std::fs::read_to_string(outputs.asm).unwrap();
    let bin = std::fs::read(outputs.executable).unwrap();

    assert!(asm.contains("; target: i8080"), "{asm}");
    assert!(asm.contains("    lxi sp,"), "{asm}");
    assert!(asm.contains("    call _main"), "{asm}");
    assert!(asm.contains("    out 0Dh"), "{asm}");
    assert!(!asm.contains("    ld "), "{asm}");
    assert!(!asm.contains("ldir"), "{asm}");
    assert!(!bin.is_empty());

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn bare_i8080_source_builds_core_language_program() {
    let root = temp_root("bare_i8080_source_core_language");
    std::fs::create_dir_all(&root).unwrap();
    let source_path = root.join("main.ezra");
    std::fs::write(
        &source_path,
        r#"
                fn main() {
                    let left: u8 = 2
                    let right: u8 = 3
                    let sum: u8 = left + right
                    if sum == 5 {
                        test.pass()
                    } else {
                        test.fail(1)
                    }
                }
            "#,
    )
    .unwrap();

    let outputs = build_source_with_build_options(&BuildCommandOptions {
        path: Some(source_path.to_string_lossy().into_owned()),
        debug_comments: false,
        default_sdk_symbols: true,
        input_kind: Some(InputKind::Ezra),
        assembler_cpu: None,
        layout_path: None,
        target: Some("bare-i8080".to_owned()),
    })
    .unwrap();
    let asm = std::fs::read_to_string(outputs.asm).unwrap();

    assert!(
        asm.contains("    adi ") || asm.contains("    add "),
        "{asm}"
    );
    assert!(asm.contains("    j"), "{asm}");
    assert!(!asm.contains("sbc hl"), "{asm}");
    assert!(!asm.contains("ldir"), "{asm}");

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn bare_i8085_source_build_accepts_i8085_inline_asm() {
    let root = temp_root("bare_i8085_source_inline_asm");
    std::fs::create_dir_all(&root).unwrap();
    let source_path = root.join("main.ezra");
    std::fs::write(
        &source_path,
        r#"
                fn main() {
                    asm volatile {
                        "rim"
                        "sim"
                    }
                }
            "#,
    )
    .unwrap();

    let outputs = build_source_with_build_options(&BuildCommandOptions {
        path: Some(source_path.to_string_lossy().into_owned()),
        debug_comments: false,
        default_sdk_symbols: true,
        input_kind: Some(InputKind::Ezra),
        assembler_cpu: None,
        layout_path: None,
        target: Some("bare-i8085".to_owned()),
    })
    .unwrap();
    let asm = std::fs::read_to_string(outputs.asm).unwrap();
    let bin = std::fs::read(outputs.executable).unwrap();

    assert!(asm.contains("; target: i8085"), "{asm}");
    assert!(bin.windows(2).any(|bytes| bytes == [0x20, 0x30]));

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn bare_i8080_source_build_rejects_i8085_inline_asm() {
    let root = temp_root("bare_i8080_rejects_i8085_inline_asm");
    std::fs::create_dir_all(&root).unwrap();
    let source_path = root.join("main.ezra");
    std::fs::write(
        &source_path,
        r#"
                fn main() {
                    asm volatile {
                        "rim"
                    }
                }
            "#,
    )
    .unwrap();

    let error = build_source_with_build_options(&BuildCommandOptions {
        path: Some(source_path.to_string_lossy().into_owned()),
        debug_comments: false,
        default_sdk_symbols: true,
        input_kind: Some(InputKind::Ezra),
        assembler_cpu: None,
        layout_path: None,
        target: Some("bare-i8080".to_owned()),
    })
    .unwrap_err();

    assert!(
        error.contains("test assembler does not support instruction `rim`"),
        "{error}"
    );

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn build_can_emit_debug_source_comments() {
    let root = temp_root("debug_build");
    std::fs::create_dir_all(&root).unwrap();
    let source_path = root.join("game.ezra");
    std::fs::write(
        &source_path,
        "fn main() { let x: u8 = 4; x += 1; test.pass() }\n",
    )
    .unwrap();

    let plain = build_source(source_path.to_str().unwrap()).unwrap();
    let plain_asm = std::fs::read_to_string(&plain.asm).unwrap();
    let debug = build_source_with_options(source_path.to_str().unwrap(), true).unwrap();
    let debug_asm = std::fs::read_to_string(&debug.asm).unwrap();

    assert!(!plain_asm.contains("; source:"), "{plain_asm}");
    assert!(debug_asm.contains("; source: let x: u8 = 4"), "{debug_asm}");
    assert!(debug_asm.contains("; source: x += 1"), "{debug_asm}");

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn commands_report_source_locations_for_semantic_errors() {
    let root = temp_root("command_diagnostics");
    std::fs::create_dir_all(&root).unwrap();
    let source_path = root.join("game.ezra");
    std::fs::write(
        &source_path,
        "fn main() { let value: u8 = 256; test.pass() }\n",
    )
    .unwrap();
    let prefix = format!("{}:1:29:", source_path.display());

    let build_error = build_source(source_path.to_str().unwrap()).unwrap_err();
    assert!(
        build_error.starts_with(&prefix),
        "expected `{build_error}` to start with `{prefix}`"
    );
    let emit_error = emit_assembly_with_command_options(&CommandOptions {
        path: source_path.to_string_lossy().into_owned(),
        debug_comments: false,
        default_sdk_symbols: true,
        layout_path: None,
        target: None,
    })
    .unwrap_err();
    assert!(
        emit_error.starts_with(&prefix),
        "expected `{emit_error}` to start with `{prefix}`"
    );
    let test_error = test_source(source_path.to_str().unwrap()).unwrap_err();
    assert!(
        test_error.starts_with(&prefix),
        "expected `{test_error}` to start with `{prefix}`"
    );

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn z80_source_check_accepts_16_bit_cfg() {
    let root = temp_root("z80_source_check");
    std::fs::create_dir_all(&root).unwrap();
    let source_path = root.join("game.ezra");
    std::fs::write(
        &source_path,
        r#"
                @cfg(pointer_width(16))
                fn main() {}
            "#,
    )
    .unwrap();

    check(&CommandOptions {
        path: source_path.to_string_lossy().into_owned(),
        debug_comments: false,
        default_sdk_symbols: true,
        layout_path: None,
        target: Some("zxspectrum-z80".to_owned()),
    })
    .unwrap();

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn commands_can_disable_default_sdk_symbols() {
    let root = temp_root("strict_sdk_symbols");
    std::fs::create_dir_all(&root).unwrap();
    let default_port_path = root.join("default_port.ezra");
    std::fs::write(
        &default_port_path,
        r#"
                fn main() {
                    let pad: u8 = in PAD1_LO
                    test.assert_eq_u8(pad, 0, 1)
                    test.pass()
                }
            "#,
    )
    .unwrap();

    let error = check(&CommandOptions {
        path: default_port_path.to_string_lossy().into_owned(),
        debug_comments: false,
        default_sdk_symbols: false,
        layout_path: None,
        target: None,
    })
    .unwrap_err();
    assert!(error.contains("unknown port `PAD1_LO`"), "{error}");

    let explicit_port_path = root.join("explicit_port.ezra");
    std::fs::write(
        &explicit_port_path,
        r#"
                // port 0x9B = 0x42
                port AGON_VDP: u8 = 0x9B

                fn main() {
                    let value: u8 = in AGON_VDP
                    test.assert_eq_u8(value, 0x42, 1)
                    test.pass()
                }
            "#,
    )
    .unwrap();

    test_source_with_command_options(&CommandOptions {
        path: explicit_port_path.to_string_lossy().into_owned(),
        debug_comments: false,
        default_sdk_symbols: false,
        layout_path: None,
        target: None,
    })
    .unwrap();

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn z80_source_rejects_24bit_literals_before_assembly() {
    let root = temp_root("z80_source_u24_literal");
    std::fs::create_dir_all(&root).unwrap();
    let source_path = root.join("game.ezra");
    std::fs::write(
        &source_path,
        r#"
                fn main() {
                    let value: u24 = 0x010000
                    test.pass()
                }
            "#,
    )
    .unwrap();

    let options = CommandOptions {
        path: source_path.to_string_lossy().into_owned(),
        debug_comments: false,
        default_sdk_symbols: true,
        layout_path: None,
        target: Some("zxspectrum-z80".to_owned()),
    };
    let error = test_source_with_command_options(&options).unwrap_err();

    assert!(
        error.contains("24-bit value 0x010000 cannot be encoded for 16-bit target `z80`"),
        "{error}"
    );
    assert!(!error.contains("<assembly>"), "{error}");

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn z80_source_emits_z80_assembly_without_ez80_adl_forms() {
    let root = temp_root("z80_source_asm");
    std::fs::create_dir_all(&root).unwrap();
    let source_path = root.join("game.ezra");
    std::fs::write(
        &source_path,
        r#"
                fn main() {
                    let a: u16 = 12
                    let b: u16 = 13
                    let c: u16 = a * b
                    test.assert_eq_u16(c, 156, 2)
                    test.pass()
                }
            "#,
    )
    .unwrap();

    let asm = emit_assembly_with_command_options(&CommandOptions {
        path: source_path.to_string_lossy().into_owned(),
        debug_comments: false,
        default_sdk_symbols: true,
        layout_path: None,
        target: Some("zxspectrum-z80".to_owned()),
    })
    .unwrap();

    assert!(asm.contains("; target: Z80"), "{asm}");
    assert!(asm.contains("ld sp, 5B00h"), "{asm}");
    assert!(asm.contains("out (0Dh), a"), "{asm}");
    assert!(!asm.contains("out0"), "{asm}");
    assert!(!asm.contains("rst.lis"), "{asm}");
    assert!(!asm.contains("mlt"), "{asm}");

    let _ = std::fs::remove_dir_all(root);
}
