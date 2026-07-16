use super::*;
use crate::vm::assemble_subset_with_symbols_at;
use ez80::Machine;
use std::panic::{AssertUnwindSafe, catch_unwind};

fn assert_ez80_emulator_decodes(syntax: &str, bytes: &[u8]) {
    let result = catch_unwind(AssertUnwindSafe(|| {
        let mut machine = ez80::PlainMachine::new();
        let mut cpu = ez80::Cpu::new_ez80();
        for (offset, byte) in bytes.iter().copied().enumerate() {
            machine.poke(offset as u32, byte);
        }
        cpu.disasm_instruction(&mut machine)
    }));
    let disasm = result
        .unwrap_or_else(|_| panic!("emulator failed to decode `{syntax}` bytes {bytes:02X?}"));
    assert_ne!(
        disasm, "ILLEGAL",
        "emulator decoded `{syntax}` bytes {bytes:02X?} as illegal"
    );
}

#[test]
fn exact_instruction_metadata_encodes_common_ops() {
    assert_eq!(
        exact_instruction(AssemblerCpu::Ez80, "nop").unwrap().bytes,
        &[0x00]
    );
    assert_eq!(
        exact_instruction(AssemblerCpu::Ez80, "reti").unwrap().bytes,
        &[0xED, 0x4D]
    );
}

#[test]
fn metadata_can_generate_z80_subset() {
    let z80 = instruction_set(AssemblerCpu::Z80).collect::<Vec<_>>();
    assert!(z80.iter().any(|instruction| instruction.syntax == "ret"));
    assert!(z80.iter().any(|instruction| instruction.syntax == "im 2"));
}

#[test]
fn ez80_emulator_decodes_all_exact_instruction_metadata() {
    for instruction in instruction_set(AssemblerCpu::Ez80) {
        assert_ez80_emulator_decodes(instruction.syntax, instruction.bytes);
    }
}

#[test]
fn ez80_emulator_decodes_representative_generated_instruction_metadata() {
    let cases = [
        "ld b, a",
        "ld a, 7Fh",
        "inc c",
        "add a, c",
        "inc hl",
        "add hl, de",
        "srl a",
        "bit 3, (hl)",
        "in a, (34h)",
        "out0 (0Ch), a",
        "rst.lis 10h",
        "xor 55h",
        "ld d, (hl)",
        "ld (hl), 43h",
        "ld c, (ix+2)",
        "ld (iy-1), e",
        "ld (ix+4), 99h",
        "xor (iy+0)",
        "rlc (ix+1)",
        "rr (iy-2)",
        "bit 3, (ix+4)",
        "res 2, (iy+5)",
        "set 7, (ix-6)",
    ];

    for syntax in cases {
        let bytes = encode_generated_instruction(AssemblerCpu::Ez80, syntax)
            .unwrap()
            .unwrap_or_else(|| panic!("missing generated encoding for `{syntax}`"));
        assert_ez80_emulator_decodes(syntax, &bytes);
    }
}

