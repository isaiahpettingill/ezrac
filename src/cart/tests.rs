use std::path::Path;

use crate::layout::parse_layout;
use crate::parser::parse_program;
use crate::target::EZRA_LOAD_ADDR;

use super::*;

#[test]
fn default_header_matches_spec_offsets() {
    let bytes = CartridgeHeader::default().serialize();

    assert_eq!(&bytes[0x00..0x04], b"EZRA");
    assert_eq!(bytes[0x04], 1);
    assert_eq!(bytes[0x05], 1);
    assert_eq!(&bytes[0x08..0x0B], &[0x40, 0x00, 0x01]);
    assert_eq!(&bytes[0x0B..0x0E], &[0x00, 0x00, 0xF0]);
    assert_eq!(&bytes[0x1A..0x1C], &[0x40, 0x00]);
    assert_eq!(bytes.len(), 64);
}

#[test]
fn cartridge_without_embeds_writes_layout_table() {
    let program = parse_program(Path::new("game.ezra"), "fn main() { test.pass() }").unwrap();
    let image = build_cartridge(&program).unwrap();

    assert_eq!(&image[0x00..0x04], b"EZRA");
    assert_eq!(
        read_addr24(&image, 0x1E),
        EZRA_LOAD_ADDR.get() + u32::from(HEADER_SIZE)
    );
    assert_eq!(read_addr24(&image, 0x21), 0);
    assert!(image[HEADER_SIZE as usize..].starts_with(b"layout ezra_default\n"));
    let layout_text = std::str::from_utf8(&image[HEADER_SIZE as usize..]).unwrap();
    assert!(layout_text.contains("symbol EZRA_LOAD_ADDR"));
}

#[test]
fn cartridge_with_embeds_writes_asset_table_names_and_payloads() {
    let source = r#"
            embed palette: bytes = bytes [0x11, 0x22, 0x33] section .assets align 1
            embed title: bytes = cstr("OK") section .rodata align 4
            fn main() { test.pass() }
        "#;
    let program = parse_program(Path::new("game.ezra"), source).unwrap();
    let image = build_cartridge(&program).unwrap();

    assert_eq!(&image[0x00..0x04], b"EZRA");
    let layout_table = image_offset(read_addr24(&image, 0x1E));
    let table = image_offset(read_addr24(&image, 0x21));
    assert_eq!(layout_table, HEADER_SIZE as usize);
    assert!(image[layout_table..].starts_with(b"layout ezra_default\n"));
    assert!(table > layout_table);

    let first_addr = read_addr24(&image, table);
    assert_eq!(first_addr, 0x100000);
    assert_eq!(&image[table + 3..table + 6], &[0x03, 0x00, 0x00]);
    assert_eq!(&image[table + 6..table + 8], &[0x00, 0x00]);
    assert_eq!(image[table + 8], SECTION_ASSETS);
    assert_eq!(image[table + 9], 0);

    let second = table + ASSET_TABLE_ENTRY_SIZE;
    let second_addr = read_addr24(&image, second);
    assert_eq!(second_addr, 0x020000);
    assert_eq!(&image[second + 3..second + 6], &[0x03, 0x00, 0x00]);
    assert_eq!(&image[second + 6..second + 8], &[0x08, 0x00]);
    assert_eq!(image[second + 8], SECTION_RODATA);
    assert_eq!(image[second + 9], 0);

    let names = second + ASSET_TABLE_ENTRY_SIZE;
    assert_eq!(&image[names..names + 14], b"palette\0title\0");
    assert_eq!(
        &image[image_offset(first_addr)..image_offset(first_addr) + 3],
        &[0x11, 0x22, 0x33]
    );
    assert_eq!(
        &image[image_offset(second_addr)..image_offset(second_addr) + 3],
        &[b'O', b'K', 0]
    );
}

