
use super::*;
use libre99_asm::{Options as Libre99AssemblerOptions, assemble as assemble_with_libre99};
use libre99_core::{
    bus::{Bus, FlatRam},
    cpu::Cpu,
};

#[test]
fn encodes_core_instruction_formats() {
    assert_eq!(
        encode_instruction("li r1, >1234", &HashMap::new(), 0).unwrap(),
        [0x02, 0x01, 0x12, 0x34]
    );
    assert_eq!(
        encode_instruction("mov r1, *r2+", &HashMap::new(), 0).unwrap(),
        [0xcc, 0x81]
    );
    assert_eq!(
        encode_instruction("a @>8300(r4), r5", &HashMap::new(), 0).unwrap(),
        [0xa1, 0x64, 0x83, 0x00]
    );
    assert_eq!(
        encode_instruction("sra r6, 4", &HashMap::new(), 0).unwrap(),
        [0x08, 0x46]
    );
    assert_eq!(
        encode_instruction("sbo -1", &HashMap::new(), 0).unwrap(),
        [0x1d, 0xff]
    );
}

#[test]
fn encodes_label_relative_jumps() {
    let labels = HashMap::from([("loop".to_owned(), 0x1000)]);
    assert_eq!(
        encode_instruction("jmp loop", &labels, 0x1004).unwrap(),
        [0x10, 0xfd]
    );
}

#[test]
fn encodes_every_documented_instruction_format() {
    let labels = HashMap::from([("target".to_owned(), 0x1008)]);
    let cases = [
        ("szcb *r1, @>8300(r2)", vec![0x58, 0x91, 0x83, 0x00]),
        ("blwp @>0000", vec![0x04, 0x20, 0x00, 0x00]),
        ("stwp r15", vec![0x02, 0xaf]),
        ("limi >000F", vec![0x03, 0x00, 0x00, 0x0f]),
        ("src r3, 15", vec![0x0b, 0xf3]),
        ("jeq target", vec![0x13, 0x03]),
        ("tb -128", vec![0x1f, 0x80]),
        ("ldcr @>8c00, 8", vec![0x32, 0x20, 0x8c, 0x00]),
        ("stcr *r4+, 1", vec![0x34, 0x74]),
        ("mpy @>9000(r1), r2", vec![0x38, 0xa1, 0x90, 0x00]),
        ("div r5, r6", vec![0x3d, 0x85]),
        ("xop r1, 2", vec![0x2c, 0x81]),
        ("rt", vec![0x04, 0x5b]),
        ("rtwp", vec![0x03, 0x80]),
    ];

    for (text, expected) in cases {
        assert_eq!(
            encode_instruction(text, &labels, 0x1000).unwrap(),
            expected,
            "{text}"
        );
    }
}

#[test]
fn matches_libre99_for_standard_instruction_encodings() {
    let cases = [
        "li r1, >1234",
        "mov r1, *r2+",
        "a @>8300(r4), r5",
        "sra r6, 4",
        "coc r1, r2",
        "ldcr @>8c00, 8",
        "mpy @>9000(r1), r2",
        "rtwp",
    ];
    let options = Libre99AssemblerOptions {
        auto_header: false,
        ..Default::default()
    };

    for text in cases {
        let libre99_source = text.to_ascii_uppercase().replace(", ", ",");
        let libre99 = assemble_with_libre99(&format!("   {libre99_source}\n"), &options)
            .unwrap_or_else(|diagnostics| panic!("Libre99 rejected `{text}`: {diagnostics:?}"));
        assert_eq!(
            encode_instruction(text, &HashMap::new(), 0).unwrap(),
            libre99.image,
            "{text}"
        );
    }
}

#[test]
fn rejects_invalid_instruction_operands() {
    for text in [
        "li r16, 1",
        "mov r0, @>1234(r16)",
        "jmp >1001",
        "sra 16, r0",
        "sbo 128",
        "ldcr r0, 16",
        "not_an_instruction r0",
    ] {
        assert!(
            encode_instruction(text, &HashMap::new(), 0x1000).is_err(),
            "{text}"
        );
    }
}

#[test]
fn data_words_use_tms9900_big_endian_order() {
    let image = crate::vm::assemble_subset_with_symbols_at(
        crate::target::AssemblerCpu::Tms9900,
        "dw >1234\n",
        0x1000,
    )
    .unwrap();
    assert_eq!(image.bytes, [0x12, 0x34]);
}

#[test]
fn emitted_instructions_execute_on_libre99() {
    let source = ["li r1, >1234", "ai r1, 1", "mov r1, @>9000"].join("\n");
    let bytes = crate::vm::assemble_subset_with_symbols_at(
        crate::target::AssemblerCpu::Tms9900,
        &source,
        0x0100,
    )
    .unwrap();
    let mut ram = FlatRam::new();
    ram.load(0x0100, &bytes.bytes);
    let mut cpu = Cpu::new();
    cpu.set_wp(0x8300);
    cpu.set_pc(0x0100);

    for _ in 0..3 {
        assert!(cpu.step(&mut ram) > 0);
    }

    assert_eq!(ram.read_word(0x8302), 0x1235);
    assert_eq!(ram.read_word(0x9000), 0x1235);
}