#[test]
fn generated_instruction_metadata_encodes_operand_families() {
    assert_eq!(
        encode_generated_instruction(AssemblerCpu::Ez80, "ld b, a").unwrap(),
        Some(vec![0x47])
    );
    assert_eq!(
        encode_generated_instruction(AssemblerCpu::Ez80, "ld a, 7Fh").unwrap(),
        Some(vec![0x3E, 0x7F])
    );
    assert_eq!(
        encode_generated_instruction(AssemblerCpu::Ez80, "inc c").unwrap(),
        Some(vec![0x0C])
    );
    assert_eq!(
        encode_generated_instruction(AssemblerCpu::Ez80, "add a, c").unwrap(),
        Some(vec![0x81])
    );
    assert_eq!(
        encode_generated_instruction(AssemblerCpu::Ez80, "inc hl").unwrap(),
        Some(vec![0x23])
    );
    assert_eq!(
        encode_generated_instruction(AssemblerCpu::Ez80, "add hl, de").unwrap(),
        Some(vec![0x19])
    );
    assert_eq!(
        encode_generated_instruction(AssemblerCpu::Ez80, "srl a").unwrap(),
        Some(vec![0xCB, 0x3F])
    );
    assert_eq!(
        encode_generated_instruction(AssemblerCpu::Ez80, "bit 3, (hl)").unwrap(),
        Some(vec![0xCB, 0x5E])
    );
    assert_eq!(
        encode_generated_instruction(AssemblerCpu::Ez80, "in a, (34h)").unwrap(),
        Some(vec![0xDB, 0x34])
    );
    assert_eq!(
        encode_generated_instruction(AssemblerCpu::Ez80, "out0 (0Ch), a").unwrap(),
        Some(vec![0xED, 0x39, 0x0C])
    );
    assert_eq!(
        encode_generated_instruction(AssemblerCpu::Ez80, "rst.lis 10h").unwrap(),
        Some(vec![0x49, 0xD7])
    );
    assert_eq!(
        encode_generated_instruction(AssemblerCpu::Ez80, "xor 55h").unwrap(),
        Some(vec![0xEE, 0x55])
    );
    assert_eq!(
        encode_generated_instruction(AssemblerCpu::Ez80, "ld d, (hl)").unwrap(),
        Some(vec![0x56])
    );
    assert_eq!(
        encode_generated_instruction(AssemblerCpu::Ez80, "ld (hl), 43h").unwrap(),
        Some(vec![0x36, 0x43])
    );
    assert_eq!(
        encode_generated_instruction(AssemblerCpu::Ez80, "ld c, (ix+2)").unwrap(),
        Some(vec![0xDD, 0x4E, 0x02])
    );
    assert_eq!(
        encode_generated_instruction(AssemblerCpu::Ez80, "ld (iy-1), e").unwrap(),
        Some(vec![0xFD, 0x73, 0xFF])
    );
    assert_eq!(
        encode_generated_instruction(AssemblerCpu::Ez80, "ld (ix+4), 99h").unwrap(),
        Some(vec![0xDD, 0x36, 0x04, 0x99])
    );
    assert_eq!(
        encode_generated_instruction(AssemblerCpu::Ez80, "xor (iy+0)").unwrap(),
        Some(vec![0xFD, 0xAE, 0x00])
    );
    assert_eq!(
        encode_generated_instruction(AssemblerCpu::Ez80, "rlc (ix+1)").unwrap(),
        Some(vec![0xDD, 0xCB, 0x01, 0x06])
    );
    assert_eq!(
        encode_generated_instruction(AssemblerCpu::Ez80, "rr (iy-2)").unwrap(),
        Some(vec![0xFD, 0xCB, 0xFE, 0x1E])
    );
    assert_eq!(
        encode_generated_instruction(AssemblerCpu::Ez80, "bit 3, (ix+4)").unwrap(),
        Some(vec![0xDD, 0xCB, 0x04, 0x5E])
    );
    assert_eq!(
        encode_generated_instruction(AssemblerCpu::Ez80, "res 2, (iy+5)").unwrap(),
        Some(vec![0xFD, 0xCB, 0x05, 0x96])
    );
    assert_eq!(
        encode_generated_instruction(AssemblerCpu::Ez80, "set 7, (ix-6)").unwrap(),
        Some(vec![0xDD, 0xCB, 0xFA, 0xFE])
    );
}

#[test]
fn generated_instruction_metadata_encodes_ix_iy_byte_aliases() {
    let cases = [
        ("ld ixh, 12h", vec![0xDD, 0x26, 0x12]),
        ("ld ixl, a", vec![0xDD, 0x6F]),
        ("ld b, ixh", vec![0xDD, 0x44]),
        ("ld ixh, ixl", vec![0xDD, 0x65]),
        ("inc ixh", vec![0xDD, 0x24]),
        ("dec ixl", vec![0xDD, 0x2D]),
        ("add a, ixh", vec![0xDD, 0x84]),
        ("xor ixl", vec![0xDD, 0xAD]),
        ("ld iyh, 34h", vec![0xFD, 0x26, 0x34]),
        ("ld iyl, a", vec![0xFD, 0x6F]),
        ("ld c, iyh", vec![0xFD, 0x4C]),
        ("ld iyh, iyl", vec![0xFD, 0x65]),
        ("inc iyh", vec![0xFD, 0x24]),
        ("dec iyl", vec![0xFD, 0x2D]),
        ("adc a, iyh", vec![0xFD, 0x8C]),
        ("cp iyl", vec![0xFD, 0xBD]),
    ];

    for (syntax, bytes) in cases {
        assert_eq!(
            encode_generated_instruction(AssemblerCpu::Ez80, syntax).unwrap(),
            Some(bytes.clone()),
            "{syntax}"
        );
        assert_ez80_emulator_decodes(syntax, &bytes);
    }
}

