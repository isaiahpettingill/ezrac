//! Host-independent virtual workspace paths and files.

use alloc::{string::String, vec::Vec};

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
