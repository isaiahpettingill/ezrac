use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

fn repository_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn build_example(name: &str) -> (PathBuf, String) {
    let root = repository_root();
    let source = format!("examples/zxspectrum-z80/{name}/src/main.ezra");
    let output = Command::new(env!("CARGO_BIN_EXE_ezrac"))
        .current_dir(&root)
        .args(["build", &source])
        .output()
        .unwrap_or_else(|error| panic!("failed to launch ezrac for `{source}`: {error}"));
    assert!(
        output.status.success(),
        "failed to build `{source}`\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let target = root
        .join("examples/zxspectrum-z80")
        .join(name)
        .join("target/zxspectrum-z80");
    let tape = target.join(format!("zx-{name}.tap"));
    let asm = fs::read_to_string(target.join(format!("zx-{name}.asm")))
        .unwrap_or_else(|error| panic!("failed to read assembly for `{name}`: {error}"));
    assert!(tape.is_file(), "missing `{}`", tape.display());
    (tape, asm)
}

fn assert_valid_tap(path: &Path) {
    let tape = fs::read(path).unwrap_or_else(|error| {
        panic!("failed to read Spectrum tape `{}`: {error}", path.display())
    });
    let mut offset = 0usize;
    let mut blocks = 0usize;
    while offset < tape.len() {
        assert!(offset + 2 <= tape.len(), "truncated TAP block length");
        let length = usize::from(u16::from_le_bytes([tape[offset], tape[offset + 1]]));
        offset += 2;
        assert!(length >= 2, "TAP block is too short");
        assert!(offset + length <= tape.len(), "truncated TAP block payload");
        let block = &tape[offset..offset + length];
        let checksum = block.iter().fold(0u8, |value, byte| value ^ byte);
        assert_eq!(checksum, 0, "invalid TAP checksum in block {blocks}");
        offset += length;
        blocks += 1;
    }
    assert_eq!(offset, tape.len());
    assert_eq!(blocks, 4, "expected BASIC and CODE header/data blocks");
}

#[test]
fn zx_spectrum_graphics_example_builds_a_loadable_tape() {
    let (tape, asm) = build_example("graphics");
    assert_valid_tap(&tape);
    assert!(asm.contains("_bitmap_address:"), "{asm}");
    assert!(asm.contains("_screen_set_pixel_byte:"), "{asm}");
    assert!(asm.contains("_screen_clear_screen:"), "{asm}");
    assert!(asm.contains("_clear_bitmap:"), "{asm}");
    assert!(asm.contains("_clear_attrs:"), "{asm}");
}

#[test]
fn zx_spectrum_input_example_builds_keyboard_and_kempston_reads() {
    let (tape, asm) = build_example("input");
    assert_valid_tap(&tape);
    assert!(asm.contains("in a, (c)"), "{asm}");
    assert!(asm.contains("in a, (1Fh)"), "{asm}");
    assert!(asm.contains("and 1Fh\n    ld a, a"), "{asm}");
    assert!(asm.contains("halt"), "{asm}");
}

#[test]
fn zx_spectrum_sound_example_builds_ula_and_ay_output() {
    let (tape, asm) = build_example("sound");
    assert_valid_tap(&tape);
    assert!(asm.contains("out (0FEh), a"), "{asm}");
    assert!(asm.contains("ld bc, 0FFFDh"), "{asm}");
    assert!(asm.contains("ld bc, 0BFFDh"), "{asm}");
    assert!(asm.contains("halt"), "{asm}");
}