#[test]
fn indexed_displacements_cover_the_full_signed_byte_range() {
    assert_eq!(
        encode_generated_instruction(AssemblerCpu::Ez80, "ld a, (ix-128)").unwrap(),
        Some(vec![0xDD, 0x7E, 0x80])
    );
    assert_eq!(
        encode_generated_instruction(AssemblerCpu::Ez80, "ld (iy+127), a").unwrap(),
        Some(vec![0xFD, 0x77, 0x7F])
    );
    for syntax in ["ld a, (ix+128)", "ld a, (iy-129)"] {
        let error = encode_generated_instruction(AssemblerCpu::Ez80, syntax).unwrap_err();
        assert!(
            error.message.contains("outside signed 8-bit range"),
            "{error}"
        );
    }
}

#[test]
fn rejects_misleading_ix_iy_byte_alias_mixes() {
    let cases = ["ld ixh, iyh", "ld h, ixh", "ld iyl, l"];

    for syntax in cases {
        let error = encode_generated_instruction(AssemblerCpu::Ez80, syntax).unwrap_err();
        assert!(
            error.message.contains("cannot mix"),
            "{syntax}: {}",
            error.message
        );
    }
}

#[test]
fn generated_instruction_metadata_encodes_all_in0_out0_register_forms() {
    let cases = [
        ("in0 b, (12h)", vec![0xED, 0x00, 0x12]),
        ("in0 c, (12h)", vec![0xED, 0x08, 0x12]),
        ("in0 d, (12h)", vec![0xED, 0x10, 0x12]),
        ("in0 e, (12h)", vec![0xED, 0x18, 0x12]),
        ("in0 h, (12h)", vec![0xED, 0x20, 0x12]),
        ("in0 l, (12h)", vec![0xED, 0x28, 0x12]),
        ("in0 a, (12h)", vec![0xED, 0x38, 0x12]),
        ("out0 (34h), b", vec![0xED, 0x01, 0x34]),
        ("out0 (34h), c", vec![0xED, 0x09, 0x34]),
        ("out0 (34h), d", vec![0xED, 0x11, 0x34]),
        ("out0 (34h), e", vec![0xED, 0x19, 0x34]),
        ("out0 (34h), h", vec![0xED, 0x21, 0x34]),
        ("out0 (34h), l", vec![0xED, 0x29, 0x34]),
        ("out0 (34h), a", vec![0xED, 0x39, 0x34]),
    ];

    for (syntax, bytes) in cases {
        assert_eq!(
            encode_generated_instruction(AssemblerCpu::Ez80, syntax).unwrap(),
            Some(bytes.clone()),
            "{syntax}"
        );
        assert_ez80_emulator_decodes(syntax, &bytes);
    }
}

