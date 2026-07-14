use super::*;

#[test]
fn assemble_file_writes_raw_binary() {
    let root = temp_root("assemble_file");
    std::fs::create_dir_all(&root).unwrap();
    let source_path = root.join("main.asm");
    let output_path = root.join("main.bin");
    std::fs::write(
        &source_path,
        r#"
                start:
                    ld a, 42h
                    rst.lis 10h
                    ret
            "#,
    )
    .unwrap();

    assemble_file(&AssembleOptions {
        path: source_path.to_string_lossy().into_owned(),
        output: Some(output_path.to_string_lossy().into_owned()),
        base_addr: Some(0x04_0000),
        assembler_cpu: None,
        layout_path: None,
        map_path: None,
        target: None,
    })
    .unwrap();

    assert_eq!(
        std::fs::read(&output_path).unwrap(),
        [0x3E, 0x42, 0x49, 0xD7, 0xC9]
    );

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn assemble_file_writes_cpm_com_for_cpm_z80_target() {
    let root = temp_root("assemble_cpm_file");
    std::fs::create_dir_all(&root).unwrap();
    let source_path = root.join("hello.asm");
    let output_path = source_path.with_extension("com");
    std::fs::write(
        &source_path,
        r#"
                start:
                    ld c, 02h
                    ld e, 48h
                    call 0005h
                    ld c, 00h
                    call 0005h
            "#,
    )
    .unwrap();

    assemble_file(&AssembleOptions {
        path: source_path.to_string_lossy().into_owned(),
        output: None,
        base_addr: None,
        assembler_cpu: None,
        layout_path: None,
        map_path: None,
        target: Some("cpm-2.2-z80".to_owned()),
    })
    .unwrap();

    assert_eq!(
        std::fs::read(&output_path).unwrap(),
        [
            0x0E, 0x02, 0x1E, 0x48, 0xCD, 0x05, 0x00, 0x0E, 0x00, 0xCD, 0x05, 0x00
        ]
    );

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn cpm_z80_examples_assemble_as_com_programs() {
    let root = temp_root("assemble_cpm_examples");
    std::fs::create_dir_all(&root).unwrap();
    let examples = Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/cpm-z80");

    for name in ["console-output", "exit", "file-read", "line-input"] {
        let output = root.join(format!("{name}.com"));
        assemble_file(&AssembleOptions {
            path: examples
                .join(format!("{name}.asm"))
                .to_string_lossy()
                .into_owned(),
            output: Some(output.to_string_lossy().into_owned()),
            base_addr: None,
            assembler_cpu: None,
            layout_path: None,
            map_path: None,
            target: Some("cpm-2.2-z80".to_owned()),
        })
        .unwrap();

        assert!(!std::fs::read(output).unwrap().is_empty());
    }

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn game_boy_vendored_sdk_macros_preprocess_and_assemble() {
    let sdk = Path::new(env!("CARGO_MANIFEST_DIR")).join("toolchains/gameboy-lr35902/sdk/asm/gb");
    let source_path = sdk.join("fixture.asm");
    let source = r#"
            include "color.inc"
            %GB_AUDIO_ENABLE
            ld hl, GB_OAM
            %GB_SPRITE_SET 32, 40, 1, OAMF_XFLIP
            %GB_SPRITE_HIDE
            ld hl, GB_TILE_DATA_0
            ld de, GB_TILE_DATA_0
            ld b, 1
            %GB_TILE_UPLOAD
            ld hl, GB_BG_MAP_0
            xor a
            %GB_TILEMAP_FILL
            %GB_WAVE_LOAD
            %GB_WAVE_PLAY 0, 20h, 0, 80h
            %GB_TIMER_START 0, 0, TAC_ENABLE + TAC_4096_HZ
            %GB_JOYPAD_READ_DPAD
            %GB_SERIAL_START 65
            %GBC_VRAM_BANK 1
            %GBC_WRAM_BANK 2
            %GBC_BG_COLOR_LOW 80h, 1Fh
            halt
        "#;
    let expanded = preprocess_assembly(
        &source_path,
        source,
        "gameboy-color-lr35902",
        AssemblerCpu::Lr35902,
    )
    .unwrap();
    let bytes = ezra::vm::assemble_subset_at(CpuFamily::Lr35902, &expanded.text, 0x0150).unwrap();
    assert!(!bytes.is_empty());
    assert_eq!(bytes.last(), Some(&0x76));
}

#[test]
fn assemble_file_can_write_layout_map() {
    let root = temp_root("assemble_layout_map");
    std::fs::create_dir_all(&root).unwrap();
    let source_path = root.join("main.asm");
    let output_path = root.join("main.com");
    let map_path = root.join("main.map");
    std::fs::write(
        &source_path,
        r#"
            section .text
            start:
                ld c, 00h
                call CPM_BDOS
            "#,
    )
    .unwrap();

    assemble_file(&AssembleOptions {
        path: source_path.to_string_lossy().into_owned(),
        output: Some(output_path.to_string_lossy().into_owned()),
        base_addr: None,
        assembler_cpu: None,
        layout_path: None,
        map_path: Some(map_path.to_string_lossy().into_owned()),
        target: Some("cpm-2.2-z80".to_owned()),
    })
    .unwrap();
    let map = std::fs::read_to_string(map_path).unwrap();

    assert_eq!(
        std::fs::read(output_path).unwrap(),
        [0x0E, 0x00, 0xCD, 0x05, 0x00]
    );
    assert!(map.contains(".text        0x000100"), "{map}");
    assert!(map.contains("start        0x000100"), "{map}");

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn assembly_build_can_reference_layout_symbols() {
    let root = temp_root("build_asm_layout_symbols");
    std::fs::create_dir_all(&root).unwrap();
    let source_path = root.join("exit.asm");
    std::fs::write(
        &source_path,
        r#"
            start:
                ld c, 00h
                call CPM_BDOS
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
        target: Some("cpm-2.2-z80".to_owned()),
    })
    .unwrap();

    assert_eq!(
        std::fs::read(outputs.executable).unwrap(),
        [0x0E, 0x00, 0xCD, 0x05, 0x00]
    );

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn assembly_build_reports_source_location_for_assembler_errors() {
    let root = temp_root("build_asm_diagnostics");
    std::fs::create_dir_all(&root).unwrap();
    let source_path = root.join("bad.asm");
    std::fs::write(
        &source_path,
        r#"
            start:
                not_an_instruction
            "#,
    )
    .unwrap();

    let error = build_source_with_build_options(&BuildCommandOptions {
        path: Some(source_path.to_string_lossy().into_owned()),
        debug_comments: false,
        default_sdk_symbols: true,
        input_kind: Some(InputKind::Assembly),
        assembler_cpu: None,
        layout_path: None,
        target: Some("cpm-2.2-z80".to_owned()),
    })
    .unwrap_err();

    assert!(error.contains("bad.asm:3:17"), "{error}");
    assert!(
        error.contains("test assembler does not support instruction `not_an_instruction`"),
        "{error}"
    );

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn assembly_build_maps_layout_sections_and_includes() {
    let root = temp_root("build_asm_sections");
    std::fs::create_dir_all(&root).unwrap();
    let source_path = root.join("main.asm");
    std::fs::write(root.join("message.inc"), "db \"OK\"\n").unwrap();
    std::fs::write(
        &source_path,
        r#"
            section .text
            start:
                ld c, 00h
                call CPM_BDOS
            section .rodata
            include "message.inc"
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
        target: Some("cpm-2.2-z80".to_owned()),
    })
    .unwrap();
    let map = std::fs::read_to_string(outputs.map).unwrap();

    assert!(map.contains(".text        0x000100"), "{map}");
    assert!(map.contains(".rodata      0x008000"), "{map}");
    assert!(map.contains("start        0x000100"), "{map}");

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn assembly_includes_expand_recursively_with_origins_and_cycles() {
    let root = temp_root("nested_assembly_includes");
    let lib = root.join("lib");
    std::fs::create_dir_all(&lib).unwrap();
    let source_path = root.join("main.asm");
    let outer_path = lib.join("outer.inc");
    let inner_path = lib.join("inner.inc");
    std::fs::write(&source_path, "include \"lib/outer.inc\"\n").unwrap();
    std::fs::write(&outer_path, "include \"inner.inc\"\n").unwrap();
    std::fs::write(&inner_path, "section .text\nret\n").unwrap();

    let source = std::fs::read_to_string(&source_path).unwrap();
    let expanded = expand_assembly_includes(&source_path, &source).unwrap();
    assert_eq!(expanded.text, "section .text\nret\n");
    assert_eq!(
        expanded.line_origins[1].file,
        inner_path.canonicalize().unwrap()
    );
    assert_eq!(expanded.line_origins[1].line, 2);

    std::fs::write(&inner_path, "include \"outer.inc\"\n").unwrap();
    let error = expand_assembly_includes(&source_path, &source).unwrap_err();
    assert!(error.contains("assembly include cycle"), "{error}");
    assert!(error.contains("outer.inc"), "{error}");
    assert!(error.contains("inner.inc"), "{error}");

    std::fs::write(&outer_path, "include \"missing.inc\"\n").unwrap();
    let error = expand_assembly_includes(&source_path, &source).unwrap_err();
    assert!(error.contains("outer.inc:1"), "{error}");
    assert!(error.contains("missing.inc"), "{error}");

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn assembly_preprocessor_expands_vendored_macros_and_target_conditionals() {
    let root = temp_root("assembly_macros");
    std::fs::create_dir_all(root.join("macros")).unwrap();
    let source_path = root.join("main.asm");
    std::fs::write(
        root.join("macros/test.inc"),
        r#"
                %define EXIT_PORT 0Eh
                %macro finish(code)
                    mvi a, $code
                    out ${EXIT_PORT}
                %endmacro
            "#,
    )
    .unwrap();
    let source = r#"
            include "macros/test.inc"
            %if cpu("i8080")
                %finish 1
            %else
                db 0
            %endif
            %macro twice()
            %%loop:
                nop
                jp %%loop
            %endmacro
            %twice
        "#;
    std::fs::write(&source_path, source).unwrap();

    let expanded =
        preprocess_assembly(&source_path, source, "bare-i8080", AssemblerCpu::I8080).unwrap();
    assert!(expanded.text.contains("mvi a, 1"), "{}", expanded.text);
    assert!(expanded.text.contains("out 0Eh"), "{}", expanded.text);
    assert!(!expanded.text.contains("db 0"), "{}", expanded.text);
    assert!(expanded.text.contains("__ezra_macro_"), "{}", expanded.text);
    assert_eq!(
        expanded.line_origins[0].file,
        normalize_include_path(&source_path)
    );

    let base_source = root.join("base.asm");
    let base_output = root.join("base.com");
    std::fs::write(
        &base_source,
        "%macro exit()\nmvi a, 0\nout 0Eh\n%endmacro\n%exit\n",
    )
    .unwrap();
    assemble_file(&AssembleOptions {
        path: base_source.to_string_lossy().into_owned(),
        output: Some(base_output.to_string_lossy().into_owned()),
        base_addr: Some(0x0100),
        assembler_cpu: None,
        layout_path: None,
        map_path: None,
        target: Some("cpm-2.2-i8080".to_owned()),
    })
    .unwrap();
    assert_eq!(
        std::fs::read(base_output).unwrap(),
        [0x3E, 0x00, 0xD3, 0x0E]
    );

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn assembly_errors_in_nested_includes_report_included_file() {
    let root = temp_root("nested_assembly_diagnostic");
    let lib = root.join("lib");
    std::fs::create_dir_all(&lib).unwrap();
    let source_path = root.join("main.asm");
    std::fs::write(&source_path, "include \"lib/outer.inc\"\n").unwrap();
    std::fs::write(lib.join("outer.inc"), "include \"bad.inc\"\n").unwrap();
    std::fs::write(lib.join("bad.inc"), "; first line\nnot_an_instruction\n").unwrap();

    let error = build_source_with_build_options(&BuildCommandOptions {
        path: Some(source_path.to_string_lossy().into_owned()),
        debug_comments: false,
        default_sdk_symbols: true,
        input_kind: Some(InputKind::Assembly),
        assembler_cpu: None,
        layout_path: None,
        target: Some("cpm-2.2-z80".to_owned()),
    })
    .unwrap_err();

    assert!(error.contains("bad.inc:2:1"), "{error}");
    assert!(error.contains("not_an_instruction"), "{error}");

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn assembly_build_respects_custom_layout_entry() {
    let root = temp_root("build_asm_custom_layout");
    std::fs::create_dir_all(&root).unwrap();
    let source_path = root.join("main.asm");
    let layout_path = root.join("custom.ezralayout");
    std::fs::write(&source_path, "ret\n").unwrap();
    std::fs::write(
        &layout_path,
        r#"
            layout custom_z80 {
                load  0x000200;
                entry 0x000220;
                stack 0x00FF00;

                region code    0x000200..0x007FFF read execute;
                region rodata  0x008000..0x009FFF read;
                region ram     0x00A000..0x00BFFF read write;
                region assets  0x00C000..0x00DFFF read;
                region scratch 0x00E000..0x00EFFF read write;
                region stack   0x00F000..0x00FFFF read write reserved;

                section .header  -> code    align 1;
                section .text    -> code    align 16;
                section .rodata  -> rodata  align 16;
                section .data    -> ram     align 16;
                section .bss     -> ram     align 16;
                section .assets  -> assets  align 256;
                section .scratch -> scratch align 16;
            }
            "#,
    )
    .unwrap();

    let outputs = build_source_with_build_options(&BuildCommandOptions {
        path: Some(source_path.to_string_lossy().into_owned()),
        debug_comments: false,
        default_sdk_symbols: true,
        input_kind: Some(InputKind::Assembly),
        assembler_cpu: None,
        layout_path: Some(layout_path.to_string_lossy().into_owned()),
        target: Some("zxspectrum-z80".to_owned()),
    })
    .unwrap();
    let map = std::fs::read_to_string(outputs.map).unwrap();
    let tape = std::fs::read(outputs.executable).unwrap();

    assert!(map.contains(".text        0x000220"), "{map}");
    let loader_data_length = usize::from(u16::from_le_bytes([tape[21], tape[22]]));
    let code_header = 21 + 2 + loader_data_length;
    assert_eq!(
        u16::from_le_bytes([tape[code_header + 16], tape[code_header + 17]]),
        0x0200
    );
    let code_data = code_header + 21;
    assert_eq!(tape[code_data + 3 + 0x20], 0xC9);

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn bare_assembly_targets_cover_each_cpu_mode() {
    let cases = [
        ("bare-i8080", "mvi a, 42h\nret\n", vec![0x3E, 0x42, 0xC9]),
        ("bare-i8085", "rim\nsim\nret\n", vec![0x20, 0x30, 0xC9]),
        ("bare-z80", "ld a, 42h\nret\n", vec![0x3E, 0x42, 0xC9]),
        ("bare-z80n", "ld a, 42h\nret\n", vec![0x3E, 0x42, 0xC9]),
        ("bare-z180", "mlt bc\nret\n", vec![0xED, 0x4C, 0xC9]),
        ("bare-ez80", "ld a, 42h\nret\n", vec![0x3E, 0x42, 0xC9]),
    ];

    for (target, source, expected) in cases {
        let root = temp_root(target);
        std::fs::create_dir_all(&root).unwrap();
        let source_path = root.join("main.asm");
        let output_path = root.join("main.bin");
        std::fs::write(&source_path, source).unwrap();

        assemble_file(&AssembleOptions {
            path: source_path.to_string_lossy().into_owned(),
            output: Some(output_path.to_string_lossy().into_owned()),
            base_addr: None,
            assembler_cpu: None,
            layout_path: None,
            map_path: None,
            target: Some(target.to_owned()),
        })
        .unwrap();

        assert_eq!(std::fs::read(output_path).unwrap(), expected, "{target}");
        let _ = std::fs::remove_dir_all(root);
    }
}

#[test]
fn cpm_z80_harness_runs_complex_assembly_fixture_and_com_format() {
    let root = temp_root("cpm_complex_fixture");
    std::fs::create_dir_all(&root).unwrap();
    let source_path = copy_fixture(&root, "z80_cpm_complex.asm");
    let output_path = root.join("z80_cpm_complex.com");

    assemble_file(&AssembleOptions {
        path: source_path.to_string_lossy().into_owned(),
        output: Some(output_path.to_string_lossy().into_owned()),
        base_addr: Some(0x0100),
        assembler_cpu: None,
        layout_path: None,
        map_path: None,
        target: Some("cpm-2.2-z80".to_owned()),
    })
    .unwrap();
    let bytes = std::fs::read(&output_path).unwrap();
    assert_eq!(output_path.extension().unwrap(), "com");
    assert!(bytes.len() > 12, "{bytes:02X?}");

    let assembly = std::fs::read_to_string(&source_path).unwrap();
    let run = ezra::vm::run_assembly_test_with_cpu_options_at(
        CpuFamily::Z80,
        &assembly,
        &TestRunOptions {
            instruction_budget: 4_000,
            initial_ports: Vec::new(),
            initial_memory: Vec::new(),
            stack_top: 0xFF00,
        },
        0x0100,
    )
    .unwrap();
    assert!(run.halted, "{run:?}");
    assert_eq!(run.debug_output, b"Z80");

    let _ = std::fs::remove_dir_all(root);
}

#[test]
#[cfg(feature = "avr")]
fn arduboy_avr_assembly_smoke_test_writes_hex() {
    let root = temp_root("arduboy_avr_assembly");
    std::fs::create_dir_all(&root).unwrap();
    let asm = root.join("blink.asm");
    std::fs::write(
        &asm,
        "start:\n    ldi r16, 0FFh\n    out 04h, r16\n    sbi 05h, 5\n    rjmp start\n",
    )
    .unwrap();
    let output = root.join("blink.hex");

    assemble_file(&AssembleOptions {
        path: asm.display().to_string(),
        output: Some(output.display().to_string()),
        map_path: None,
        base_addr: Some(0),
        target: Some("arduboy-avr".to_owned()),
        assembler_cpu: None,
        layout_path: None,
    })
    .unwrap();

    let hex = std::fs::read_to_string(output).unwrap();
    assert!(hex.starts_with(":"));
    assert!(hex.contains(":00000001FF"));
    let _ = std::fs::remove_dir_all(root);
}

#[test]
#[cfg(feature = "avr")]
fn avr_aliases_encode_their_underlying_instructions() {
    let cases = [
        ("clr r16", vec![0x00, 0x27]),
        ("lsl r16", vec![0x00, 0x0F]),
        ("tst r16", vec![0x00, 0x23]),
    ];

    for (instruction, expected) in cases {
        let bytes =
            ezra::vm::assemble_subset_with_symbols_at(AssemblerCpu::Avr, instruction, 0).unwrap();
        assert_eq!(bytes.bytes, expected, "{instruction}");
    }
}

#[test]
#[cfg(feature = "chip8")]
fn chip8_family_assembly_targets_encode_dialect_opcodes() {
    let root = temp_root("assemble_chip8_family");
    std::fs::create_dir_all(&root).unwrap();

    let cases = [
        (
            "chip8-vm-chip8",
            "start:\n    cls\n    ld v0, 12h\n    add v0, 1\n    jp start\n",
            vec![0x00, 0xE0, 0x60, 0x12, 0x70, 0x01, 0x12, 0x00],
        ),
        (
            "schip-vm-schip",
            "start:\n    high\n    scroll-down 4\n    drw v0, v1, 0\n    exit\n",
            vec![0x00, 0xFF, 0x00, 0xC4, 0xD0, 0x10, 0x00, 0xFD],
        ),
        (
            "xochip-vm-xochip",
            "start:\n    long i, sprite\n    plane v1\n    audio\nsprite:\n    db 0AAh\n",
            vec![0xF0, 0x00, 0x02, 0x08, 0xF1, 0x01, 0xF0, 0x02, 0xAA],
        ),
    ];

    for (target, source, expected) in cases {
        let source_path = root.join(format!("{target}.asm"));
        let output_path = root.join(format!("{target}.ch8"));
        std::fs::write(&source_path, source).unwrap();
        assemble_file(&AssembleOptions {
            path: source_path.to_string_lossy().into_owned(),
            output: Some(output_path.to_string_lossy().into_owned()),
            base_addr: None,
            assembler_cpu: None,
            layout_path: None,
            map_path: None,
            target: Some(target.to_owned()),
        })
        .unwrap();
        assert_eq!(std::fs::read(output_path).unwrap(), expected, "{target}");
    }

    let _ = std::fs::remove_dir_all(root);
}

#[test]
#[cfg(feature = "chip8")]
fn chip8_rejects_xochip_long_i_instruction() {
    let error =
        ezra::vm::assemble_subset_with_symbols_at(AssemblerCpu::Chip8, "long i, 0x1234\n", 0x0200)
            .unwrap_err();

    assert!(error.message.contains("chip8 assembler"), "{error}");
}

#[test]
#[cfg(feature = "m6800")]
fn assemble_file_writes_m6800_raw_binary() {
    let root = temp_root("assemble_m6800_file");
    std::fs::create_dir_all(&root).unwrap();
    let source_path = root.join("main.asm");
    let output_path = root.join("main.bin");
    std::fs::write(
        &source_path,
        r#"
            VALUE equ 20h
            start:
                ldaa #42h
                staa <VALUE
                ldx #$1234
            loop:
                dex
                bne loop
                jmp >C000h
        "#,
    )
    .unwrap();

    assemble_file(&AssembleOptions {
        path: source_path.to_string_lossy().into_owned(),
        output: Some(output_path.to_string_lossy().into_owned()),
        base_addr: Some(0x8000),
        assembler_cpu: Some(AssemblerCpu::M6800),
        layout_path: None,
        map_path: None,
        target: Some("bare-m6800".to_owned()),
    })
    .unwrap();

    assert_eq!(
        std::fs::read(&output_path).unwrap(),
        [
            0x86, 0x42, 0x97, 0x20, 0xCE, 0x12, 0x34, 0x09, 0x26, 0xFD, 0x7E, 0xC0, 0x00
        ]
    );

    let _ = std::fs::remove_dir_all(root);
}

#[test]
#[cfg(feature = "tms9900")]
fn assemble_file_writes_tms9900_raw_binary() {
    let root = temp_root("assemble_tms9900_file");
    std::fs::create_dir_all(&root).unwrap();
    let source_path = root.join("main.asm");
    let output_path = root.join("main.bin");
    std::fs::write(
        &source_path,
        r#"
            start:
                li r1, >1234
                mov r1, @>8c00
                jmp start
        "#,
    )
    .unwrap();

    assemble_file(&AssembleOptions {
        path: source_path.to_string_lossy().into_owned(),
        output: Some(output_path.to_string_lossy().into_owned()),
        base_addr: Some(0xa000),
        assembler_cpu: None,
        layout_path: None,
        map_path: None,
        target: Some("bare-tms9900".to_owned()),
    })
    .unwrap();

    assert_eq!(
        std::fs::read(&output_path).unwrap(),
        [0x02, 0x01, 0x12, 0x34, 0xc0, 0x60, 0x8c, 0x00, 0x10, 0xfb]
    );

    let _ = std::fs::remove_dir_all(root);
}

#[test]
#[cfg(feature = "m6800")]
fn m6800_rejects_non_m6800_instruction() {
    let error =
        ezra::vm::assemble_subset_with_symbols_at(AssemblerCpu::M6800, "ld a, 7Fh\n", 0x1000)
            .unwrap_err();

    assert!(error.message.contains("M6800 instruction"), "{error}");
}

#[test]
#[cfg(feature = "m6800")]
fn m6800_target_rejects_ezra_source_codegen() {
    let root = temp_root("m6800_source_codegen");
    std::fs::create_dir_all(&root).unwrap();
    let source_path = root.join("main.ezra");
    std::fs::write(&source_path, "fn main() {}\n").unwrap();

    let error = build_source_with_build_options(&BuildCommandOptions {
        path: Some(source_path.to_string_lossy().into_owned()),
        debug_comments: false,
        default_sdk_symbols: false,
        input_kind: Some(InputKind::Ezra),
        assembler_cpu: None,
        layout_path: None,
        target: Some("bare-m6800".to_owned()),
    })
    .unwrap_err();

    assert!(error.contains("CPU `m6800`"), "{error}");

    let _ = std::fs::remove_dir_all(root);
}
