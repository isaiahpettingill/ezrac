use super::*;
use crate::target::AssemblerCpu;

fn encode(text: &str) -> Vec<u8> {
    encode_cpu(AssemblerCpu::Msp430, text)
}

fn encode_cpu(cpu: AssemblerCpu, text: &str) -> Vec<u8> {
    encode_instruction(cpu, text, &HashMap::new(), 0).unwrap()
}

#[test]
fn encodes_core_msp430_instruction_forms() {
    assert_eq!(encode("nop"), vec![0x03, 0x43]);
    assert_eq!(encode("mov #1,r4"), vec![0x14, 0x43]);
    assert_eq!(encode("mov #0x1234,r4"), vec![0x34, 0x40, 0x34, 0x12]);
    assert_eq!(encode("add r4,r5"), vec![0x05, 0x54]);
    assert_eq!(encode("mov 4(r5),r6"), vec![0x16, 0x45, 0x04, 0x00]);
    assert_eq!(encode("mov @r5+,r6"), vec![0x36, 0x45]);
    assert_eq!(encode("call r4"), vec![0x84, 0x12]);
}

#[test]
fn encodes_relative_jumps_and_aliases() {
    assert_eq!(encode("jmp 0x0004"), vec![0x01, 0x3C]);
    assert_eq!(encode("jnz 0x0000"), vec![0xFF, 0x23]);
    assert_eq!(encode("br r4"), encode("mov r4,r0"));
    assert_eq!(encode("ret"), encode("mov @r1+,r0"));
}

#[test]
fn sizes_literal_and_symbol_immediates_with_extension_words() {
    assert_eq!(
        instruction_len(AssemblerCpu::Msp430, "mov #0x1234,r4").unwrap(),
        4
    );
    assert_eq!(
        instruction_len(AssemblerCpu::Msp430, "call #_main").unwrap(),
        4
    );
}

#[test]
fn encodes_msp430x_address_and_extended_alu_forms() {
    assert_eq!(
        encode_cpu(AssemblerCpu::Msp430X, "mov.a #0x12345,r5"),
        vec![0x85, 0x01, 0x45, 0x23]
    );
    assert_eq!(
        encode_cpu(AssemblerCpu::Msp430X, "mov.a r5,&0x23456"),
        vec![0x62, 0x05, 0x56, 0x34]
    );
    assert_eq!(
        encode_cpu(AssemblerCpu::Msp430X2, "mov.a &0x23456,r5"),
        vec![0x25, 0x02, 0x56, 0x34]
    );
    assert_eq!(
        encode_cpu(AssemblerCpu::Msp430X, "mov.a 0(r10),r5"),
        vec![0x35, 0x0A, 0x00, 0x00]
    );
    assert_eq!(
        encode_cpu(AssemblerCpu::Msp430X, "mov.a @r0,r0"),
        vec![0x00, 0x00]
    );
    assert_eq!(
        encode_cpu(AssemblerCpu::Msp430X, "add.a #4,r10"),
        vec![0xAA, 0x00, 0x04, 0x00]
    );
    assert_eq!(
        encode_cpu(AssemblerCpu::Msp430X, "cmp.a r5,r4"),
        vec![0xD4, 0x05]
    );
    assert_eq!(
        encode_cpu(AssemblerCpu::Msp430X, "and.a #0xFFFFF,r0"),
        vec![0x80, 0x1F, 0x30, 0xF0, 0xFF, 0xFF]
    );
    assert_eq!(
        encode_cpu(AssemblerCpu::Msp430X, "calla r0"),
        vec![0x40, 0x13]
    );
}

#[test]
fn rejects_invalid_msp430_forms() {
    let error = encode_instruction(AssemblerCpu::Msp430, "mov #0x10000,r4", &HashMap::new(), 0)
        .unwrap_err();
    assert!(error.message.contains("outside the 16-bit"), "{error}");

    let error =
        encode_instruction(AssemblerCpu::Msp430, "mov.a r4,r5", &HashMap::new(), 0).unwrap_err();
    assert!(error.message.contains("address instructions"), "{error}");
}