#[test]
fn generated_instruction_metadata_encodes_z80n_extensions() {
    let cases = [
        ("swapnib", vec![0xED, 0x23]),
        ("mirror a", vec![0xED, 0x24]),
        ("test 7Fh", vec![0xED, 0x27, 0x7F]),
        ("bsla de,b", vec![0xED, 0x28]),
        ("bsra de,b", vec![0xED, 0x29]),
        ("bsrl de,b", vec![0xED, 0x2A]),
        ("bsrf de,b", vec![0xED, 0x2B]),
        ("brlc de,b", vec![0xED, 0x2C]),
        ("mul d,e", vec![0xED, 0x30]),
        ("add hl,a", vec![0xED, 0x31]),
        ("add de,a", vec![0xED, 0x32]),
        ("add bc,a", vec![0xED, 0x33]),
        ("add hl,1234h", vec![0xED, 0x34, 0x34, 0x12]),
        ("add de,2345h", vec![0xED, 0x35, 0x45, 0x23]),
        ("add bc,3456h", vec![0xED, 0x36, 0x56, 0x34]),
        ("push 1234h", vec![0xED, 0x8A, 0x12, 0x34]),
        ("outinb", vec![0xED, 0x90]),
        ("nextreg 12h,34h", vec![0xED, 0x91, 0x12, 0x34]),
        ("nextreg 12h,a", vec![0xED, 0x92, 0x12]),
        ("pixeldn", vec![0xED, 0x93]),
        ("pixelad", vec![0xED, 0x94]),
        ("setae", vec![0xED, 0x95]),
        ("jp (c)", vec![0xED, 0x98]),
        ("ldix", vec![0xED, 0xA4]),
        ("ldws", vec![0xED, 0xA5]),
        ("lddx", vec![0xED, 0xAC]),
        ("ldirx", vec![0xED, 0xB4]),
        ("ldpirx", vec![0xED, 0xB7]),
        ("lddrx", vec![0xED, 0xBC]),
    ];

    for (syntax, bytes) in cases {
        assert_eq!(
            encode_generated_instruction(AssemblerCpu::Z80N, syntax).unwrap(),
            Some(bytes),
            "{syntax}"
        );
        assert_eq!(
            encode_generated_instruction(AssemblerCpu::Z80, syntax).unwrap(),
            None,
            "{syntax}"
        );
        assert_eq!(
            encode_generated_instruction(AssemblerCpu::Z180, syntax).unwrap(),
            None,
            "{syntax}"
        );
    }
}

#[test]
fn generated_instruction_metadata_encodes_z180_extensions() {
    let cases = [
        ("slp", vec![0xED, 0x76], true),
        ("mlt bc", vec![0xED, 0x4C], true),
        ("otim", vec![0xED, 0x83], false),
        ("otimr", vec![0xED, 0x93], false),
        ("otdm", vec![0xED, 0x8B], false),
        ("otdmr", vec![0xED, 0x9B], false),
        ("tst b", vec![0xED, 0x04], false),
        ("tst c", vec![0xED, 0x0C], false),
        ("tst (hl)", vec![0xED, 0x34], false),
        ("tst a", vec![0xED, 0x3C], false),
        ("tst 5Ah", vec![0xED, 0x64, 0x5A], false),
        ("tstio 80h", vec![0xED, 0x74, 0x80], false),
    ];

    for (syntax, bytes, also_ez80) in cases {
        assert_eq!(
            encode_generated_instruction(AssemblerCpu::Z180, syntax).unwrap(),
            Some(bytes.clone()),
            "{syntax}"
        );
        assert_eq!(
            encode_generated_instruction(AssemblerCpu::Z80, syntax).unwrap(),
            None,
            "{syntax}"
        );
        assert_eq!(
            encode_generated_instruction(AssemblerCpu::Z80N, syntax).unwrap(),
            None,
            "{syntax}"
        );
        assert_eq!(
            encode_generated_instruction(AssemblerCpu::Ez80, syntax).unwrap(),
            also_ez80.then_some(bytes),
            "{syntax}"
        );
    }
}

#[test]
fn direct24_metadata_encodes_sp_loads_and_stores() {
    assert_eq!(
        direct24_instruction(AssemblerCpu::Ez80, "ld sp, (040000h)").unwrap(),
        Direct24Instruction {
            prefix: &[0xED, 0x7B],
            addr: "040000h",
        }
    );
    assert_eq!(
        direct24_instruction(AssemblerCpu::Ez80, "ld (040003h), sp").unwrap(),
        Direct24Instruction {
            prefix: &[0xED, 0x73],
            addr: "040003h",
        }
    );
    assert_ez80_emulator_decodes("ld sp, (040000h)", &[0xED, 0x7B, 0x00, 0x00, 0x04]);
    assert_ez80_emulator_decodes("ld (040003h), sp", &[0xED, 0x73, 0x03, 0x00, 0x04]);
}

