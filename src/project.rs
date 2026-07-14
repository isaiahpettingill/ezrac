use std::path::{Path, PathBuf};

use crate::diagnostic::Diagnostic;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProjectConfig {
    pub path: PathBuf,
    pub root: PathBuf,
    pub input: Option<PathBuf>,
    pub target: Option<String>,
    pub output: Option<String>,
    pub input_kind: Option<String>,
    pub assembler_cpu: Option<String>,
    pub executable: Option<String>,
    pub test_target: Option<String>,
    pub layout_file: Option<PathBuf>,
    pub cartridge: Option<CartridgeConfig>,
    pub assets: AssetConfig,
    pub sdk_paths: Vec<PathBuf>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CartridgeConfig {
    pub layout_file: PathBuf,
    pub manifest_file: Option<PathBuf>,
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

    let target = value
        .get("build")
        .and_then(|build| build.get("target"))
        .map(required_string("build.target"))
        .transpose()?;

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
        output,
        input_kind,
        assembler_cpu,
        executable,
        test_target,
        layout_file,
        cartridge,
        assets,
        sdk_paths,
    })
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
mod tests;
