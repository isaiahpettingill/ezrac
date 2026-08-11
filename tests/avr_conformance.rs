#![cfg(feature = "avr")]

use std::collections::HashMap;

use avr_emulator::{DecodeError, Profile, decode};
use ezra::{asm::avr::encode_instruction, target::AssemblerCpu};

fn word(bytes: &[u8]) -> u16 {
    u16::from_le_bytes([bytes[0], bytes[1]])
}

#[test]
fn every_assembler_opcode_form_decodes_in_a_supported_core() {
    let labels = HashMap::new();
    let cases = [
        "nop",
        "add r1,r2",
        "adc r1,r2",
        "adiw r24,63",
        "sub r1,r2",
        "subi r16,255",
        "sbc r1,r2",
        "sbci r16,255",
        "sbiw r30,63",
        "and r1,r2",
        "andi r16,255",
        "or r1,r2",
        "ori r16,255",
        "eor r1,r2",
        "com r1",
        "neg r1",
        "inc r1",
        "dec r1",
        "asr r1",
        "lsr r1",
        "ror r1",
        "swap r1",
        "lsl r1",
        "rol r1",
        "clr r1",
        "tst r1",
        "ser r16",
        "sbr r16,1",
        "cbr r16,1",
        "cp r1,r2",
        "cpc r1,r2",
        "cpi r16,255",
        "cpse r1,r2",
        "mov r1,r2",
        "movw r2,r4",
        "mul r1,r2",
        "muls r16,r17",
        "mulsu r16,r17",
        "fmul r16,r17",
        "fmuls r16,r17",
        "fmulsu r16,r17",
        "bld r1,7",
        "bst r1,7",
        "sbrc r1,7",
        "sbrs r1,7",
        "sbi 31,7",
        "cbi 31,7",
        "sbic 31,7",
        "sbis 31,7",
        "in r1,63",
        "out 63,r1",
        "bset 7",
        "bclr 7",
        "sec",
        "clc",
        "rjmp 2",
        "rcall 2",
        "brbs 0,2",
        "brbc 0,2",
        "breq 2",
        "brne 2",
        "brcs 2",
        "brcc 2",
        "brmi 2",
        "brpl 2",
        "brvs 2",
        "brvc 2",
        "brlt 2",
        "brge 2",
        "brhs 2",
        "brhc 2",
        "brts 2",
        "brtc 2",
        "brie 2",
        "brid 2",
        "push r1",
        "pop r1",
        "ret",
        "reti",
        "ijmp",
        "eijmp",
        "icall",
        "eicall",
        "lpm",
        "elpm",
        "spm",
        "spm z+",
        "break",
        "sleep",
        "wdr",
        "lpm r1,z",
        "lpm r1,z+",
        "elpm r1,z",
        "elpm r1,z+",
        "xch z,r1",
        "las z,r1",
        "lac z,r1",
        "lat z,r1",
        "des 15",
        "ld r1,x",
        "ld r1,x+",
        "ld r1,-x",
        "ld r1,y",
        "ld r1,y+",
        "ld r1,-y",
        "ld r1,z",
        "ld r1,z+",
        "ld r1,-z",
        "st x,r1",
        "st x+,r1",
        "st -x,r1",
        "st y,r1",
        "st y+,r1",
        "st -y,r1",
        "st z,r1",
        "st z+,r1",
        "st -z,r1",
        "ldd r1,y+1",
        "ldd r1,z+2",
        "std y+3,r1",
        "std z+63,r1",
        "lds r1,0xffff",
        "sts 0xffff,r1",
        "jmp 0x7ffffe",
        "call 0x7ffffe",
    ];

    let profiles = [
        Profile::avre(),
        Profile::avre_plus(),
        Profile::avrxm(),
        Profile::avrxt(),
    ];
    for source in cases {
        let bytes = encode_instruction(source, &labels, 0)
            .unwrap_or_else(|error| panic!("assembler rejected `{source}`: {error}"));
        assert!(bytes.len() == 2 || bytes.len() == 4, "{source}: {bytes:?}");
        let instruction = word(&bytes);
        assert!(
            profiles
                .iter()
                .any(|profile| decode(instruction, *profile).is_ok()),
            "assembler emitted undecodable `{source}`: 0x{instruction:04x}"
        );
    }
}

#[test]
fn assembler_profile_gates_match_documented_core_families() {
    let labels = HashMap::new();
    let encoded = |source: &str, cpu| {
        encode_instruction_for_test(cpu, source, &labels)
            .unwrap_or_else(|error| panic!("{cpu:?} rejected `{source}`: {error}"))
    };

    for cpu in [
        AssemblerCpu::AvrTiny,
        AssemblerCpu::AvrMega,
        AssemblerCpu::AvrDx,
    ] {
        assert!(
            ezra::asm::avr::encode_instruction_for_variant(
                "xch z,r18",
                &labels,
                0,
                ezra::asm::avr::AvrVariant::from_assembler_cpu(cpu).unwrap(),
            )
            .is_err()
        );
    }
    assert!(encoded("xch z,r18", AssemblerCpu::AvrXmega).len() == 2);
    assert!(encoded("eicall", AssemblerCpu::AvrDx).len() == 2);
    assert!(encoded("des 3", AssemblerCpu::AvrXmega).len() == 2);
    assert!(
        ezra::asm::avr::encode_instruction_for_variant(
            "des 3",
            &labels,
            0,
            ezra::asm::avr::AvrVariant::Dx,
        )
        .is_err()
    );
}

fn encode_instruction_for_test(
    cpu: AssemblerCpu,
    source: &str,
    labels: &HashMap<String, u32>,
) -> Result<Vec<u8>, ezra::diagnostic::Diagnostic> {
    ezra::asm::avr::encode_instruction_for_variant(
        source,
        labels,
        0,
        ezra::asm::avr::AvrVariant::from_assembler_cpu(cpu).unwrap(),
    )
}

#[test]
fn decoder_errors_keep_the_emitted_word_for_reserved_or_unsupported_forms() {
    let labels = HashMap::new();
    let bytes = encode_instruction("xch z,r1", &labels, 0).unwrap();
    let result = decode(word(&bytes), Profile::avrxt());
    assert!(matches!(
        result,
        Err(DecodeError::Unsupported { word: emitted, .. }) if emitted == word(&bytes)
    ));
}