#[test]
fn cartridge_reports_missing_embedded_files() {
    let root = std::env::temp_dir().join(format!(
        "ezra_missing_cart_embed_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let source_path = root.join("game.ezra");
    let source = r#"
            embed blob: bytes = file("assets/missing.bin")
            fn main() { test.pass() }
        "#;
    let program = parse_program(&source_path, source).unwrap();
    let error = build_cartridge(&program).unwrap_err();

    assert_eq!(
        error.message,
        format!(
            "embedded file `{}` not found",
            root.join("assets/missing.bin").display()
        )
    );
}

#[test]
fn cartridge_file_embeds_fall_back_to_project_root() {
    let relative_dir = Path::new("target").join(format!(
        "ezra_project_root_cart_embed_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&relative_dir).unwrap();
    std::fs::write(relative_dir.join("blob.bin"), [0xCA, 0xFE]).unwrap();
    let source_root = std::env::temp_dir().join(format!(
        "ezra_project_root_cart_source_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let source_path = source_root.join("nested/game.ezra");
    let embed_path = format!("{}/blob.bin", relative_dir.display()).replace('\\', "/");
    let source = format!(
        r#"
            embed blob: bytes = file("{embed_path}") section .assets align 1
            fn main() {{ test.pass() }}
            "#
    );
    let program = parse_program(&source_path, &source).unwrap();
    let image = build_cartridge(&program).unwrap();
    let table = image_offset(read_addr24(&image, 0x21));
    let blob_addr = read_addr24(&image, table);
    let blob = image_offset(blob_addr);

    let _ = std::fs::remove_dir_all(&relative_dir);
    let _ = std::fs::remove_dir_all(&source_root);
    assert_eq!(&image[blob..blob + 2], &[0xCA, 0xFE]);
}

#[test]
fn cartridge_embed_expressions_use_defined_arithmetic() {
    let source = r#"
            embed values: bytes = bytes [
                0x11u8,
                5 / 0,
                5 % 0,
                ((-8) >> 2) + 256,
                ((-1i8) >> 64) + 256,
            ] section .assets align 1

            fn main() { test.pass() }
        "#;
    let program = parse_program(Path::new("game.ezra"), source).unwrap();
    let image = build_cartridge(&program).unwrap();
    let table = image_offset(read_addr24(&image, 0x21));
    let values_addr = read_addr24(&image, table);
    let values = image_offset(values_addr);

    assert_eq!(&image[values..values + 5], &[0x11, 0x00, 0x00, 0xFE, 0xFF]);
}

#[test]
fn cartridge_embed_expressions_can_use_constants() {
    let source = r#"
            alias byte = u8
            const VALUE: byte = 0x1FF
            const COUNT: u8 = 3
            const ALIGN: u8 = 4
            const FILL: u8 = 0x42

            embed values: bytes = bytes [VALUE, COUNT + 1] section .assets align ALIGN
            embed repeated: bytes = repeat(FILL, COUNT) section .assets align 1

            fn main() { test.pass() }
        "#;
    let program = parse_program(Path::new("game.ezra"), source).unwrap();
    let image = build_cartridge(&program).unwrap();
    let table = image_offset(read_addr24(&image, 0x21));
    let values_addr = read_addr24(&image, table);
    let repeated_addr = read_addr24(&image, table + ASSET_TABLE_ENTRY_SIZE);
    let values = image_offset(values_addr);
    let repeated = image_offset(repeated_addr);

    assert_eq!(values_addr % 4, 0);
    assert_eq!(&image[values..values + 2], &[0xFF, 0x04]);
    assert_eq!(&image[repeated..repeated + 3], &[0x42, 0x42, 0x42]);
}

#[test]
fn cartridge_embed_expressions_can_use_forward_constants() {
    let source = r#"
            alias byte = u8

            embed values: bytes = bytes [VALUE, cast<byte>(DEVICE)] section .assets align ALIGN
            embed repeated: bytes = repeat(FILL, COUNT) section .assets align 1

            const VALUE: byte = RAW_VALUE
            volatile mmio DEVICE: ptr<u8> = BASE_ADDR + 0x23
            const RAW_VALUE: u16 = 0x1FF
            const BASE_ADDR: u24 = 0x040100
            const COUNT: u8 = 2
            const ALIGN: u8 = 8
            const FILL: u8 = VALUE & 0x7F

            fn main() { test.pass() }
        "#;
    let program = parse_program(Path::new("game.ezra"), source).unwrap();
    let image = build_cartridge(&program).unwrap();
    let table = image_offset(read_addr24(&image, 0x21));
    let values_addr = read_addr24(&image, table);
    let repeated_addr = read_addr24(&image, table + ASSET_TABLE_ENTRY_SIZE);
    let values = image_offset(values_addr);
    let repeated = image_offset(repeated_addr);

    assert_eq!(values_addr % 8, 0);
    assert_eq!(&image[values..values + 2], &[0xFF, 0x23]);
    assert_eq!(&image[repeated..repeated + 2], &[0x7F, 0x7F]);
}

#[test]
fn cartridge_embed_expressions_support_casts_and_comparisons() {
    let source = r#"
            alias byte = u8

            const RAW: u16 = 0x1234
            const DEVICE: ptr<u8> = cast<ptr<u8>>(0x040123)

            embed values: bytes = bytes [
                cast<byte>(RAW),
                cast<u8>(-1),
                cast<bool>(2),
                cast<u8>(cast<ptr24>(DEVICE)),
                RAW > 0x1000,
                RAW == 0xFFFF,
                true && false,
                false || (RAW != 0),
                !false,
            ] section .assets align cast<u8>(4)

            fn main() { test.pass() }
        "#;
    let program = parse_program(Path::new("game.ezra"), source).unwrap();
    let image = build_cartridge(&program).unwrap();
    let table = image_offset(read_addr24(&image, 0x21));
    let values_addr = read_addr24(&image, table);
    let values = image_offset(values_addr);

    assert_eq!(values_addr % 4, 0);
    assert_eq!(
        &image[values..values + 9],
        &[0x34, 0xFF, 0x01, 0x23, 0x01, 0x00, 0x00, 0x01, 0x01]
    );
}

#[test]
fn embed_expression_division_overflow_is_defined() {
    let min = Expr::Int(i64::MIN);
    let minus_one = Expr::Unary {
        op: UnaryOp::Neg,
        expr: Box::new(Expr::Int(1)),
    };
    let div = Expr::Binary {
        left: Box::new(min.clone()),
        op: BinaryOp::Div,
        right: Box::new(minus_one.clone()),
    };
    let rem = Expr::Binary {
        left: Box::new(min),
        op: BinaryOp::Mod,
        right: Box::new(minus_one),
    };

    let constants = HashMap::new();
    let aliases = HashMap::new();
    assert_eq!(
        eval_embed_expr_with_aliases(&div, &constants, &aliases).unwrap(),
        i64::MIN
    );
    assert_eq!(
        eval_embed_expr_with_aliases(&rem, &constants, &aliases).unwrap(),
        0
    );
}

#[test]
fn cartridge_places_embeds_in_custom_layout_sections() {
    let source = r#"
            embed sprite_a: bytes = bytes [0xA1, 0xA2] section .sprites align 1
            embed font: bytes = bytes [0xF0] section .fonts align 1
            embed sprite_b: bytes = bytes [0xB1] section .sprites align 1
            fn main() { test.pass() }
        "#;
    let layout = parse_layout(
        r#"
                layout custom_embeds {
                    load 0x010000;
                    entry 0x010040;
                    stack 0xF00000;

                    region code 0x010000..0x01FFFF read execute;
                    region sprites 0x120000..0x1200FF read;
                    region fonts 0x130000..0x1300FF read;

                    section .header -> code align 64;
                    section .text -> code align 16;
                    section .sprites -> sprites align 1;
                    section .fonts -> fonts align 1;
                }
            "#,
    )
    .unwrap();
    let program = parse_program(Path::new("game.ezra"), source).unwrap();
    let image = build_cartridge_with_layout_code_and_symbols(&program, &layout, &[], &[]).unwrap();

    let table = image_offset(read_addr24(&image, 0x21));
    let sprite_a_addr = read_addr24(&image, table);
    assert_eq!(sprite_a_addr, 0x120000);
    assert_eq!(image[table + 8], SECTION_CUSTOM_BASE);

    let font = table + ASSET_TABLE_ENTRY_SIZE;
    let font_addr = read_addr24(&image, font);
    assert_eq!(font_addr, 0x130000);
    assert_eq!(image[font + 8], SECTION_CUSTOM_BASE + 1);

    let sprite_b = font + ASSET_TABLE_ENTRY_SIZE;
    let sprite_b_addr = read_addr24(&image, sprite_b);
    assert_eq!(sprite_b_addr, 0x120002);
    assert_eq!(image[sprite_b + 8], SECTION_CUSTOM_BASE);

    assert_eq!(
        &image[image_offset(sprite_a_addr)..image_offset(sprite_a_addr) + 2],
        &[0xA1, 0xA2]
    );
    assert_eq!(
        &image[image_offset(font_addr)..image_offset(font_addr) + 1],
        &[0xF0]
    );
    assert_eq!(
        &image[image_offset(sprite_b_addr)..image_offset(sprite_b_addr) + 1],
        &[0xB1]
    );
}

#[test]
fn cartridge_map_reports_final_placements() {
    let source = r#"
            embed palette: bytes = bytes [0x11, 0x22] section .assets align 1
            embed title: bytes = cstr("OK") section .rodata align 4
            fn main() { test.pass() }
        "#;
    let program = parse_program(Path::new("game.ezra"), source).unwrap();
    let symbols = vec![AssemblySymbol {
        name: "__ezra_start".to_owned(),
        addr: 0x010040,
    }];
    let map = build_cartridge_map(&program, &Layout::ezra_default(), 4, &symbols).unwrap();

    assert!(
        map.starts_with("section      start      end        size\n"),
        "{map}"
    );
    assert!(
        map.contains(".header      0x010000 0x01003F 0x000040"),
        "{map}"
    );
    assert!(
        map.contains(".text        0x010040 0x010043 0x000004"),
        "{map}"
    );
    assert!(
        map.contains(".layout_table") && map.contains(".symbol_table"),
        "{map}"
    );
    assert!(
        map.contains(".rodata      0x020000 0x020002 0x000003"),
        "{map}"
    );
    assert!(
        map.contains(".data        0x040000 0x040000 0x000000"),
        "{map}"
    );
    assert!(
        map.contains(".bss         0x040000 0x040000 0x000000"),
        "{map}"
    );
    assert!(
        map.contains(".assets      0x100000 0x100001 0x000002"),
        "{map}"
    );
    assert!(
        map.contains(".scratch     0xE00000 0xE00000 0x000000"),
        "{map}"
    );
    assert!(
        map.contains(".assets:palette 0x100000 0x100001 0x000002"),
        "{map}"
    );
    assert!(
        map.contains(".rodata:title 0x020000 0x020002 0x000003"),
        "{map}"
    );
}

#[test]
fn cartridge_map_places_shared_region_sections_sequentially() {
    let source = r#"
            embed first: bytes = bytes [0xA1, 0xA2] section .bank1 align 1
            embed second: bytes = bytes [0xB1] section .bank2 align 1
            fn main() { test.pass() }
        "#;
    let program = parse_program(Path::new("game.ezra"), source).unwrap();
    let layout = parse_layout(
        r#"
                layout banked {
                    load 0x010000;
                    entry 0x010040;
                    stack 0xF00000;

                    region code 0x010000..0x01FFFF read execute;
                    region bank 0x120000..0x12FFFF read;

                    section .header -> code align 64;
                    section .text -> code align 16;
                    section .bank1 -> bank align 256;
                    section .bank2 -> bank align 256;
                }
            "#,
    )
    .unwrap();
    let map = build_cartridge_map(&program, &layout, 4, &[]).unwrap();

    assert!(
        map.contains(".bank1       0x120000 0x120001 0x000002"),
        "{map}"
    );
    assert!(
        map.contains(".bank2       0x120100 0x120100 0x000001"),
        "{map}"
    );
    assert!(
        map.contains(".bank1:first 0x120000 0x120001 0x000002"),
        "{map}"
    );
    assert!(
        map.contains(".bank2:second 0x120100 0x120100 0x000001"),
        "{map}"
    );
}

#[test]
fn cartridge_map_reports_global_ram_usage() {
    let source = r#"
            alias byte = u8
            const COUNT: u8 = 3

            struct Pair {
                lo: u8
                hi: u16
            }

            global score: byte = COUNT + 4
            global coords: [u8; COUNT] = [0, 0, 0]
            global origin: Pair = Pair { lo: 0, hi: 0 }

            fn main() { test.pass() }
        "#;
    let program = parse_program(Path::new("game.ezra"), source).unwrap();
    let map = build_cartridge_map(&program, &Layout::ezra_default(), 4, &[]).unwrap();

    assert!(
        map.contains(".data        0x040000 0x040000 0x000001"),
        "{map}"
    );
    assert!(
        map.contains(".bss         0x040010 0x040015 0x000006"),
        "{map}"
    );
}

#[test]
fn cartridge_map_reports_string_literal_rodata_usage() {
    let source = r#"
            global title: ptr<u8> = "EZ"

            fn same(a: ptr<u8>, b: ptr<u8>) -> bool {
                return a == b
            }

            fn main() {
                let text: ptr<u8> = "OK"
                test.assert_eq_u8(same(text, "OK"), true, 1)
                test.pass()
            }
        "#;
    let program = parse_program(Path::new("game.ezra"), source).unwrap();
    let map = build_cartridge_map(&program, &Layout::ezra_default(), 4, &[]).unwrap();

    assert!(
        map.contains(".rodata      0x020000 0x020005 0x000006"),
        "{map}"
    );
}

#[test]
fn cartridge_with_code_places_text_at_entry_and_metadata_after_it() {
    let program = parse_program(Path::new("game.ezra"), "fn main() { test.pass() }").unwrap();
    let code = [0x31, 0x00, 0x00, 0xF0, 0xCD, 0x55, 0x00, 0x01];
    let image = build_cartridge_with_code(&program, &code).unwrap();

    assert_eq!(read_addr24(&image, 0x08), EZRA_ENTRY_ADDR.get());
    assert_eq!(
        &image[HEADER_SIZE as usize..HEADER_SIZE as usize + code.len()],
        &code
    );

    let layout_table = image_offset(read_addr24(&image, 0x1E));
    assert_eq!(layout_table, HEADER_SIZE as usize + code.len());
    assert!(image[layout_table..].starts_with(b"layout ezra_default\n"));
}

#[test]
fn cartridge_with_code_can_start_after_header_padding() {
    let program = parse_program(Path::new("game.ezra"), "fn main() { test.pass() }").unwrap();
    let code = [0x31, 0x00, 0x00, 0xF0];
    let layout = parse_layout(
        r#"
                layout padded_entry {
                    load 0x020000;
                    entry 0x020080;
                    stack 0xF00000;

                    region code 0x020000..0x02FFFF read execute;
                    section .header -> code align 64;
                    section .text -> code align 16;
                }
            "#,
    )
    .unwrap();

    let image =
        build_cartridge_with_layout_code_and_symbols(&program, &layout, &code, &[]).unwrap();

    assert_eq!(read_addr24(&image, 0x08), 0x020080);
    assert!(
        image[HEADER_SIZE as usize..0x80]
            .iter()
            .all(|byte| *byte == 0)
    );
    assert_eq!(&image[0x80..0x80 + code.len()], &code);
    let layout_table = usize::try_from(read_addr24(&image, 0x1E) - layout.load.get()).unwrap();
    assert_eq!(layout_table, 0x80 + code.len());
    assert!(image[layout_table..].starts_with(b"layout padded_entry\n"));
}

#[test]
fn cartridge_rejects_entry_inside_header() {
    let program = parse_program(Path::new("game.ezra"), "fn main() { test.pass() }").unwrap();
    let layout = parse_layout(
        r#"
                layout bad_entry {
                    load 0x020000;
                    entry 0x020020;
                    stack 0xF00000;

                    region code 0x020000..0x02FFFF read execute;
                    section .header -> code align 64;
                    section .text -> code align 16;
                }
            "#,
    )
    .unwrap();

    let error =
        build_cartridge_with_layout_code_and_symbols(&program, &layout, &[0x00], &[]).unwrap_err();

    assert_eq!(
        error.message,
        "entry 0x020020 overlaps the cartridge header at 0x020000"
    );
}

#[test]
fn cartridge_rejects_text_section_that_exceeds_region() {
    let program = parse_program(Path::new("game.ezra"), "fn main() { test.pass() }").unwrap();
    let layout = parse_layout(
        r#"
                layout tiny_text {
                    load 0x020000;
                    entry 0x020040;
                    stack 0xF00000;

                    region code 0x020000..0x020043 read execute;
                    section .header -> code align 64;
                    section .text -> code align 1;
                }
            "#,
    )
    .unwrap();

    let error =
        build_cartridge_with_layout_code_and_symbols(&program, &layout, &[0; 5], &[]).unwrap_err();

    assert_eq!(
        error.message,
        "section `.text` does not fit in region `code`"
    );
}

#[test]
fn cartridge_rejects_text_metadata_that_exceeds_region() {
    let program = parse_program(Path::new("game.ezra"), "fn main() { test.pass() }").unwrap();
    let layout = parse_layout(
        r#"
                layout tiny_metadata {
                    load 0x020000;
                    entry 0x020040;
                    stack 0xF00000;

                    region code 0x020000..0x020040 read execute;
                    section .header -> code align 64;
                    section .text -> code align 1;
                }
            "#,
    )
    .unwrap();

    let build_error =
        build_cartridge_with_layout_code_and_symbols(&program, &layout, &[], &[]).unwrap_err();
    let map_error = build_cartridge_map(&program, &layout, 0, &[]).unwrap_err();

    assert_eq!(
        build_error.message,
        "section `.text` does not fit in region `code`"
    );
    assert_eq!(
        map_error.message,
        "section `.text` does not fit in region `code`"
    );
}

#[test]
fn cartridge_rejects_text_entry_outside_region() {
    let program = parse_program(Path::new("game.ezra"), "fn main() { test.pass() }").unwrap();
    let layout = parse_layout(
        r#"
                layout misplaced_text {
                    load 0x020000;
                    entry 0x020080;
                    stack 0xF00000;

                    region code 0x020000..0x02007F read execute;
                    section .header -> code align 64;
                    section .text -> code align 1;
                }
            "#,
    )
    .unwrap();

    let error =
        build_cartridge_with_layout_code_and_symbols(&program, &layout, &[], &[]).unwrap_err();

    assert_eq!(
        error.message,
        "section `.text` does not fit in region `code`"
    );
}

#[test]
fn cartridge_with_symbols_writes_symbol_table_after_layout() {
    let program = parse_program(Path::new("game.ezra"), "fn main() { test.pass() }").unwrap();
    let code = [0x31, 0x00, 0x00, 0xF0];
    let symbols = [
        AssemblySymbol {
            name: "__ezra_start".to_owned(),
            addr: EZRA_ENTRY_ADDR.get(),
        },
        AssemblySymbol {
            name: "_main".to_owned(),
            addr: EZRA_ENTRY_ADDR.get() + 0x24,
        },
    ];
    let image = build_cartridge_with_code_and_symbols(&program, &code, &symbols).unwrap();

    let layout_table = image_offset(read_addr24(&image, 0x1E));
    let symbol_table = image_offset(read_addr24(&image, 0x24));
    assert!(symbol_table > layout_table);

    let text = std::str::from_utf8(&image[symbol_table..]).unwrap();
    assert!(text.starts_with("symbol __ezra_start 0x010040\n"), "{text}");
    assert!(text.contains("symbol _main 0x010064\n"), "{text}");
}

#[test]
fn cartridge_rejects_symbols_outside_address_space() {
    let program = parse_program(Path::new("game.ezra"), "fn main() { test.pass() }").unwrap();
    let symbols = [AssemblySymbol {
        name: "_bad".to_owned(),
        addr: 0x01_000000,
    }];

    let image_error = build_cartridge_with_code_and_symbols(&program, &[], &symbols).unwrap_err();
    let map_error =
        build_cartridge_map(&program, &Layout::ezra_default(), 0, &symbols).unwrap_err();

    assert_eq!(
        image_error.message,
        "assembly symbol `_bad` address 0x1000000 is outside the 24-bit address space"
    );
    assert_eq!(map_error.message, image_error.message);
}

#[test]
fn cartridge_rejects_duplicate_symbols() {
    let program = parse_program(Path::new("game.ezra"), "fn main() { test.pass() }").unwrap();
    let symbols = [
        AssemblySymbol {
            name: "_main".to_owned(),
            addr: EZRA_ENTRY_ADDR.get(),
        },
        AssemblySymbol {
            name: "_main".to_owned(),
            addr: EZRA_ENTRY_ADDR.get() + 4,
        },
    ];

    let image_error = build_cartridge_with_code_and_symbols(&program, &[], &symbols).unwrap_err();
    let map_error =
        build_cartridge_map(&program, &Layout::ezra_default(), 0, &symbols).unwrap_err();

    assert_eq!(image_error.message, "duplicate assembly symbol `_main`");
    assert_eq!(map_error.message, image_error.message);
}

#[test]
fn cartridge_rejects_embed_that_exceeds_section_region() {
    let source = r#"
            embed too_big: bytes = repeat(0xAA, 3) section .assets align 1
            fn main() { test.pass() }
        "#;
    let layout = parse_layout(
        r#"
                layout tiny_assets {
                    load 0x010000;
                    entry 0x010040;
                    stack 0xF00000;

                    region code 0x010000..0x01FFFF read execute;
                    region assets 0x100000..0x100001 read;

                    section .header -> code align 64;
                    section .text -> code align 16;
                    section .assets -> assets align 1;
                }
            "#,
    )
    .unwrap();
    let program = parse_program(Path::new("game.ezra"), source).unwrap();

    let error =
        build_cartridge_with_layout_code_and_symbols(&program, &layout, &[], &[]).unwrap_err();

    assert_eq!(
        error.message,
        "embed `too_big` exceeds section `.assets` region `assets`"
    );
}

#[test]
fn cartridge_rejects_embed_alignment_outside_address_space() {
    let source = r#"
            embed sprite: bytes = bytes [0xAA] align 0x100000000
            fn main() { test.pass() }
        "#;
    let program = parse_program(Path::new("game.ezra"), source).unwrap();

    let error = build_cartridge(&program).unwrap_err();

    assert_eq!(
        error.message,
        "embed `sprite` alignment 4294967296 exceeds 24-bit address space"
    );
}

#[test]
fn cartridge_rejects_non_integer_embed_alignment() {
    let cases = [
        r#"
            embed sprite: bytes = bytes [0xAA] align true
            fn main() { test.pass() }
            "#,
        r#"
            embed sprite: bytes = bytes [0xAA] align (1 == 1)
            fn main() { test.pass() }
            "#,
        r#"
            const ALIGN: bool = true
            embed sprite: bytes = bytes [0xAA] align ALIGN
            fn main() { test.pass() }
            "#,
        r#"
            embed sprite: bytes = bytes [0xAA] align (true + 1)
            fn main() { test.pass() }
            "#,
        r#"
            embed sprite: bytes = bytes [0xAA] align cast<bool>(1)
            fn main() { test.pass() }
            "#,
    ];

    for source in cases {
        let program = parse_program(Path::new("game.ezra"), source).unwrap();

        let error = build_cartridge(&program).unwrap_err();

        assert_eq!(
            error.message,
            "embed `sprite` alignment must be an integer constant"
        );
    }
}

#[test]
fn cartridge_rejects_layout_without_header_section() {
    let program = parse_program(Path::new("game.ezra"), "fn main() { test.pass() }").unwrap();
    let layout = parse_layout(
        r#"
                layout missing_header {
                    load 0x020000;
                    entry 0x020040;
                    stack 0xF00000;

                    region code 0x020000..0x02FFFF read execute;
                    section .text -> code align 16;
                }
            "#,
    )
    .unwrap();

    let error =
        build_cartridge_with_layout_code_and_symbols(&program, &layout, &[], &[]).unwrap_err();

    assert_eq!(error.message, "layout has no section `.header`");
}

#[test]
fn cartridge_rejects_header_section_that_exceeds_region() {
    let program = parse_program(Path::new("game.ezra"), "fn main() { test.pass() }").unwrap();
    let layout = parse_layout(
        r#"
                layout tiny_header {
                    load 0x020000;
                    entry 0x020040;
                    stack 0xF00000;

                    region header 0x020000..0x02003E read;
                    region code 0x020040..0x02FFFF read execute;
                    section .header -> header align 64;
                    section .text -> code align 16;
                }
            "#,
    )
    .unwrap();

    let error =
        build_cartridge_with_layout_code_and_symbols(&program, &layout, &[], &[]).unwrap_err();

    assert_eq!(
        error.message,
        "section `.header` does not fit in region `header`"
    );
}

fn read_addr24(bytes: &[u8], offset: usize) -> u32 {
    u32::from(bytes[offset])
        | (u32::from(bytes[offset + 1]) << 8)
        | (u32::from(bytes[offset + 2]) << 16)
}

fn image_offset(addr: u32) -> usize {
    usize::try_from(addr - EZRA_LOAD_ADDR.get()).unwrap()
}
