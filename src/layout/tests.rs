use super::*;

#[test]
fn default_layout_validates() {
    assert_eq!(Layout::ezra_default().validate(), Ok(()));
}

#[test]
fn ez180n_layout_places_text_at_console_load_address() {
    let layout = Layout::ez180n();

    assert_eq!(layout.validate(), Ok(()));
    assert_eq!(layout.load.get(), 0x00_FFC0);
    assert_eq!(layout.entry.get(), 0x01_0000);
    assert_eq!(
        layout_symbol_value(&layout, "EZRA_CODE_BASE"),
        Some(0x01_0000)
    );
}

#[test]
fn z80_default_layout_validates_and_stays_in_16_bit_address_space() {
    let layout = Layout::z80_default();

    assert_eq!(layout.validate(), Ok(()));
    assert!(layout.load.get() <= 0xFFFF);
    assert!(layout.entry.get() <= 0xFFFF);
    assert!(layout.stack.get() <= 0xFFFF);
    assert!(
        layout
            .regions
            .iter()
            .all(|region| region.end.get() <= 0xFFFF)
    );
    assert!(
        layout
            .symbols
            .iter()
            .all(|symbol| symbol.value.get() <= 0xFFFF)
    );
}

#[test]
fn ti99_4a_layout_reserves_console_io_and_uses_cartridge_rom() {
    let layout = Layout::ti99_4a_tms9900();

    layout.validate().unwrap();
    assert_eq!(layout.load.get(), 0x6000);
    assert_eq!(layout.entry.get(), 0x6000);
    assert_eq!(layout_symbol_value(&layout, "TI99_WORKSPACE"), Some(0x8300));
    assert!(
        layout
            .regions
            .iter()
            .any(|region| region.name == "vdp_write")
    );
}

#[test]
fn bare_6502_layout_reserves_zero_page_and_hardware_stack() {
    let layout = Layout::bare_6502();
    layout.validate().unwrap();

    assert_eq!(layout.load.get(), 0x0200);
    assert_eq!(layout.entry.get(), 0x0200);
    assert_eq!(layout.stack.get(), 0x01FF);
    assert!(
        layout
            .regions
            .iter()
            .any(|region| region.name == "zero_page")
    );
}

#[test]
fn bare_m68k_layout_uses_a_24_bit_stack_and_ram() {
    let layout = Layout::bare_m68k();
    layout.validate().unwrap();

    assert_eq!(layout.load.get(), 0x000100);
    assert_eq!(layout.entry.get(), 0x000100);
    assert_eq!(layout.stack.get(), 0xFF0000);
    assert_eq!(
        layout_symbol_value(&layout, "EZRA_RAM_BASE"),
        Some(0x080000)
    );
}

#[test]
fn ez80_test_harness_layouts_validate() {
    let flat = Layout::ez80_test_flat();
    let split = Layout::ez80_test_split();

    assert_eq!(flat.validate(), Ok(()));
    assert_eq!(flat.name, "ez80_test_flat");
    assert_eq!(flat.entry.get(), 0x01_0040);
    assert_eq!(flat.stack.get(), 0x0F_FF00);

    assert_eq!(split.validate(), Ok(()));
    assert_eq!(split.name, "ez80_test_split");
    assert_eq!(split.entry.get(), 0x02_0040);
    assert_eq!(split.stack.get(), 0x1F_FF00);
    assert!(
        split
            .regions
            .iter()
            .any(|region| { region.name == "rom" && region.flags.contains(RegionFlags::RESERVED) })
    );
}

#[test]
fn cpm_z80_com_layout_uses_com_entry_and_stays_in_16_bit_address_space() {
    let layout = Layout::cpm_z80_com();

    assert_eq!(layout.validate(), Ok(()));
    assert_eq!(layout.load.get(), 0x0100);
    assert_eq!(layout.entry.get(), 0x0100);
    assert!(
        layout
            .regions
            .iter()
            .all(|region| region.end.get() <= 0xFFFF)
    );
    assert!(
        layout
            .symbols
            .iter()
            .all(|symbol| symbol.value.get() <= 0xFFFF)
    );
}

