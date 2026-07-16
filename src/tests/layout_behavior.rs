use super::*;

#[test]
fn commands_report_source_locations_for_layout_errors() {
    let root = temp_root("layout_diagnostics");
    std::fs::create_dir_all(&root).unwrap();
    let source_path = root.join("game.ezra");
    let parse_layout_path = root.join("parse.ezralayout");
    let invalid_layout_path = root.join("invalid.ezralayout");
    std::fs::write(&source_path, "fn main() { test.pass() }\n").unwrap();
    std::fs::write(
        &parse_layout_path,
        r#"
                layout broken {
                    load 0x010000;
            "#,
    )
    .unwrap();
    std::fs::write(
        &invalid_layout_path,
        r#"
                layout invalid {
                    load 0x010000;
                    entry 0x010040;
                    stack 0xF00000;

                    region code 0x010000..0x01FFFF read execute;
                    region overlap 0x018000..0x02FFFF read;
                    section .text -> code align 24;
                }
            "#,
    )
    .unwrap();

    let parse_prefix = format!("{}:1:1:", parse_layout_path.display());
    let parse_error = emit_assembly_with_command_options(&CommandOptions {
        path: source_path.to_string_lossy().into_owned(),
        debug_comments: false,
        default_sdk_symbols: true,
        layout_path: Some(parse_layout_path.to_string_lossy().into_owned()),
        target: None,
    })
    .unwrap_err();
    assert!(
        parse_error.starts_with(&parse_prefix),
        "expected `{parse_error}` to start with `{parse_prefix}`"
    );

    let invalid_prefix = format!("{}:1:1:", invalid_layout_path.display());
    let invalid_error = emit_assembly_with_command_options(&CommandOptions {
        path: source_path.to_string_lossy().into_owned(),
        debug_comments: false,
        default_sdk_symbols: true,
        layout_path: Some(invalid_layout_path.to_string_lossy().into_owned()),
        target: None,
    })
    .unwrap_err();
    assert!(
        invalid_error.contains(&invalid_prefix),
        "expected `{invalid_error}` to contain `{invalid_prefix}`"
    );
    assert!(
        invalid_error.contains("section `.text` alignment must be a power of two"),
        "{invalid_error}"
    );
    assert!(
        invalid_error.contains("regions `code` and `overlap` overlap"),
        "{invalid_error}"
    );

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn commands_reject_custom_layouts_missing_required_sections() {
    let root = temp_root("layout_missing_required_sections");
    std::fs::create_dir_all(&root).unwrap();
    let source_path = root.join("game.ezra");
    let layout_path = root.join("game.ezralayout");
    std::fs::write(&source_path, "fn main() { test.pass() }\n").unwrap();
    std::fs::write(
        &layout_path,
        r#"
                layout incomplete {
                    load 0x020000;
                    entry 0x020040;
                    stack 0xEFFE00;

                    region code 0x020000..0x02FFFF read execute;
                    section .header -> code align 64;
                    section .text -> code align 16;
                }
            "#,
    )
    .unwrap();

    let error = check(&CommandOptions {
        path: source_path.to_string_lossy().into_owned(),
        debug_comments: false,
        default_sdk_symbols: true,
        layout_path: Some(layout_path.to_string_lossy().into_owned()),
        target: None,
    })
    .unwrap_err();
    let prefix = format!("layout is invalid:\n{}:1:1:", layout_path.display());
    assert!(
        error.starts_with(&prefix),
        "expected `{error}` to start with `{prefix}`"
    );
    for section in [".rodata", ".data", ".bss", ".assets", ".scratch"] {
        let diagnostic = format!("layout is missing required section `{section}`");
        assert!(
            error.contains(&diagnostic),
            "expected `{error}` to contain `{diagnostic}`"
        );
    }

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn zxspectrum_target_uses_spectrum_layout() {
    let root = temp_root("z80_default_layout");
    std::fs::create_dir_all(&root).unwrap();
    let source_path = root.join("game.ezra");
    std::fs::write(&source_path, "fn main() {}\n").unwrap();

    let settings = resolve_build_settings(
        &CommandOptions {
            path: source_path.to_string_lossy().into_owned(),
            debug_comments: false,
            default_sdk_symbols: true,
            layout_path: None,
            target: Some("zxspectrum-z80".to_owned()),
        },
        &source_path,
    )
    .unwrap();

    assert_eq!(settings.target.triple.cpu, CpuFamily::Z80);
    assert_eq!(settings.target.memory.pointer_width_bits, 16);
    assert_eq!(settings.target.memory.address_width_bits, 16);
    assert_eq!(settings.layout.name, "zx_spectrum_z80");
    assert_eq!(settings.layout.load.get(), 0x8000);
    assert_eq!(settings.layout.entry.get(), 0x8000);
    assert!(
        settings
            .layout
            .symbols
            .iter()
            .any(|symbol| symbol.name == "ZX_SCREEN_BASE" && symbol.value.get() == 0x4000)
    );
    assert!(
        settings
            .layout
            .regions
            .iter()
            .all(|region| region.end.get() <= 0xFFFF)
    );

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn ti_ce_targets_use_tice_layout_and_sdk() {
    for (target, expected_layout) in [
        ("ti84plusce-ez80", "ti84plusce-ez80_layout"),
        ("ti83premiumce-ez80", "ti83premiumce-ez80_layout"),
    ] {
        let root = temp_root(target);
        std::fs::create_dir_all(&root).unwrap();
        let source_path = root.join("game.ezra");
        std::fs::write(
            &source_path,
            r#"
                    import tice.os
                    import tice.lcd

                    fn main() {
                        lcd.set_first_pixel(4)
                        os.idle()
                    }
                "#,
        )
        .unwrap();

        let outputs = build_source_with_command_options(&CommandOptions {
            path: source_path.to_string_lossy().into_owned(),
            debug_comments: false,
            default_sdk_symbols: false,
            layout_path: None,
            target: Some(target.to_owned()),
        })
        .unwrap();
        let settings = resolve_build_settings(
            &CommandOptions {
                path: source_path.to_string_lossy().into_owned(),
                debug_comments: false,
                default_sdk_symbols: false,
                layout_path: None,
                target: Some(target.to_owned()),
            },
            &source_path,
        )
        .unwrap();
        let asm = std::fs::read_to_string(outputs.asm).unwrap();
        let map = std::fs::read_to_string(outputs.map).unwrap();
        let bin = std::fs::read(&outputs.executable).unwrap();

        assert_eq!(settings.layout.name, expected_layout);
        assert_eq!(settings.output_format, OutputFormat::Ti8xp);
        assert_eq!(settings.layout.entry.get(), 0xD1_A881);
        assert!(asm.contains("; target: eZ80 ADL mode"), "{asm}");
        assert!(!asm.contains("    di\n"), "{asm}");
        assert!(!asm.contains("    ld sp,"), "{asm}");
        assert!(asm.contains("__ezra_exit:\n    ret"), "{asm}");
        assert!(asm.contains("ld (0D40000h), a"), "{asm}");
        assert!(map.contains(".text        0xD1A881"), "{map}");
        assert_eq!(
            outputs.executable.extension().and_then(|ext| ext.to_str()),
            Some("8xp")
        );
        assert_ti8xp(&bin, b"GAME\0\0\0\0", &[0xEF, 0x7B]);

        let _ = std::fs::remove_dir_all(root);
    }
}

#[test]
fn ti_z80_targets_use_ti_layout_and_sdk() {
    for (target, expected_layout) in [
        ("ti83-z80", "ti83-z80_layout"),
        ("ti83plus-z80", "ti83plus-z80_layout"),
        ("ti84-z80", "ti84-z80_layout"),
        ("ti84plus-z80", "ti84plus-z80_layout"),
    ] {
        let root = temp_root(target);
        std::fs::create_dir_all(&root).unwrap();
        let source_path = root.join("game.ezra");
        std::fs::write(
            &source_path,
            r#"
                    import ti.os
                    import ti.lcd

                    fn main() {
                        lcd.set_first_byte(4)
                        os.idle()
                    }
                "#,
        )
        .unwrap();

        let outputs = build_source_with_command_options(&CommandOptions {
            path: source_path.to_string_lossy().into_owned(),
            debug_comments: false,
            default_sdk_symbols: false,
            layout_path: None,
            target: Some(target.to_owned()),
        })
        .unwrap();
        let settings = resolve_build_settings(
            &CommandOptions {
                path: source_path.to_string_lossy().into_owned(),
                debug_comments: false,
                default_sdk_symbols: false,
                layout_path: None,
                target: Some(target.to_owned()),
            },
            &source_path,
        )
        .unwrap();
        let asm = std::fs::read_to_string(outputs.asm).unwrap();
        let map = std::fs::read_to_string(outputs.map).unwrap();
        let bin = std::fs::read(&outputs.executable).unwrap();

        assert_eq!(settings.target.triple.cpu, CpuFamily::Z80);
        assert_eq!(settings.output_format, OutputFormat::Ti8xp);
        assert_eq!(settings.layout.name, expected_layout);
        assert_eq!(settings.layout.entry.get(), 0x9D95);
        assert!(asm.contains("; target: Z80"), "{asm}");
        assert!(!asm.contains("    di\n"), "{asm}");
        assert!(!asm.contains("    ld sp,"), "{asm}");
        assert!(asm.contains("__ezra_exit:\n    ret"), "{asm}");
        assert!(asm.contains("ld (9340h), a"), "{asm}");
        assert!(map.contains(".text        0x009D95"), "{map}");
        assert_eq!(
            outputs.executable.extension().and_then(|ext| ext.to_str()),
            Some("8xp")
        );
        assert_ti8xp(&bin, b"GAME\0\0\0\0", &[0xBB, 0x6D]);

        let _ = std::fs::remove_dir_all(root);
    }
}

#[test]
fn cpm_z80_target_uses_com_layout() {
    let root = temp_root("cpm_z80_layout");
    std::fs::create_dir_all(&root).unwrap();
    let source_path = root.join("game.ezra");
    std::fs::write(&source_path, "fn main() {}\n").unwrap();

    let settings = resolve_build_settings(
        &CommandOptions {
            path: source_path.to_string_lossy().into_owned(),
            debug_comments: false,
            default_sdk_symbols: true,
            layout_path: None,
            target: Some("cpm-2.2-z80".to_owned()),
        },
        &source_path,
    )
    .unwrap();

    assert_eq!(settings.target.output_format, OutputFormat::CpmCom);
    assert_eq!(settings.layout.name, "cpm_z80_com");
    assert_eq!(settings.layout.load.get(), 0x0100);
    assert_eq!(settings.layout.entry.get(), 0x0100);

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn ez80_split_harness_target_uses_split_layout_and_memory() {
    let root = temp_root("ez80_split_harness");
    std::fs::create_dir_all(&root).unwrap();
    let source_path = root.join("game.ezra");
    std::fs::write(
        &source_path,
        r#"
                global marker: u8 = 0x42

                fn main() {
                    marker = marker + 1
                    test.assert_eq_u24(EZRA_RAM_BASE, 0x100000, 1)
                    test.assert_eq_u24(EZRA_STACK_TOP, 0x1FFF00, 2)
                    test.assert_eq_u8(marker, 0x43, 3)
                    test.pass()
                }
            "#,
    )
    .unwrap();

    test_source_with_command_options(&CommandOptions {
        path: source_path.to_string_lossy().into_owned(),
        debug_comments: false,
        default_sdk_symbols: true,
        layout_path: None,
        target: Some("ezra-test-split-ez80".to_owned()),
    })
    .unwrap();

    let outputs = build_source_with_command_options(&CommandOptions {
        path: source_path.to_string_lossy().into_owned(),
        debug_comments: false,
        default_sdk_symbols: true,
        layout_path: None,
        target: Some("ezra-test-split-ez80".to_owned()),
    })
    .unwrap();
    let map = std::fs::read_to_string(outputs.map).unwrap();
    assert!(map.contains(".text        0x020040"), "{map}");
    assert!(
        map.contains(".data        0x100000 0x100000 0x000001"),
        "{map}"
    );

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn z80_target_rejects_layout_addresses_above_16_bit_space() {
    let root = temp_root("z80_layout_diagnostic");
    std::fs::create_dir_all(&root).unwrap();
    let source_path = root.join("game.ezra");
    let layout_path = root.join("game.ezralayout");
    std::fs::write(
        &source_path,
        r#"
                @cfg(cpu("z80"))
                fn main() {}
            "#,
    )
    .unwrap();
    std::fs::write(
        &layout_path,
        r#"
                layout too_large {
                    load 0x010000;
                    entry 0x010040;
                    stack 0x01FF00;

                    region header 0x010000..0x01003F read;
                    region code 0x010040..0x017FFF read execute;
                    region rodata 0x018000..0x019FFF read;
                    region ram 0x01A000..0x01BFFF read write;
                    region assets 0x01C000..0x01DFFF read;
                    region scratch 0x01E000..0x01EFFF read write;
                    region stack 0x01F000..0x01FFFF read write reserved;

                    section .header -> header align 1;
                    section .text -> code align 16;
                    section .rodata -> rodata align 16;
                    section .data -> ram align 16;
                    section .bss -> ram align 16;
                    section .assets -> assets align 256;
                    section .scratch -> scratch align 16;
                }
            "#,
    )
    .unwrap();

    let error = check(&CommandOptions {
        path: source_path.to_string_lossy().into_owned(),
        debug_comments: false,
        default_sdk_symbols: true,
        layout_path: Some(layout_path.to_string_lossy().into_owned()),
        target: Some("zxspectrum-z80".to_owned()),
    })
    .unwrap_err();

    assert!(
        error.contains("requires addresses outside the 16-bit address space"),
        "{error}"
    );
    assert!(error.contains("load address 0x010000"), "{error}");

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn test_command_can_use_custom_layout_file() {
    let root = temp_root("custom_layout_test");
    std::fs::create_dir_all(&root).unwrap();
    let source_path = root.join("game.ezra");
    let layout_path = root.join("game.ezralayout");
    std::fs::write(
        &source_path,
        r#"
                embed banked: bytes = bytes [0xA1, 0xA2] section .bank1 align 256
                embed banked2: bytes = bytes [0xB1] section .bank2 align 256
                global marker: u8 = 0x42

                fn main() {
                    test.assert_eq_u8(marker, 0x42, 1)
                    test.assert_eq_u24(EZRA_RAM_BASE, 0x030000, 2)
                    test.assert_eq_u24(EZRA_VRAM_BASE, 0x090000, 3)
                    test.assert_eq_u24(EZRA_CODE_BASE, 0x020040, 4)
                    test.assert_eq_u24(cast<ptr24>(banked.ptr), 0x120000, 5)
                    test.assert_eq_u8(*(banked.ptr + 1), 0xA2, 6)
                    test.assert_eq_u24(cast<ptr24>(banked2.ptr), 0x120100, 7)
                    test.assert_eq_u8(*(banked2.ptr), 0xB1, 8)
                    test.pass()
                }
            "#,
    )
    .unwrap();
    std::fs::write(
        &layout_path,
        r#"
                layout custom_test {
                    load 0x020000;
                    entry 0x020040;
                    stack 0xEFFE80;

                    region code 0x020000..0x02FFFF read execute;
                    region ram 0x030000..0x03FFFF read write;
                    region rodata 0x040000..0x04FFFF read;
                    region bank 0x120000..0x12FFFF read;
                    region scratch 0xE00000..0xE0FFFF read write;
                    section .header -> code align 64;
                    section .text -> code align 16;
                    section .rodata -> rodata align 16;
                    section .data -> ram align 16;
                    section .bss -> ram align 16;
                    section .assets -> bank align 256;
                    section .scratch -> scratch align 16;
                    section .bank1 -> bank align 256;
                    section .bank2 -> bank align 256;

                    symbol EZRA_RAM_BASE = 0x030000;
                    symbol EZRA_VRAM_BASE = 0x090000;
                }
            "#,
    )
    .unwrap();

    test_source_with_command_options(&CommandOptions {
        path: source_path.to_string_lossy().into_owned(),
        debug_comments: false,
        default_sdk_symbols: true,
        layout_path: Some(layout_path.to_string_lossy().into_owned()),
        target: None,
    })
    .unwrap();

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn check_command_can_use_custom_layout_file() {
    let root = temp_root("custom_layout_check");
    std::fs::create_dir_all(&root).unwrap();
    let source_path = root.join("game.ezra");
    let layout_path = root.join("game.ezralayout");
    std::fs::write(
        &source_path,
        r#"
                fn main() {
                    test.assert_eq_u24(EZRA_RAM_BASE, 0x030000, 1)
                    test.assert_eq_u24(EZRA_STACK_TOP, 0xEFFE00, 2)
                    test.pass()
                }
            "#,
    )
    .unwrap();
    std::fs::write(
        &layout_path,
        r#"
                layout check_custom {
                    load 0x020000;
                    entry 0x020040;
                    stack 0xEFFE00;

                    region code 0x020000..0x02FFFF read execute;
                    region ram 0x030000..0x03FFFF read write;
                    region rodata 0x040000..0x04FFFF read;
                    region assets 0x120000..0x12FFFF read;
                    region scratch 0xE00000..0xE0FFFF read write;
                    section .header -> code align 64;
                    section .text -> code align 16;
                    section .rodata -> rodata align 16;
                    section .data -> ram align 16;
                    section .bss -> ram align 16;
                    section .assets -> assets align 256;
                    section .scratch -> scratch align 16;

                    symbol EZRA_RAM_BASE = 0x030000;
                }
            "#,
    )
    .unwrap();

    check(&CommandOptions {
        path: source_path.to_string_lossy().into_owned(),
        debug_comments: false,
        default_sdk_symbols: true,
        layout_path: Some(layout_path.to_string_lossy().into_owned()),
        target: None,
    })
    .unwrap();

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn agon_mos_target_uses_builtin_sdk_and_layout() {
    let root = temp_root("agon_builtin_sdk");
    std::fs::create_dir_all(root.join("src")).unwrap();
    let source_path = root.join("src/main.ezra");
    std::fs::write(
        root.join("Ezra.toml"),
        r#"
                [build]
                target = "agonlight-mos-ez80"
            "#,
    )
    .unwrap();
    std::fs::write(
        &source_path,
        r#"
                import agon.vdp

                fn main() {
                    vdp.clear_screen()
                    vdp.vdu(65)
                    vdp.vdp_exit_emulator(0)
                }
            "#,
    )
    .unwrap();

    let outputs = build_source(source_path.to_str().unwrap()).unwrap();
    let map = std::fs::read_to_string(&outputs.map).unwrap();
    let asm = std::fs::read_to_string(&outputs.asm).unwrap();
    let bin = std::fs::read(&outputs.executable).unwrap();

    assert!(map.contains(".text        0x040045"), "{map}");
    assert!(asm.contains("rst.lis 10h"), "{asm}");
    assert!(asm.contains("out0 (00h), a"), "{asm}");
    assert_eq!(&bin[0..4], &[0xC3, 0x45, 0x00, 0x04]);
    assert_eq!(&bin[64..69], b"MOS\0\x01");

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn build_can_use_custom_layout_file() {
    let root = temp_root("custom_layout_build");
    std::fs::create_dir_all(&root).unwrap();
    let source_path = root.join("game.ezra");
    let layout_path = root.join("game.ezralayout");
    std::fs::write(
        &source_path,
        r#"
                global marker: u8 = 0x5A
                fn main() {
                    test.assert_eq_u8(marker, 0x5A, 1)
                    test.assert_eq_u24(EZRA_RAM_BASE, 0x030000, 2)
                    test.assert_eq_u24(EZRA_AUDIO_BASE, 0x0D0000, 3)
                    test.assert_eq_u24(EZRA_CODE_BASE, 0x020080, 4)
                    test.pass()
                }
            "#,
    )
    .unwrap();
    std::fs::write(
        &layout_path,
        r#"
                layout custom {
                    load 0x020000;
                    entry 0x020040;
                    stack 0xEFFF00;

                    region code 0x020000..0x02FFFF read execute;
                    region ram 0x030000..0x03FFFF read write;
                    region rodata 0x040000..0x04FFFF read;
                    region assets 0x120000..0x12FFFF read;
                    region scratch 0xE00000..0xE0FFFF read write;
                    section .header -> code align 64;
                    section .text -> code align 16;
                    section .rodata -> rodata align 16;
                    section .data -> ram align 16;
                    section .bss -> ram align 16;
                    section .assets -> assets align 256;
                    section .scratch -> scratch align 16;

                    symbol EZRA_LOAD_ADDR = 0x020000;
                    symbol EZRA_ENTRY_ADDR = 0x020040;
                    symbol EZRA_CODE_BASE = 0x020000 + cast<u8>(0x0180);
                    symbol EZRA_STACK_TOP = 0xEFFEFF + cast<bool>(0x1234);
                    symbol EZRA_RAM_BASE = 0x020000 + cast<ptr<u8>>(0x1010000);
                    symbol EZRA_AUDIO_BASE = 0x0CFF00 + cast<u16>(0x010100);
                }
            "#,
    )
    .unwrap();

    let outputs = build_source_with_command_options(&CommandOptions {
        path: source_path.to_string_lossy().into_owned(),
        debug_comments: false,
        default_sdk_symbols: true,
        layout_path: Some(layout_path.to_string_lossy().into_owned()),
        target: None,
    })
    .unwrap();

    let map = std::fs::read_to_string(&outputs.map).unwrap();
    let asm = std::fs::read_to_string(&outputs.asm).unwrap();
    let bin = std::fs::read(&outputs.executable).unwrap();

    assert!(
        map.starts_with("section      start      end        size\n"),
        "{map}"
    );
    assert!(map.contains(".text        0x020040"), "{map}");
    assert!(asm.contains("    ld sp, EFFF00h"), "{asm}");
    assert!(asm.contains("    ld (030000h), a"), "{asm}");
    assert!(!asm.contains("    ld (040000h), a"), "{asm}");
    assert_eq!(
        outputs.executable.extension().and_then(|ext| ext.to_str()),
        Some("bin")
    );
    assert_eq!(&bin[0..5], &[0xF3, 0x31, 0x00, 0xFF, 0xEF]);

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn emit_asm_can_use_custom_layout_file() {
    let root = temp_root("custom_layout_emit");
    std::fs::create_dir_all(&root).unwrap();
    let source_path = root.join("game.ezra");
    let layout_path = root.join("game.ezralayout");
    std::fs::write(&source_path, "fn main() { test.pass() }\n").unwrap();
    std::fs::write(
        &layout_path,
        r#"
                layout custom {
                    load 0x020000;
                    entry 0x020040;
                    stack 0xEFFE00;

                    region code 0x020000..0x02FFFF read execute;
                    region rodata 0x040000..0x04FFFF read;
                    region ram 0x050000..0x05FFFF read write;
                    region assets 0x120000..0x12FFFF read;
                    region scratch 0xE00000..0xE0FFFF read write;
                    section .header -> code align 64;
                    section .text -> code align 16;
                    section .rodata -> rodata align 16;
                    section .data -> ram align 16;
                    section .bss -> ram align 16;
                    section .assets -> assets align 256;
                    section .scratch -> scratch align 16;
                }
            "#,
    )
    .unwrap();

    let asm = emit_assembly_with_command_options(&CommandOptions {
        path: source_path.to_string_lossy().into_owned(),
        debug_comments: false,
        default_sdk_symbols: true,
        layout_path: Some(layout_path.to_string_lossy().into_owned()),
        target: None,
    })
    .unwrap();

    assert!(asm.contains("    ld sp, EFFE00h"), "{asm}");

    let _ = std::fs::remove_dir_all(root);
}
