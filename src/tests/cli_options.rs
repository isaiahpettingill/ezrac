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

    assert_eq!(options.path, "main.asm");
    assert_eq!(options.output, Some("out.bin".to_owned()));
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

    assert_eq!(options.path, "main.asm");
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

    assert_eq!(options.path.as_deref(), Some("main.txt"));
    assert_eq!(options.input_kind, Some(InputKind::Assembly));
    assert_eq!(options.assembler_cpu, Some(AssemblerCpu::Z180));
}

#[test]
fn assemble_options_parse_cpu() {
    let options =
        AssembleOptions::parse(&["--cpu".to_owned(), "z80n".to_owned(), "main.asm".to_owned()])
            .unwrap();

    assert_eq!(options.path, "main.asm");
    assert_eq!(options.assembler_cpu, Some(AssemblerCpu::Z80N));
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
    assert_eq!(options.command.path, "game.ezra");
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
fn install_syntax_options_require_editor_selection() {
    let error = InstallSyntaxOptions::parse(&[]).unwrap_err();

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
