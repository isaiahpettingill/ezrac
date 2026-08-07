use super::*;
use crate::ast::{EmbedSource, Expr};

fn materialized_embed_bytes(program: &Program, name: &str) -> Vec<u8> {
    let embed = program
        .declarations
        .iter()
        .find_map(|declaration| match declaration {
            Declaration::Embed(embed) if embed.name == name => Some(embed),
            _ => None,
        })
        .expect("materialized embed declaration");
    let EmbedSource::Bytes(values) = &embed.source else {
        panic!("workspace file embed was not materialized");
    };
    values
        .iter()
        .map(|value| match value {
            Expr::Int(value) => *value as u8,
            _ => panic!("materialized workspace byte is not an integer"),
        })
        .collect()
}

#[test]
fn artifact_size_report_is_stable_and_separates_payload_from_address_gaps() {
    let report = ArtifactSizeReport::from_sections(
        &[
            ArtifactSectionMeasurement {
                name: ".text".to_owned(),
                start: 0x100,
                end: 0x110,
                size: 0x10,
            },
            ArtifactSectionMeasurement {
                name: ".bss".to_owned(),
                start: 0x120,
                end: 0x128,
                size: 8,
            },
            ArtifactSectionMeasurement {
                name: ".assets".to_owned(),
                start: 0x200,
                end: 0x203,
                size: 3,
            },
        ],
        &[],
        23,
    );

    assert_eq!(report.text, 0x10);
    assert_eq!(report.assets, 3);
    assert_eq!(report.bss, 8);
    assert_eq!(report.machine_code_payload, 0x13);
    assert_eq!(report.address_span, 0x103);
    assert_eq!(report.address_gaps, 0x103 - 0x1B);
    assert_eq!(report.final_package, 23);
    assert_eq!(
        report.to_stable_string(),
        "ezrac-size-v1\ntext=16\nrodata=0\ninitialized_data=0\nassets=3\nbss=8\nruntime_helpers=unknown\nmachine_code_payload=19\naddress_span=259\naddress_gaps=232\nfinal_package=23\nsection:.assets=3\nsection:.bss=8\nsection:.text=16\n"
    );
}

#[test]
fn size_budget_diagnostics_name_the_overflowing_section() {
    let files = [WorkspaceFile::text("main.ezra", "fn main() {}")];
    let mut build = BuildRequest::for_target("cpm-2.2-z80").unwrap();
    build.size_budgets = SizeBudgets::default().section(".text", 0);
    let error = build_workspace_with_request(
        &Workspace::new(&files),
        "main.ezra",
        &CompileRequest::new("main.ezra", "cpm-2.2-z80"),
        &build,
    )
    .unwrap_err();

    assert!(
        error.message.contains("size budget exceeded for `.text`"),
        "{error}"
    );
    assert!(error.message.contains("reduce that section"), "{error}");
}

#[test]
fn compiles_in_memory_source_to_ez80_assembly() {
    let request = CompileRequest::new("memory.ezra", "custom-unknown-ez80");
    let compilation = compile_source_to_assembly("fn main() {}", &request).unwrap();

    assert!(compilation.report.has_main);
    assert!(compilation.assembly.contains("__ezra_start:"));
}

#[cfg(feature = "i8086")]
#[test]
fn compiles_in_memory_source_to_i8086_assembly() {
    let request = CompileRequest::new("memory.ezra", "bare-i8086");
    let compilation = compile_source_to_assembly(
            "fn twice(value: u16) -> u16 { return value * 2 }\nfn main() { let value: u16 = twice(21) }",
            &request,
        )
        .unwrap();

    assert!(compilation.report.has_main);
    assert!(compilation.assembly.contains("target: Intel 8086"));
    assert!(compilation.assembly.contains("call near _twice"));
}

