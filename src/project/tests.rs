use super::*;

#[test]
fn parses_project_target_layout_and_sdk_paths() {
    let path = Path::new("/project/Ezra.toml");
    let config = parse_project_config(
        path,
        r#"[project]
name = "demo"

[build]
input = "src/main.ezra"
target = "agonlight-console8-ez80-1.0"
output = "bin"
input_kind = "ezra"
assembler_cpu = "ez80"
executable = "demo"

[layout]
file = "layouts/demo.ezralayout"

[cartridge]
layout = "cartridges/agon.toml"
manifest = "cartridges/manifest.toml"

[assets]
section = ".assets"
align = 16

[assets.targets."gameboy-*"]
section = ".rodata"
align = 32

[assets.targets."zxspectrum-*"]
align = 256

[sdk]
paths = ["sdk", "../shared"]
"#,
    )
    .unwrap();

    assert_eq!(
        config.target.as_deref(),
        Some("agonlight-console8-ez80-1.0")
    );
    assert_eq!(config.output.as_deref(), Some("bin"));
    assert_eq!(config.input_kind.as_deref(), Some("ezra"));
    assert_eq!(config.assembler_cpu.as_deref(), Some("ez80"));
    assert_eq!(config.input, Some(PathBuf::from("/project/src/main.ezra")));
    assert_eq!(config.executable.as_deref(), Some("demo"));
    assert_eq!(
        config.layout_file,
        Some(PathBuf::from("/project/layouts/demo.ezralayout"))
    );
    assert_eq!(
        config.cartridge,
        Some(CartridgeConfig {
            layout_file: PathBuf::from("/project/cartridges/agon.toml"),
            manifest_file: Some(PathBuf::from("/project/cartridges/manifest.toml")),
        })
    );
    assert_eq!(
        config.assets.placement_for("gameboy-dmg-lr35902"),
        AssetPlacement {
            section: Some(".rodata".to_owned()),
            align: Some(32),
        }
    );
    assert_eq!(
        config.assets.placement_for("zxspectrum-z80"),
        AssetPlacement {
            section: Some(".assets".to_owned()),
            align: Some(256),
        }
    );
    assert_eq!(
        config.sdk_paths,
        vec![
            PathBuf::from("/project/sdk"),
            PathBuf::from("/project/../shared")
        ]
    );
}

#[test]
fn parses_library_lsp_mode() {
    let config = parse_project_config(
        Path::new("/project/Ezra.toml"),
        "[lsp]\nmode = \"library\"\n",
    )
    .unwrap();

    assert_eq!(config.lsp_mode, LspMode::Library);
}

#[test]
fn rejects_unknown_lsp_mode() {
    let error = parse_project_config(
        Path::new("/project/Ezra.toml"),
        "[lsp]\nmode = \"shared\"\n",
    )
    .unwrap_err();

    assert!(error.message.contains("lsp.mode"), "{error}");
}

#[test]
fn cartridge_config_requires_a_layout() {
    let error = parse_project_config(
        Path::new("/project/Ezra.toml"),
        r#"[cartridge]
manifest = "cart.toml"
"#,
    )
    .unwrap_err();

    assert!(
        error.message.contains("cartridge.layout` is required"),
        "{}",
        error.message
    );
}
