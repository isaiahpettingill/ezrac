use super::*;
use crate::target::parse_target_triple;

#[test]
fn z80_assembler_uses_16_bit_absolute_branches() {
    let bytes =
        assemble_subset_at(CpuFamily::Z80, "call 0005h\njp done\ndone:\nret\n", 0x0100).unwrap();

    assert_eq!(bytes, [0xCD, 0x05, 0x00, 0xC3, 0x06, 0x01, 0xC9]);
}

#[test]
fn lr35902_assembler_has_exact_golden_encodings() {
    let cases: &[(&str, &[u8])] = &[
        ("ld bc, 1234h", &[0x01, 0x34, 0x12]),
        ("ld (1234h), sp", &[0x08, 0x34, 0x12]),
        ("stop", &[0x10, 0x00]),
        ("ld (hl+), a", &[0x22]),
        ("ldi a, (hl)", &[0x2A]),
        ("ld (hld), a", &[0x32]),
        ("ldd a, (hl)", &[0x3A]),
        ("ldh (80h), a", &[0xE0, 0x80]),
        ("ldh a, (0FF80h)", &[0xF0, 0x80]),
        ("ld (c), a", &[0xE2]),
        ("ld a, (c)", &[0xF2]),
        ("add sp, -128", &[0xE8, 0x80]),
        ("ld hl, sp+127", &[0xF8, 0x7F]),
        ("jp (hl)", &[0xE9]),
        ("swap (hl)", &[0xCB, 0x36]),
        ("bit 7, (hl)", &[0xCB, 0x7E]),
        ("res 3, a", &[0xCB, 0x9F]),
        ("set 0, b", &[0xCB, 0xC0]),
        ("rst 38h", &[0xFF]),
    ];

    for (syntax, expected) in cases {
        assert_eq!(
            assemble_subset_at(CpuFamily::Lr35902, syntax, 0x0150).unwrap(),
            *expected,
            "{syntax}"
        );
    }
}

#[test]
fn lr35902_assembler_encodes_branch_and_address_boundaries() {
    let source = "start:\n jr nz, forward\n jr start\nforward:\n jp z, start\n call c, 0FFFFh\n";
    assert_eq!(
        assemble_subset_at(CpuFamily::Lr35902, source, 0x0150).unwrap(),
        [0x20, 0x02, 0x18, 0xFC, 0xCA, 0x50, 0x01, 0xDC, 0xFF, 0xFF]
    );
    assert_eq!(relative_offset(0x0150, 0x00D2).unwrap(), 0x80);
    assert_eq!(relative_offset(0x0150, 0x01D1).unwrap(), 0x7F);
    assert!(relative_offset(0x0150, 0x00D1).is_err());
    assert!(relative_offset(0x0150, 0x01D2).is_err());

    let error = assemble_subset_at(CpuFamily::Lr35902, "jp 10000h", 0x0150).unwrap_err();
    assert!(error.message.contains("outside u16 range"), "{error}");
}

#[test]
fn lr35902_assembler_rejects_z80_only_syntax_with_clear_diagnostics() {
    for syntax in [
        "out (1), a",
        "in a, (1)",
        "exx",
        "djnz 0",
        "ld ix, 1234h",
        "ld iy, 1234h",
        "jp po, 1234h",
    ] {
        let error = assemble_subset_at(CpuFamily::Lr35902, syntax, 0x0150).unwrap_err();
        assert!(
            error.message.contains("LR35902") || error.message.contains("branch condition"),
            "unexpected diagnostic for `{syntax}`: {error}"
        );
    }

    for (syntax, expected) in [
        ("bit 8, a", "outside 0..7"),
        ("push sp", "stack register"),
        ("ld (hl), (hl)", "use `halt`"),
        ("rst 40h", "restart vector"),
        ("add sp, 128", "outside -128..127"),
        ("ldh (100h), a", "outside FF00h..FFFFh"),
    ] {
        let error = assemble_subset_at(CpuFamily::Lr35902, syntax, 0x0150).unwrap_err();
        assert!(
            error.message.contains(expected),
            "unexpected diagnostic: {error}"
        );
    }
}