#[cfg(feature = "i8086")]
#[test]
fn arbitrary_i8086_target_uses_a_16_bit_layout() {
    let files = [WorkspaceFile::text("main.ezra", "fn main() {}")];
    let request = CompileRequest::new("main.ezra", "custom-board-i8086");
    let build = build_workspace(&Workspace::new(&files), "main.ezra", &request).unwrap();
    let options = assembly_options_for_target(&request.target, CpuFamily::I8086, false, true);

    assert_eq!(options.entry_addr.get(), 0);
    assert_eq!(options.stack_top.get(), 0xFFFF);
    assert!(!build.machine_code.is_empty());
    assert!(build.map.contains(".text        0x000000"));
    assert!(!build.map.contains(".header"));
}

#[cfg(feature = "avr")]
#[test]
fn compiles_in_memory_source_to_avr_assembly() {
    let request = CompileRequest::new("memory.ezra", "bare-avr");
    let compilation = compile_source_to_assembly("fn main() {}", &request).unwrap();

    assert!(compilation.assembly.contains("target: AVR register ABI"));
}

#[cfg(feature = "mos6502")]
#[test]
fn compiles_nes_source_with_the_ricoh_2a03_backend() {
    let compilation = compile_source_to_assembly(
        "fn main() {}",
        &CompileRequest::new("memory.ezra", "nes-2a03"),
    )
    .unwrap();

    assert!(compilation.assembly.contains("target: MOS 6502"));
    assert!(compilation.assembly.contains("__ezra_start:"));
}

#[test]
fn compiles_all_builtin_sms_sdk_modules() {
    let source = "import sms.system\nimport sms.vdp\nimport sms.video\nimport sms.palette\nimport sms.memory\nimport sms.input\nfn main() { video.init_mode4(); palette.set_background(0, palette.BLUE); video.set_name_table_entry(0, 0); system.wait_vblank(); let player1: u8 = input.read_player1(); let player2: u8 = input.read_player2(); let status: u8 = vdp.read_status(); if (status | player1 | player2) != 0 { video.enable_display() } }";
    let compilation = compile_source_to_assembly(
        source,
        &CompileRequest::new("memory.ezra", "sega-master-system-z80"),
    )
    .unwrap();

    assert!(compilation.assembly.contains("out (BEh), a"));
    assert!(compilation.assembly.contains("in a, (BFh)"));
    assert!(compilation.assembly.contains("in a, (DCh)"));
    assert!(compilation.assembly.contains("in a, (DDh)"));
    assert!(compilation.assembly.contains("ld sp, DFF0h"));
}

#[cfg(feature = "mos6502")]
#[test]
fn compiles_all_builtin_nes_sdk_modules() {
    let source = "import nes.ppu\nimport nes.palette\nimport nes.sprites\nimport nes.input\nimport nes.audio\nimport nes.timing\nimport nes.memory\nfn main() { ppu.disable_rendering(); audio.disable(); timing.wait_two_vblanks(); palette.set_background(palette.DARK_BLUE); palette.set_sprite_color(0, 1, palette.WHITE); sprites.set(0, 120, 112, 0, 0); let buttons: u8 = input.read_controller1(); if buttons != 0 { ppu.set_mask(ppu.MASK_SPRITES) } memory.clear_internal_ram() }";
    let compilation =
        compile_source_to_assembly(source, &CompileRequest::new("memory.ezra", "nes-2a03"))
            .unwrap();

    assert!(compilation.assembly.contains("sta $2004"));
    assert!(compilation.assembly.contains("sta $4016"));
    assert!(compilation.assembly.contains("sta $0700,x"));
}

#[cfg(feature = "i8086")]
#[test]
fn imported_aliases_are_resolved_before_i8086_inline_asm_class_validation() {
    let files = [
        WorkspaceFile::text(
            "main.ezra",
            "import types\nfn main() { let value: Word = 1 asm(in value: Word as reg16, clobber ax) { \"nop\" } }",
        ),
        WorkspaceFile::text("types.ezra", "pub alias Word = u16"),
    ];
    let compilation = compile_workspace_to_assembly(
        &Workspace::new(&files),
        "main.ezra",
        &CompileRequest::new("main.ezra", "bare-i8086"),
    )
    .unwrap();

    assert!(compilation.assembly.contains("mov ax,"));
}

