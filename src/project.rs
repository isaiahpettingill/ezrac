use std::path::{Path, PathBuf};

use crate::diagnostic::Diagnostic;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum LspMode {
    #[default]
    Application,
    Library,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProjectConfig {
    pub path: PathBuf,
    pub root: PathBuf,
    pub input: Option<PathBuf>,
    pub target: Option<String>,
    /// All configured build targets. The first target is the default used by CLI builds.
    pub targets: Vec<String>,
    pub output: Option<String>,
    pub input_kind: Option<String>,
    pub assembler_cpu: Option<String>,
    pub executable: Option<String>,
    pub lsp_mode: LspMode,
    pub test_target: Option<String>,
    pub layout_file: Option<PathBuf>,
    pub cartridge: Option<CartridgeConfig>,
    pub gameboy: Option<GameBoyConfig>,
    pub arduboy: Option<ArduboyConfig>,
    pub zxspectrum: Option<ZxSpectrumConfig>,
    pub banking: BankingConfig,
    pub assets: AssetConfig,
    pub sdk_paths: Vec<PathBuf>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CartridgeConfig {
    pub layout_file: PathBuf,
    pub manifest_file: Option<PathBuf>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ArduboyConfig {
    pub title: String,
    pub author: String,
    pub version: String,
    pub description: Option<String>,
    pub date: Option<String>,
    pub genre: Option<String>,
    pub source_url: Option<String>,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum GameBoyMapper {
    #[default]
    RomOnly,
    Mbc1,
    Mbc5,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct GameBoyConfig {
    pub mapper: GameBoyMapper,
    pub rom_banks: Option<u16>,
    pub ram_banks: u8,
    pub battery: bool,
    pub rumble: bool,
    /// One 16 KiB payload per switchable ROM bank, starting at bank 2.
    pub bank_files: Vec<PathBuf>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ZxSpectrumConfig {
    pub banks: Vec<ZxSpectrumBank>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ZxSpectrumBank {
    pub page: u8,
    pub file: PathBuf,
    pub name: Option<String>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct BankingConfig {
    pub enabled: bool,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct AssetConfig {
    pub default: AssetPlacement,
    pub targets: Vec<(String, AssetPlacement)>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct AssetPlacement {
    pub section: Option<String>,
    pub align: Option<u32>,
}

impl AssetConfig {
    pub fn placement_for(&self, target: &str) -> AssetPlacement {
        let mut placement = self.default.clone();
        for (pattern, target_placement) in &self.targets {
            if target_pattern_matches(pattern, target) {
                if target_placement.section.is_some() {
                    placement.section.clone_from(&target_placement.section);
                }
                if target_placement.align.is_some() {
                    placement.align = target_placement.align;
                }
            }
        }
        placement
    }
}

pub fn load_nearest_project_config(
    source_path: &Path,
) -> Result<Option<ProjectConfig>, Diagnostic> {
    let source_dir = source_path.parent().unwrap_or_else(|| Path::new("."));
    for dir in source_dir.ancestors() {
        let path = dir.join("Ezra.toml");
        if path.exists() {
            return load_project_config(&path).map(Some);
        }
    }
    Ok(None)
}

pub fn load_project_config(path: &Path) -> Result<ProjectConfig, Diagnostic> {
    let source = std::fs::read_to_string(path).map_err(|error| {
        Diagnostic::new(format!("failed to read `{}`: {error}", path.display()))
    })?;
    parse_project_config(path, &source)
}

pub fn parse_project_config(path: &Path, source: &str) -> Result<ProjectConfig, Diagnostic> {
    let root = path
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .to_path_buf();
    let value = toml::from_str::<toml::Value>(source.trim_start()).map_err(|error| {
        Diagnostic::new(format!("failed to parse `{}`: {error}", path.display()))
    })?;

    let targets = match value.get("build").and_then(|build| build.get("target")) {
        Some(toml::Value::String(target)) => vec![target.clone()],
        Some(toml::Value::Array(targets)) => {
            let targets = targets
                .iter()
                .map(required_string("build.target"))
                .collect::<Result<Vec<_>, _>>()?;
            if targets.is_empty() {
                return Err(Diagnostic::new(
                    "project field `build.target` must contain at least one target",
                ));
            }
            targets
        }
        Some(_) => {
            return Err(Diagnostic::new(
                "project field `build.target` must be a string or an array of strings",
            ));
        }
        None => Vec::new(),
    };
    let target = targets.first().cloned();

    let input = value
        .get("build")
        .and_then(|build| build.get("input"))
        .map(required_string("build.input"))
        .transpose()?
        .map(|input| root.join(input));

    let output = value
        .get("build")
        .and_then(|build| build.get("output"))
        .map(required_string("build.output"))
        .transpose()?;

    let input_kind = value
        .get("build")
        .and_then(|build| build.get("input_kind"))
        .map(required_string("build.input_kind"))
        .transpose()?;

    let assembler_cpu = value
        .get("build")
        .and_then(|build| build.get("assembler_cpu"))
        .map(required_string("build.assembler_cpu"))
        .transpose()?;

    let executable = value
        .get("build")
        .and_then(|build| build.get("executable"))
        .map(required_string("build.executable"))
        .transpose()?;

    let lsp_mode = value
        .get("lsp")
        .and_then(|lsp| lsp.get("mode"))
        .map(required_string("lsp.mode"))
        .transpose()?
        .map(|mode| match mode.as_str() {
            "application" => Ok(LspMode::Application),
            "library" => Ok(LspMode::Library),
            _ => Err(Diagnostic::new(format!(
                "project field `lsp.mode` must be `application` or `library`, got `{mode}`"
            ))),
        })
        .transpose()?
        .unwrap_or_default();

    let test_target = value
        .get("test")
        .and_then(|test| test.get("target"))
        .map(required_string("test.target"))
        .transpose()?;

    let layout_file = value
        .get("layout")
        .and_then(|layout| layout.get("file"))
        .map(required_string("layout.file"))
        .transpose()?
        .map(|file| root.join(file));

    let cartridge = parse_cartridge_config(&value, &root)?;
    let gameboy = parse_gameboy_config(&value, &root)?;
    let arduboy = parse_arduboy_config(&value, output.as_deref() == Some("arduboy"))?;
    let zxspectrum = parse_zxspectrum_config(&value, &root)?;
    let banking = parse_banking_config(&value)?;
    let assets = parse_asset_config(&value)?;

    let sdk_paths = match value.get("sdk").and_then(|sdk| sdk.get("paths")) {
        Some(toml::Value::Array(paths)) => paths
            .iter()
            .map(required_string("sdk.paths"))
            .collect::<Result<Vec<_>, _>>()?
            .into_iter()
            .map(|path| root.join(path))
            .collect(),
        Some(_) => {
            return Err(Diagnostic::new(
                "project field `sdk.paths` must be an array",
            ));
        }
        None => Vec::new(),
    };

    Ok(ProjectConfig {
        path: path.to_path_buf(),
        root,
        input,
        target,
        targets,
        output,
        input_kind,
        assembler_cpu,
        executable,
        lsp_mode,
        test_target,
        layout_file,
        cartridge,
        gameboy,
        arduboy,
        zxspectrum,
        banking,
        assets,
        sdk_paths,
    })
}

fn parse_arduboy_config(
    value: &toml::Value,
    required: bool,
) -> Result<Option<ArduboyConfig>, Diagnostic> {
    let Some(arduboy) = value.get("arduboy") else {
        return if required {
            Err(Diagnostic::new(
                "`[arduboy]` is required when `build.output = \"arduboy\"`",
            ))
        } else {
            Ok(None)
        };
    };
    if !arduboy.is_table() {
        return Err(Diagnostic::new("project field `arduboy` must be a table"));
    }
    let required_value = |key: &str, field: &'static str| {
        arduboy
            .get(key)
            .map(required_string(field))
            .transpose()?
            .ok_or_else(|| Diagnostic::new(format!("project field `{field}` is required")))
    };
    Ok(Some(ArduboyConfig {
        title: required_value("title", "arduboy.title")?,
        author: required_value("author", "arduboy.author")?,
        version: required_value("version", "arduboy.version")?,
        description: arduboy
            .get("description")
            .map(required_string("arduboy.description"))
            .transpose()?,
        date: arduboy
            .get("date")
            .map(required_string("arduboy.date"))
            .transpose()?,
        genre: arduboy
            .get("genre")
            .map(required_string("arduboy.genre"))
            .transpose()?,
        source_url: arduboy
            .get("source_url")
            .map(required_string("arduboy.source_url"))
            .transpose()?,
    }))
}

fn parse_gameboy_config(
    value: &toml::Value,
    root: &Path,
) -> Result<Option<GameBoyConfig>, Diagnostic> {
    let Some(gameboy) = value.get("gameboy") else {
        return Ok(None);
    };
    let mapper = gameboy
        .get("mapper")
        .map(required_string("gameboy.mapper"))
        .transpose()?
        .ok_or_else(|| Diagnostic::new("project field `gameboy.mapper` is required"))?;
    let mapper = match mapper.as_str() {
        "rom-only" => GameBoyMapper::RomOnly,
        "mbc1" => GameBoyMapper::Mbc1,
        "mbc5" => GameBoyMapper::Mbc5,
        _ => {
            return Err(Diagnostic::new(format!(
                "project field `gameboy.mapper` must be `rom-only`, `mbc1`, or `mbc5`, got `{mapper}`"
            )));
        }
    };
    let rom_banks = gameboy
        .get("rom_banks")
        .map(|value| {
            value
                .as_integer()
                .and_then(|value| u16::try_from(value).ok())
                .ok_or_else(|| {
                    Diagnostic::new("project field `gameboy.rom_banks` must be a positive integer")
                })
        })
        .transpose()?;
    let ram_banks = gameboy
        .get("ram_banks")
        .map(|value| {
            value
                .as_integer()
                .and_then(|value| u8::try_from(value).ok())
                .filter(|value| matches!(*value, 0 | 1 | 4 | 8 | 16))
                .ok_or_else(|| {
                    Diagnostic::new(
                        "project field `gameboy.ram_banks` must be one of 0, 1, 4, 8, or 16",
                    )
                })
        })
        .transpose()?
        .unwrap_or(0);
    let battery = gameboy
        .get("battery")
        .map(|value| {
            value
                .as_bool()
                .ok_or_else(|| Diagnostic::new("project field `gameboy.battery` must be a boolean"))
        })
        .transpose()?
        .unwrap_or(false);
    let rumble = gameboy
        .get("rumble")
        .map(|value| {
            value
                .as_bool()
                .ok_or_else(|| Diagnostic::new("project field `gameboy.rumble` must be a boolean"))
        })
        .transpose()?
        .unwrap_or(false);
    let bank_files = match gameboy.get("bank_files") {
        Some(toml::Value::Array(files)) => files
            .iter()
            .map(required_string("gameboy.bank_files"))
            .collect::<Result<Vec<_>, _>>()?
            .into_iter()
            .map(|file| root.join(file))
            .collect(),
        Some(_) => {
            return Err(Diagnostic::new(
                "project field `gameboy.bank_files` must be an array of paths",
            ));
        }
        None => Vec::new(),
    };
    Ok(Some(GameBoyConfig {
        mapper,
        rom_banks,
        ram_banks,
        battery,
        rumble,
        bank_files,
    }))
}

fn parse_zxspectrum_config(
    value: &toml::Value,
    root: &Path,
) -> Result<Option<ZxSpectrumConfig>, Diagnostic> {
    let Some(zxspectrum) = value.get("zxspectrum") else {
        return Ok(None);
    };
    let zxspectrum = zxspectrum
        .as_table()
        .ok_or_else(|| Diagnostic::new("project field `zxspectrum` must be a table"))?;
    let banks = match zxspectrum.get("banks") {
        Some(toml::Value::Array(banks)) => banks,
        Some(_) => {
            return Err(Diagnostic::new(
                "project field `zxspectrum.banks` must be an array of bank tables",
            ));
        }
        None => {
            return Ok(Some(ZxSpectrumConfig::default()));
        }
    };

    let mut seen_pages = std::collections::HashSet::new();
    let mut parsed_banks = Vec::with_capacity(banks.len());
    for (index, bank) in banks.iter().enumerate() {
        let field = |name: &str| format!("zxspectrum.banks[{index}].{name}");
        let bank = bank.as_table().ok_or_else(|| {
            Diagnostic::new(format!(
                "project field `zxspectrum.banks[{index}]` must be a table"
            ))
        })?;
        let page = bank
            .get("page")
            .and_then(toml::Value::as_integer)
            .and_then(|page| u8::try_from(page).ok())
            .filter(|page| matches!(*page, 1 | 3 | 4 | 6 | 7))
            .ok_or_else(|| {
                Diagnostic::new(format!(
                    "project field `{}` must be a u8 value of 1, 3, 4, 6, or 7",
                    field("page")
                ))
            })?;
        if !seen_pages.insert(page) {
            return Err(Diagnostic::new(format!(
                "project field `{}` duplicates ZX Spectrum RAM page {page}",
                field("page")
            )));
        }

        let file = bank
            .get("file")
            .and_then(toml::Value::as_str)
            .ok_or_else(|| {
                Diagnostic::new(format!(
                    "project field `{}` must be a project-relative path string",
                    field("file")
                ))
            })?;
        let file = Path::new(file);
        if file.is_absolute() || file.has_root() {
            return Err(Diagnostic::new(format!(
                "project field `{}` must be a project-relative path",
                field("file")
            )));
        }

        let name = match bank.get("name") {
            Some(toml::Value::String(name)) if !name.is_empty() && name.is_ascii() => {
                Some(name.clone())
            }
            Some(toml::Value::String(_)) => {
                return Err(Diagnostic::new(format!(
                    "project field `{}` must be a nonempty ASCII string",
                    field("name")
                )));
            }
            Some(_) => {
                return Err(Diagnostic::new(format!(
                    "project field `{}` must be a string",
                    field("name")
                )));
            }
            None => None,
        };

        parsed_banks.push(ZxSpectrumBank {
            page,
            file: root.join(file),
            name,
        });
    }

    Ok(Some(ZxSpectrumConfig {
        banks: parsed_banks,
    }))
}

fn parse_banking_config(value: &toml::Value) -> Result<BankingConfig, Diagnostic> {
    let Some(banking) = value.get("banking") else {
        return Ok(BankingConfig::default());
    };
    let banking = banking
        .as_table()
        .ok_or_else(|| Diagnostic::new("project field `banking` must be a table"))?;
    let enabled = banking
        .get("enabled")
        .map(|value| {
            value
                .as_bool()
                .ok_or_else(|| Diagnostic::new("project field `banking.enabled` must be a boolean"))
        })
        .transpose()?
        .unwrap_or(false);
    Ok(BankingConfig { enabled })
}

fn parse_asset_config(value: &toml::Value) -> Result<AssetConfig, Diagnostic> {
    let Some(assets) = value.get("assets") else {
        return Ok(AssetConfig::default());
    };
    let default = parse_asset_placement(assets, "assets")?;
    let mut targets = Vec::new();
    if let Some(target_table) = assets.get("targets") {
        let table = target_table
            .as_table()
            .ok_or_else(|| Diagnostic::new("project field `assets.targets` must be a table"))?;
        for (pattern, placement) in table {
            targets.push((
                pattern.clone(),
                parse_asset_placement(placement, "assets.targets")?,
            ));
        }
    }
    Ok(AssetConfig { default, targets })
}

fn parse_asset_placement(
    value: &toml::Value,
    field: &'static str,
) -> Result<AssetPlacement, Diagnostic> {
    let section = value
        .get("section")
        .map(required_string(field))
        .transpose()?;
    let align = value
        .get("align")
        .map(|value| {
            value
                .as_integer()
                .and_then(|value| u32::try_from(value).ok())
                .filter(|value| value.is_power_of_two())
                .ok_or_else(|| {
                    Diagnostic::new(format!(
                        "project field `{field}.align` must be a positive power of two"
                    ))
                })
        })
        .transpose()?;
    Ok(AssetPlacement { section, align })
}

fn target_pattern_matches(pattern: &str, target: &str) -> bool {
    match pattern.split_once('*') {
        Some((prefix, suffix)) => target.starts_with(prefix) && target.ends_with(suffix),
        None => pattern == target,
    }
}

fn parse_cartridge_config(
    value: &toml::Value,
    root: &Path,
) -> Result<Option<CartridgeConfig>, Diagnostic> {
    let Some(cartridge) = value.get("cartridge") else {
        return Ok(None);
    };
    let layout_file = cartridge
        .get("layout")
        .map(required_string("cartridge.layout"))
        .transpose()?
        .ok_or_else(|| Diagnostic::new("project field `cartridge.layout` is required"))?;
    let manifest_file = cartridge
        .get("manifest")
        .map(required_string("cartridge.manifest"))
        .transpose()?;
    Ok(Some(CartridgeConfig {
        layout_file: root.join(layout_file),
        manifest_file: manifest_file.map(|file| root.join(file)),
    }))
}

fn required_string(field: &'static str) -> impl Fn(&toml::Value) -> Result<String, Diagnostic> {
    move |value| {
        value
            .as_str()
            .map(str::to_owned)
            .ok_or_else(|| Diagnostic::new(format!("project field `{field}` must be a string")))
    }
}

#[cfg(test)]
mod arduboy_tests {
    use super::*;

    #[test]
    fn parses_arduboy_schema_v2_metadata() {
        let config = parse_project_config(
            Path::new("/project/Ezra.toml"),
            r#"
                [build]
                output = "arduboy"

                [arduboy]
                title = "Pocket Game"
                author = "EZRA"
                version = "1.2.3"
                description = "A tiny game"
                date = "2026-07-17"
                genre = "Game"
                source_url = "https://example.com/pocket-game"
            "#,
        )
        .unwrap();

        assert_eq!(
            config.arduboy,
            Some(ArduboyConfig {
                title: "Pocket Game".to_owned(),
                author: "EZRA".to_owned(),
                version: "1.2.3".to_owned(),
                description: Some("A tiny game".to_owned()),
                date: Some("2026-07-17".to_owned()),
                genre: Some("Game".to_owned()),
                source_url: Some("https://example.com/pocket-game".to_owned()),
            })
        );
    }

    #[test]
    fn arduboy_output_requires_metadata_table_and_required_fields() {
        let error = parse_project_config(
            Path::new("/project/Ezra.toml"),
            "[build]\noutput = \"arduboy\"\n",
        )
        .unwrap_err();
        assert!(error.message.contains("`[arduboy]` is required"), "{error}");

        let error = parse_project_config(
            Path::new("/project/Ezra.toml"),
            "[build]\noutput = \"arduboy\"\n\n[arduboy]\ntitle = \"Game\"\n",
        )
        .unwrap_err();
        assert!(error.message.contains("arduboy.author"), "{error}");
    }
}

#[cfg(test)]
mod tests;
