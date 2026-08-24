use super::*;

fn labels() -> HashMap<String, u32> {
    HashMap::new()
}

#[test]
fn encodes_native_m6809_instructions() {
    for (source, expected) in [
        ("nop", vec![0x12]),
        ("mul", vec![0x3D]),
        ("lbra 1000h", vec![0x16, 0x0F, 0xFD]),
        ("lbsr 1000h", vec![0x17, 0x0F, 0xFD]),
        ("lda #12h", vec![0x86, 0x12]),
        ("ldd #1234h", vec![0xCC, 0x12, 0x34]),
        ("ldy #1234h", vec![0x10, 0x8E, 0x12, 0x34]),
        ("lda ,x", vec![0xA6, 0x84]),
        ("lda 1,x", vec![0xA6, 0x01]),
        ("lda 100,x", vec![0xA6, 0x88, 0x64]),
        ("lda [8,s]", vec![0xA6, 0xF8, 0x08]),
        ("lda >1234h", vec![0xB6, 0x12, 0x34]),
        ("lda [$1234]", vec![0xA6, 0x9F, 0x12, 0x34]),
        ("leax 5,y", vec![0x30, 0x25]),
        ("exg d,x", vec![0x1E, 0x01]),
        ("pshs y,x,b,a", vec![0x34, 0x36]),
    ] {
        assert_eq!(
            emit_instruction(source, &labels(), 0x0).unwrap(),
            Some(expected),
            "{source}"
        );
    }
}

#[test]
fn keeps_m6800_accumulator_aliases() {
    assert_eq!(
        emit_instruction("ldaa #12h", &labels(), 0).unwrap(),
        Some(vec![0x86, 0x12])
    );
    assert_eq!(
        emit_instruction("staa >1234h", &labels(), 0).unwrap(),
        Some(vec![0xB7, 0x12, 0x34])
    );
}

#[test]
fn sizes_indexed_operands_consistently_with_encoding() {
    for source in [
        "leas -36,s",
        "leas 36,s",
        "leas -128,s",
        "leas -129,s",
        "leau 15,u",
        "leau 16,u",
        "leax 127,x",
        "leay 128,x",
        "leas <-5,s",
        "leau [100,u]",
    ] {
        let encoded = emit_instruction(source, &labels(), 0x0)
            .unwrap()
            .expect(source);
        let length = instruction_len(source).unwrap().expect(source);
        assert_eq!(encoded.len(), length, "{source}");
    }
}
