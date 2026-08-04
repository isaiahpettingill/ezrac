
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
