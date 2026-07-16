use super::*;

fn zx_tap_blocks(tape: &[u8]) -> Vec<(u8, &[u8])> {
    let mut blocks = Vec::new();
    let mut offset = 0;
    while offset < tape.len() {
        let length = usize::from(u16::from_le_bytes([tape[offset], tape[offset + 1]]));
        let block = &tape[offset + 2..offset + 2 + length];
        blocks.push((block[0], &block[1..block.len() - 1]));
        offset += length + 2;
    }
    assert_eq!(offset, tape.len());
    blocks
}

#[test]
fn game_boy_targets_write_valid_dmg_and_cgb_roms() {
    use ez80::{Cpu, Machine, PlainMachine};

    let root = temp_root("game_boy_roms");
    std::fs::create_dir_all(&root).unwrap();
    let source_path = root.join("main.asm");
    std::fs::write(
        &source_path,
        "di\nld sp, 0FFFEh\nld a, 42h\nldh (80h), a\nhalt\n",
    )
    .unwrap();
    for (target, cgb_flag) in [
        ("gameboy-dmg-lr35902", 0x00),
        ("gameboy-color-lr35902", 0xC0),
    ] {
        let output = root.join(format!("{target}.gb"));
        assemble_file(&AssembleOptions {
            path: source_path.to_string_lossy().into_owned(),
            output: Some(output.to_string_lossy().into_owned()),
            base_addr: None,
            assembler_cpu: None,
            layout_path: None,
            map_path: None,
            target: Some(target.to_owned()),
        })
        .unwrap();
        let rom = std::fs::read(output).unwrap();
        assert_eq!(rom.len(), 0x8000);
        assert_eq!(&rom[0x0100..0x0104], &[0xC3, 0x50, 0x01, 0x00]);
        assert_eq!(rom[0x0143], cgb_flag);
        assert_eq!(&rom[0x0150..0x0155], &[0xF3, 0x31, 0xFE, 0xFF, 0x3E]);

        let mut machine = PlainMachine::new();
        for (address, byte) in rom.iter().copied().enumerate() {
            machine.poke(address as u32, byte);
        }
        let mut cpu = Cpu::new_gameboy();
        cpu.state.set_pc(0x0100);
        for _ in 0..16 {
            if cpu.is_halted() {
                break;
            }
            cpu.fast_execute_instruction(&mut machine);
        }
        assert!(
            cpu.is_halted(),
            "{target} ROM did not halt in Game Boy CPU mode"
        );
        assert_eq!(
            machine.peek(0xFF80),
            0x42,
            "{target} ROM did not execute LR35902 LDH semantics"
        );

        let header = rom[0x0134..=0x014C]
            .iter()
            .fold(0u8, |sum, byte| sum.wrapping_sub(*byte).wrapping_sub(1));
        assert_eq!(rom[0x014D], header);
        let global = rom
            .iter()
            .enumerate()
            .filter(|(index, _)| !matches!(*index, 0x014E | 0x014F))
            .fold(0u16, |sum, (_, byte)| sum.wrapping_add(u16::from(*byte)));
        assert_eq!(&rom[0x014E..0x0150], &global.to_be_bytes());
    }
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn game_boy_targets_compile_ezra_source_with_embedded_assets() {
    use ez80::{Cpu, Machine, PlainMachine};

    let root = temp_root("game_boy_ezra_source");
    std::fs::create_dir_all(&root).unwrap();
    let source_path = root.join("main.ezra");
    std::fs::write(
        &source_path,
        r#"
                embed tile: bytes = bytes [0x42, 0x18, 0x24, 0x42]

                fn main() {
                    asm volatile {
                        "ld hl, _tile"
                        "ld a, (hl)"
                        "ldh (80h), a"
                    }
                }
            "#,
    )
    .unwrap();

    for target in ["gameboy-dmg-lr35902", "gameboy-color-lr35902"] {
        let outputs = build_source_with_build_options(&BuildCommandOptions {
            path: Some(source_path.to_string_lossy().into_owned()),
            debug_comments: false,
            default_sdk_symbols: true,
            input_kind: Some(InputKind::Ezra),
            assembler_cpu: None,
            layout_path: None,
            target: Some(target.to_owned()),
        })
        .unwrap();
        let rom = std::fs::read(outputs.executable).unwrap();
        assert_eq!(rom.len(), 0x8000);

        let mut machine = PlainMachine::new();
        for (address, byte) in rom.iter().copied().enumerate() {
            machine.poke(address as u32, byte);
        }
        let mut cpu = Cpu::new_gameboy();
        cpu.state.set_pc(0x0100);
        for _ in 0..32 {
            if cpu.is_halted() {
                break;
            }
            cpu.fast_execute_instruction(&mut machine);
        }
        assert!(cpu.is_halted(), "{target} source ROM did not halt");
        assert_eq!(machine.peek(0xFF80), 0x42);
    }

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn game_boy_source_examples_build_as_roms() {
    let examples = Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/gameboy");
    for name in [
        "serial-hello",
        "background",
        "sprite",
        "input-audio",
        "color-input",
    ] {
        let source = examples.join(name).join("src/main.ezra");
        for (target, cgb_flag) in [
            ("gameboy-dmg-lr35902", 0x00),
            ("gameboy-color-lr35902", 0xC0),
        ] {
            let outputs = build_source_with_build_options(&BuildCommandOptions {
                path: Some(source.to_string_lossy().into_owned()),
                debug_comments: false,
                default_sdk_symbols: true,
                input_kind: Some(InputKind::Ezra),
                assembler_cpu: None,
                layout_path: None,
                target: Some(target.to_owned()),
            })
            .unwrap_or_else(|error| {
                panic!("failed to build Game Boy example `{name}` for `{target}`: {error}")
            });
            let expected_extension = if target.starts_with("gameboy-color-") {
                "gbc"
            } else {
                "gb"
            };
            assert_eq!(
                outputs
                    .executable
                    .extension()
                    .and_then(|value| value.to_str()),
                Some(expected_extension)
            );
            let rom = std::fs::read(outputs.executable).unwrap();
            assert_eq!(rom.len(), 0x8000);
            assert_eq!(
                rom[0x0143], cgb_flag,
                "wrong compatibility byte for {target}"
            );
        }
    }
}

#[cfg(feature = "mos6502")]
#[test]
fn commodore64_source_example_builds_as_prg() {
    let source =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/commodore64/hello/src/main.ezra");
    let outputs = build_source_with_build_options(&BuildCommandOptions {
        path: Some(source.to_string_lossy().into_owned()),
        debug_comments: false,
        default_sdk_symbols: true,
        input_kind: Some(InputKind::Ezra),
        assembler_cpu: None,
        layout_path: None,
        target: Some("commodore64-6502".to_owned()),
    })
    .unwrap();
    assert_eq!(
        outputs
            .executable
            .extension()
            .and_then(|value| value.to_str()),
        Some("prg")
    );
    let program = std::fs::read(outputs.executable).unwrap();
    assert!(program.len() > 2);
    assert_eq!(&program[..2], &0x0801u16.to_le_bytes());
    assert_eq!(
        &program[2..14],
        &[
            0x0B, 0x08, 0x0A, 0x00, 0x9E, b'2', b'0', b'6', b'1', 0x00, 0x00, 0x00
        ],
        "C64 PRG should include a BASIC `10 SYS2061` autostart loader"
    );
    assert_eq!(
        program[14], 0xD8,
        "C64 program should begin with CLD startup code after its loader"
    );
}

#[cfg(feature = "mos6502")]
#[test]
fn commodore64_crt_build_writes_a_standard_autostart_cartridge() {
    let root = temp_root("commodore64_crt");
    std::fs::create_dir_all(&root).unwrap();
    let source = root.join("main.ezra");
    std::fs::write(&source, "fn main() {}\n").unwrap();
    std::fs::write(
        root.join("Ezra.toml"),
        "[build]\ninput = \"main.ezra\"\ntarget = \"commodore64-6502\"\noutput = \"crt\"\n",
    )
    .unwrap();

    let outputs = build_source_with_build_options(&BuildCommandOptions::with_path(
        source.to_string_lossy().into_owned(),
        false,
    ))
    .unwrap();
    assert_eq!(
        outputs
            .executable
            .extension()
            .and_then(|value| value.to_str()),
        Some("crt")
    );
    let cartridge = std::fs::read(outputs.executable).unwrap();
    assert_eq!(&cartridge[..16], b"C64 CARTRIDGE   ");
    assert_eq!(&cartridge[0x40..0x44], b"CHIP");
    assert_eq!(
        &cartridge[0x50..0x59],
        &[0x09, 0x80, 0x09, 0x80, b'C', b'B', b'M', b'8', b'0']
    );
    assert_eq!(
        cartridge[0x59], 0xD8,
        "C64 cartridge entry should begin with CLD"
    );
}

#[test]
fn bare_source_build_can_emit_com_and_intel_hex() {
    for (name, output, extension, prefix) in [
        ("bare_z80_source_com", "com", "com", ""),
        ("bare_z80_source_hex", "hex", "hex", ":020000040000FA"),
    ] {
        let root = temp_root(name);
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(
            root.join("Ezra.toml"),
            format!(
                r#"
                    [project]
                    name = "bare-demo"

                    [build]
                    input = "main.ezra"
                    target = "bare-z80"
                    output = "{output}"
                    executable = "demo"
                    "#
            ),
        )
        .unwrap();
        let source_path = root.join("main.ezra");
        std::fs::write(&source_path, "fn main() {}\n").unwrap();

        let outputs = build_source(source_path.to_str().unwrap()).unwrap();
        let bytes = std::fs::read(&outputs.executable).unwrap();

        assert_eq!(
            outputs.executable.extension().and_then(|ext| ext.to_str()),
            Some(extension)
        );
        if !prefix.is_empty() {
            let text = String::from_utf8(bytes).unwrap();
            assert!(text.starts_with(prefix), "{text}");
            assert!(text.ends_with(":00000001FF\n"), "{text}");
        }

        let _ = std::fs::remove_dir_all(root);
    }
}

#[test]
fn zxspectrum_source_build_uses_sdk_and_writes_loadable_tape() {
    let root = temp_root("zxspectrum_sdk_build");
    std::fs::create_dir_all(&root).unwrap();
    let source_path = root.join("game.ezra");
    std::fs::write(
        &source_path,
        r#"
                import zx.io
                import zx.rom
                import zx.screen

                fn main() {
                    let ula: u8 = io.read_ula()
                    screen.border(ula)
                    rom.print_char(65)
                }
            "#,
    )
    .unwrap();

    let outputs = build_source_with_command_options(&CommandOptions {
        path: source_path.to_string_lossy().into_owned(),
        debug_comments: false,
        default_sdk_symbols: false,
        layout_path: None,
        target: Some("zxspectrum-z80".to_owned()),
    })
    .unwrap();

    let asm = std::fs::read_to_string(outputs.asm).unwrap();
    let map = std::fs::read_to_string(outputs.map).unwrap();
    let tape = std::fs::read(&outputs.executable).unwrap();
    assert!(asm.contains("; target: Z80"), "{asm}");
    assert!(asm.contains("out (0FEh), a"), "{asm}");
    assert!(
        asm.contains(
            "ld a, 0FFh
    in a, (0FEh)"
        ),
        "{asm}"
    );
    assert!(asm.contains("rst 10h"), "{asm}");
    assert!(map.contains(".text        0x008000"), "{map}");
    assert_eq!(
        outputs.executable.extension().and_then(|ext| ext.to_str()),
        Some("tap")
    );
    let blocks = zx_tap_blocks(&tape);
    assert_eq!(blocks.len(), 4);
    assert_eq!(blocks[0].0, 0x00);
    assert_eq!(blocks[0].1[0], 0); // BASIC program
    assert_eq!(u16::from_le_bytes([blocks[0].1[13], blocks[0].1[14]]), 10);
    assert_eq!(blocks[1].0, 0xff);
    assert!(
        blocks[1]
            .1
            .windows(6)
            .any(|bytes| bytes == [0xef, b' ', b'"', b'"', b' ', 0xaf])
    );
    assert_eq!(blocks[2].0, 0x00);
    assert_eq!(blocks[2].1[0], 3); // CODE
    assert_eq!(
        u16::from_le_bytes([blocks[2].1[13], blocks[2].1[14]]),
        0x8000
    );
    assert_eq!(blocks[3].0, 0xff);
    assert_eq!(&blocks[3].1[..3], &[0xF3, 0x31, 0x00]);

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn spectrum_tap_preserves_a_custom_load_address() {
    let mut settings = resolve_build_settings(
        &CommandOptions {
            path: "game.ezra".to_owned(),
            debug_comments: false,
            default_sdk_symbols: true,
            layout_path: None,
            target: Some("zxspectrum-z80".to_owned()),
        },
        Path::new("game.ezra"),
    )
    .unwrap();
    settings.layout.load = Address24::new(0x8001);
    let tape = zx_spectrum_tap_bytes(&settings, None, &[0x00]).unwrap();
    let blocks = zx_tap_blocks(&tape);
    assert_eq!(
        u16::from_le_bytes([blocks[2].1[13], blocks[2].1[14]]),
        0x8001
    );
}

#[test]
fn ti_ce_target_can_override_output_to_raw_bin() {
    let root = temp_root("ti_ce_bin_override");
    std::fs::create_dir_all(&root).unwrap();
    let source_path = root.join("game.ezra");
    std::fs::write(
        root.join("Ezra.toml"),
        r#"
                [build]
                target = "ti84plusce-ez80"
                output = "bin"
            "#,
    )
    .unwrap();
    std::fs::write(&source_path, "fn main() {}\n").unwrap();

    let outputs = build_source(source_path.to_str().unwrap()).unwrap();
    let bin = std::fs::read(&outputs.executable).unwrap();

    assert_eq!(
        outputs.executable.extension().and_then(|ext| ext.to_str()),
        Some("bin")
    );
    assert_eq!(bin[0], 0xCD); // call _main; preserve the TI-OS stack

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn ti_ce_target_rejects_unimplemented_8ek_app_output() {
    let root = temp_root("ti_ce_8ek_output");
    std::fs::create_dir_all(&root).unwrap();
    let source_path = root.join("game.ezra");
    std::fs::write(
        root.join("Ezra.toml"),
        r#"
                [build]
                target = "ti84plusce-ez80"
                output = "8ek"
            "#,
    )
    .unwrap();
    std::fs::write(&source_path, "fn main() {}\n").unwrap();

    let error = build_source(source_path.to_str().unwrap()).unwrap_err();
    assert!(
        error.contains("TI flash application output `.8ek` is not implemented"),
        "{error}"
    );

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn ti_z80_target_can_override_output_to_raw_bin() {
    let root = temp_root("ti_z80_bin_override");
    std::fs::create_dir_all(&root).unwrap();
    let source_path = root.join("game.ezra");
    std::fs::write(
        root.join("Ezra.toml"),
        r#"
                [build]
                target = "ti84plus-z80"
                output = "bin"
            "#,
    )
    .unwrap();
    std::fs::write(&source_path, "fn main() {}\n").unwrap();

    let outputs = build_source(source_path.to_str().unwrap()).unwrap();
    let bin = std::fs::read(&outputs.executable).unwrap();

    assert_eq!(
        outputs.executable.extension().and_then(|ext| ext.to_str()),
        Some("bin")
    );
    assert_eq!(bin[0], 0xCD); // call _main; preserve the TI-OS stack

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn ti_z80_target_rejects_unimplemented_8xk_app_output() {
    let root = temp_root("ti_z80_8xk_output");
    std::fs::create_dir_all(&root).unwrap();
    let source_path = root.join("game.ezra");
    std::fs::write(
        root.join("Ezra.toml"),
        r#"
                [build]
                target = "ti84plus-z80"
                output = "8xk"
            "#,
    )
    .unwrap();
    std::fs::write(&source_path, "fn main() {}\n").unwrap();

    let error = build_source(source_path.to_str().unwrap()).unwrap_err();
    assert!(
        error.contains("TI flash application output `.8xk` is not implemented"),
        "{error}"
    );

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn agon_mos_target_uses_expanded_builtin_sdk_modules() {
    let root = temp_root("agon_expanded_sdk");
    std::fs::create_dir_all(root.join("src")).unwrap();
    let source_path = root.join("src/main.ezra");
    std::fs::write(
        root.join("Ezra.toml"),
        r#"
                [build]
                target = "agonlight-mos-ez80"
                executable = "expanded-sdk"
            "#,
    )
    .unwrap();
    std::fs::write(
        &source_path,
        r#"
                import agon.console
                import agon.gpio
                import agon.keyboard
                import agon.mouse
                import agon.vdp

                fn main() {
                    console.color(vdp.COLOR_GREEN)
                    console.background(vdp.COLOR_BLACK)
                    console.print_line("SDK")
                    vdp.line(0, 0, 16, 16)
                    mouse.enable()
                    let key: u8 = keyboard.ascii()
                    gpio.set_port_b_direction(gpio.ALL_OUTPUTS)
                    gpio.write_port_b(key)
                }
            "#,
    )
    .unwrap();

    let outputs = build_source(source_path.to_str().unwrap()).unwrap();
    let asm = std::fs::read_to_string(&outputs.asm).unwrap();
    let bin = std::fs::read(&outputs.executable).unwrap();

    assert!(asm.contains("rst.lis 08h"), "{asm}");
    assert!(asm.contains("rst.lis 10h"), "{asm}");
    assert!(asm.contains("out0 (9Ah), a"), "{asm}");
    assert_eq!(&bin[64..69], b"MOS\0\x01");

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn cpm_z80_source_build_writes_com_binary() {
    let root = temp_root("cpm_source_build");
    std::fs::create_dir_all(&root).unwrap();
    let source_path = root.join("game.ezra");
    std::fs::write(&source_path, "fn main() { test.pass() }\n").unwrap();

    let outputs = build_source_with_command_options(&CommandOptions {
        path: source_path.to_string_lossy().into_owned(),
        debug_comments: false,
        default_sdk_symbols: true,
        layout_path: None,
        target: Some("cpm-2.2-z80".to_owned()),
    })
    .unwrap();

    let asm = std::fs::read_to_string(outputs.asm).unwrap();
    let com = std::fs::read(&outputs.executable).unwrap();
    assert_eq!(
        outputs.executable.extension().and_then(|ext| ext.to_str()),
        Some("com")
    );
    assert!(asm.contains("; target: Z80"), "{asm}");
    assert_eq!(&com[0..3], &[0xF3, 0x31, 0x00]);

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn cpm_z80_source_examples_use_sdk_and_write_com_binaries() {
    let root = temp_root("cpm_source_example");
    std::fs::create_dir_all(&root).unwrap();
    let examples = Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/cpm-z80");
    for name in ["console-output", "line-input", "file-read"] {
        let source_path = root.join(format!("{name}.ezra"));
        std::fs::copy(examples.join(format!("{name}.ezra")), &source_path).unwrap();
        let outputs = build_source_with_command_options(&CommandOptions {
            path: source_path.to_string_lossy().into_owned(),
            debug_comments: false,
            default_sdk_symbols: true,
            layout_path: None,
            target: Some("cpm-2.2-z80".to_owned()),
        })
        .unwrap_or_else(|error| panic!("failed to build CP/M example `{name}`: {error}"));

        let asm = std::fs::read_to_string(outputs.asm).unwrap();
        let com = std::fs::read(&outputs.executable).unwrap();
        assert_eq!(
            outputs.executable.extension().and_then(|ext| ext.to_str()),
            Some("com")
        );
        assert!(asm.contains("; target: Z80"), "{asm}");
        assert!(asm.contains("    call 0005h"), "{asm}");
        assert_eq!(&com[0..3], &[0xF3, 0x31, 0x00]);
    }

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn cpm_8080_source_build_uses_sdk_and_writes_com_binary() {
    let root = temp_root("cpm_8080_source_build");
    std::fs::create_dir_all(&root).unwrap();
    let source_path = root.join("game.ezra");
    std::fs::write(
        &source_path,
        r#"
                import cpm.bdos

                fn main() {
                    bdos.console_output(65)
                    bdos.system_reset()
                }
            "#,
    )
    .unwrap();

    let outputs = build_source_with_command_options(&CommandOptions {
        path: source_path.to_string_lossy().into_owned(),
        debug_comments: false,
        default_sdk_symbols: false,
        layout_path: None,
        target: Some("cpm-2.2-i8080".to_owned()),
    })
    .unwrap();

    let asm = std::fs::read_to_string(outputs.asm).unwrap();
    let com = std::fs::read(&outputs.executable).unwrap();
    assert_eq!(
        outputs.executable.extension().and_then(|ext| ext.to_str()),
        Some("com")
    );
    assert!(asm.contains("; target: i8080"), "{asm}");
    assert!(asm.contains("    call 0005h"), "{asm}");
    assert!(asm.contains("    mov c,"), "{asm}");
    assert!(!asm.contains("    ld "), "{asm}");
    assert_eq!(&com[0..3], &[0xF3, 0x31, 0x00]);

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn cpm_8085_source_build_uses_sdk_and_writes_com_binary() {
    let root = temp_root("cpm_8085_source_build");
    std::fs::create_dir_all(&root).unwrap();
    let source_path = root.join("game.ezra");
    std::fs::write(
        &source_path,
        r#"
                import cpm.bdos

                fn main() {
                    asm volatile {
                        "rim"
                        "sim"
                    }
                    bdos.console_output(65)
                    bdos.system_reset()
                }
            "#,
    )
    .unwrap();

    let outputs = build_source_with_command_options(&CommandOptions {
        path: source_path.to_string_lossy().into_owned(),
        debug_comments: false,
        default_sdk_symbols: false,
        layout_path: None,
        target: Some("cpm-2.2-i8085".to_owned()),
    })
    .unwrap();

    let asm = std::fs::read_to_string(outputs.asm).unwrap();
    let com = std::fs::read(&outputs.executable).unwrap();
    assert_eq!(
        outputs.executable.extension().and_then(|ext| ext.to_str()),
        Some("com")
    );
    assert!(asm.contains("; target: i8085"), "{asm}");
    assert!(asm.contains("    rim"), "{asm}");
    assert!(asm.contains("    sim"), "{asm}");
    assert!(asm.contains("    call 0005h"), "{asm}");
    assert!(com.windows(2).any(|bytes| bytes == [0x20, 0x30]));
    assert_eq!(&com[0..3], &[0xF3, 0x31, 0x00]);

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn commands_accept_intel_hex_output_format() {
    let root = temp_root("intel_hex_output");
    std::fs::create_dir_all(&root).unwrap();
    let source_path = root.join("game.ezra");
    std::fs::write(&source_path, "fn main() { test.pass() }\n").unwrap();
    std::fs::write(
        root.join("Ezra.toml"),
        r#"
                [build]
                target = "agonlight-console8-ez80"
                output = "hex"
            "#,
    )
    .unwrap();

    let outputs = build_source(source_path.to_str().unwrap()).unwrap();
    assert_eq!(
        outputs.executable.extension().and_then(|ext| ext.to_str()),
        Some("hex")
    );

    let _ = std::fs::remove_dir_all(root);
}