#[test]
fn lr35902_assembler_covers_every_documented_opcode() {
    let mut base = HashSet::new();
    let mut cb = HashSet::new();
    let mut add = |syntax: String| {
        let bytes = assemble_subset_at(CpuFamily::Lr35902, &syntax, 0)
            .unwrap_or_else(|error| panic!("{syntax}: {error}"));
        if bytes[0] == 0xCB {
            cb.insert(bytes[1]);
        } else {
            base.insert(bytes[0]);
        }
    };
    for syntax in [
        "nop",
        "rlca",
        "ld (1234h), sp",
        "rrca",
        "stop",
        "rla",
        "jr 2",
        "rra",
        "daa",
        "cpl",
        "scf",
        "ccf",
        "halt",
        "jp 1234h",
        "ret",
        "call 1234h",
        "reti",
        "ldh (80h), a",
        "ldh (c), a",
        "add sp, 1",
        "jp hl",
        "ld (1234h), a",
        "ldh a, (80h)",
        "ldh a, (c)",
        "di",
        "ld hl, sp+1",
        "ld sp, hl",
        "ld a, (1234h)",
        "ei",
    ] {
        add(syntax.to_owned());
    }
    let r8 = ["b", "c", "d", "e", "h", "l", "(hl)", "a"];
    let r16 = ["bc", "de", "hl", "sp"];
    let memory = ["(bc)", "(de)", "(hl+)", "(hl-)"];
    for (index, register) in r16.iter().enumerate() {
        add(format!("ld {register}, 1234h"));
        add(format!("inc {register}"));
        add(format!("dec {register}"));
        add(format!("add hl, {register}"));
        add(format!("ld {}, a", memory[index]));
        add(format!("ld a, {}", memory[index]));
    }
    for register in r8 {
        add(format!("inc {register}"));
        add(format!("dec {register}"));
        add(format!("ld {register}, 12h"));
    }
    for dst in r8 {
        for src in r8 {
            if !(dst == "(hl)" && src == "(hl)") {
                add(format!("ld {dst}, {src}"));
            }
        }
    }
    for (operation, explicit_a) in [
        ("add", true),
        ("adc", true),
        ("sub", false),
        ("sbc", true),
        ("and", false),
        ("xor", false),
        ("or", false),
        ("cp", false),
    ] {
        for register in r8 {
            add(if explicit_a {
                format!("{operation} a, {register}")
            } else {
                format!("{operation} {register}")
            });
        }
        add(if explicit_a {
            format!("{operation} a, 12h")
        } else {
            format!("{operation} 12h")
        });
    }
    for condition in ["nz", "z", "nc", "c"] {
        add(format!("jr {condition}, 2"));
        add(format!("ret {condition}"));
        add(format!("jp {condition}, 1234h"));
        add(format!("call {condition}, 1234h"));
    }
    for register in ["bc", "de", "hl", "af"] {
        add(format!("push {register}"));
        add(format!("pop {register}"));
    }
    for vector in (0..=0x38).step_by(8) {
        add(format!("rst {vector}"));
    }
    for operation in ["rlc", "rrc", "rl", "rr", "sla", "sra", "swap", "srl"] {
        for register in r8 {
            add(format!("{operation} {register}"));
        }
    }
    for operation in ["bit", "res", "set"] {
        for bit in 0..8 {
            for register in r8 {
                add(format!("{operation} {bit}, {register}"));
            }
        }
    }
    let invalid = [
        0xD3, 0xDB, 0xDD, 0xE3, 0xE4, 0xEB, 0xEC, 0xED, 0xF4, 0xFC, 0xFD,
    ];
    // 244 executable base instructions plus the CB prefix itself.
    assert_eq!(base.len(), 244);
    assert!(invalid.into_iter().all(|opcode| !base.contains(&opcode)));
    assert_eq!(cb.len(), 256);
}

#[test]
fn assembles_interrupt_enable_and_disable_instructions() {
    let bytes = assemble_ez80_subset_at("di\nei\nret\n", EZRA_LOAD_ADDR.get()).unwrap();

    assert_eq!(bytes, [0xF3, 0xFB, 0xC9]);
}

#[test]
fn assembles_nop_instruction() {
    let bytes = assemble_ez80_subset_at("nop\nret\n", EZRA_LOAD_ADDR.get()).unwrap();

    assert_eq!(bytes, [0x00, 0xC9]);
}

#[test]
fn assembles_register_exchange_instructions() {
    let bytes = assemble_ez80_subset_at("ex de, hl\nexx\n", EZRA_LOAD_ADDR.get()).unwrap();

    assert_eq!(bytes, [0xEB, 0xD9]);
}

#[test]
fn assembles_interrupt_return_instructions() {
    let bytes = assemble_ez80_subset_at("reti\nretn\n", EZRA_LOAD_ADDR.get()).unwrap();

    assert_eq!(bytes, [0xED, 0x4D, 0xED, 0x45]);
}

#[test]
fn assembles_restart_instructions() {
    let asm = "rst 00h\nrst 08h\nrst 10h\nrst 18h\nrst 20h\nrst 28h\nrst 30h\nrst 38h\n";
    let bytes = assemble_ez80_subset_at(asm, EZRA_LOAD_ADDR.get()).unwrap();

    assert_eq!(bytes, [0xC7, 0xCF, 0xD7, 0xDF, 0xE7, 0xEF, 0xF7, 0xFF]);
}

#[test]
fn assembles_lis_restart_instructions() {
    let bytes = assemble_ez80_subset_at("rst.lis 10h\n", EZRA_LOAD_ADDR.get()).unwrap();

    assert_eq!(bytes, [0x49, 0xD7]);
}

#[test]
fn assembles_common_control_and_special_register_instructions() {
    let asm = r#"
            halt
            im 0
            im 1
            im 2
            rld
            rrd
            ld i, a
            ld r, a
            ld a, i
            ld a, r
        "#;
    let bytes = assemble_ez80_subset_at(asm, EZRA_LOAD_ADDR.get()).unwrap();

    assert_eq!(
        bytes,
        [
            0x76, 0xED, 0x46, 0xED, 0x56, 0xED, 0x5E, 0xED, 0x6F, 0xED, 0x67, 0xED, 0x47, 0xED,
            0x4F, 0xED, 0x57, 0xED, 0x5F,
        ]
    );
}

