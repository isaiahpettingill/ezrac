use std::path::{Path, PathBuf};

use crate::diagnostic::Diagnostic;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProjectConfig {
    pub path: PathBuf,
    pub root: PathBuf,
    pub target: Option<String>,
    pub output: Option<String>,
    pub layout_file: Option<PathBuf>,
    pub cartridge: Option<CartridgeConfig>,
    pub sdk_paths: Vec<PathBuf>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CartridgeConfig {
    pub layout_file: PathBuf,
    pub manifest_file: Option<PathBuf>,
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
    let value = source.parse::<toml::Value>().map_err(|error| {
        Diagnostic::new(format!("failed to parse `{}`: {error}", path.display()))
    })?;

    let target = value
        .get("build")
        .and_then(|build| build.get("target"))
        .map(required_string("build.target"))
        .transpose()?;

    let output = value
        .get("build")
        .and_then(|build| build.get("output"))
        .map(required_string("build.output"))
        .transpose()?;

    let layout_file = value
        .get("layout")
        .and_then(|layout| layout.get("file"))
        .map(required_string("layout.file"))
        .transpose()?
        .map(|file| root.join(file));

    let cartridge = parse_cartridge_config(&value, &root)?;

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
        target,
        output,
        layout_file,
        cartridge,
        sdk_paths,
    })
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
mod tests {
    use super::*;

    #[test]
    fn parses_project_target_layout_and_sdk_paths() {
        let path = Path::new("/project/Ezra.toml");
        let config = parse_project_config(
            path,
            r#"
                [project]
                name = "demo"

                [build]
                target = "agonlight-console8-ez80-1.0"
                output = "bin"

                [layout]
                file = "layouts/demo.ezralayout"

                [cartridge]
                layout = "cartridges/agon.toml"
                manifest = "cartridges/manifest.toml"

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
            config.sdk_paths,
            vec![
                PathBuf::from("/project/sdk"),
                PathBuf::from("/project/../shared")
            ]
        );
    }

    #[test]
    fn cartridge_config_requires_a_layout() {
        let error = parse_project_config(
            Path::new("/project/Ezra.toml"),
            r#"
                [cartridge]
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
}
