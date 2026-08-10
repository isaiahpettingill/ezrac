#![cfg(feature = "mos6502")]

use std::{fs, path::PathBuf, process::Command};

fn repository_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn build(source: &str) {
    let output = Command::new(env!("CARGO_BIN_EXE_ezrac"))
        .current_dir(repository_root())
        .args(["build", source])
        .output()
        .unwrap_or_else(|error| panic!("failed to launch ezrac for `{source}`: {error}"));
    assert!(
        output.status.success(),
        "failed to build `{source}`\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn assert_lorom(path: &str) {
    let rom = fs::read(repository_root().join(path)).unwrap();
    assert_eq!(rom.len(), 0x8000);
    assert_eq!(rom[0x7FD5], 0x20);
    assert_eq!(&rom[0x7FFC..0x7FFE], &0x8000u16.to_le_bytes());
    let checksum = u16::from_le_bytes([rom[0x7FDE], rom[0x7FDF]]);
    let complement = u16::from_le_bytes([rom[0x7FDC], rom[0x7FDD]]);
    assert_eq!(checksum ^ complement, 0xFFFF);
}

#[test]
fn snes_source_example_builds_a_valid_lorom_image() {
    build("examples/snes-5a22/source-hello/src/main.ezra");
    assert_lorom("examples/snes-5a22/source-hello/target/snes-5a22/source-hello.sfc");
}

#[test]
fn snes_assembly_example_builds_a_valid_lorom_image() {
    build("examples/snes-5a22/hello-world/hello.asm");
    assert_lorom("examples/snes-5a22/hello-world/target/snes-5a22/hello.sfc");
}