#[test]
fn assembles_more_16_bit_register_instructions() {
    let asm = r#"
            inc bc
            inc de
            inc hl
            inc sp
            dec bc
            dec de
            dec hl
            dec sp
            adc hl, bc
            adc hl, de
            adc hl, hl
            adc hl, sp
            sbc hl, hl
            sbc hl, sp
            ld sp, hl
            ld sp, ix
            ld sp, iy
            ex (sp), hl
            ex (sp), ix
            ex (sp), iy
            jp (hl)
            jp (ix)
            jp (iy)
        "#;
    let bytes = assemble_ez80_subset_at(asm, EZRA_LOAD_ADDR.get()).unwrap();

    assert_eq!(
        bytes,
        [
            0x03, 0x13, 0x23, 0x33, 0x0B, 0x1B, 0x2B, 0x3B, 0xED, 0x4A, 0xED, 0x5A, 0xED, 0x6A,
            0xED, 0x7A, 0xED, 0x62, 0xED, 0x72, 0xF9, 0xDD, 0xF9, 0xFD, 0xF9, 0xE3, 0xDD, 0xE3,
            0xFD, 0xE3, 0xE9, 0xDD, 0xE9, 0xFD, 0xE9,
        ]
    );
}

#[test]
fn assembles_hl_indirect_alu_and_cb_instructions() {
    let asm = r#"
            add a, (hl)
            adc a, (hl)
            sub (hl)
            sbc a, (hl)
            and (hl)
            xor (hl)
            or (hl)
            cp (hl)
            inc (hl)
            dec (hl)
            rlc (hl)
            rrc (hl)
            rl (hl)
            rr (hl)
            sla (hl)
            sra (hl)
            srl (hl)
            bit 0, (hl)
            res 1, (hl)
            set 7, (hl)
        "#;
    let bytes = assemble_ez80_subset_at(asm, EZRA_LOAD_ADDR.get()).unwrap();

    assert_eq!(
        bytes,
        [
            0x86, 0x8E, 0x96, 0x9E, 0xA6, 0xAE, 0xB6, 0xBE, 0x34, 0x35, 0xCB, 0x06, 0xCB, 0x0E,
            0xCB, 0x16, 0xCB, 0x1E, 0xCB, 0x26, 0xCB, 0x2E, 0xCB, 0x3E, 0xCB, 0x46, 0xCB, 0x8E,
            0xCB, 0xFE,
        ]
    );
}

#[test]
fn assembles_ix_iy_indexed_load_store_and_alu_forms() {
    let asm = r#"
            ld b, (ix+1)
            ld c, (iy-2)
            ld (ix+3), d
            ld (iy-4), e
            ld (ix+5), 7Fh
            inc (iy+6)
            dec (ix-7)
            add a, (ix+8)
            adc a, (iy+9)
            sub (ix+10)
            sbc a, (iy+11)
            and (ix+12)
            xor (iy+13)
            or (ix+14)
            cp (iy+15)
        "#;
    let bytes = assemble_ez80_subset_at(asm, EZRA_LOAD_ADDR.get()).unwrap();

    assert_eq!(
        bytes,
        [
            0xDD, 0x46, 0x01, 0xFD, 0x4E, 0xFE, 0xDD, 0x72, 0x03, 0xFD, 0x73, 0xFC, 0xDD, 0x36,
            0x05, 0x7F, 0xFD, 0x34, 0x06, 0xDD, 0x35, 0xF9, 0xDD, 0x86, 0x08, 0xFD, 0x8E, 0x09,
            0xDD, 0x96, 0x0A, 0xFD, 0x9E, 0x0B, 0xDD, 0xA6, 0x0C, 0xFD, 0xAE, 0x0D, 0xDD, 0xB6,
            0x0E, 0xFD, 0xBE, 0x0F,
        ]
    );
}

#[test]
fn assembles_ix_iy_indexed_cb_forms() {
    let asm = r#"
            rlc (ix+1)
            rr (iy-2)
            bit 3, (ix+4)
            res 2, (iy+5)
            set 7, (ix-6)
        "#;
    let bytes = assemble_ez80_subset_at(asm, EZRA_LOAD_ADDR.get()).unwrap();

    assert_eq!(
        bytes,
        [
            0xDD, 0xCB, 0x01, 0x06, 0xFD, 0xCB, 0xFE, 0x1E, 0xDD, 0xCB, 0x04, 0x5E, 0xFD, 0xCB,
            0x05, 0x96, 0xDD, 0xCB, 0xFA, 0xFE,
        ]
    );
}

#[test]
fn assembles_more_ix_iy_16_bit_forms() {
    let asm = r#"
            inc ix
            inc iy
            dec ix
            dec iy
            add ix, bc
            add ix, de
            add ix, ix
            add iy, bc
            add iy, de
            add iy, iy
            ld ix, (040000h)
            ld iy, (040003h)
            ld (040006h), ix
            ld (040009h), iy
        "#;
    let bytes = assemble_ez80_subset_at(asm, EZRA_LOAD_ADDR.get()).unwrap();

    assert_eq!(
        bytes,
        [
            0xDD, 0x23, 0xFD, 0x23, 0xDD, 0x2B, 0xFD, 0x2B, 0xDD, 0x09, 0xDD, 0x19, 0xDD, 0x29,
            0xFD, 0x09, 0xFD, 0x19, 0xFD, 0x29, 0xDD, 0x2A, 0x00, 0x00, 0x04, 0xFD, 0x2A, 0x03,
            0x00, 0x04, 0xDD, 0x22, 0x06, 0x00, 0x04, 0xFD, 0x22, 0x09, 0x00, 0x04,
        ]
    );
}