#[test]
fn overlapping_regions_are_reported() {
    let mut layout = Layout::ezra_default();
    layout
        .regions
        .push(region("bad", 0x01_8000, 0x02_8000, &[RegionFlags::READ]));

    let errors = layout.validate().unwrap_err();

    assert!(errors.iter().any(|error| error.message.contains("overlap")));
}

#[test]
fn parses_default_layout_file_shape() {
    let source = r#"
            layout ezra_default {
                load  0x010000;
                entry 0x010040;
                stack 0xF00000;

                region low       0x000000..0x00FFFF reserved;
                region code      0x010000..0x01FFFF read execute;
                region rodata    0x020000..0x03FFFF read;
                region ram       0x040000..0x07FFFF read write;
                region vram      0x080000..0x0BFFFF read write volatile;
                region audio     0x0C0000..0x0FFFFF read write volatile;
                region assets    0x100000..0xDFFFFF read;
                region scratch   0xE00000..0xEFFFFF read write;
                region stack     0xF00000..0xFFFFFF read write reserved;

                section .header  -> code   align 64;
                section .text    -> code   align 16;
                section .rodata  -> rodata align 16;
                section .data    -> ram    align 16;
                section .bss     -> ram    align 16;
                section .assets  -> assets align 256;
                section .scratch -> scratch align 16;

                symbol EZRA_LOAD_ADDR   = 0x010000;
                symbol EZRA_ENTRY_ADDR  = 0x010040;
                symbol EZRA_CODE_BASE   = 0x010040;
                symbol EZRA_STACK_TOP   = 0xF00000;
                symbol EZRA_RAM_BASE    = 0x040000;
                symbol EZRA_VRAM_BASE   = 0x080000;
                symbol EZRA_AUDIO_BASE  = 0x0C0000;
                symbol EZRA_ASSET_BASE  = 0x100000;
                symbol EZRA_RODATA_BASE = 0x020000;
            }
        "#;

    let layout = parse_layout(source).unwrap();

    assert_eq!(layout, Layout::ezra_default());
    assert_eq!(layout.validate(), Ok(()));
}

#[test]
fn parses_layout_symbol_expressions() {
    let source = r#"
            layout exprs {
                load 0x010000;
                entry 0x010040;
                stack 0xF00000;

                symbol TEXT_END = 0x010040 + 0x20 * 3;
                symbol MIRROR = TEXT_END + (0b1000 | 0b0011);
                symbol DIV_ZERO = 0x123456 / 0;
                symbol MOD_ZERO = 0x123456 % 0;
                symbol CAST_BYTE = cast<u8>(0x1234);
                symbol CAST_BOOL = cast<bool>(0x20);
                symbol CAST_PTR = cast<ptr<u8>>(0x1020003);
                symbol NEG_WRAP = cast<u8>(-3);
                symbol NOT_ZERO = !0;
                symbol BIT_NOT = ~0x00FF & 0xFFFF;
                symbol SHIFT_LEFT = 3 << 4;
                symbol SHIFT_RIGHT = 0x8000 >> 8;
                symbol LOGIC = (1 < 2) && (3 >= 3) || false;
                symbol COMPARE = 0x20 != 0x10;
            }
        "#;

    let layout = parse_layout(source).unwrap();

    assert_eq!(layout.symbols[0].value.get(), 0x010040 + 0x20 * 3);
    assert_eq!(
        layout.symbols[1].value.get(),
        0x010040 + 0x20 * 3 + (0b1000 | 0b0011)
    );
    assert_eq!(layout.symbols[2].value.get(), 0);
    assert_eq!(layout.symbols[3].value.get(), 0);
    assert_eq!(layout.symbols[4].value.get(), 0x34);
    assert_eq!(layout.symbols[5].value.get(), 1);
    assert_eq!(layout.symbols[6].value.get(), 0x020003);
    assert_eq!(layout.symbols[7].value.get(), 253);
    assert_eq!(layout.symbols[8].value.get(), 1);
    assert_eq!(layout.symbols[9].value.get(), (!0x00FF_u32) & 0xFFFF);
    assert_eq!(layout.symbols[10].value.get(), 3 << 4);
    assert_eq!(layout.symbols[11].value.get(), 0x8000 >> 8);
    assert_eq!(layout.symbols[12].value.get(), 1);
    assert_eq!(layout.symbols[13].value.get(), 1);
}