#[test]
fn generated_instruction_metadata_encodes_ez80_mode_suffixes() {
    let cases = [
        ("nop.sis", vec![0x40, 0x00]),
        ("ld.lis b, a", vec![0x49, 0x47]),
        ("xor.sil 55h", vec![0x52, 0xEE, 0x55]),
        ("out0.lil (0Ch), a", vec![0x5B, 0xED, 0x39, 0x0C]),
        ("ld.lil b, (ix+2)", vec![0x5B, 0xDD, 0x46, 0x02]),
        ("res.sil 3, (iy-1)", vec![0x52, 0xFD, 0xCB, 0xFF, 0x9E]),
        ("in.lis b, (c)", vec![0x49, 0xED, 0x40]),
    ];

    for (syntax, bytes) in cases {
        assert_eq!(
            encode_generated_instruction(AssemblerCpu::Ez80, syntax).unwrap(),
            Some(bytes.clone()),
            "{syntax}"
        );
        assert_ez80_emulator_decodes(syntax, &bytes);
        assert_eq!(
            generated_instruction_len(AssemblerCpu::Ez80, syntax).unwrap(),
            Some(bytes.len()),
            "{syntax}"
        );
        assert_eq!(
            encode_generated_instruction(AssemblerCpu::Z80, syntax).unwrap(),
            None,
            "{syntax}"
        );
    }
}

#[test]
fn schur_agon_examples_accept_spasm_ng_long_mode_shorthand() {
    // `hello_world.asm`, `extest.asm`, and `stacktest.asm` in
    // schur/Agon-Light-Assembly use these forms.
    let encoded_cases = [
        ("ex.l de, hl", vec![0x5B, 0xEB]),
        ("push.lis de", vec![0x49, 0xD5]),
        ("pop.lis de", vec![0x49, 0xD1]),
        ("ld.sis sp, ix", vec![0x40, 0xDD, 0xF9]),
        ("rst.lil 10h", vec![0x5B, 0xD7]),
    ];

    for (syntax, bytes) in encoded_cases {
        assert_eq!(
            encode_generated_instruction(AssemblerCpu::Ez80, syntax).unwrap(),
            Some(bytes.clone()),
            "{syntax}"
        );
        assert_eq!(
            generated_instruction_len(AssemblerCpu::Ez80, syntax).unwrap(),
            Some(bytes.len()),
            "{syntax}"
        );
    }

    // spasm-ng's `.L` is the shorthand used by the reference hello-world
    // example for a 24-bit immediate load.
    assert_eq!(
        generated_instruction_len(AssemblerCpu::Ez80, "ld.l hl, 6A9BF4h").unwrap(),
        Some(5)
    );
    assert_eq!(
        ez80_mode_suffixed_instruction(AssemblerCpu::Ez80, "ld.l hl, 6A9BF4h"),
        Some((0x5B, "ld hl, 6A9BF4h".to_owned()))
    );
    assert_eq!(
        ez80_mode_suffixed_instruction(AssemblerCpu::Z80, "ld.l hl, 6A9BF4h"),
        None
    );
}

#[test]
fn agon_ez80asm_style_instruction_case_and_whitespace_are_accepted() {
    let cases = [
        ("PUSH\tIX", vec![0xDD, 0xE5]),
        ("RST.LIL 08h", vec![0x5B, 0xCF]),
        ("LD\tA,\t05h", vec![0x3E, 0x05]),
        // Verified with AgonPlatform/agon-ez80asm's native assembler.
        ("LD HL, (IX+6)", vec![0xDD, 0x27, 0x06]),
        ("LD HL, (HL)", vec![0xED, 0x27]),
    ];

    for (syntax, bytes) in cases {
        assert_eq!(
            encode_generated_instruction(AssemblerCpu::Ez80, syntax).unwrap(),
            Some(bytes),
            "{syntax}"
        );
    }
}