#[test]
fn assembles_ix_iy_byte_alias_forms() {
    let asm = r#"
            ld ixh, 12h
            ld ixl, a
            ld b, ixh
            ld ixh, ixl
            inc ixh
            dec ixl
            add a, ixh
            xor ixl
            ld iyh, 34h
            ld iyl, a
            ld c, iyh
            ld iyh, iyl
            inc iyh
            dec iyl
            adc a, iyh
            cp iyl
        "#;
    let bytes = assemble_ez80_subset_at(asm, EZRA_LOAD_ADDR.get()).unwrap();

    assert_eq!(
        bytes,
        [
            0xDD, 0x26, 0x12, 0xDD, 0x6F, 0xDD, 0x44, 0xDD, 0x65, 0xDD, 0x24, 0xDD, 0x2D, 0xDD,
            0x84, 0xDD, 0xAD, 0xFD, 0x26, 0x34, 0xFD, 0x6F, 0xFD, 0x4C, 0xFD, 0x65, 0xFD, 0x24,
            0xFD, 0x2D, 0xFD, 0x8C, 0xFD, 0xBD,
        ]
    );
}

#[test]
fn assembles_full_in0_out0_register_forms() {
    let asm = r#"
            in0 b, (12h)
            in0 c, (12h)
            in0 d, (12h)
            in0 e, (12h)
            in0 h, (12h)
            in0 l, (12h)
            in0 a, (12h)
            out0 (34h), b
            out0 (34h), c
            out0 (34h), d
            out0 (34h), e
            out0 (34h), h
            out0 (34h), l
            out0 (34h), a
        "#;
    let bytes = assemble_ez80_subset_at(asm, EZRA_LOAD_ADDR.get()).unwrap();

    assert_eq!(
        bytes,
        [
            0xED, 0x00, 0x12, 0xED, 0x08, 0x12, 0xED, 0x10, 0x12, 0xED, 0x18, 0x12, 0xED, 0x20,
            0x12, 0xED, 0x28, 0x12, 0xED, 0x38, 0x12, 0xED, 0x01, 0x34, 0xED, 0x09, 0x34, 0xED,
            0x11, 0x34, 0xED, 0x19, 0x34, 0xED, 0x21, 0x34, 0xED, 0x29, 0x34, 0xED, 0x39, 0x34,
        ]
    );
}

#[test]
fn assembles_ez80_mode_suffix_prefix_forms() {
    let asm = r#"
            nop.sis
            ld.lis b, a
            xor.sil 55h
            out0.lil (0Ch), a
        "#;
    let bytes = assemble_ez80_subset_at(asm, EZRA_LOAD_ADDR.get()).unwrap();

    assert_eq!(
        bytes,
        [
            0x40, 0x00, 0x49, 0x47, 0x52, 0xEE, 0x55, 0x5B, 0xED, 0x39, 0x0C
        ]
    );
}

#[test]
fn assembles_mode_suffixed_relocatable_operands() {
    let asm = r#"
            ld.lil hl, target
            jp.lil target
        target:
            ret
        "#;
    let bytes = assemble_ez80_subset_at(asm, EZRA_LOAD_ADDR.get()).unwrap();

    assert_eq!(
        bytes,
        [
            0x5B, 0x21, 0x0A, 0x00, 0x01, 0x5B, 0xC3, 0x0A, 0x00, 0x01, 0xC9,
        ]
    );
}

#[test]
fn assembles_sp_direct24_loads_and_stores() {
    let asm = r#"
            ld sp, (040000h)
            ld (040003h), sp
        "#;
    let bytes = assemble_ez80_subset_at(asm, EZRA_LOAD_ADDR.get()).unwrap();

    assert_eq!(
        bytes,
        [0xED, 0x7B, 0x00, 0x00, 0x04, 0xED, 0x73, 0x03, 0x00, 0x04]
    );
}

#[test]
fn assembles_standard_io_instructions() {
    let asm = r#"
            in a, (12h)
            out (34h), a
            in b, (c)
            in c, (c)
            in d, (c)
            in e, (c)
            in h, (c)
            in l, (c)
            in a, (c)
            out (c), b
            out (c), c
            out (c), d
            out (c), e
            out (c), h
            out (c), l
            out (c), a
        "#;
    let bytes = assemble_ez80_subset_at(asm, EZRA_LOAD_ADDR.get()).unwrap();

    assert_eq!(
        bytes,
        [
            0xDB, 0x12, 0xD3, 0x34, 0xED, 0x40, 0xED, 0x48, 0xED, 0x50, 0xED, 0x58, 0xED, 0x60,
            0xED, 0x68, 0xED, 0x78, 0xED, 0x41, 0xED, 0x49, 0xED, 0x51, 0xED, 0x59, 0xED, 0x61,
            0xED, 0x69, 0xED, 0x79,
        ]
    );
}

#[test]
fn assembles_arithmetic_shift_right_accumulator() {
    let bytes = assemble_ez80_subset_at("sra a\nret\n", EZRA_LOAD_ADDR.get()).unwrap();

    assert_eq!(bytes, [0xCB, 0x2F, 0xC9]);
}

#[test]
fn assembles_8_bit_register_loads() {
    let asm = r#"
            ld b, 12h
            ld c, 34h
            ld d, 56h
            ld e, 78h
            ld h, 9Ah
            ld l, 0BCh
            ld a, 0DEh
            ld e, a
            ld a, e
            ld l, b
        "#;
    let bytes = assemble_ez80_subset_at(asm, EZRA_LOAD_ADDR.get()).unwrap();

    assert_eq!(
        bytes,
        [
            0x06, 0x12, 0x0E, 0x34, 0x16, 0x56, 0x1E, 0x78, 0x26, 0x9A, 0x2E, 0xBC, 0x3E, 0xDE,
            0x5F, 0x7B, 0x68,
        ]
    );
}

