use super::*;

#[test]
fn assemble_options_parse_base_and_output() {
    let options = AssembleOptions::parse(&[
        "--base".to_owned(),
        "040000h".to_owned(),
        "-o".to_owned(),
        "out.bin".to_owned(),
        "main.asm".to_owned(),
    ])
    .unwrap();

    assert_eq!(options.path, PathBuf::from("main.asm"));
    assert_eq!(options.output, Some(PathBuf::from("out.bin")));
    assert_eq!(options.base_addr, Some(0x04_0000));
    assert_eq!(options.target, None);
}

#[test]
fn assemble_options_parse_target() {
    let options = AssembleOptions::parse(&[
        "--target".to_owned(),
        "cpm-2.2-z80".to_owned(),
        "main.asm".to_owned(),
    ])
    .unwrap();

    assert_eq!(options.path, PathBuf::from("main.asm"));
    assert_eq!(options.target, Some("cpm-2.2-z80".to_owned()));
    assert_eq!(options.output, None);
    assert_eq!(options.base_addr, None);
}

#[test]
fn build_options_parse_input_kind() {
    let options = BuildCommandOptions::parse(&[
        "--input-kind".to_owned(),
        "assembly".to_owned(),
        "--cpu".to_owned(),
        "z180".to_owned(),
        "main.txt".to_owned(),
    ])
    .unwrap();

    assert_eq!(options.path.as_deref(), Some(Path::new("main.txt")));
    assert_eq!(options.input_kind, Some(InputKind::Assembly));
    assert_eq!(options.assembler_cpu, Some(AssemblerCpu::Z180));
}

#[test]
fn source_commands_parse_optimization_levels_and_pass_overrides() {
    let build = BuildCommandOptions::parse(&[
        "-O3".to_owned(),
        "--disable-optimization".to_owned(),
        "function-inlining".to_owned(),
        "--enable-optimization".to_owned(),
        "idempotent-operations".to_owned(),
        "main.ezra".to_owned(),
    ])
    .unwrap();
    assert_eq!(build.optimization_level, Some(3));
    assert_eq!(
        build.disable_optimizations,
        [OptimizationPass::FunctionInlining]
    );
    assert_eq!(
        build.enable_optimizations,
        [OptimizationPass::IdempotentOperations]
    );

    let command = CommandOptions::parse(&[
        "--opt-level".to_owned(),
        "0".to_owned(),
        "main.ezra".to_owned(),
    ])
    .unwrap();
    assert_eq!(command.optimization_level, Some(0));
}

#[test]
fn source_commands_reject_unknown_optimization_options() {
    let level = CommandOptions::parse(&["-O9".to_owned(), "main.ezra".to_owned()]).unwrap_err();
    assert!(level.contains("0, 1, 2, or 3"), "{level}");

    let pass = CommandOptions::parse(&[
        "--enable-optimization".to_owned(),
        "unknown".to_owned(),
        "main.ezra".to_owned(),
    ])
    .unwrap_err();
    assert!(pass.contains("scalar-simplification"), "{pass}");
}

#[test]
fn size_budget_options_parse_target_sections_and_hex_counts() {
    let (remaining, budgets) = parse_size_budget_args(&[
        "--size-budget".to_owned(),
        "target=0x200".to_owned(),
        "--size-budget=.text=1_024".to_owned(),
        "--size-budget".to_owned(),
        "runtime_helpers=20h".to_owned(),
        "main.ezra".to_owned(),
    ])
    .unwrap();

    assert_eq!(remaining, ["main.ezra"]);
    assert_eq!(budgets.target, Some(0x200));
    assert_eq!(budgets.sections.get(".text"), Some(&1_024));
    assert_eq!(budgets.runtime_helpers, Some(0x20));
}

#[test]
fn size_budget_options_report_invalid_specifications() {
    let error = parse_size_budget_args(&["--size-budget=.text=nope".to_owned()]).unwrap_err();
    assert!(error.contains("invalid size budget byte count"), "{error}");
}

#[test]
fn assemble_options_parse_cpu() {
    let options =
        AssembleOptions::parse(&["--cpu".to_owned(), "z80n".to_owned(), "main.asm".to_owned()])
            .unwrap();

    assert_eq!(options.path, PathBuf::from("main.asm"));
    assert_eq!(options.assembler_cpu, Some(AssemblerCpu::Z80N));
}

#[test]
fn assemble_options_parse_r800_cpu() {
    let options =
        AssembleOptions::parse(&["--cpu".to_owned(), "r800".to_owned(), "main.asm".to_owned()])
            .unwrap();

    assert_eq!(options.assembler_cpu, Some(AssemblerCpu::R800));
}

#[test]
fn emit_ir_options_parse_stage() {
    let options = EmitIrOptions::parse(&[
        "--stage".to_owned(),
        "hir".to_owned(),
        "game.ezra".to_owned(),
    ])
    .unwrap();

    assert_eq!(options.stage, IrStage::Hir);
    assert_eq!(options.command.path, PathBuf::from("game.ezra"));

    let ezir = EmitIrOptions::parse(&[
        "--stage".to_owned(),
        "ezir".to_owned(),
        "game.ezra".to_owned(),
    ])
    .unwrap();
    assert_eq!(ezir.stage, IrStage::Ezir);
}