#[test]
fn generated_instruction_metadata_encodes_both_ez80_lea_index_forms() {
    let cases = [
        ("lea hl, ix+2", vec![0xED, 0x22, 0x02]),
        ("lea hl, iy+2", vec![0xED, 0x23, 0x02]),
        ("lea hl, iy-128", vec![0xED, 0x23, 0x80]),
        ("lea hl, iy+127", vec![0xED, 0x23, 0x7F]),
    ];

    for (syntax, bytes) in cases {
        assert_eq!(
            encode_generated_instruction(AssemblerCpu::Ez80, syntax).unwrap(),
            Some(bytes.clone()),
            "{syntax}"
        );
        assert_ez80_emulator_decodes(syntax, &bytes);
    }
}

#[test]
fn branch_metadata_describes_control_flow_widths() {
    let call = branch_instruction(AssemblerCpu::Ez80, "call nz, _main").unwrap();
    assert_eq!(call.opcode, 0xC4);
    assert_eq!(call.target, "_main");
    assert_eq!(call.encoded_len(), 4);

    let jr = branch_instruction(AssemblerCpu::Ez80, "jr z, .done").unwrap();
    assert_eq!(jr.opcode, 0x28);
    assert_eq!(jr.target, ".done");
    assert_eq!(jr.encoded_len(), 2);
}

#[test]
fn intel_8080_8085_metadata_covers_io_branches_and_rim_sim_gating() {
    let intel_io = [("in 34h", vec![0xDB, 0x34]), ("out 56h", vec![0xD3, 0x56])];
    for cpu in [AssemblerCpu::I8080, AssemblerCpu::I8085] {
        for (syntax, bytes) in &intel_io {
            assert_eq!(
                encode_generated_instruction(cpu, syntax).unwrap(),
                Some(bytes.clone()),
                "{cpu:?}: {syntax}"
            );
            assert!(instruction_effects(syntax).uses_ports, "{syntax}");
        }
    }

    let branches = [
        ("jmp 1234h", 0xC3),
        ("jnz 1234h", 0xC2),
        ("jz 1234h", 0xCA),
        ("jnc 1234h", 0xD2),
        ("jc 1234h", 0xDA),
        ("jpo 1234h", 0xE2),
        ("jpe 1234h", 0xEA),
        ("jp 1234h", 0xF2),
        ("jm 1234h", 0xFA),
        ("call 1234h", 0xCD),
        ("cnz 1234h", 0xC4),
        ("cz 1234h", 0xCC),
        ("cnc 1234h", 0xD4),
        ("cc 1234h", 0xDC),
        ("cpo 1234h", 0xE4),
        ("cpe 1234h", 0xEC),
        ("cp 1234h", 0xF4),
        ("cm 1234h", 0xFC),
    ];
    for cpu in [AssemblerCpu::I8080, AssemblerCpu::I8085] {
        for (syntax, opcode) in branches {
            let branch = branch_instruction(cpu, syntax).unwrap();
            assert_eq!(branch.opcode, opcode, "{cpu:?}: {syntax}");
            assert_eq!(branch.width, BranchWidth::Absolute16, "{cpu:?}: {syntax}");
            assert_eq!(branch.encoded_len(), 3, "{cpu:?}: {syntax}");
            assert_eq!(
                coverage_bytes(cpu, syntax).unwrap(),
                Some(vec![opcode, 0x34, 0x12]),
                "{cpu:?}: {syntax}"
            );
        }
    }

    assert_eq!(
        encode_generated_instruction(AssemblerCpu::I8080, "rim").unwrap(),
        None
    );
    assert_eq!(
        encode_generated_instruction(AssemblerCpu::I8080, "sim").unwrap(),
        None
    );
    assert_eq!(
        encode_generated_instruction(AssemblerCpu::I8085, "rim").unwrap(),
        Some(vec![0x20])
    );
    assert_eq!(
        encode_generated_instruction(AssemblerCpu::I8085, "sim").unwrap(),
        Some(vec![0x30])
    );
}