#[test]
fn assembles_bc_de_indirect_accumulator_loads_and_stores() {
    let asm = r#"
            ld a, (bc)
            ld (bc), a
            ld a, (de)
            ld (de), a
        "#;
    let bytes = assemble_ez80_subset_at(asm, EZRA_LOAD_ADDR.get()).unwrap();

    assert_eq!(bytes, [0x0A, 0x02, 0x1A, 0x12]);
}

#[test]
fn assembles_bc_de_direct_memory_loads_and_stores() {
    let asm = r#"
            ld bc, (040100h)
            ld de, (040103h)
            ld (040106h), bc
            ld (040109h), de
        "#;
    let bytes = assemble_ez80_subset_at(asm, EZRA_LOAD_ADDR.get()).unwrap();

    assert_eq!(
        bytes,
        [
            0xED, 0x4B, 0x00, 0x01, 0x04, 0xED, 0x5B, 0x03, 0x01, 0x04, 0xED, 0x43, 0x06, 0x01,
            0x04, 0xED, 0x53, 0x09, 0x01, 0x04,
        ]
    );
}

#[test]
fn assembles_all_direct24_loads_and_stores() {
    let asm = r#"
            ld a, (ix_buffer)
            ld hl, (iy_buffer)
            ld bc, (040006h)
            ld de, (040009h)
            ld ix, (04000Ch)
            ld iy, (04000Fh)
            ld (ix_buffer), a
            ld (iy_buffer), hl
            ld (040018h), bc
            ld (04001Bh), de
            ld (04001Eh), ix
            ld (040021h), iy
        ix_buffer:
            nop
        iy_buffer:
            nop
        "#;
    let bytes = assemble_ez80_subset_at(asm, 0x040000).unwrap();

    assert_eq!(
        bytes,
        [
            0x3A, 0x38, 0x00, 0x04, 0x2A, 0x39, 0x00, 0x04, 0xED, 0x4B, 0x06, 0x00, 0x04, 0xED,
            0x5B, 0x09, 0x00, 0x04, 0xDD, 0x2A, 0x0C, 0x00, 0x04, 0xFD, 0x2A, 0x0F, 0x00, 0x04,
            0x32, 0x38, 0x00, 0x04, 0x22, 0x39, 0x00, 0x04, 0xED, 0x43, 0x18, 0x00, 0x04, 0xED,
            0x53, 0x1B, 0x00, 0x04, 0xDD, 0x22, 0x1E, 0x00, 0x04, 0xFD, 0x22, 0x21, 0x00, 0x04,
            0x00, 0x00,
        ]
    );
}

#[test]
fn direct24_labels_starting_with_index_register_names_are_not_index_indirect() {
    let asm = r#"
            ld a, (ix_label)
            ld hl, (iy_label)
            ld (ix_label), a
            ld (iy_label), hl
        ix_label:
            nop
        iy_label:
            nop
        "#;
    let bytes = assemble_ez80_subset_at(asm, 0x040000).unwrap();

    assert_eq!(
        bytes,
        [
            0x3A, 0x10, 0x00, 0x04, 0x2A, 0x11, 0x00, 0x04, 0x32, 0x10, 0x00, 0x04, 0x22, 0x11,
            0x00, 0x04, 0x00, 0x00,
        ]
    );
}

#[test]
fn assembles_hl_indirect_8_bit_loads_and_stores() {
    let asm = r#"
            ld b, (hl)
            ld c, (hl)
            ld d, (hl)
            ld e, (hl)
            ld h, (hl)
            ld l, (hl)
            ld a, (hl)
            ld (hl), b
            ld (hl), c
            ld (hl), d
            ld (hl), e
            ld (hl), h
            ld (hl), l
            ld (hl), a
        "#;
    let bytes = assemble_ez80_subset_at(asm, EZRA_LOAD_ADDR.get()).unwrap();

    assert_eq!(
        bytes,
        [
            0x46, 0x4E, 0x56, 0x5E, 0x66, 0x6E, 0x7E, 0x70, 0x71, 0x72, 0x73, 0x74, 0x75, 0x77,
        ]
    );
}

#[test]
fn assembles_hl_indirect_immediate_store() {
    let bytes = assemble_ez80_subset_at("ld (hl), 43h\n", EZRA_LOAD_ADDR.get()).unwrap();

    assert_eq!(bytes, [0x36, 0x43]);
}

#[test]
fn assembles_8_bit_register_inc_and_dec() {
    let asm = r#"
            inc b
            inc c
            inc d
            inc e
            inc h
            inc l
            inc a
            dec b
            dec c
            dec d
            dec e
            dec h
            dec l
            dec a
        "#;
    let bytes = assemble_ez80_subset_at(asm, EZRA_LOAD_ADDR.get()).unwrap();

    assert_eq!(
        bytes,
        [
            0x04, 0x0C, 0x14, 0x1C, 0x24, 0x2C, 0x3C, 0x05, 0x0D, 0x15, 0x1D, 0x25, 0x2D, 0x3D,
        ]
    );
}