#[cfg(feature = "i8086")]
#[test]
fn virtual_workspace_compilation_strictly_validates_i8086_inline_assembly() {
    let files = [WorkspaceFile::text(
        "main.ezra",
        "fn main() { asm volatile { \"pusha\" } }",
    )];
    let error = compile_workspace_to_assembly(
        &Workspace::new(&files),
        "main.ezra",
        &CompileRequest::new("main.ezra", "bare-i8086"),
    )
    .unwrap_err();

    assert!(
        error
            .message
            .contains("assembler does not support 8086 instruction `pusha`"),
        "{error}"
    );
}

#[test]
fn build_layout_validation_rejects_text_that_exceeds_its_region() {
    let layout = Layout::bare_16("i8086");
    let error = validate_text_section_fit(&layout, 0x8001).unwrap_err();

    assert_eq!(
        error.message,
        "section `.text` does not fit in region `code`"
    );
}

#[test]
fn compiles_imports_from_a_virtual_workspace() {
    let files = [
        WorkspaceFile::text(
            "src/main.ezra",
            "import math\nfn main() { let value: u8 = math.VALUE }\n",
        ),
        WorkspaceFile::text("src/math.ezra", "pub const VALUE: u8 = 42\n"),
    ];
    let request = CompileRequest::new("ignored.ezra", "custom-unknown-ez80");
    let compilation =
        compile_workspace_to_assembly(&Workspace::new(&files), "src/main.ezra", &request).unwrap();

    assert!(compilation.report.has_main);
    assert!(compilation.assembly.contains("_main:"));
}

#[test]
fn materializes_root_relative_workspace_assets() {
    let files = [
        WorkspaceFile::text(
            "src/main.ezra",
            "embed blob: bytes = file(\"assets/blob.bin\")\nfn main() {}\n",
        ),
        WorkspaceFile::new("src/assets/blob.bin", &[0xA5, 0x00, 0xFF]),
    ];
    let compilation = compile_workspace_to_assembly(
        &Workspace::new(&files),
        "src/main.ezra",
        &CompileRequest::new("ignored.ezra", "cpm-2.2-z80"),
    )
    .unwrap();

    assert_eq!(
        materialized_embed_bytes(&compilation.program, "blob"),
        [0xA5, 0x00, 0xFF]
    );
}

#[test]
fn materializes_imported_module_relative_workspace_assets() {
    let files = [
        WorkspaceFile::text("src/main.ezra", "import lib.media\nfn main() {}\n"),
        WorkspaceFile::text(
            "src/lib/media.ezra",
            "pub embed sprite: bytes = file(\"assets/sprite.bin\")\n",
        ),
        WorkspaceFile::new("src/lib/assets/sprite.bin", &[0xDE, 0xAD]),
    ];
    let build = build_workspace(
        &Workspace::new(&files),
        "src/main.ezra",
        &CompileRequest::new("ignored.ezra", "cpm-2.2-z80"),
    )
    .unwrap();

    assert_eq!(
        materialized_embed_bytes(&build.program, "sprite"),
        [0xDE, 0xAD]
    );
    assert!(!build.machine_code.is_empty());
}

#[test]
fn reports_missing_virtual_workspace_assets() {
    let files = [WorkspaceFile::text(
        "src/main.ezra",
        "embed blob: bytes = file(\"assets/missing.bin\")\nfn main() {}\n",
    )];
    let error = compile_workspace_to_assembly(
        &Workspace::new(&files),
        "src/main.ezra",
        &CompileRequest::new("ignored.ezra", "cpm-2.2-z80"),
    )
    .unwrap_err();

    assert_eq!(
        error.message,
        "virtual workspace asset `assets/missing.bin` referenced from `src/main.ezra` was not found (resolved as `src/assets/missing.bin`)"
    );
}

