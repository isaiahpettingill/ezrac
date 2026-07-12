use super::*;

#[test]
fn emits_and_runs_generic_hardware_port_examples() {
    let source = r#"
            port PAD1_LO: u8 = 0x01
            port PAD1_HI: u8 = 0x02
            port TI_LCD_CMD: u8 = 0x10
            port TI_LCD_DATA: u8 = 0x11
            port AGON_VDP_DATA: u8 = 0x9B

            fn read_pad_low() -> u8 {
                return in PAD1_LO
            }

            fn ti_lcd_command(cmd: u8) {
                out TI_LCD_CMD, cmd
            }

            fn ti_lcd_data(value: u8) {
                out TI_LCD_DATA, value
            }

            fn agon_vdp_byte(value: u8) {
                out AGON_VDP_DATA, value
            }

            fn main() {
                let pad_lo: u8 = read_pad_low()
                let pad_hi: u8 = in PAD1_HI
                ti_lcd_command(0x2A)
                ti_lcd_data(pad_lo)
                agon_vdp_byte(pad_hi)
                test.assert_eq_u8(pad_lo, 0, 1)
                test.assert_eq_u8(pad_hi, 0, 2)
                test.pass()
            }
        "#;
    let program = parse_program(Path::new("game.ezra"), source).unwrap();
    let asm = emit_ez80_assembly(&program).unwrap();
    let run = run_assembly_test(&asm, 2_000).unwrap();

    assert!(asm.contains("in0 a, (01h)"), "{asm}");
    assert!(asm.contains("in0 a, (02h)"), "{asm}");
    assert!(asm.contains("out0 (10h), a"), "{asm}");
    assert!(asm.contains("out0 (11h), a"), "{asm}");
    assert!(asm.contains("out0 (9Bh), a"), "{asm}");
    assert!(run.halted, "{asm}");
    assert_eq!(run.result_code, 0, "{asm}");
}

#[test]
fn emits_and_runs_default_fantasy_port_map_symbols() {
    let source = r#"
            fn main() {
                let pad2: u8 = in PAD2_LO
                let status: u8 = in EXT_STATUS
                out VIDEO_CMD, VIDEO_SET_MODE
                out EXT_ADDR0, pad2
                out EXT_COMMAND, status
                test.assert_eq_u8(pad2, 0x33, 1)
                test.assert_eq_u8(status, 0x44, 2)
                test.pass()
            }
        "#;
    let program = parse_program(Path::new("game.ezra"), source).unwrap();
    let asm = emit_ez80_assembly(&program).unwrap();
    let run = run_assembly_test_with_options(
        &asm,
        &TestRunOptions {
            instruction_budget: 4_000,
            initial_ports: vec![(0x03, 0x33), (0x17, 0x44)],
            initial_memory: Vec::new(),
            stack_top: EZRA_STACK_TOP.get(),
        },
    )
    .unwrap();

    assert!(asm.contains("in0 a, (03h)"), "{asm}");
    assert!(asm.contains("in0 a, (17h)"), "{asm}");
    assert!(asm.contains("out0 (09h), a"), "{asm}");
    assert!(asm.contains("out0 (10h), a"), "{asm}");
    assert!(asm.contains("out0 (16h), a"), "{asm}");
    assert!(run.halted, "{asm}");
    assert_eq!(run.result_code, 0, "{asm}");
    assert_eq!(run.ports[0x09], 3, "{asm}");
    assert_eq!(run.ports[0x10], 0x33, "{asm}");
    assert_eq!(run.ports[0x16], 0x44, "{asm}");
}

#[test]
fn can_disable_default_sdk_symbols_for_target_specific_hardware() {
    let source = r#"
            const SCREEN: ptr<u8> = 0x040180
            port TI_KEYGROUP: u8 = 0x01
            port AGON_VDP: u8 = 0x9B

            fn main() {
                let keys: u8 = in TI_KEYGROUP
                *(SCREEN) = keys
                out AGON_VDP, *SCREEN
                test.assert_eq_u8(*SCREEN, 0x2C, 1)
                test.pass()
            }
        "#;
    let program = parse_program(Path::new("game.ezra"), source).unwrap();
    let asm = emit_ez80_assembly_with_options(
        &program,
        AssemblyOptions {
            default_sdk_symbols: false,
            ..AssemblyOptions::default()
        },
    )
    .unwrap();
    let run = run_assembly_test_with_options(
        &asm,
        &TestRunOptions {
            instruction_budget: 4_000,
            initial_ports: vec![(0x01, 0x2C)],
            initial_memory: Vec::new(),
            stack_top: EZRA_STACK_TOP.get(),
        },
    )
    .unwrap();

    assert!(asm.contains("in0 a, (01h)"), "{asm}");
    assert!(asm.contains("out0 (9Bh), a"), "{asm}");
    assert!(run.halted, "{asm}");
    assert_eq!(run.result_code, 0, "{asm}");
    assert_eq!(run.ports[0x9B], 0x2C, "{asm}");

    let default_port_source = r#"
            fn main() {
                let pad: u8 = in PAD1_LO
                test.assert_eq_u8(pad, 0, 1)
                test.pass()
            }
        "#;
    let program = parse_program(Path::new("game.ezra"), default_port_source).unwrap();
    let error = emit_ez80_assembly_with_options(
        &program,
        AssemblyOptions {
            default_sdk_symbols: false,
            ..AssemblyOptions::default()
        },
    )
    .unwrap_err();

    assert_eq!(error.message, "unknown port `PAD1_LO`");
}

