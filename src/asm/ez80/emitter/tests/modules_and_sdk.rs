use super::*;

#[test]
fn emits_and_runs_imported_module_qualified_calls() {
    let root = std::env::temp_dir().join(format!(
        "ezra_module_calls_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(root.join("lib")).unwrap();
    let main_path = root.join("game.ezra");
    std::fs::write(
        root.join("lib/math.ezra"),
        "pub fn add(a: u8, b: u8) -> u8 { return a + b }\n",
    )
    .unwrap();
    std::fs::write(
        &main_path,
        r#"
            import lib.math
            fn main() {
                let value: u8 = math.add(2, 3)
                test.assert_eq_u8(value, 5, 1)
                math.add(1, 2)
                test.assert_eq_u8(lib.math.add(4, 5), 9, 2)
                test.pass()
            }
            "#,
    )
    .unwrap();

    let program = load_program(&main_path).unwrap();
    let asm = emit_ez80_assembly(&program).unwrap();
    let run = run_assembly_test(&asm, 4_000).unwrap();

    let _ = std::fs::remove_dir_all(&root);
    assert!(asm.contains("_math_add:"), "{asm}");
    assert!(asm.contains("    call _math_add"), "{asm}");
    assert!(run.halted, "{asm}");
    assert_eq!(run.result_code, 0, "{asm}");
}

#[test]
fn rejects_function_assembly_label_collisions() {
    let root = std::env::temp_dir().join(format!(
        "ezra_label_collision_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(root.join("lib")).unwrap();
    let main_path = root.join("game.ezra");
    std::fs::write(
        root.join("lib/math.ezra"),
        "pub fn add(a: u8, b: u8) -> u8 { return a + b }\n",
    )
    .unwrap();
    std::fs::write(
        &main_path,
        r#"
            import lib.math

            fn lib_math_add(a: u8, b: u8) -> u8 {
                return a - b
            }

            fn main() {
                test.pass()
            }
            "#,
    )
    .unwrap();

    let program = load_program(&main_path).unwrap();
    let error = emit_ez80_assembly(&program).unwrap_err();

    let _ = std::fs::remove_dir_all(&root);
    assert_eq!(
        error.message,
        "function `lib_math_add` emits assembly label `_lib_math_add` already used by function `lib.math.add`"
    );
}

#[test]
fn rejects_reserved_function_assembly_labels() {
    let cases = [
        (
            r#"
                fn _ezra_pass() {
                    test.pass()
                }

                fn main() {
                    test.pass()
                }
                "#,
            "function `_ezra_pass` emits reserved assembly label `__ezra_pass`",
        ),
        (
            r#"
                extern asm fn _ezra_memcpy(dst: ptr<u8>, src: ptr<u8>, len: u24)

                fn main() {
                    test.pass()
                }
                "#,
            "function `_ezra_memcpy` emits reserved assembly label `__ezra_memcpy`",
        ),
    ];

    for (source, expected) in cases {
        let program = parse_program(Path::new("game.ezra"), source).unwrap();
        let error = emit_ez80_assembly(&program).unwrap_err();

        assert_eq!(error.message, expected);
    }
}

#[test]
fn emits_and_runs_imported_module_qualified_constants() {
    let root = std::env::temp_dir().join(format!(
        "ezra_module_constants_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(root.join("lib")).unwrap();
    let main_path = root.join("game.ezra");
    std::fs::write(
        root.join("lib/hw.ezra"),
        r#"
            pub const VALUE: u8 = 0x37
            pub volatile mmio SCRATCH: ptr<u8> = 0x040120
            "#,
    )
    .unwrap();
    std::fs::write(
        &main_path,
        r#"
            import lib.hw
            fn main() {
                mem.poke8(hw.SCRATCH, hw.VALUE)
                test.assert_eq_u8(mem.peek8(hw.SCRATCH), 0x37, 1)
                mem.poke8(lib.hw.SCRATCH, lib.hw.VALUE + 1)
                test.assert_eq_u8(mem.peek8(hw.SCRATCH), 0x38, 2)
                mem.poke8(lib.hw.SCRATCH + 1, lib.hw.VALUE + 2)
                test.assert_eq_u8(mem.peek8(hw.SCRATCH + 1), 0x39, 3)
                test.pass()
            }
            "#,
    )
    .unwrap();

    let program = load_program(&main_path).unwrap();
    let asm = emit_ez80_assembly(&program).unwrap();
    let run = run_assembly_test(&asm, 6_000).unwrap();

    let _ = std::fs::remove_dir_all(&root);
    assert!(run.halted, "{asm}");
    assert_eq!(run.result_code, 0, "{asm}");
}

#[test]
fn emits_and_runs_imported_module_qualified_types() {
    let root = std::env::temp_dir().join(format!(
        "ezra_module_types_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(root.join("lib")).unwrap();
    let main_path = root.join("game.ezra");
    std::fs::write(
        root.join("lib/types.ezra"),
        r#"
            pub alias Byte = u8
            pub struct Pair {
                lo: Byte
                hi: Byte
            }
            "#,
    )
    .unwrap();
    std::fs::write(
        &main_path,
        r#"
            import lib.types
            fn main() {
                let lo: types.Byte = 3
                let pair: types.Pair = types.Pair { lo: lo, hi: 4 }
                let full_pair: lib.types.Pair = lib.types.Pair {
                    lo: cast<lib.types.Byte>(5),
                    hi: 6,
                }
                test.assert_eq_u8(pair.lo, 3, 1)
                test.assert_eq_u8(pair.hi, 4, 2)
                test.assert_eq_u8(full_pair.lo, 5, 3)
                test.assert_eq_u8(full_pair.hi, 6, 4)
                test.pass()
            }
            "#,
    )
    .unwrap();

    let program = load_program(&main_path).unwrap();
    let asm = emit_ez80_assembly(&program).unwrap();
    let run = run_assembly_test(&asm, 6_000).unwrap();

    let _ = std::fs::remove_dir_all(&root);
    assert!(run.halted, "{asm}");
    assert_eq!(run.result_code, 0, "{asm}");
}

#[test]
fn emits_and_runs_imported_module_qualified_globals() {
    let root = std::env::temp_dir().join(format!(
        "ezra_module_globals_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(root.join("lib")).unwrap();
    let main_path = root.join("game.ezra");
    std::fs::write(root.join("lib/state.ezra"), "pub global score: u8 = 5\n").unwrap();
    std::fs::write(
        &main_path,
        r#"
            import lib.state
            fn main() {
                state.score += 2
                test.assert_eq_u8(state.score, 7, 1)
                test.assert_eq_u8(score, 7, 2)
                lib.state.score += 1
                test.assert_eq_u8(state.score, 8, 3)
                test.pass()
            }
            "#,
    )
    .unwrap();

    let program = load_program(&main_path).unwrap();
    let asm = emit_ez80_assembly(&program).unwrap();
    let run = run_assembly_test(&asm, 4_000).unwrap();

    let _ = std::fs::remove_dir_all(&root);
    assert!(run.halted, "{asm}");
    assert_eq!(run.result_code, 0, "{asm}");
}

#[test]
fn emits_and_runs_imported_module_qualified_array_globals() {
    let root = std::env::temp_dir().join(format!(
        "ezra_module_array_globals_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(root.join("lib")).unwrap();
    let main_path = root.join("game.ezra");
    std::fs::write(
        root.join("lib/state.ezra"),
        r#"
            pub const LEN: u8 = 3
            pub global bytes: [u8; LEN] = [1, 2, 3]
            "#,
    )
    .unwrap();
    std::fs::write(
        &main_path,
        r#"
            import lib.state
            global short_sized: [u8; state.LEN] = [4, 5, 6]
            global full_sized: [u8; lib.state.LEN] = [7, 8, 9]
            global copied_short: [u8; state.LEN] = state.bytes
            global copied_full: [u8; lib.state.LEN] = lib.state.bytes

            fn main() {
                test.assert_eq_u8(state.bytes[1], 2, 1)
                state.bytes[2] = state.bytes[1] + 5
                test.assert_eq_u8(bytes[2], 7, 2)
                let ptr: ptr<u8> = &state.bytes[0]
                test.assert_eq_u8(*(ptr + 2), 7, 3)
                lib.state.bytes[0] = lib.state.bytes[2] + 1
                test.assert_eq_u8(state.bytes[0], 8, 4)
                test.assert_eq_u8(short_sized[2], 6, 5)
                test.assert_eq_u8(full_sized[2], 9, 6)
                test.assert_eq_u8(copied_short[0], 1, 7)
                test.assert_eq_u8(copied_short[2], 3, 8)
                test.assert_eq_u8(copied_full[0], 1, 9)
                test.assert_eq_u8(copied_full[2], 3, 10)
                test.pass()
            }
            "#,
    )
    .unwrap();

    let program = load_program(&main_path).unwrap();
    let asm = emit_ez80_assembly(&program).unwrap();
    let run = run_assembly_test(&asm, 8_000).unwrap();

    let _ = std::fs::remove_dir_all(&root);
    assert!(run.halted, "{asm}");
    assert_eq!(run.result_code, 0, "{asm}");
}

#[test]
fn emits_and_runs_imported_module_qualified_embeds() {
    let root = std::env::temp_dir().join(format!(
        "ezra_module_embeds_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(root.join("lib")).unwrap();
    let main_path = root.join("game.ezra");
    std::fs::write(
        root.join("lib/assets.ezra"),
        "pub embed sprite: bytes = bytes [0x41, 0x42]\n",
    )
    .unwrap();
    std::fs::write(
        &main_path,
        r#"
            import lib.assets
            fn main() {
                test.assert_eq_u24(assets.sprite.len, 2, 1)
                test.assert_eq_u8(*(assets.sprite.ptr + 0), 0x41, 2)
                test.assert_eq_u8(*(assets.sprite.ptr + 1), 0x42, 3)
                test.assert_eq_u8(*(sprite.ptr + 1), 0x42, 4)
                test.assert_eq_u8(*(lib.assets.sprite.ptr + 0), 0x41, 5)
                test.pass()
            }
            "#,
    )
    .unwrap();

    let program = load_program(&main_path).unwrap();
    let asm = emit_ez80_assembly(&program).unwrap();
    let run = run_assembly_test(&asm, 8_000).unwrap();

    let _ = std::fs::remove_dir_all(&root);
    assert!(run.halted, "{asm}");
    assert_eq!(run.result_code, 0, "{asm}");
}

#[test]
fn emits_and_runs_imported_module_qualified_ports() {
    let root = std::env::temp_dir().join(format!(
        "ezra_module_ports_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(root.join("lib")).unwrap();
    let main_path = root.join("game.ezra");
    std::fs::write(
        root.join("lib/hw.ezra"),
        r#"
            pub port PAD_LO: u8 = 0x01
            pub port DEBUG: u8 = 0x0C
            "#,
    )
    .unwrap();
    std::fs::write(
        &main_path,
        r#"
            import lib.hw
            fn main() {
                let pad: u8 = in hw.PAD_LO
                out hw.DEBUG, 'P'
                test.assert_eq_u8(pad, 0, 1)
                let full_pad: u8 = in lib.hw.PAD_LO
                out lib.hw.DEBUG, 'Q'
                test.assert_eq_u8(full_pad, 0, 2)
                test.pass()
            }
            "#,
    )
    .unwrap();

    let program = load_program(&main_path).unwrap();
    let asm = emit_ez80_assembly(&program).unwrap();
    let run = run_assembly_test(&asm, 4_000).unwrap();

    let _ = std::fs::remove_dir_all(&root);
    assert!(asm.contains("in0 a, (01h)"), "{asm}");
    assert!(asm.contains("out0 (0Ch), a"), "{asm}");
    assert!(run.halted, "{asm}");
    assert_eq!(run.result_code, 0, "{asm}");
    assert_eq!(run.debug_output, b"PQ", "{asm}");
}

#[test]
fn emits_and_runs_imported_sdk_style_game_frame() {
    let root = std::env::temp_dir().join(format!(
        "ezra_sdk_frame_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(root.join("sdk")).unwrap();
    std::fs::create_dir_all(root.join("assets")).unwrap();
    let main_path = root.join("game.ezra");
    std::fs::write(root.join("assets/player.bin"), [0x2A, 0x7E]).unwrap();
    std::fs::write(
        root.join("sdk/input.ezra"),
        r#"
            pub const BTN_RIGHT: u16 = 0x0080
            pub port PAD_LO: u8 = 0x01
            pub port PAD_HI: u8 = 0x02

            pub fn read_pad(index: u8) -> u16 {
                let lo: u8 = in PAD_LO
                let hi: u8 = in PAD_HI
                let wide_hi: u16 = cast<u16>(hi) << 8
                if index == 0 {
                    return BTN_RIGHT | cast<u16>(lo) | wide_hi
                }
                return 0
            }

            pub fn pressed(pad: u16, button: u16) -> bool {
                return (pad & button) != 0
            }
            "#,
    )
    .unwrap();
    std::fs::write(
        root.join("sdk/video.ezra"),
        r#"
            pub const VIDEO_PRESENT: u8 = 1
            pub volatile mmio VRAM_BASE: ptr<u8> = 0x040180
            pub port VIDEO_CMD: u8 = 0x09

            pub fn present() {
                out VIDEO_CMD, VIDEO_PRESENT
            }

            pub fn clear(value: u8) {
                let i: u8 = 0
                while i < 4 {
                    *(VRAM_BASE + cast<u24>(i)) = value
                    i += 1
                }
            }

            pub fn poke(offset: u24, value: u8) {
                *(VRAM_BASE + offset) = value
            }

            pub fn peek(offset: u24) -> u8 {
                return *(VRAM_BASE + offset)
            }
            "#,
    )
    .unwrap();
    std::fs::write(
        root.join("sdk/math.ezra"),
        r#"
            pub const SUBPX_SHIFT: u8 = 8
            pub const SUBPX_ONE: i24 = 256

            pub fn subpx_from_int(v: i16) -> i24 {
                return cast<i24>(v) * SUBPX_ONE
            }

            pub fn subpx_to_int(v: i24) -> i16 {
                return cast<i16>(v / SUBPX_ONE)
            }
            "#,
    )
    .unwrap();
    std::fs::write(
        &main_path,
        r#"
            import sdk.input
            import sdk.video
            import sdk.math

            alias pos = i24

            embed player_sprite: bytes = file("assets/player.bin") section .assets align 16

            global player_x: pos = 20 * SUBPX_ONE
            global player_y: pos = 20 * SUBPX_ONE

            fn update() {
                let pad: u16 = input.read_pad(0)
                if input.pressed(pad, BTN_RIGHT) {
                    player_x += SUBPX_ONE
                }
            }

            fn draw() {
                let sx: u16 = cast<u16>(math.subpx_to_int(player_x))
                let sy: u16 = cast<u16>(math.subpx_to_int(player_y))
                let offset: u24 = cast<u24>(sy) * 32 + cast<u24>(sx)
                let color: u8 = *player_sprite.ptr
                video.poke(offset, color)
            }

            fn main() {
                video.clear(0)
                let frames: u8 = 0
                loop {
                    update()
                    draw()
                    video.present()
                    frames += 1
                    if frames == 2 {
                        break
                    }
                }

                test.assert_eq_u24(cast<u24>(player_x), 0x001600, 1)
                test.assert_eq_u8(video.peek(661), 0x2A, 2)
                test.assert_eq_u8(video.peek(0), 0, 3)
                test.assert_eq_u8(video.peek(662), 0x2A, 4)
                test.assert_eq_u24(player_sprite.len, 2, 5)
                test.pass()
            }
            "#,
    )
    .unwrap();

    let program = load_program(&main_path).unwrap();
    let asm = emit_ez80_assembly(&program).unwrap();
    let run = run_assembly_test(&asm, 100_000).unwrap();

    let _ = std::fs::remove_dir_all(&root);
    assert!(asm.contains("_input_read_pad:"), "{asm}");
    assert!(asm.contains("_video_poke:"), "{asm}");
    assert!(asm.contains("_math_subpx_to_int:"), "{asm}");
    assert!(asm.contains("in0 a, (01h)"), "{asm}");
    assert!(asm.contains("out0 (09h), a"), "{asm}");
    assert!(run.halted, "{asm}");
    assert_eq!(run.result_code, 0, "{asm}");
}

#[test]
fn emits_and_runs_audio_sdk_style_port_sequence() {
    let root = std::env::temp_dir().join(format!(
        "ezra_audio_sdk_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(root.join("sdk")).unwrap();
    let main_path = root.join("game.ezra");
    std::fs::write(
        root.join("sdk/audio.ezra"),
        r#"
            pub const AUDIO_SUBMIT_BUFFER: u8 = 1
            pub const AUDIO_STOP: u8 = 2
            pub volatile mmio AUDIO_BASE: ptr<u8> = 0x0C1234
            pub port AUDIO_CMD: u8 = 0x0A
            pub port EXT_ADDR0: u8 = 0x10
            pub port EXT_ADDR1: u8 = 0x11
            pub port EXT_ADDR2: u8 = 0x12
            pub port EXT_LEN0: u8 = 0x13
            pub port EXT_LEN1: u8 = 0x14

            pub fn submit(addr: ptr<u8>, len: u16) {
                let raw: u24 = cast<u24>(addr)
                out EXT_ADDR0, cast<u8>(raw)
                out EXT_ADDR1, cast<u8>(raw >> 8)
                out EXT_ADDR2, cast<u8>(raw >> 16)
                out EXT_LEN0, cast<u8>(len)
                out EXT_LEN1, cast<u8>(len >> 8)
                out AUDIO_CMD, AUDIO_SUBMIT_BUFFER
            }

            pub fn stop() {
                out AUDIO_CMD, AUDIO_STOP
            }
            "#,
    )
    .unwrap();
    std::fs::write(
        &main_path,
        r#"
            import sdk.audio

            fn main() {
                audio.submit(audio.AUDIO_BASE + 0x56, 0x2345)
                test.pass()
            }
            "#,
    )
    .unwrap();

    let program = load_program(&main_path).unwrap();
    let asm = emit_ez80_assembly(&program).unwrap();
    let run = run_assembly_test(&asm, 8_000).unwrap();

    let _ = std::fs::remove_dir_all(&root);
    assert!(asm.contains("_audio_submit:"), "{asm}");
    assert!(asm.contains("out0 (0Ah), a"), "{asm}");
    assert!(asm.contains("out0 (10h), a"), "{asm}");
    assert!(asm.contains("out0 (14h), a"), "{asm}");
    assert!(run.halted, "{asm}");
    assert_eq!(run.result_code, 0, "{asm}");
    assert_eq!(run.ports[0x10], 0x8A, "{asm}");
    assert_eq!(run.ports[0x11], 0x12, "{asm}");
    assert_eq!(run.ports[0x12], 0x0C, "{asm}");
    assert_eq!(run.ports[0x13], 0x45, "{asm}");
    assert_eq!(run.ports[0x14], 0x23, "{asm}");
    assert_eq!(run.ports[0x0A], 1, "{asm}");
}
