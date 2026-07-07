use crate::target::CpuFamily;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct InstructionSpec {
    pub syntax: &'static str,
    pub cpus: &'static [CpuFamily],
    pub bytes: &'static [u8],
}

const Z80_AND_EZ80: &[CpuFamily] = &[CpuFamily::Z80, CpuFamily::Ez80];

pub const EXACT_INSTRUCTIONS: &[InstructionSpec] = &[
    z80_ez80("nop", &[0x00]),
    z80_ez80("di", &[0xF3]),
    z80_ez80("ei", &[0xFB]),
    z80_ez80("halt", &[0x76]),
    z80_ez80("ret", &[0xC9]),
    z80_ez80("ret nz", &[0xC0]),
    z80_ez80("ret z", &[0xC8]),
    z80_ez80("ret nc", &[0xD0]),
    z80_ez80("ret c", &[0xD8]),
    z80_ez80("ret po", &[0xE0]),
    z80_ez80("ret pe", &[0xE8]),
    z80_ez80("ret p", &[0xF0]),
    z80_ez80("ret m", &[0xF8]),
    z80_ez80("reti", &[0xED, 0x4D]),
    z80_ez80("retn", &[0xED, 0x45]),
    z80_ez80("or a", &[0xB7]),
    z80_ez80("xor a", &[0xAF]),
    z80_ez80("scf", &[0x37]),
    z80_ez80("ccf", &[0x3F]),
    z80_ez80("cpl", &[0x2F]),
    z80_ez80("daa", &[0x27]),
    z80_ez80("neg", &[0xED, 0x44]),
    z80_ez80("rlca", &[0x07]),
    z80_ez80("rla", &[0x17]),
    z80_ez80("rrca", &[0x0F]),
    z80_ez80("rra", &[0x1F]),
    z80_ez80("rld", &[0xED, 0x6F]),
    z80_ez80("rrd", &[0xED, 0x67]),
    z80_ez80("ex de, hl", &[0xEB]),
    z80_ez80("ex af, af'", &[0x08]),
    z80_ez80("exx", &[0xD9]),
    z80_ez80("im 0", &[0xED, 0x46]),
    z80_ez80("im 1", &[0xED, 0x56]),
    z80_ez80("im 2", &[0xED, 0x5E]),
];

const fn z80_ez80(syntax: &'static str, bytes: &'static [u8]) -> InstructionSpec {
    InstructionSpec {
        syntax,
        cpus: Z80_AND_EZ80,
        bytes,
    }
}

pub fn exact_instruction(cpu: CpuFamily, text: &str) -> Option<&'static InstructionSpec> {
    EXACT_INSTRUCTIONS
        .iter()
        .find(|instruction| instruction.syntax == text && instruction.cpus.contains(&cpu))
}

pub fn instruction_set(cpu: CpuFamily) -> impl Iterator<Item = &'static InstructionSpec> {
    EXACT_INSTRUCTIONS
        .iter()
        .filter(move |instruction| instruction.cpus.contains(&cpu))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exact_instruction_metadata_encodes_common_ops() {
        assert_eq!(
            exact_instruction(CpuFamily::Ez80, "nop").unwrap().bytes,
            &[0x00]
        );
        assert_eq!(
            exact_instruction(CpuFamily::Ez80, "reti").unwrap().bytes,
            &[0xED, 0x4D]
        );
    }

    #[test]
    fn metadata_can_generate_z80_subset() {
        let z80 = instruction_set(CpuFamily::Z80).collect::<Vec<_>>();
        assert!(z80.iter().any(|instruction| instruction.syntax == "ret"));
        assert!(z80.iter().any(|instruction| instruction.syntax == "im 2"));
    }
}