#[test]
fn emits_and_runs_default_video_audio_base_pointer_symbols() {
    let source = r#"
            fn main() {
                *(VRAM_BASE + 1) = 0x4A;
                *(AUDIO_BASE + 2) = 0x5B;
                test.assert_eq_u8(*(VRAM_BASE + 1), 0x4A, 1)
                test.assert_eq_u8(*(AUDIO_BASE + 2), 0x5B, 2)
                test.pass()
            }
        "#;
    let program = parse_program(Path::new("game.ezra"), source).unwrap();
    let asm = emit_ez80_assembly_with_options(
        &program,
        AssemblyOptions {
            vram_base: Address24::new(0x04_0180),
            audio_base: Address24::new(0x04_0190),
            ..AssemblyOptions::default()
        },
    )
    .unwrap();
    let run = run_assembly_test(&asm, 4_000).unwrap();

    assert!(asm.contains("040180h"), "{asm}");
    assert!(asm.contains("040190h"), "{asm}");
    assert!(run.halted, "{asm}");
    assert_eq!(run.result_code, 0, "{asm}");
}

#[test]
fn emits_and_runs_ti84_plus_ce_style_sdk_modules() {
    let root = std::env::temp_dir().join(format!(
        "ezra_ti84_sdk_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(root.join("ti84")).unwrap();
    let main_path = root.join("game.ezra");
    std::fs::write(
        root.join("ti84/keys.ezra"),
        r#"
            pub port KEY_LO: u8 = 0x01
            pub port KEY_HI: u8 = 0x02

            pub fn scan() -> u16 {
                let lo: u8 = in KEY_LO
                let hi: u8 = in KEY_HI
                return cast<u16>(lo) | (cast<u16>(hi) << 8)
            }
            "#,
    )
    .unwrap();
    std::fs::write(
        root.join("ti84/lcd.ezra"),
        r#"
            pub port LCD_CMD: u8 = 0x10
            pub port LCD_DATA: u8 = 0x11
            pub volatile mmio LCD_SHADOW: ptr<u8> = 0x040240

            pub fn command(value: u8) {
                out LCD_CMD, value
            }

            pub fn write(value: u8) {
                *(LCD_SHADOW) = value
                out LCD_DATA, value
            }
            "#,
    )
    .unwrap();
    std::fs::write(
        &main_path,
        r#"
            import ti84.keys
            import ti84.lcd

            fn main() {
                let keys: u16 = keys.scan()
                lcd.command(0x2A)
                lcd.write(cast<u8>(keys))

                test.assert_eq_u16(keys, 0x1205, 1)
                test.assert_eq_u8(*(lcd.LCD_SHADOW), 0x05, 2)
                test.pass()
            }
            "#,
    )
    .unwrap();

    let program = load_program(&main_path).unwrap();
    let asm = emit_ez80_assembly(&program).unwrap();
    let run = run_assembly_test_with_options(
        &asm,
        &TestRunOptions {
            instruction_budget: 8_000,
            initial_ports: vec![(0x01, 0x05), (0x02, 0x12)],
            initial_memory: Vec::new(),
            stack_top: EZRA_STACK_TOP.get(),
        },
    )
    .unwrap();

    let _ = std::fs::remove_dir_all(&root);
    assert!(asm.contains("_keys_scan:"), "{asm}");
    assert!(asm.contains("_lcd_write:"), "{asm}");
    assert!(asm.contains("in0 a, (01h)"), "{asm}");
    assert!(asm.contains("in0 a, (02h)"), "{asm}");
    assert!(asm.contains("out0 (10h), a"), "{asm}");
    assert!(asm.contains("out0 (11h), a"), "{asm}");
    assert!(run.halted, "{asm}");
    assert_eq!(run.result_code, 0, "{asm}");
}

#[test]
fn emits_and_runs_agon_light_style_sdk_modules() {
    let root = std::env::temp_dir().join(format!(
        "ezra_agon_sdk_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(root.join("agon")).unwrap();
    let main_path = root.join("game.ezra");
    std::fs::write(
        root.join("agon/vdp.ezra"),
        r#"
            pub port VDP_DATA: u8 = 0x9B
            pub volatile mmio VDP_SHADOW: ptr<u8> = 0x040260

            pub fn byte(value: u8) {
                *(VDP_SHADOW) = value
                out VDP_DATA, value
            }
            "#,
    )
    .unwrap();
    std::fs::write(
        root.join("agon/system.ezra"),
        r#"
            pub port STATUS: u8 = 0x17

            pub fn status() -> u8 {
                return in STATUS
            }
            "#,
    )
    .unwrap();
    std::fs::write(
        &main_path,
        r#"
            import agon.vdp
            import agon.system

            fn main() {
                let sys_status: u8 = system.status()
                vdp.byte(sys_status ^ 0xFF)

                test.assert_eq_u8(sys_status, 0xA0, 1)
                test.assert_eq_u8(*(vdp.VDP_SHADOW), 0x5F, 2)
                test.pass()
            }
            "#,
    )
    .unwrap();

    let program = load_program(&main_path).unwrap();
    let asm = emit_ez80_assembly(&program).unwrap();
    let run = run_assembly_test_with_options(
        &asm,
        &TestRunOptions {
            instruction_budget: 6_000,
            initial_ports: vec![(0x17, 0xA0)],
            initial_memory: Vec::new(),
            stack_top: EZRA_STACK_TOP.get(),
        },
    )
    .unwrap();

    let _ = std::fs::remove_dir_all(&root);
    assert!(asm.contains("_vdp_byte:"), "{asm}");
    assert!(asm.contains("_system_status:"), "{asm}");
    assert!(asm.contains("in0 a, (17h)"), "{asm}");
    assert!(asm.contains("out0 (9Bh), a"), "{asm}");
    assert!(run.halted, "{asm}");
    assert_eq!(run.result_code, 0, "{asm}");
}

#[test]
fn emits_and_runs_type_aliases() {
    let source = r#"
            alias subpx = i24
            alias addr = ptr<u8>
            alias byte = u8

            volatile mmio SCRATCH: addr = 0x040180
            global player_x: subpx = 0x000100

            fn add_pos(x: subpx, dx: subpx) -> subpx {
                return x + dx
            }

            fn main() {
                let x: subpx = add_pos(player_x, 0x000080)
                let p: addr = cast<addr>(0x040181)
                let value: byte = 0x37
                mem.poke8(SCRATCH, value)
                mem.poke8(p, mem.peek8(SCRATCH) + 1)
                test.assert_eq_u24(x, 0x000180, 1)
                test.assert_eq_u8(mem.peek8(p), 0x38, 2)
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
fn emits_and_runs_repeated_volatile_mmio_dereferences() {
    let source = r#"
            volatile mmio STATUS: ptr<u8> = 0x040270
            volatile mmio CONTROL: ptr<u8> = 0x040271

            fn main() {
                *STATUS;
                *STATUS;
                *(CONTROL) = 0x34;
                *(CONTROL) = 0x35;
                test.assert_eq_u8(*STATUS, 0x12, 1)
                test.assert_eq_u8(*CONTROL, 0x35, 2)
                test.pass()
            }
        "#;
    let program = parse_program(Path::new("game.ezra"), source).unwrap();
    let asm = emit_ez80_assembly(&program).unwrap();
    let run = run_assembly_test_with_options(
        &asm,
        &TestRunOptions {
            instruction_budget: 4_000,
            initial_ports: Vec::new(),
            initial_memory: vec![(0x040270, 0x12)],
            stack_top: EZRA_STACK_TOP.get(),
        },
    )
    .unwrap();

    assert!(run.halted, "{asm}");
    assert_eq!(run.result_code, 0, "{asm}");
    assert!(asm.matches("    ld hl, 040270h").count() >= 3, "{asm}");
    assert!(asm.matches("    ld a, (hl)").count() >= 4, "{asm}");
    assert!(asm.matches("    ld (hl), a").count() >= 2, "{asm}");
}

#[test]
fn preserves_order_between_ports_and_volatile_mmio() {
    let source = r#"
            port FIRST: u8 = 0x20
            port SECOND: u8 = 0x21
            port THIRD: u8 = 0x22
            volatile mmio STATUS: ptr<u8> = 0x040270

            fn main() {
                out FIRST, 0x11
                *(STATUS) = 0x22
                out SECOND, *STATUS
                *(STATUS) = 0x33
                out THIRD, *STATUS
                test.assert_eq_u8(*STATUS, 0x33, 1)
                test.pass()
            }
        "#;
    let program = parse_program(Path::new("game.ezra"), source).unwrap();
    let asm = emit_ez80_assembly(&program).unwrap();
    let run = run_assembly_test(&asm, 4_000).unwrap();

    assert!(asm.contains("out0 (20h), a"), "{asm}");
    assert!(asm.contains("out0 (21h), a"), "{asm}");
    assert!(asm.contains("out0 (22h), a"), "{asm}");
    assert!(asm.matches("    ld hl, 040270h").count() >= 4, "{asm}");
    assert!(asm.matches("    ld a, (hl)").count() >= 3, "{asm}");
    assert!(asm.matches("    ld (hl), a").count() >= 2, "{asm}");
    assert!(run.halted, "{asm}");
    assert_eq!(run.result_code, 0, "{asm}");
    assert_eq!(run.ports[0x20], 0x11, "{asm}");
    assert_eq!(run.ports[0x21], 0x22, "{asm}");
    assert_eq!(run.ports[0x22], 0x33, "{asm}");
}
