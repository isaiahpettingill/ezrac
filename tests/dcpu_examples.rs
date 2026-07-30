#![cfg(feature = "dcpu")]

use std::{fs, path::PathBuf, process::Command};

fn repository_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn build_example(name: &str, source: &str) -> Vec<u8> {
    let root = repository_root();
    let output = Command::new(env!("CARGO_BIN_EXE_ezrac"))
        .current_dir(&root)
        .args(["build", source])
        .output()
        .unwrap_or_else(|error| panic!("failed to launch ezrac for `{source}`: {error}"));
    assert!(
        output.status.success(),
        "failed to build `{source}`\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let path = root
        .join("examples/dcpu-16")
        .join(name)
        .join("target/generic-dcpu-bare")
        .join(format!("dcpu16-{name}.bin"));
    let bytes = fs::read(&path)
        .unwrap_or_else(|error| panic!("failed to read DCPU image `{}`: {error}", path.display()));
    assert!(!bytes.is_empty(), "DCPU image is empty");
    assert_eq!(bytes.len() % 2, 0, "DCPU image must contain full words");
    bytes
}

#[test]
fn dcpu_lem_assembly_example_builds_little_endian_words() {
    let bytes = build_example("lem-hello", "examples/dcpu-16/lem-hello/main.asm");
    let words = bytes
        .as_chunks::<2>()
        .0
        .iter()
        .map(|bytes| u16::from_le_bytes(*bytes))
        .collect::<Vec<_>>();

    assert!(
        words.contains(&0x8000),
        "expected LEM screen word address: {words:?}"
    );
    assert!(
        words.contains(&0xf045),
        "expected encoded E glyph: {words:?}"
    );
}

#[test]
fn dcpu_source_example_builds_little_endian_image() {
    let bytes = build_example("arithmetic", "examples/dcpu-16/arithmetic/src/main.ezra");
    let words = bytes
        .as_chunks::<2>()
        .0
        .iter()
        .map(|bytes| u16::from_le_bytes(*bytes))
        .collect::<Vec<_>>();

    assert!(
        words.contains(&0x7fc1),
        "expected LEM screen write: {words:?}"
    );
    assert!(
        words.len() > 12,
        "expected generated startup and arithmetic code"
    );
}
