//! Host-independent virtual workspace paths and files.

use alloc::{format, string::String, vec::Vec};

use crate::{
    ast::{Declaration, EmbedSource, Expr, Program},
    compat::source_path_text,
    diagnostic::Diagnostic,
};

/// Normalize a virtual path without consulting a host filesystem.
///
/// Both slash styles are accepted. `.` components are removed and `..`
/// components pop one ordinary component without escaping the workspace root.
pub fn normalize_virtual_path(path: &str) -> String {
    let mut components: Vec<&str> = Vec::new();
    for component in path.split(['/', '\\']) {
        match component {
            "" | "." => {}
            ".." => {
                components.pop();
            }
            component => components.push(component),
        }
    }
    components.join("/")
}

/// A caller-owned file in a virtual Ezra workspace.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct WorkspaceFile<'a> {
    pub path: &'a str,
    pub contents: &'a [u8],
}

impl<'a> WorkspaceFile<'a> {
    pub const fn new(path: &'a str, contents: &'a [u8]) -> Self {
        Self { path, contents }
    }

    pub const fn text(path: &'a str, contents: &'a str) -> Self {
        Self::new(path, contents.as_bytes())
    }
}

/// An immutable in-memory project tree.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Workspace<'a> {
    pub files: &'a [WorkspaceFile<'a>],
}

impl<'a> Workspace<'a> {
    pub const fn new(files: &'a [WorkspaceFile<'a>]) -> Self {
        Self { files }
    }

    pub fn file(&self, path: &str) -> Option<&'a [u8]> {
        let path = normalize_virtual_path(path);
        self.files
            .iter()
            .find(|file| normalize_virtual_path(file.path) == path)
            .map(|file| file.contents)
    }
}

pub(crate) fn materialize_workspace_embeds(
    program: &mut Program,
    workspace: &Workspace<'_>,
) -> Result<(), Diagnostic> {
    let source_path = normalize_virtual_path(&source_path_text(&program.source_path));
    for declaration in &mut program.declarations {
        materialize_declaration_embeds(declaration, &source_path, workspace)?;
    }
    Ok(())
}

fn materialize_declaration_embeds(
    declaration: &mut Declaration,
    source_path: &str,
    workspace: &Workspace<'_>,
) -> Result<(), Diagnostic> {
    match declaration {
        Declaration::Cfg { declaration, .. } => {
            materialize_declaration_embeds(declaration, source_path, workspace)
        }
        Declaration::Embed(embed) => {
            let EmbedSource::File(asset_path) = &embed.source else {
                return Ok(());
            };
            let asset_path = asset_path.clone();
            let resolved = resolve_virtual_asset_path(source_path, &asset_path);
            let bytes = workspace.file(&resolved).ok_or_else(|| {
                Diagnostic::new(format!(
                    "virtual workspace asset `{asset_path}` referenced from `{source_path}` was not found (resolved as `{resolved}`)"
                ))
            })?;
            embed.source = EmbedSource::Bytes(
                bytes
                    .iter()
                    .copied()
                    .map(|byte| Expr::Int(i64::from(byte)))
                    .collect(),
            );
            Ok(())
        }
        _ => Ok(()),
    }
}

fn resolve_virtual_asset_path(source_path: &str, asset_path: &str) -> String {
    let source_path = normalize_virtual_path(source_path);
    let source_directory = source_path
        .rsplit_once('/')
        .map(|(directory, _)| directory)
        .unwrap_or("");
    if source_directory.is_empty() {
        normalize_virtual_path(asset_path)
    } else {
        normalize_virtual_path(&format!("{source_directory}/{asset_path}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_paths_independently_of_the_host() {
        assert_eq!(
            normalize_virtual_path(r".\src//lib/../main.ezra"),
            "src/main.ezra"
        );
        assert_eq!(
            normalize_virtual_path("../../sdk/math.ezra"),
            "sdk/math.ezra"
        );
    }

    #[test]
    fn workspace_lookup_uses_normalized_paths() {
        let files = [WorkspaceFile::text("src/math.ezra", "pub const N: u8 = 1")];
        let workspace = Workspace::new(&files);
        assert!(workspace.file(r"src\.\math.ezra").is_some());
    }
}
