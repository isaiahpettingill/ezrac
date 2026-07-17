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
fn parses_explicit_banking_configuration() {
    let config = parse_project_config(
        Path::new("/project/Ezra.toml"),
        "[banking]\nenabled = true\n",
    )
    .unwrap();

    assert_eq!(config.banking, BankingConfig { enabled: true });
}

#[test]
fn defaults_and_validates_explicit_banking_configuration() {
    let config = parse_project_config(Path::new("/project/Ezra.toml"), "").unwrap();
    assert_eq!(config.banking, BankingConfig { enabled: false });

    let error = parse_project_config(
        Path::new("/project/Ezra.toml"),
        "[banking]\nenabled = \"yes\"\n",
    )
    .unwrap_err();
    assert!(error.message.contains("banking.enabled"), "{error}");
}

#[test]
fn parses_gameboy_banking_configuration() {
    let config = parse_project_config(
        Path::new("/project/Ezra.toml"),
        r#"[gameboy]
mapper = "mbc5"
rom_banks = 32
ram_banks = 4
battery = true
rumble = true
bank_files = ["assets/level-2.bin", "assets/level-3.bin"]
"#,
    )
    .unwrap();

    assert_eq!(
        config.gameboy,
        Some(GameBoyConfig {
            mapper: GameBoyMapper::Mbc5,
            rom_banks: Some(32),
            ram_banks: 4,
            battery: true,
            rumble: true,
            bank_files: vec![
                PathBuf::from("/project/assets/level-2.bin"),
                PathBuf::from("/project/assets/level-3.bin"),
            ],
        })
    );
}

#[test]
fn rejects_invalid_gameboy_mapper() {
    let error = parse_project_config(
        Path::new("/project/Ezra.toml"),
        "[gameboy]\nmapper = \"mbc3\"\n",
    )
    .unwrap_err();

    assert!(error.message.contains("gameboy.mapper"), "{error}");
}

#[test]
fn parses_zxspectrum_banking_configuration_without_reading_bank_files() {
    let config = parse_project_config(
        Path::new("/project/Ezra.toml"),
        r#"[zxspectrum]

[[zxspectrum.banks]]
page = 1
file = "assets/level-1.bin"
name = "level-1"

[[zxspectrum.banks]]
page = 7
file = "assets/level-7.bin"
"#,
    )
    .unwrap();

    assert_eq!(
        config.zxspectrum,
        Some(ZxSpectrumConfig {
            banks: vec![
                ZxSpectrumBank {
                    page: 1,
                    file: PathBuf::from("/project/assets/level-1.bin"),
                    name: Some("level-1".to_owned()),
                },
                ZxSpectrumBank {
                    page: 7,
                    file: PathBuf::from("/project/assets/level-7.bin"),
                    name: None,
                },
            ],
        })
    );
}

#[test]
fn rejects_invalid_zxspectrum_bank_configuration() {
    for (source, expected) in [
        (
            r#"[zxspectrum]
[[zxspectrum.banks]]
page = 2
file = "assets/bank.bin"
"#,
            "must be a u8 value of 1, 3, 4, 6, or 7",
        ),
        (
            r#"[zxspectrum]
[[zxspectrum.banks]]
page = 1
file = "assets/first.bin"
[[zxspectrum.banks]]
page = 1
file = "assets/second.bin"
"#,
            "duplicates ZX Spectrum RAM page 1",
        ),
        (
            r#"[zxspectrum]
[[zxspectrum.banks]]
page = 3
file = "assets/bank.bin"
name = ""
"#,
            "must be a nonempty ASCII string",
        ),
        (
            r#"[zxspectrum]
[[zxspectrum.banks]]
page = 4
file = "assets/bank.bin"
name = "café"
"#,
            "must be a nonempty ASCII string",
        ),
        (
            r#"[zxspectrum]
[[zxspectrum.banks]]
page = 4
file = "/outside-project.bin"
"#,
            "must be a project-relative path",
        ),
    ] {
        let error = parse_project_config(Path::new("/project/Ezra.toml"), source).unwrap_err();
        assert!(error.message.contains(expected), "{error}");
    }
}

#[test]
fn parses_multiple_build_targets() {
    let config = parse_project_config(
        Path::new("/project/Ezra.toml"),
        "[build]\ntarget = [\"agonlight-mos-ez80\", \"cpm-2.2-z80\"]\n",
    )
    .unwrap();

    assert_eq!(config.target.as_deref(), Some("agonlight-mos-ez80"));
    assert_eq!(config.targets, vec!["agonlight-mos-ez80", "cpm-2.2-z80"]);
}

#[test]
fn rejects_empty_build_target_array() {
    let error = parse_project_config(Path::new("/project/Ezra.toml"), "[build]\ntarget = []\n")
        .unwrap_err();

    assert!(error.message.contains("at least one target"), "{error}");
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
