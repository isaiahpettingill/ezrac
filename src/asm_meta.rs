use crate::target::CpuFamily;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct InstructionSpec {
    pub syntax: &'static str,
    pub cpu: CpuFamily,
    pub bytes: &'static [u8],
}

pub const EZ80_EXACT_INSTRUCTIONS: &[InstructionSpec] = &[
    spec("nop", &[0x00]),
    spec("di", &[0xF3]),
    spec("ei", &[0xFB]),
    spec("halt", &[0x76]),
    spec("ret", &[0xC9]),
    spec("ret nz", &[0xC0]),
    spec("ret z", &[0xC8]),
    spec("ret nc", &[0xD0]),
    spec("ret c", &[0xD8]),
    spec("ret po", &[0xE0]),
    spec("ret pe", &[0xE8]),
    spec("ret p", &[0xF0]),
    spec("ret m", &[0xF8]),
    spec("reti", &[0xED, 0x4D]),
    spec("retn", &[0xED, 0x45]),
    spec("or a", &[0xB7]),
    spec("xor a", &[0xAF]),
    spec("scf", &[0x37]),
    spec("ccf", &[0x3F]),
    spec("cpl", &[0x2F]),
    spec("daa", &[0x27]),
    spec("neg", &[0xED, 0x44]),
    spec("rlca", &[0x07]),
    spec("rla", &[0x17]),
    spec("rrca", &[0x0F]),
    spec("rra", &[0x1F]),
    spec("rld", &[0xED, 0x6F]),
    spec("rrd", &[0xED, 0x67]),
    spec("ex de, hl", &[0xEB]),
    spec("ex af, af'", &[0x08]),
    spec("exx", &[0xD9]),
    spec("im 0", &[0xED, 0x46]),
    spec("im 1", &[0xED, 0x56]),
    spec("im 2", &[0xED, 0x5E]),
];

const fn spec(syntax: &'static str, bytes: &'static [u8]) -> InstructionSpec {
    InstructionSpec {
        syntax,
        cpu: CpuFamily::Ez80,
        bytes,
    }
}

pub fn exact_instruction(text: &str) -> Option<&'static InstructionSpec> {
    EZ80_EXACT_INSTRUCTIONS
        .iter()
        .find(|instruction| instruction.syntax == text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exact_instruction_metadata_encodes_common_ops() {
        assert_eq!(exact_instruction("nop").unwrap().bytes, &[0x00]);
        assert_eq!(exact_instruction("reti").unwrap().bytes, &[0xED, 0x4D]);
    }
}