#[test]
fn skips_inactive_root_and_imported_workspace_embeds_before_materializing() {
    let files = [
        WorkspaceFile::text(
            "src/main.ezra",
            r#"
                    @cfg(cpu("ez80"))
                    embed root_missing: bytes = file("assets/root-missing.bin")
                    import lib.media
                    fn main() {}
                "#,
        ),
        WorkspaceFile::text(
            "src/lib/media.ezra",
            r#"
                    @cfg(cpu("ez80"))
                    pub embed imported_missing: bytes = file("assets/imported-missing.bin")
                "#,
        ),
    ];
    let compilation = compile_workspace_to_assembly(
        &Workspace::new(&files),
        "src/main.ezra",
        &CompileRequest::new("ignored.ezra", "cpm-2.2-z80"),
    )
    .unwrap();

    assert!(!compilation.program.declarations.iter().any(|declaration| {
        matches!(
            declaration,
            Declaration::Embed(embed)
                if matches!(embed.name.as_str(), "root_missing" | "imported_missing")
        )
    }));
}

#[test]
fn active_root_workspace_embeds_still_require_assets() {
    let files = [WorkspaceFile::text(
        "src/main.ezra",
        r#"
                @cfg(cpu("z80"))
                embed root_missing: bytes = file("assets/root-missing.bin")
                fn main() {}
            "#,
    )];
    let error = compile_workspace_to_assembly(
        &Workspace::new(&files),
        "src/main.ezra",
        &CompileRequest::new("ignored.ezra", "cpm-2.2-z80"),
    )
    .unwrap_err();

    assert_eq!(
        error.message,
        "virtual workspace asset `assets/root-missing.bin` referenced from `src/main.ezra` was not found (resolved as `src/assets/root-missing.bin`)"
    );
}

#[test]
fn active_imported_workspace_embeds_still_require_assets() {
    let files = [
        WorkspaceFile::text("src/main.ezra", "import lib.media\nfn main() {}\n"),
        WorkspaceFile::text(
            "src/lib/media.ezra",
            r#"
                    @cfg(cpu("z80"))
                    pub embed imported_missing: bytes = file("assets/imported-missing.bin")
                "#,
        ),
    ];
    let error = compile_workspace_to_assembly(
        &Workspace::new(&files),
        "src/main.ezra",
        &CompileRequest::new("ignored.ezra", "cpm-2.2-z80"),
    )
    .unwrap_err();

    assert_eq!(
        error.message,
        "virtual workspace asset `assets/imported-missing.bin` referenced from `src/lib/media.ezra` was not found (resolved as `src/lib/assets/imported-missing.bin`)"
    );
}

#[test]
fn builds_and_packages_virtual_workspace_for_agon() {
    let files = [WorkspaceFile::text(
        "main.ezra",
        r#"
                fn main() {
                    let text: ptr<u8> = "OK"
                    test.assert_eq_u8(*text, 'O', 1)
                }
            "#,
    )];
    let build = build_workspace(
        &Workspace::new(&files),
        "main.ezra",
        &CompileRequest::new("main.ezra", "agonlight-mos-ez80"),
    )
    .unwrap();

    assert_eq!(build.executable_extension, "bin");
    assert_eq!(&build.executable[64..69], b"MOS\0\x01");
    assert!(build.assembly.contains("section .rodata\norg 060000h"));
    assert!(build.machine_code.len() > 0x05_FFFF - 0x04_0045 + 1);
}

#[test]
fn builds_and_packages_virtual_workspace_for_cpm() {
    let files = [WorkspaceFile::text("main.ezra", "fn main() {}")];
    let build = build_workspace(
        &Workspace::new(&files),
        "main.ezra",
        &CompileRequest::new("main.ezra", "cpm-2.2-z80"),
    )
    .unwrap();

    assert_eq!(build.executable_extension, "com");
    assert_eq!(build.executable, build.machine_code);
}

