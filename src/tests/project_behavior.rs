use super::*;

#[test]
fn init_project_writes_default_scaffold() {
    let root = temp_root("init_project");
    init_project(&InitOptions {
        path: root.clone(),
        name: Some("demo".to_owned()),
        target: "agonlight-mos-ez80".to_owned(),
        force: false,
    })
    .unwrap();

    let config = std::fs::read_to_string(root.join("Ezra.toml")).unwrap();
    let main = std::fs::read_to_string(root.join("src/main.ezra")).unwrap();
    let gitignore = std::fs::read_to_string(root.join(".gitignore")).unwrap();

    assert!(config.contains("name = \"demo\""), "{config}");
    assert!(
        config.contains("target = \"agonlight-mos-ez80\""),
        "{config}"
    );
    assert!(main.contains("import agon.console"), "{main}");
    assert!(main.contains("console.print_line"), "{main}");
    assert!(gitignore.contains("target/"), "{gitignore}");

    let error = init_project(&InitOptions {
        path: root.clone(),
        name: Some("demo".to_owned()),
        target: "agonlight-mos-ez80".to_owned(),
        force: false,
    })
    .unwrap_err();
    assert!(error.contains("refusing to overwrite"), "{error}");

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn build_uses_project_input_kind_for_assembly() {
    let root = temp_root("build_asm_project_kind");
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(
        root.join("Ezra.toml"),
        r#"
            [project]
            name = "asm-demo"

            [build]
            target = "cpm-2.2-z80"
            output = "com"
            input_kind = "assembly"
            executable = "demo"
            "#,
    )
    .unwrap();
    let source_path = root.join("src/main.txt");
    std::fs::write(
        &source_path,
        r#"
            start:
                ld c, 00h
                call 0005h
            "#,
    )
    .unwrap();

    let outputs = build_source(source_path.to_str().unwrap()).unwrap();
    let expected_base = root.join("target/cpm-2.2-z80/src/demo");

    assert_eq!(outputs.asm, expected_base.with_extension("asm"));
    assert_eq!(outputs.map, expected_base.with_extension("map"));
    assert_eq!(outputs.executable, expected_base.with_extension("com"));
    assert_eq!(
        std::fs::read(outputs.executable).unwrap(),
        [0x0E, 0x00, 0xCD, 0x05, 0x00]
    );

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn build_uses_project_input_when_path_is_omitted() {
    let _lock = CWD_LOCK.lock().unwrap();
    let root = temp_root("build_project_input");
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(
        root.join("Ezra.toml"),
        r#"
            [project]
            name = "asm-demo"

            [build]
            input = "src/main.asm"
            target = "cpm-2.2-z80"
            input_kind = "assembly"
            executable = "demo"
            "#,
    )
    .unwrap();
    std::fs::write(
        root.join("src/main.asm"),
        r#"
            start:
                ld c, 00h
                call 0005h
            "#,
    )
    .unwrap();

    let _cwd = CurrentDirGuard::switch_to(&root);
    let outputs = build_source_with_build_options(&BuildCommandOptions {
        path: None,
        debug_comments: false,
        default_sdk_symbols: true,
        input_kind: None,
        assembler_cpu: None,
        layout_path: None,
        target: None,
    })
    .unwrap();
    let expected_base = root.join("target/cpm-2.2-z80/src/demo");

    assert_eq!(outputs.asm, expected_base.with_extension("asm"));
    assert_eq!(outputs.map, expected_base.with_extension("map"));
    assert_eq!(outputs.executable, expected_base.with_extension("com"));
    assert_eq!(
        std::fs::read(outputs.executable).unwrap(),
        [0x0E, 0x00, 0xCD, 0x05, 0x00]
    );

    drop(_cwd);
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn ez80_harness_project_config_writes_target_artifacts() {
    let root = temp_root("ez80_harness_project_artifacts");
    std::fs::create_dir_all(root.join("src")).unwrap();
    let source_path = root.join("src/game.ezra");
    std::fs::write(
        root.join("Ezra.toml"),
        r#"
                [build]
                target = "ezra-test-flat-ez80"
                executable = "harness-game"
            "#,
    )
    .unwrap();
    std::fs::write(
        &source_path,
        r#"
                global marker: u8 = 0x5A
                fn main() {
                    test.assert_eq_u8(marker, 0x5A, 1)
                    test.pass()
                }
            "#,
    )
    .unwrap();

    let outputs = build_source(source_path.to_str().unwrap()).unwrap();
    let expected_base = root
        .join("target")
        .join("ezra-test-flat-ez80")
        .join("src")
        .join("harness-game");

    assert_eq!(outputs.asm, expected_base.with_extension("asm"));
    assert_eq!(outputs.map, expected_base.with_extension("map"));
    assert_eq!(outputs.executable, expected_base.with_extension("bin"));
    let map = std::fs::read_to_string(outputs.map).unwrap();
    assert!(map.contains(".text        0x010040"), "{map}");
    assert!(map.contains(".data        0x050000"), "{map}");

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn commands_use_ezra_toml_target_and_layout() {
    let root = temp_root("project_config_layout");
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::create_dir_all(root.join("layouts")).unwrap();
    let source_path = root.join("src/game.ezra");
    let layout_path = root.join("layouts/agon.ezralayout");
    std::fs::write(
        root.join("Ezra.toml"),
        r#"
                [build]
                target = "agonlight-console8-ez80-1.0"

                [layout]
                file = "layouts/agon.ezralayout"
            "#,
    )
    .unwrap();
    std::fs::write(
        &source_path,
        r#"
                fn main() {
                    test.assert_eq_u24(EZRA_RAM_BASE, 0x030000, 1)
                    test.pass()
                }
            "#,
    )
    .unwrap();
    std::fs::write(
        &layout_path,
        r#"
                layout project_layout {
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
        layout_path: None,
        target: None,
    })
    .unwrap();

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn cli_target_overrides_project_target() {
    let root = temp_root("target_override");
    std::fs::create_dir_all(&root).unwrap();
    let source_path = root.join("game.ezra");
    std::fs::write(&source_path, "fn main() { test.pass() }\n").unwrap();
    std::fs::write(
        root.join("Ezra.toml"),
        r#"
                [build]
                target = "zxspectrum-z80"
            "#,
    )
    .unwrap();

    check(&CommandOptions {
        path: source_path.to_string_lossy().into_owned(),
        debug_comments: false,
        default_sdk_symbols: true,
        layout_path: None,
        target: Some("ti84plusce-ez80".to_owned()),
    })
    .unwrap();

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn examples_ignore_generated_artifacts_but_not_handwritten_assembly() {
    let gitignore =
        std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join(".gitignore")).unwrap();

    assert!(gitignore.contains("/examples/**/target"), "{gitignore}");
    assert!(!gitignore.contains("/examples/**/*.asm"), "{gitignore}");
}