#[test]
fn assembles_8_bit_accumulator_alu_register_forms() {
    let asm = r#"
            add a, b
            add a, e
            adc a, c
            adc a, h
            sub d
            sub l
            sbc a, b
            sbc a, e
            and h
            or e
            xor l
            cp d
            cp a
        "#;
    let bytes = assemble_ez80_subset_at(asm, EZRA_LOAD_ADDR.get()).unwrap();

    assert_eq!(
        bytes,
        [
            0x80, 0x83, 0x89, 0x8C, 0x92, 0x95, 0x98, 0x9B, 0xA4, 0xB3, 0xAD, 0xBA, 0xBF,
        ]
    );
}

#[test]
fn assembles_ez80_mlt_register_forms() {
    let bytes = assemble_ez80_subset_at(
        r#"
            mlt bc
            mlt de
            mlt hl
            mlt sp
            "#,
        EZRA_LOAD_ADDR.get(),
    )
    .unwrap();

    assert_eq!(bytes, [0xED, 0x4C, 0xED, 0x5C, 0xED, 0x6C, 0xED, 0x7C]);
}

#[test]
fn assembles_ez80_block_transfer_instructions() {
    let bytes = assemble_ez80_subset_at(
        r#"
            ldi
            ldir
            ldd
            lddr
            "#,
        EZRA_LOAD_ADDR.get(),
    )
    .unwrap();

    assert_eq!(bytes, [0xED, 0xA0, 0xED, 0xB0, 0xED, 0xA8, 0xED, 0xB8]);
}

#[test]
fn assembles_ez80_block_compare_instructions() {
    let bytes = assemble_ez80_subset_at(
        r#"
            cpi
            cpir
            cpd
            cpdr
            "#,
        EZRA_LOAD_ADDR.get(),
    )
    .unwrap();

    assert_eq!(bytes, [0xED, 0xA1, 0xED, 0xB1, 0xED, 0xA9, 0xED, 0xB9]);
}

#[test]
fn assembles_ez80_block_io_instructions() {
    let bytes = assemble_ez80_subset_at(
        r#"
            ini
            inir
            ind
            indr
            outi
            otir
            outd
            otdr
            "#,
        EZRA_LOAD_ADDR.get(),
    )
    .unwrap();

    assert_eq!(
        bytes,
        [
            0xED, 0xA2, 0xED, 0xB2, 0xED, 0xAA, 0xED, 0xBA, 0xED, 0xA3, 0xED, 0xB3, 0xED, 0xAB,
            0xED, 0xBB,
        ]
    );
}

#[test]
fn assembles_8_bit_accumulator_alu_immediate_forms() {
    let asm = r#"
            add a, 01h
            adc a, 02h
            sub 02h
            sbc a, 03h
            and 03h
            xor 04h
            or 05h
            cp 06h
        "#;
    let bytes = assemble_ez80_subset_at(asm, EZRA_LOAD_ADDR.get()).unwrap();

    assert_eq!(
        bytes,
        [
            0xC6, 0x01, 0xCE, 0x02, 0xD6, 0x02, 0xDE, 0x03, 0xE6, 0x03, 0xEE, 0x04, 0xF6, 0x05,
            0xFE, 0x06,
        ]
    );
}

#[test]
fn assembles_misc_accumulator_alu_instructions() {
    let bytes = assemble_ez80_subset_at("scf\nccf\ncpl\ndaa\nneg\n", EZRA_LOAD_ADDR.get()).unwrap();

    assert_eq!(bytes, [0x37, 0x3F, 0x2F, 0x27, 0xED, 0x44]);
}

#[test]
fn assembles_accumulator_rotate_shorthands() {
    let bytes = assemble_ez80_subset_at("rlca\nrla\nrrca\nrra\n", EZRA_LOAD_ADDR.get()).unwrap();

    assert_eq!(bytes, [0x07, 0x17, 0x0F, 0x1F]);
}

#[test]
fn assembles_bit_register_instructions() {
    let asm = "bit 0, b\nbit 1, c\nbit 2, d\nbit 3, e\nbit 4, h\nbit 5, l\nbit 7, a\nres 0, b\nset 7, a\n";
    let bytes = assemble_ez80_subset_at(asm, EZRA_LOAD_ADDR.get()).unwrap();

    assert_eq!(
        bytes,
        [
            0xCB, 0x40, 0xCB, 0x49, 0xCB, 0x52, 0xCB, 0x5B, 0xCB, 0x64, 0xCB, 0x6D, 0xCB, 0x7F,
            0xCB, 0x80, 0xCB, 0xFF,
        ]
    );
}

#[test]
fn assembles_relative_jumps() {
    let asm = r#"
            jr next
            ret
        next:
            jr z, done
            jr nz, done
            jr c, done
            jr nc, done
        done:
            jr next
        "#;
    let bytes = assemble_ez80_subset_at(asm, EZRA_LOAD_ADDR.get()).unwrap();

    assert_eq!(
        bytes,
        [
            0x18, 0x01, 0xC9, 0x28, 0x06, 0x20, 0x04, 0x38, 0x02, 0x30, 0x00, 0x18, 0xF6,
        ]
    );
}

#[test]
fn assembles_current_address_jumps() {
    let asm = r#"
            jp $
            jr $
        "#;
    let bytes = assemble_ez80_subset_at(asm, EZRA_LOAD_ADDR.get()).unwrap();

    assert_eq!(bytes, [0xC3, 0x00, 0x00, 0x01, 0x18, 0xFE]);
}