#[test]
fn ez80_coverage_includes_suffixed_indexed_and_io_forms() {
    let coverage = instruction_coverage(AssemblerCpu::Ez80).unwrap();
    let expected = [
        ("in.sis b, (c)", vec![0x40, 0xED, 0x40]),
        ("out.lis (c), h", vec![0x49, 0xED, 0x61]),
        ("ld.lil b, (ix+2)", vec![0x5B, 0xDD, 0x46, 0x02]),
        ("ld.sil (iy-2), a", vec![0x52, 0xFD, 0x77, 0xFE]),
        ("set.lil 7, (ix-1)", vec![0x5B, 0xDD, 0xCB, 0xFF, 0xFE]),
    ];

    for (syntax, bytes) in expected {
        let row = coverage
            .iter()
            .find(|row| row.syntax == syntax)
            .unwrap_or_else(|| panic!("missing coverage for `{syntax}`"));
        assert_eq!(row.bytes, bytes, "{syntax}");
        assert!(row.vm_sizing_supported, "{syntax}");
        assert_ez80_emulator_decodes(syntax, &row.bytes);
    }
}

#[test]
fn instruction_analysis_unifies_encoding_and_codegen_effects() {
    let out = analyze_instruction(AssemblerCpu::Ez80, "out0 (0Ch), a").unwrap();
    assert_eq!(out.encoded_len, Some(3));
    assert!(out.effects.uses_ports);
    assert!(!out.effects.uses_memory);

    let block = analyze_instruction(AssemblerCpu::Ez80, "ldir").unwrap();
    assert_eq!(block.encoded_len, Some(2));
    assert!(block.effects.uses_memory);
    assert!(block.effects.changes_flags);
    assert_eq!(block.effects.modified_registers, ["bc", "de", "hl"]);

    let indexed = analyze_instruction(AssemblerCpu::Ez80, "ld a, (ix+2)").unwrap();
    assert_eq!(indexed.encoded_len, Some(3));
    assert_eq!(indexed.effects.referenced_special_registers, ["ix"]);
    assert_eq!(indexed.effects.modified_registers, ["a"]);

    let suffixed = analyze_instruction(AssemblerCpu::Ez80, "out0.lil (0Ch), a").unwrap();
    assert_eq!(suffixed.encoded_len, Some(4));
    assert!(suffixed.effects.uses_ports);

    let comment = instruction_effects("nop ; ix and out are only words in a comment");
    assert!(comment.referenced_special_registers.is_empty());
    assert!(!comment.uses_ports);
}

#[test]
fn machine_readable_coverage_agrees_with_encoding_and_vm_sizing() {
    for cpu in [
        AssemblerCpu::I8080,
        AssemblerCpu::I8085,
        AssemblerCpu::Z80,
        AssemblerCpu::Z80N,
        AssemblerCpu::Z180,
        AssemblerCpu::Ez80,
    ] {
        let coverage = instruction_coverage(cpu).unwrap();
        assert!(!coverage.is_empty(), "{cpu:?}");
        for row in coverage {
            assert!(row.vm_sizing_supported, "{cpu:?}: {}", row.syntax);
            assert_eq!(
                coverage_bytes(cpu, &row.syntax).unwrap(),
                Some(row.bytes.clone()),
                "{cpu:?}: {}",
                row.syntax
            );
            assert_eq!(row.effects, instruction_effects(&row.syntax));
        }
    }
}

#[test]
fn every_documented_cpu_family_form_assembles_through_the_standalone_path() {
    for cpu in [
        AssemblerCpu::I8080,
        AssemblerCpu::I8085,
        AssemblerCpu::Z80,
        AssemblerCpu::Z80N,
        AssemblerCpu::Z180,
        AssemblerCpu::Ez80,
    ] {
        for row in instruction_coverage(cpu).unwrap() {
            let source = format!("{}\n.done:\nnop\n", row.syntax);
            let assembled = assemble_subset_with_symbols_at(cpu, &source, 0x0100)
                .unwrap_or_else(|error| panic!("{cpu:?}: {}: {error}", row.syntax));
            assert_eq!(
                &assembled.bytes[..row.bytes.len()],
                row.bytes.as_slice(),
                "{cpu:?}: {}",
                row.syntax
            );
            let analysis = analyze_instruction(cpu, &row.syntax).unwrap();
            assert_eq!(
                analysis.encoded_len,
                Some(row.bytes.len()),
                "{cpu:?}: {}",
                row.syntax
            );
            assert_eq!(analysis.effects, row.effects, "{cpu:?}: {}", row.syntax);
        }
    }
}
