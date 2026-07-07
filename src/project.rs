use std::path::{Path, PathBuf};

use crate::diagnostic::Diagnostic;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProjectConfig {
    pub path: PathBuf,
    pub root: PathBuf,
    pub target: Option<String>,
    pub layout_file: Option<PathBuf>,
    pub sdk_paths: Vec<PathBuf>,
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

    let layout_file = value
        .get("layout")
        .and_then(|layout| layout.get("file"))
        .map(required_string("layout.file"))
        .transpose()?
        .map(|file| root.join(file));

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
        layout_file,
        sdk_paths,
    })
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

                [layout]
                file = "layouts/demo.ezralayout"

                [sdk]
                paths = ["sdk", "../shared"]
            "#,
        )
        .unwrap();

        assert_eq!(
            config.target.as_deref(),
            Some("agonlight-console8-ez80-1.0")
        );
        assert_eq!(
            config.layout_file,
            Some(PathBuf::from("/project/layouts/demo.ezralayout"))
        );
        assert_eq!(
            config.sdk_paths,
            vec![
                PathBuf::from("/project/sdk"),
                PathBuf::from("/project/../shared")
            ]
        );
    }
}