#[cfg(feature = "mos6502")]
#[test]
fn builds_and_packages_virtual_workspace_for_c64() {
    let files = [WorkspaceFile::text("main.ezra", "fn main() {}")];
    let build = build_workspace(
        &Workspace::new(&files),
        "main.ezra",
        &CompileRequest::new("main.ezra", "commodore64-6502"),
    )
    .unwrap();

    assert_eq!(build.executable_extension, "prg");
    assert_eq!(&build.executable[..2], &[0x01, 0x08]);
}

#[test]
fn links_multisection_assembly_with_a_public_build_request() {
    let mut build = BuildRequest::for_target("agonlight-mos-ez80").unwrap();
    build.package_context.image_kind = crate::package::PackageImageKind::LoadImage;
    let assembly = r#"
            section .header
                db 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0A5h
            section .text
                ld hl, rodata_value
                ld de, data_value
                ret
            section .rodata
            rodata_value:
                db 0AAh, 0BBh
            section .data
            data_value:
                db 0CCh, 0DDh
        "#;
    let preprocessed = preprocess_assembly_source(
        "memory.asm",
        assembly,
        AssemblyPreprocessOptions::for_compiled_features("agonlight-mos-ez80", "ez80"),
    )
    .unwrap();
    let linked =
        link_assembly_program(Path::new("memory.asm"), &preprocessed.program, &build).unwrap();

    assert!(linked.map.contains(".rodata"));
    assert!(linked.map.contains(".data"));
    assert_eq!(&linked.executable[64..69], b"MOS\0\x01");
    assert_eq!(linked.executable[10], 0xA5);
}

#[test]
fn links_flat_assembly_at_an_explicit_base_with_a_public_build_request() {
    let build = BuildRequest::for_target("cpm-2.2-z80").unwrap();
    let preprocessed = preprocess_assembly_source(
        "memory.asm",
        "start:\n    jp start\n",
        AssemblyPreprocessOptions::for_compiled_features("cpm-2.2-z80", "z80"),
    )
    .unwrap();
    let linked = link_assembly_program_at(
        Path::new("memory.asm"),
        &preprocessed.program,
        0x0200,
        &build,
    )
    .unwrap();

    assert_eq!(&linked.machine_code[..3], &[0xC3, 0x00, 0x02]);
    assert_eq!(linked.executable, linked.machine_code);
    assert!(
        linked.map.contains(".text        0x000100"),
        "{}",
        linked.map
    );
    assert!(
        linked.map.contains("start        0x000200"),
        "{}",
        linked.map
    );
}

#[test]
fn workspace_build_honors_explicit_output_format() {
    let files = [WorkspaceFile::text("main.ezra", "fn main() {}")];
    let request = CompileRequest::new("main.ezra", "custom-unknown-ez80");
    let mut build = BuildRequest::for_target("custom-unknown-ez80").unwrap();
    build.output_format = OutputFormat::IntelHex;
    let result =
        build_workspace_with_request(&Workspace::new(&files), "main.ezra", &request, &build)
            .unwrap();

    assert_eq!(result.output_format, OutputFormat::IntelHex);
    assert_eq!(result.executable_extension, "hex");
    assert!(result.executable.starts_with(b":02000004"));
    assert!(result.executable.ends_with(b":00000001FF\n"));
}

#[test]
fn rejects_incompatible_platform_cpu_combinations() {
    let request = CompileRequest::new("memory.ezra", "zxspectrum-ez80");
    let error = compile_source_to_assembly("fn main() {}", &request).unwrap_err();

    assert_eq!(
        error.message,
        "target `zxspectrum-ez80` requires CPU `z80`, not `ez80`"
    );
}

#[test]
fn resolves_sdk_roots_for_in_memory_compilation() {
    let root = std::env::temp_dir().join(format!("ezrac-api-{}", std::process::id()));
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join("math.ezra"), "pub const VALUE: u8 = 42\n").unwrap();
    let mut request = CompileRequest::new(root.join("main.ezra"), "custom-unknown-ez80");
    request.sdk_paths.push(root.clone());

    let compilation = compile_source_to_assembly(
        "import math\nfn main() { let value: u8 = math.VALUE }\n",
        &request,
    )
    .unwrap();
    assert!(compilation.report.has_main);

    let _ = std::fs::remove_dir_all(root);
}