#[test]
fn layout_arithmetic_overflow_is_defined() {
    assert_eq!(
        eval_layout_binary_op(i128::MIN, "/", -1).unwrap(),
        i128::MIN
    );
    assert_eq!(eval_layout_binary_op(i128::MIN, "%", -1).unwrap(), 0);
    assert_eq!(eval_layout_unary_op("-", i128::MIN).unwrap(), i128::MIN);
}

#[test]
fn parsed_layout_uses_existing_validator() {
    let source = r#"
            layout bad {
                load 0x010000;
                entry 0x010040;
                stack 0xF00000;

                region code 0x010000..0x01FFFF read execute;
                region also_code 0x018000..0x02FFFF read;
                section .text -> missing align 24;
            }
        "#;

    let layout = parse_layout(source).unwrap();
    let errors = layout.validate().unwrap_err();

    assert!(
        errors.iter().any(|error| error.message.contains("overlap")),
        "{errors:?}"
    );
    assert!(
        errors
            .iter()
            .any(|error| error.message.contains("alignment")),
        "{errors:?}"
    );
    assert!(
        errors
            .iter()
            .any(|error| error.message.contains("unknown region")),
        "{errors:?}"
    );
}

#[test]
fn rejects_duplicate_layout_names() {
    let source = r#"
            layout duplicate_names {
                load 0x010000;
                entry 0x010040;
                stack 0xF00000;

                region code 0x010000..0x01FFFF read execute;
                region code 0x020000..0x02FFFF read;
                section .text -> code align 16;
                section .text -> code align 32;
                symbol BASE = 0x010000;
                symbol BASE = 0x020000;
            }
        "#;

    let layout = parse_layout(source).unwrap();
    let errors = layout.validate().unwrap_err();

    assert!(
        errors
            .iter()
            .any(|error| error.message == "duplicate region `code`"),
        "{errors:?}"
    );
    assert!(
        errors
            .iter()
            .any(|error| error.message == "duplicate section `.text`"),
        "{errors:?}"
    );
    assert!(
        errors
            .iter()
            .any(|error| error.message == "duplicate symbol `BASE`"),
        "{errors:?}"
    );
}

#[test]
fn rejects_layouts_missing_required_sections() {
    let source = r#"
            layout missing_sections {
                load 0x010000;
                entry 0x010040;
                stack 0xF00000;

                region code 0x010000..0x01FFFF read execute;
                region ram 0x040000..0x07FFFF read write;
                section .header -> code align 64;
                section .text -> code align 16;
            }
        "#;

    let layout = parse_layout(source).unwrap();
    let errors = layout.validate().unwrap_err();

    for section in [".rodata", ".data", ".bss", ".assets", ".scratch"] {
        assert!(
            errors.iter().any(|error| {
                error.message == format!("layout is missing required section `{section}`")
            }),
            "{errors:?}"
        );
    }
}

#[test]
fn rejects_layout_address_outside_24_bit_space() {
    let error = parse_layout(
        r#"
                layout too_wide {
                    load 0x1000000;
                    entry 0x010040;
                    stack 0xF00000;
                }
            "#,
    )
    .unwrap_err();

    assert!(error.message.contains("outside the 24-bit address space"));
}
