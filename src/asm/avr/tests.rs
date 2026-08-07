use super::{encode_instruction, instruction_len};
use std::collections::HashMap;

fn word(value: u16) -> Vec<u8> {
    vec![value as u8, (value >> 8) as u8]
}

#[test]
fn encodes_all_documented_mnemonics_and_aliases() {
    let labels = HashMap::new();
    let cases: &[(&str, u16)] = &[
        ("nop", 0x0000),
        ("add r1,r2", 0x0c12),
        ("adc r1,r2", 0x1c12),
        ("adiw r24,63", 0x96cf),
        ("sub r1,r2", 0x1812),
        ("subi r16,255", 0x5f0f),
        ("sbc r1,r2", 0x0812),
        ("sbci r16,255", 0x4f0f),
        ("sbiw r30,63", 0x97ff),
        ("and r1,r2", 0x2012),
        ("andi r16,255", 0x7f0f),
        ("or r1,r2", 0x2812),
        ("ori r16,255", 0x6f0f),
        ("eor r1,r2", 0x2412),
        ("com r1", 0x9410),
        ("neg r1", 0x9411),
        ("inc r1", 0x9413),
        ("dec r1", 0x941a),
        ("asr r1", 0x9415),
        ("lsr r1", 0x9416),
        ("ror r1", 0x9417),
        ("swap r1", 0x9412),
        ("lsl r1", 0x0c11),
        ("rol r1", 0x1c11),
        ("clr r1", 0x2411),
        ("tst r1", 0x2011),
        ("ser r16", 0xef0f),
        ("sbr r16,1", 0x6001),
        ("cbr r16,1", 0x7f0e),
        ("cp r1,r2", 0x1412),
        ("cpc r1,r2", 0x0412),
        ("cpi r16,255", 0x3f0f),
        ("cpse r1,r2", 0x1012),
        ("mov r1,r2", 0x2c12),
        ("movw r2,r4", 0x0112),
        ("mul r1,r2", 0x9c12),
        ("muls r16,r17", 0x0201),
        ("mulsu r16,r17", 0x0301),
        ("fmul r16,r17", 0x0309),
        ("fmuls r16,r17", 0x0381),
        ("fmulsu r16,r17", 0x0389),
        ("bld r1,7", 0xf817),
        ("bst r1,7", 0xfa17),
        ("sbrc r1,7", 0xfc17),
        ("sbrs r1,7", 0xfe17),
        ("sbi 31,7", 0x9aff),
        ("cbi 31,7", 0x98ff),
        ("sbic 31,7", 0x99ff),
        ("sbis 31,7", 0x9bff),
        ("in r1,63", 0xb61f),
        ("out 63,r1", 0xbe1f),
        ("bset 7", 0x9478),
        ("bclr 7", 0x94f8),
        ("sec", 0x9408),
        ("sez", 0x9418),
        ("sen", 0x9428),
        ("sev", 0x9438),
        ("ses", 0x9448),
        ("seh", 0x9458),
        ("set", 0x9468),
        ("sei", 0x9478),
        ("clc", 0x9488),
        ("clz", 0x9498),
        ("cln", 0x94a8),
        ("clv", 0x94b8),
        ("cls", 0x94c8),
        ("clh", 0x94d8),
        ("clt", 0x94e8),
        ("cli", 0x94f8),
        ("rjmp 2", 0xc000),
        ("rcall 2", 0xd000),
        ("brbs 0,2", 0xf000),
        ("brbc 0,2", 0xf400),
        ("breq 2", 0xf001),
        ("brne 2", 0xf401),
        ("brcs 2", 0xf000),
        ("brlo 2", 0xf000),
        ("brcc 2", 0xf400),
        ("brsh 2", 0xf400),
        ("brmi 2", 0xf002),
        ("brpl 2", 0xf402),
        ("brvs 2", 0xf003),
        ("brvc 2", 0xf403),
        ("brlt 2", 0xf004),
        ("brge 2", 0xf404),
        ("brhs 2", 0xf005),
        ("brhc 2", 0xf405),
        ("brts 2", 0xf006),
        ("brtc 2", 0xf406),
        ("brie 2", 0xf007),
        ("brid 2", 0xf407),
        ("push r1", 0x921f),
        ("pop r1", 0x901f),
        ("ret", 0x9508),
        ("reti", 0x9518),
        ("ijmp", 0x9409),
        ("eijmp", 0x9419),
        ("icall", 0x9509),
        ("eicall", 0x9519),
        ("lpm", 0x95c8),
        ("elpm", 0x95d8),
        ("spm", 0x95e8),
        ("spm z+", 0x95f8),
        ("break", 0x9598),
        ("sleep", 0x9588),
        ("wdr", 0x95a8),
        ("lpm r1,z", 0x9014),
        ("lpm r1,z+", 0x9015),
        ("elpm r1,z", 0x9016),
        ("elpm r1,z+", 0x9017),
        ("xch z,r1", 0x9214),
        ("las z,r1", 0x9215),
        ("lac z,r1", 0x9216),
        ("lat z,r1", 0x9217),
        ("des 15", 0x94fb),
    ];
    for &(source, expected) in cases {
        assert_eq!(
            encode_instruction(source, &labels, 0).unwrap(),
            word(expected),
            "{source}"
        );
    }
}