#[test]
fn assembles_all_absolute_conditional_jumps() {
    let asm = r#"
            jp nz, target
            jp z, target
            jp nc, target
            jp c, target
            jp po, target
            jp pe, target
            jp p, target
            jp m, target
        target:
            ret
        "#;
    let bytes = assemble_ez80_subset_at(asm, EZRA_LOAD_ADDR.get()).unwrap();

    assert_eq!(
        bytes,
        [
            0xC2, 0x20, 0x00, 0x01, 0xCA, 0x20, 0x00, 0x01, 0xD2, 0x20, 0x00, 0x01, 0xDA, 0x20,
            0x00, 0x01, 0xE2, 0x20, 0x00, 0x01, 0xEA, 0x20, 0x00, 0x01, 0xF2, 0x20, 0x00, 0x01,
            0xFA, 0x20, 0x00, 0x01, 0xC9,
        ]
    );
}

#[test]
fn assembles_all_conditional_call_instructions() {
    let asm = r#"
            call nz, target
            call z, target
            call nc, target
            call c, target
            call po, target
            call pe, target
            call p, target
            call m, target
        target:
            ret
        "#;
    let bytes = assemble_ez80_subset_at(asm, EZRA_LOAD_ADDR.get()).unwrap();

    assert_eq!(
        bytes,
        [
            0xC4, 0x20, 0x00, 0x01, 0xCC, 0x20, 0x00, 0x01, 0xD4, 0x20, 0x00, 0x01, 0xDC, 0x20,
            0x00, 0x01, 0xE4, 0x20, 0x00, 0x01, 0xEC, 0x20, 0x00, 0x01, 0xF4, 0x20, 0x00, 0x01,
            0xFC, 0x20, 0x00, 0x01, 0xC9,
        ]
    );
}

#[test]
fn rejects_duplicate_assembly_labels() {
    let asm = r#"
        again:
            jp again
        again:
            ret
        "#;
    let error = assemble_ez80_subset_at(asm, EZRA_LOAD_ADDR.get()).unwrap_err();

    assert_eq!(error.message, "duplicate assembly label `again`");
}

#[test]
fn rejects_unknown_assembly_labels() {
    let error = assemble_ez80_subset_at("jp missing_label\n", EZRA_LOAD_ADDR.get()).unwrap_err();

    assert_eq!(error.message, "unknown assembly label `missing_label`");
}

#[test]
fn assembles_equates_data_strings_and_org_padding() {
    let bytes = assemble_ez80_subset_at(
        r#"
            VALUE equ 41h
            db VALUE, "BC"
            dw VALUE + 1
            org 010008h
            db 44h
            "#,
        0x010000,
    )
    .unwrap();

    assert_eq!(
        bytes,
        [0x41, 0x42, 0x43, 0x42, 0x00, 0x00, 0x00, 0x00, 0x44]
    );
}

#[test]
fn assembles_address_expressions_in_instructions() {
    let bytes = assemble_ez80_subset_at(
        r#"
            TARGET = 010010h
            jp TARGET + 2
            "#,
        0x010000,
    )
    .unwrap();

    assert_eq!(bytes, [0xC3, 0x12, 0x00, 0x01]);
}

#[test]
fn z80n_and_z180_inherit_z80_instruction_encoding() {
    for cpu in [AssemblerCpu::Z80N, AssemblerCpu::Z180] {
        let assembled = assemble_subset_with_symbols_at(cpu, "ld a, 7Fh\nret\n", 0x0100).unwrap();

        assert_eq!(assembled.bytes, [0x3E, 0x7F, 0xC9]);
    }
}

#[test]
fn z180_and_ez80_accept_z180_lineage_instructions() {
    for cpu in [AssemblerCpu::Z180, AssemblerCpu::Ez80] {
        let assembled =
            assemble_subset_with_symbols_at(cpu, "mlt bc\nout0 (34h), a\n", 0x0100).unwrap();

        assert_eq!(assembled.bytes, [0xED, 0x4C, 0xED, 0x39, 0x34]);
    }
}

#[test]
fn z80_and_z80n_reject_z180_ez80_only_instructions() {
    for cpu in [AssemblerCpu::Z80, AssemblerCpu::Z80N] {
        let error = assemble_subset_with_symbols_at(cpu, "mlt bc\n", 0x0100).unwrap_err();

        assert_eq!(
            error.message,
            "test assembler does not support instruction `mlt bc`"
        );
    }
}

#[test]
fn i8080_and_i8085_accept_intel_8080_mnemonics() {
    for cpu in [AssemblerCpu::I8080, AssemblerCpu::I8085] {
        let assembled = assemble_subset_with_symbols_at(
            cpu,
            r#"
                lxi h, 1234h
                mvi a, 42h
                mov m, a
                inr m
                dad h
                xchg
                xthl
                sphl
                pchl
                start:
                jnz start
                call start
                ret
                "#,
            0x0100,
        )
        .unwrap();

        assert_eq!(
            assembled.bytes,
            [
                0x21, 0x34, 0x12, 0x3E, 0x42, 0x77, 0x34, 0x29, 0xEB, 0xE3, 0xF9, 0xE9, 0xC2, 0x0C,
                0x01, 0xCD, 0x0C, 0x01, 0xC9,
            ]
        );
    }
}

#[test]
fn i8085_accepts_rim_sim_but_i8080_rejects_them() {
    let assembled =
        assemble_subset_with_symbols_at(AssemblerCpu::I8085, "rim\nsim\n", 0x0100).unwrap();
    assert_eq!(assembled.bytes, [0x20, 0x30]);

    let error = assemble_subset_with_symbols_at(AssemblerCpu::I8080, "rim\n", 0x0100).unwrap_err();
    assert_eq!(
        error.message,
        "test assembler does not support instruction `rim`"
    );
}

