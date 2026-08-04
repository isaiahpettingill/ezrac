#![cfg(feature = "mos6502")]

use std::{fs, path::PathBuf, process::Command};

fn repository_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

#[test]
fn nes_source_builds_a_valid_nrom_image() {
    let root = repository_root();
    let source = "examples/nes-2a03/source-hello/src/main.ezra";
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

    let image =
        fs::read(root.join("examples/nes-2a03/source-hello/target/nes-2a03/src/source-hello.nes"))
            .unwrap();
    assert_eq!(image.len(), 0x6010);
    assert_eq!(&image[..8], b"NES\x1A\x01\x01\0\0");
    assert!(image[0x4010..0x4018].iter().all(|byte| *byte == 0xFF));
    assert!(image[0x4018..].iter().all(|byte| *byte == 0));
    assert!(
        image[0x10..0x400A]
            .windows(3)
            .any(|instruction| instruction == [0x8D, 0x04, 0x20]),
        "generated program never writes sprite data to OAMDATA"
    );
    for offset in [0x400A, 0x400C, 0x400E] {
        assert_eq!(&image[offset..offset + 2], &0xC000u16.to_le_bytes());
    }
}

#[test]
fn nes_hello_world_assembly_builds_a_valid_nrom_image() {
    let root = repository_root();
    let source = "examples/nes-2a03/hello-world/helloWorld.asm";
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

    let image_path = root.join("examples/nes-2a03/hello-world/target/nes-2a03/helloWorld.nes");
    let image = fs::read(&image_path).unwrap_or_else(|error| {
        panic!(
            "failed to read NES image `{}`: {error}",
            image_path.display()
        )
    });

    assert_eq!(image.len(), 0x6010, "expected NROM-128 image size");
    assert_eq!(&image[..4], b"NES\x1A");
    assert_eq!(&image[4..8], &[1, 1, 0, 0]);

    // The upstream program writes the 11 four-byte OAM entries for HELLO WORLD!
    // into $0200 before triggering the DMA transfer from $0200.
    let hello_oam = [
        0x6c, 0x00, 0x00, 0x3d, 0x6c, 0x01, 0x00, 0x46, 0x6c, 0x02, 0x00, 0x4f, 0x6c, 0x02, 0x00,
        0x58, 0x6c, 0x03, 0x00, 0x61, 0x75, 0x04, 0x00, 0x3d, 0x75, 0x03, 0x00, 0x46, 0x75, 0x05,
        0x00, 0x4f, 0x75, 0x02, 0x00, 0x58, 0x75, 0x06, 0x00, 0x62, 0x75, 0x07, 0x00, 0x6b,
    ];
    assert!(
        image[16..0x4010]
            .windows(hello_oam.len())
            .any(|window| window == hello_oam),
        "HELLO WORLD! sprite table is missing from PRG-ROM"
    );

    // The upstream source stores eight 8x8 glyphs as 16-byte NES tiles, then
    // pads the remainder of the 8 KiB CHR-ROM page with zeroes.
    let hello_chr = [
        0b11000011, 0b11000011, 0b11000011, 0b11111111, 0b11111111, 0b11000011, 0b11000011,
        0b11000011, 0, 0, 0, 0, 0, 0, 0, 0, 0b11111111, 0b11111111, 0b11000000, 0b11111100,
        0b11111100, 0b11000000, 0b11111111, 0b11111111, 0, 0, 0, 0, 0, 0, 0, 0, 0b11000000,
        0b11000000, 0b11000000, 0b11000000, 0b11000000, 0b11000000, 0b11111111, 0b11111111, 0, 0,
        0, 0, 0, 0, 0, 0, 0b01111110, 0b11100111, 0b11000011, 0b11000011, 0b11000011, 0b11000011,
        0b11100111, 0b01111110, 0, 0, 0, 0, 0, 0, 0, 0, 0b11000011, 0b11000011, 0b11000011,
        0b11000011, 0b11011011, 0b11011011, 0b11100111, 0b01000010, 0, 0, 0, 0, 0, 0, 0, 0,
        0b01111110, 0b11100111, 0b11000011, 0b11000011, 0b11111100, 0b11001100, 0b11000110,
        0b11000011, 0, 0, 0, 0, 0, 0, 0, 0, 0b11110000, 0b11001110, 0b11000010, 0b11000011,
        0b11000011, 0b11000010, 0b11001110, 0b11110000, 0, 0, 0, 0, 0, 0, 0, 0, 0b00011000,
        0b00011000, 0b00011000, 0b00011000, 0b00011000, 0, 0b00011000, 0b00011000, 0, 0, 0, 0, 0,
        0, 0, 0,
    ];
    assert_eq!(&image[0x4010..0x4010 + hello_chr.len()], &hello_chr);
    assert!(
        image[0x4010 + hello_chr.len()..]
            .iter()
            .all(|byte| *byte == 0)
    );

    let nmi = u16::from_le_bytes([image[0x400A], image[0x400B]]);
    let reset = u16::from_le_bytes([image[0x400C], image[0x400D]]);
    let irq = u16::from_le_bytes([image[0x400E], image[0x400F]]);
    for (name, vector) in [("NMI", nmi), ("reset", reset), ("IRQ", irq)] {
        assert!(
            (0xC000..=0xFFFF).contains(&vector),
            "{name} vector points outside PRG-ROM: 0x{vector:04X}"
        );
    }
}