#[test]
fn init_options_parse_path_name_target_and_force() {
    let options = InitOptions::parse(&[
        "--name".to_owned(),
        "cafe".to_owned(),
        "--target".to_owned(),
        "agonlight-mos-ez80".to_owned(),
        "--force".to_owned(),
        "game".to_owned(),
    ])
    .unwrap();

    assert_eq!(options.path, PathBuf::from("game"));
    assert_eq!(options.name.as_deref(), Some("cafe"));
    assert_eq!(options.target, "agonlight-mos-ez80");
    assert!(options.force);
}

#[test]
fn disk_options_parse_format_label_output_and_named_files() {
    let options = DiskCommandOptions::parse(&[
        "--format".to_owned(),
        "dcpu".to_owned(),
        "--label".to_owned(),
        "TOOLS".to_owned(),
        "--output".to_owned(),
        "tools.dsk".to_owned(),
        "--file".to_owned(),
        "BOOT.BIN=build/main.bin".to_owned(),
        "README.TXT".to_owned(),
    ])
    .unwrap();

    assert_eq!(options.format, DiskFormat::M35Fd);
    assert_eq!(options.label, "TOOLS");
    assert_eq!(options.output, PathBuf::from("tools.dsk"));
    assert_eq!(
        options.files,
        [
            DiskInput {
                name: "BOOT.BIN".to_owned(),
                path: PathBuf::from("build/main.bin"),
            },
            DiskInput {
                name: "README.TXT".to_owned(),
                path: PathBuf::from("README.TXT"),
            },
        ]
    );
}

#[test]
fn disk_options_infer_common_image_extensions() {
    for (output, expected) in [
        ("cpm.dsk", DiskFormat::Fat12_720K),
        ("dos.IMG", DiskFormat::Fat12_1440K),
        ("game.d64", DiskFormat::Commodore1541),
    ] {
        let options =
            DiskCommandOptions::parse(&["--output".to_owned(), output.to_owned()]).unwrap();
        assert_eq!(options.format, expected);
    }
}

#[test]
fn disk_options_accept_an_equals_path_with_an_explicit_file_syntax() {
    let options = DiskCommandOptions::parse(&[
        "--output".to_owned(),
        "image.dsk".to_owned(),
        "--file=assets/level=one.bin".to_owned(),
    ])
    .unwrap();

    assert_eq!(options.files[0].name, "level=one.bin");
    assert_eq!(options.files[0].path, PathBuf::from("assets/level=one.bin"));
}

#[cfg(unix)]
#[test]
fn cli_option_parsing_preserves_non_utf8_paths() {
    use std::os::unix::ffi::{OsStrExt, OsStringExt};

    let path = std::ffi::OsString::from_vec(b"source-\xFF.asm".to_vec());
    let options = BuildCommandOptions::parse(&[path.clone()]).unwrap();
    assert_eq!(
        options.path.as_deref().unwrap().as_os_str().as_bytes(),
        path.as_os_str().as_bytes()
    );
}

#[test]
fn install_syntax_options_require_editor_selection() {
    let error = InstallSyntaxOptions::parse::<String>(&[]).unwrap_err();

    assert!(error.contains("requires `--all`"), "{error}");
}

#[test]
fn install_syntax_options_parse_selected_editors() {
    let options = InstallSyntaxOptions::parse(&[
        "--editor".to_owned(),
        "vim".to_owned(),
        "nvim".to_owned(),
        "--dry-run".to_owned(),
    ])
    .unwrap();

    assert_eq!(options.editors, [SyntaxEditor::Vim, SyntaxEditor::Neovim]);
    assert!(options.dry_run);
    assert!(!options.all);
}

#[test]
fn micro_config_home_prefers_micro_then_xdg_then_home() {
    let home = PathBuf::from("home");
    let xdg = PathBuf::from("xdg");
    let micro = PathBuf::from("custom-micro");

    assert_eq!(
        resolve_micro_config_home(Some(micro.clone()), Some(xdg.clone()), Ok(home.clone()))
            .unwrap(),
        micro
    );
    assert_eq!(
        resolve_micro_config_home(None, Some(xdg.clone()), Ok(home.clone())).unwrap(),
        xdg.join("micro")
    );
    assert_eq!(
        resolve_micro_config_home(None, None, Ok(home.clone())).unwrap(),
        home.join(".config/micro")
    );
}

#[test]
fn micro_config_home_only_needs_home_for_its_fallback() {
    assert_eq!(
        resolve_micro_config_home(
            Some(PathBuf::from("custom-micro")),
            None,
            Err("no home".to_owned()),
        )
        .unwrap(),
        PathBuf::from("custom-micro")
    );
    assert_eq!(
        resolve_micro_config_home(None, Some(PathBuf::from("xdg")), Err("no home".to_owned()),)
            .unwrap(),
        PathBuf::from("xdg/micro")
    );
}