#[test]
fn i8080_rejects_z80_extension_syntax() {
    let error =
        assemble_subset_with_symbols_at(AssemblerCpu::I8080, "ld a, 7Fh\n", 0x0100).unwrap_err();

    assert_eq!(
        error.message,
        "test assembler does not support instruction `ld a, 7Fh`"
    );
}

#[test]
fn rejects_invalid_numeric_jump_operands() {
    let error = assemble_ez80_subset_at("jp 0xBADHEX\n", EZRA_LOAD_ADDR.get()).unwrap_err();

    assert_eq!(error.message, "invalid numeric operand `0xBADHEX`");
}

#[test]
fn rejects_address_operands_outside_address_space() {
    let error = assemble_ez80_subset_at("jp 0x1000000\n", EZRA_LOAD_ADDR.get()).unwrap_err();

    assert_eq!(
        error.message,
        "address operand `0x1000000` is outside the 24-bit address space"
    );
}

#[test]
fn rejects_invalid_restart_targets() {
    for (asm, expected) in [
        (
            "rst 07h\n",
            "restart target 0x7 is not one of 0x00, 0x08, ..., 0x38",
        ),
        (
            "rst 40h\n",
            "restart target 0x40 is not one of 0x00, 0x08, ..., 0x38",
        ),
    ] {
        let error = assemble_ez80_subset_at(asm, EZRA_LOAD_ADDR.get()).unwrap_err();

        assert_eq!(error.message, expected);
    }
}

#[test]
fn rejects_invalid_bit_register_operands() {
    for (asm, expected) in [
        ("bit 8, a\n", "bit index 8 is outside 0..7"),
        ("bit 0, ix\n", "invalid bit register `ix`"),
        ("set 8, a\n", "bit index 8 is outside 0..7"),
        ("res 0, ix\n", "invalid bit register `ix`"),
    ] {
        let error = assemble_ez80_subset_at(asm, EZRA_LOAD_ADDR.get()).unwrap_err();

        assert_eq!(error.message, expected);
    }
}

#[test]
fn rejects_relative_jumps_outside_signed_byte_range() {
    let padding = "ret\n".repeat(128);
    let asm = format!("jr far\n{padding}far:\nret\n");
    let error = assemble_ez80_subset_at(&asm, EZRA_LOAD_ADDR.get()).unwrap_err();

    assert_eq!(
        error.message,
        "relative jump target 0x010082 is out of range from 0x010000"
    );
}

#[test]
fn assembles_djnz_relative_loop() {
    let asm = r#"
            ld a, 03h
            ld b, a
        loop:
            djnz loop
            ret
        "#;
    let bytes = assemble_ez80_subset_at(asm, EZRA_LOAD_ADDR.get()).unwrap();

    assert_eq!(bytes, [0x3E, 0x03, 0x47, 0x10, 0xFE, 0xC9]);
}
#[test]
fn assembles_all_conditional_return_instructions() {
    let asm = "ret nz\nret z\nret nc\nret c\nret po\nret pe\nret p\nret m\n";
    let bytes = assemble_ez80_subset_at(asm, EZRA_LOAD_ADDR.get()).unwrap();

    assert_eq!(bytes, [0xC0, 0xC8, 0xD0, 0xD8, 0xE0, 0xE8, 0xF0, 0xF8]);
}

#[test]
fn rejects_assembly_base_outside_address_space() {
    let error = assemble_ez80_subset_at("ret\n", 0x01_000000).unwrap_err();

    assert_eq!(
        error.message,
        "assembly base address 0x1000000 is outside the 24-bit address space"
    );
}

#[test]
fn rejects_assembly_that_exceeds_address_space() {
    let error = assemble_ez80_subset_at("nop\nnop\n", 0xFF_FFFF).unwrap_err();

    assert_eq!(
        error.message,
        "assembly instruction at 0x1000000 with length 0x1 exceeds the 24-bit address space"
    );
}

#[test]
fn rejects_assembly_labels_outside_address_space() {
    let error = assemble_ez80_subset_at("nop\nend:\n", 0xFF_FFFF).unwrap_err();

    assert_eq!(
        error.message,
        "assembly label `end` address 0x1000000 is outside the 24-bit address space"
    );
}

#[test]
fn mos6502_assembler_encodes_common_addressing_modes() {
    let bytes = assemble_subset_with_symbols_at(
        AssemblerCpu::Mos6502,
        "start:\nlda #01h\nsta $0200\nldx #$05\nloop:\ndex\nbne loop\njmp (1234h)\n",
        0xC000,
    )
    .unwrap();
    assert_eq!(
        bytes.bytes,
        [
            0xA9, 0x01, 0x8D, 0x00, 0x02, 0xA2, 0x05, 0xCA, 0xD0, 0xFD, 0x6C, 0x34, 0x12,
        ]
    );
}

#[test]
fn mos6502_is_parsed_as_own_assembler_cpu_family() {
    assert_eq!(AssemblerCpu::parse("6502").unwrap(), AssemblerCpu::Mos6502);
    assert_eq!(
        AssemblerCpu::parse("mos6502").unwrap(),
        AssemblerCpu::Mos6502
    );
    assert_eq!(
        parse_target_triple("c64-6502-bare").unwrap().cpu,
        CpuFamily::Mos6502
    );
}
