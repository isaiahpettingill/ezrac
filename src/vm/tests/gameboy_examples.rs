use super::*;

const BACKGROUND_TILE: &str = include_str!("../../../tests/fixtures/gameboy/background-tile.asm");
const SPRITE: &str = include_str!("../../../tests/fixtures/gameboy/sprite.asm");

#[test]
fn assembles_cc0_background_tile_example() {
    let assembled =
        assemble_subset_with_symbols_at(AssemblerCpu::Lr35902, BACKGROUND_TILE, 0x0150).unwrap();

    assert_eq!(
        &assembled.bytes[..8],
        &[0xF3, 0x31, 0x00, 0xE0, 0xF0, 0x44, 0xFE, 0x90]
    );
    assert!(
        assembled
            .symbols
            .iter()
            .any(|symbol| symbol.name == "tile_data")
    );
    assert!(
        assembled
            .bytes
            .windows(2)
            .any(|bytes| bytes == [0x3E, 0x91])
    );
}

#[test]
fn assembles_cc0_sprite_example() {
    let assembled = assemble_subset_with_symbols_at(AssemblerCpu::Lr35902, SPRITE, 0x0150).unwrap();

    assert!(
        assembled
            .symbols
            .iter()
            .any(|symbol| symbol.name == "hide_unused")
    );
    assert!(
        assembled
            .bytes
            .windows(2)
            .any(|bytes| bytes == [0x3E, 0x50])
    );
    assert!(
        assembled
            .bytes
            .windows(2)
            .any(|bytes| bytes == [0x3E, 0x82])
    );
}

#[test]
fn rgbds_numeric_literals_and_flag_expressions_are_supported() {
    let assembled = assemble_subset_with_symbols_at(
        AssemblerCpu::Lr35902,
        "FLAGS equ %10000000 | %00010000 | %00000001\nld a, FLAGS\nld hl, $8000\n",
        0x0150,
    )
    .unwrap();

    assert_eq!(assembled.bytes, [0x3E, 0x91, 0x21, 0x00, 0x80]);
}