#[test]
fn encodes_every_pointer_mode_and_long_instruction() {
    let labels = HashMap::new();
    for (source, expected) in [
        ("ld r1,x", 0x901c),
        ("ld r1,x+", 0x901d),
        ("ld r1,-x", 0x901e),
        ("ld r1,y", 0x8018),
        ("ld r1,y+", 0x9019),
        ("ld r1,-y", 0x901a),
        ("ld r1,z", 0x8010),
        ("ld r1,z+", 0x9011),
        ("ld r1,-z", 0x9012),
        ("st x,r1", 0x921c),
        ("st x+,r1", 0x921d),
        ("st -x,r1", 0x921e),
        ("st y,r1", 0x8218),
        ("st y+,r1", 0x9219),
        ("st -y,r1", 0x921a),
        ("st z,r1", 0x8210),
        ("st z+,r1", 0x9211),
        ("st -z,r1", 0x9212),
        ("ldd r1,y+63", 0xac1f),
        ("ldd r1,z+63", 0xac17),
        ("std y+63,r1", 0xae1f),
        ("std z+63,r1", 0xae17),
    ] {
        assert_eq!(
            encode_instruction(source, &labels, 0).unwrap(),
            word(expected),
            "{source}"
        );
    }
    assert_eq!(
        encode_instruction("lds r31,0xffff", &labels, 0).unwrap(),
        vec![0xf0, 0x91, 0xff, 0xff]
    );
    assert_eq!(
        encode_instruction("sts 0xffff,r31", &labels, 0).unwrap(),
        vec![0xf0, 0x93, 0xff, 0xff]
    );
    assert_eq!(
        encode_instruction("jmp 0x7ffffe", &labels, 0).unwrap(),
        vec![0xfd, 0x95, 0xff, 0xff]
    );
    assert_eq!(
        encode_instruction("call 0x7ffffe", &labels, 0).unwrap(),
        vec![0xff, 0x95, 0xff, 0xff]
    );
    assert_eq!(instruction_len("lds r1, symbol").unwrap(), 4);
}

#[test]
fn validates_boundaries_alignment_and_case_insensitive_labels() {
    let labels = HashMap::from([("MiXeD".to_string(), 4096)]);
    assert_eq!(
        encode_instruction("RJMP mixed", &labels, 4094).unwrap(),
        word(0xc000)
    );
    let empty = HashMap::new();
    for source in ["rjmp 4097", "rcall 4097", "breq 3", "jmp 3", "call 3"] {
        assert!(encode_instruction(source, &empty, 0).is_err(), "{source}");
    }
    assert!(encode_instruction("rjmp 4096", &empty, 0).is_ok());
    assert!(encode_instruction("rjmp 4294960000", &empty, u32::MAX - 1).is_err());
    assert!(encode_instruction("breq 128", &empty, 0).is_ok());
    assert!(encode_instruction("breq 130", &empty, 0).is_err());
    for source in [
        "ldi r15,0",
        "adiw r22,0",
        "adiw r24,64",
        "muls r15,r16",
        "mulsu r24,r16",
        "movw r1,r2",
        "sbi 32,0",
        "sbi 0,8",
        "in r0,64",
        "ldd r0,y+64",
        "lds r0,65536",
        "des 16",
        "jmp 0x800000",
    ] {
        assert!(encode_instruction(source, &empty, 0).is_err(), "{source}");
    }
}