#[cfg(feature = "i8086")]
#[test]
fn builds_msdos_com_as_raw_bytes_at_0100h_with_a_reserved_psp() {
    let profile = resolve_target_profile(Some("msdos-com-i8086")).unwrap();
    let layout = default_layout_for_target("msdos-com-i8086");
    let options = assembly_options_for_target("msdos-com-i8086", CpuFamily::I8086, false, true);
    let files = [WorkspaceFile::text("main.ezra", "fn main() {}")];
    let build = build_workspace(
        &Workspace::new(&files),
        "main.ezra",
        &CompileRequest::new("main.ezra", "msdos-com-i8086"),
    )
    .unwrap();
    let start = build
        .symbols
        .iter()
        .find(|symbol| symbol.name == "__ezra_start")
        .unwrap();
    let psp = layout
        .regions
        .iter()
        .find(|region| region.name == "psp")
        .unwrap();

    assert_eq!(profile.output_format, OutputFormat::CpmCom);
    assert_eq!(profile.output_format.extension(), "com");
    assert_eq!(layout.load.get(), 0x0100);
    assert_eq!(layout.entry.get(), 0x0100);
    assert_eq!((psp.start.get(), psp.end.get()), (0, 0x00FF));
    assert!(psp.flags.contains(crate::layout::RegionFlags::RESERVED));
    assert!(
        layout
            .regions
            .iter()
            .all(|region| region.end.get() <= 0xFFFF)
    );
    assert!(
        layout
            .symbols
            .iter()
            .all(|symbol| symbol.value.get() <= 0xFFFF)
    );
    assert_eq!(options.entry_addr.get(), 0x0100);
    assert!(options.dos_executable);
    assert_eq!(start.addr, 0x0100);
    assert_eq!(build.executable_extension, "com");
    assert_eq!(build.executable, build.machine_code);
    assert!(build.assembly.contains("    mov ax,0x4c00\n    int 0x21\n"));
}

#[cfg(feature = "i8086")]
#[test]
fn std_api_accepts_versioned_msdos_i8086_targets() {
    let request = CompileRequest::new("main.ezra", "msdos-com-i8086-6.22");
    let files = [WorkspaceFile::text("main.ezra", "fn main() {}")];
    let build = build_workspace(&Workspace::new(&files), "main.ezra", &request).unwrap();
    let options = assembly_options_for_target(&request.target, CpuFamily::I8086, false, true);

    assert_eq!(build.output_format, OutputFormat::CpmCom);
    assert_eq!(build.executable_extension, "com");
    assert!(options.dos_executable);
    assert!(build.assembly.contains("    int 0x21\n"));
}

#[test]
fn rejects_non_i8086_and_noncanonical_msdos_targets() {
    let cpu_error = resolve_target_profile(Some("msdos-com-z80")).unwrap_err();
    assert_eq!(
        cpu_error,
        "target `msdos-com-z80` requires CPU `i8086`, not `z80`"
    );

    #[cfg(feature = "i8086")]
    {
        let name_error = resolve_target_profile(Some("msdos-i8086")).unwrap_err();
        assert_eq!(
            name_error,
            "unsupported MS-DOS target `msdos-i8086`; expected `msdos-com-i8086`"
        );
    }
}

#[test]
fn ez180n_gaem_uses_a_flat_output_map() {
    for target in [
        "ez180n-i8080",
        "ez180n-i8085",
        "ez180n-z80",
        "ez180n-z80n",
        "ez180n-z180",
        "ez180n-ez80",
    ] {
        let build = BuildRequest::for_target(target).unwrap();
        assert!(uses_flat_output_map(&build), "{target}");
    }
}
