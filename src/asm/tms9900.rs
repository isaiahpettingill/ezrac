use std::collections::HashMap;

use crate::diagnostic::Diagnostic;

pub fn instruction_len(text: &str) -> Result<usize, Diagnostic> {
    Ok(encode(text, &HashMap::new(), 0, false)?.len())
}

pub fn encode_instruction(
    text: &str,
    labels: &HashMap<String, u32>,
    pc: u32,
) -> Result<Vec<u8>, Diagnostic> {
    encode(text, labels, pc, true)
}

fn encode(
    text: &str,
    labels: &HashMap<String, u32>,
    pc: u32,
    resolve: bool,
) -> Result<Vec<u8>, Diagnostic> {
    let (mnemonic, operands) = split_instruction(text)?;
    let mnemonic = mnemonic.to_ascii_lowercase();

    if let Some(base) = two_operand_base(&mnemonic) {
        let (source, destination) = split_operands(operands)?;
        let source = parse_operand(source, labels, resolve)?;
        let destination = parse_operand(destination, labels, resolve)?;
        let mut bytes = word(base | ((destination.field as u16) << 6) | source.field as u16);
        bytes.extend(source.extensions);
        bytes.extend(destination.extensions);
        return Ok(bytes);
    }

    if let Some(base) = single_operand_base(&mnemonic) {
        let operand = parse_operand(required_operand(operands, &mnemonic)?, labels, resolve)?;
        let mut bytes = word(base | operand.field as u16);
        bytes.extend(operand.extensions);
        return Ok(bytes);
    }

    if let Some(base) = immediate_base(&mnemonic) {
        return encode_immediate(base, &mnemonic, operands, labels, resolve);
    }

    if let Some(base) = shift_base(&mnemonic) {
        let (first, second) = split_operands(operands)?;
        // Accept the established count/register spelling as well as the
        // register/count spelling used by TI assemblers and newer emitters.
        let (register, count) = match register_number(first) {
            Ok(register) => (register, value(second, labels, resolve)?),
            Err(_) => (register_number(second)?, value(first, labels, resolve)?),
        };
        if count > 15 {
            return Err(Diagnostic::new(format!(
                "TMS9900 shift count `{count}` is outside 0..15"
            )));
        }
        return Ok(word(base | ((count as u16) << 4) | register as u16));
    }

    if let Some(base) = jump_base(&mnemonic) {
        if !resolve {
            required_operand(operands, &mnemonic)?;
            return Ok(word(base));
        }
        let target = value(required_operand(operands, &mnemonic)?, labels, true)?;
        let next = pc.wrapping_add(2);
        if target & 1 != 0 {
            return Err(Diagnostic::new(format!(
                "TMS9900 {mnemonic} target is not word-aligned"
            )));
        }
        let delta = target as i64 - next as i64;
        if delta % 2 != 0 {
            return Err(Diagnostic::new(format!(
                "TMS9900 {mnemonic} target is not word-aligned"
            )));
        }
        let displacement = delta / 2;
        if !(-128..=127).contains(&displacement) {
            return Err(Diagnostic::new(format!(
                "TMS9900 {mnemonic} target is outside the -128..127 word range"
            )));
        }
        return Ok(word(base | (displacement as i8 as u8) as u16));
    }

    if let Some(base) = cru_bit_base(&mnemonic) {
        let offset = signed_value(required_operand(operands, &mnemonic)?, labels, resolve)?;
        if !(-128..=127).contains(&offset) {
            return Err(Diagnostic::new(format!(
                "TMS9900 {mnemonic} CRU offset is outside -128..127"
            )));
        }
        return Ok(word(base | (offset as i8 as u8) as u16));
    }

    if matches!(mnemonic.as_str(), "ldcr" | "stcr") {
        let (operand, count) = split_operands(operands)?;
        let operand = parse_operand(operand, labels, resolve)?;
        let count = value(count, labels, resolve)?;
        if count > 15 {
            return Err(Diagnostic::new(format!(
                "TMS9900 {mnemonic} count `{count}` is outside 0..15"
            )));
        }
        let base = if mnemonic == "ldcr" { 0x3000 } else { 0x3400 };
        let mut bytes = word(base | ((count as u16) << 6) | operand.field as u16);
        bytes.extend(operand.extensions);
        return Ok(bytes);
    }

    if matches!(
        mnemonic.as_str(),
        "coc" | "czc" | "xor" | "mpy" | "div" | "xop"
    ) {
        let (source, destination) = split_operands(operands)?;
        let source = parse_operand(source, labels, resolve)?;
        let destination = if mnemonic == "xop" {
            value(destination, labels, resolve)?
        } else {
            u32::from(register_number(destination)?)
        };
        if destination > 15 {
            return Err(Diagnostic::new(format!(
                "TMS9900 {mnemonic} register/field `{destination}` is outside 0..15"
            )));
        }
        let base = match mnemonic.as_str() {
            "coc" => 0x2000,
            "czc" => 0x2400,
            "xor" => 0x2800,
            "xop" => 0x2c00,
            "mpy" => 0x3800,
            "div" => 0x3c00,
            _ => unreachable!("mnemonic checked above"),
        };
        let mut bytes = word(base | ((destination as u16) << 6) | source.field as u16);
        bytes.extend(source.extensions);
        return Ok(bytes);
    }

    match mnemonic.as_str() {
        "nop" if operands.is_empty() => Ok(word(0x1000)),
        "rt" if operands.is_empty() => Ok(word(0x045b)),
        "idle" if operands.is_empty() => Ok(word(0x0340)),
        "rset" if operands.is_empty() => Ok(word(0x0360)),
        "rtwp" if operands.is_empty() => Ok(word(0x0380)),
        "ckon" if operands.is_empty() => Ok(word(0x03a0)),
        "ckof" if operands.is_empty() => Ok(word(0x03c0)),
        "lrex" if operands.is_empty() => Ok(word(0x03e0)),
        _ => Err(Diagnostic::new(format!(
            "assembler does not support TMS9900 instruction `{text}`"
        ))),
    }
}

fn two_operand_base(mnemonic: &str) -> Option<u16> {
    Some(match mnemonic {
        "szc" => 0x4000,
        "szcb" => 0x5000,
        "s" => 0x6000,
        "sb" => 0x7000,
        "c" => 0x8000,
        "cb" => 0x9000,
        "a" => 0xa000,
        "ab" => 0xb000,
        "mov" => 0xc000,
        "movb" => 0xd000,
        "soc" => 0xe000,
        "socb" => 0xf000,
        _ => return None,
    })
}

fn single_operand_base(mnemonic: &str) -> Option<u16> {
    Some(match mnemonic {
        "blwp" => 0x0400,
        "b" => 0x0440,
        "x" => 0x0480,
        "clr" => 0x04c0,
        "neg" => 0x0500,
        "inv" => 0x0540,
        "inc" => 0x0580,
        "inct" => 0x05c0,
        "dec" => 0x0600,
        "dect" => 0x0640,
        "bl" => 0x0680,
        "swpb" => 0x06c0,
        "seto" => 0x0700,
        "abs" => 0x0740,
        _ => return None,
    })
}

fn immediate_base(mnemonic: &str) -> Option<u16> {
    Some(match mnemonic {
        "li" => 0x0200,
        "ai" => 0x0220,
        "andi" => 0x0240,
        "ori" => 0x0260,
        "ci" => 0x0280,
        "stwp" => 0x02a0,
        "stst" => 0x02c0,
        "lwpi" => 0x02e0,
        "limi" => 0x0300,
        _ => return None,
    })
}

fn shift_base(mnemonic: &str) -> Option<u16> {
    Some(match mnemonic {
        "sra" => 0x0800,
        "srl" => 0x0900,
        "sla" => 0x0a00,
        "src" => 0x0b00,
        _ => return None,
    })
}

fn jump_base(mnemonic: &str) -> Option<u16> {
    Some(match mnemonic {
        "jmp" => 0x1000,
        "jlt" => 0x1100,
        "jle" => 0x1200,
        "jeq" => 0x1300,
        "jhe" => 0x1400,
        "jgt" => 0x1500,
        "jne" => 0x1600,
        "jnc" => 0x1700,
        "joc" => 0x1800,
        "jno" => 0x1900,
        "jl" => 0x1a00,
        "jh" => 0x1b00,
        "jop" => 0x1c00,
        _ => return None,
    })
}

fn cru_bit_base(mnemonic: &str) -> Option<u16> {
    Some(match mnemonic {
        "sbo" => 0x1d00,
        "sbz" => 0x1e00,
        "tb" => 0x1f00,
        _ => return None,
    })
}

fn encode_immediate(
    base: u16,
    mnemonic: &str,
    operands: &str,
    labels: &HashMap<String, u32>,
    resolve: bool,
) -> Result<Vec<u8>, Diagnostic> {
    match mnemonic {
        "stwp" | "stst" => {
            let register = register_number(required_operand(operands, mnemonic)?)?;
            Ok(word(base | register as u16))
        }
        "lwpi" | "limi" => {
            let immediate = word_value(required_operand(operands, mnemonic)?, labels, resolve)?;
            let mut bytes = word(base);
            bytes.extend(word(immediate));
            Ok(bytes)
        }
        _ => {
            let (register, immediate) = split_operands(operands)?;
            let register = register_number(register)?;
            let immediate = word_value(immediate, labels, resolve)?;
            let mut bytes = word(base | register as u16);
            bytes.extend(word(immediate));
            Ok(bytes)
        }
    }
}

struct Operand {
    field: u8,
    extensions: Vec<u8>,
}

fn parse_operand(
    text: &str,
    labels: &HashMap<String, u32>,
    resolve: bool,
) -> Result<Operand, Diagnostic> {
    let text = text.trim();
    if let Ok(register) = register_number(text) {
        return Ok(Operand {
            field: register,
            extensions: Vec::new(),
        });
    }
    if let Some(register) = text
        .strip_prefix('*')
        .and_then(|value| value.strip_suffix('+'))
    {
        return Ok(Operand {
            field: 0x30 | register_number(register)?,
            extensions: Vec::new(),
        });
    }
    if let Some(register) = text.strip_prefix('*') {
        return Ok(Operand {
            field: 0x10 | register_number(register)?,
            extensions: Vec::new(),
        });
    }
    let symbolic = text.strip_prefix('@').ok_or_else(|| {
        Diagnostic::new(format!(
            "invalid TMS9900 operand `{text}`; expected R0..R15, *Rn, *Rn+, or @address[(Rn)]"
        ))
    })?;
    let (address, register) = if let Some((address, index)) = symbolic.rsplit_once('(') {
        let index = index
            .strip_suffix(')')
            .ok_or_else(|| Diagnostic::new(format!("invalid TMS9900 indexed operand `{text}`")))?;
        let register = register_number(index.trim())?;
        if register == 0 {
            return Err(Diagnostic::new(format!(
                "TMS9900 indexed operand `{text}` cannot use R0; use @address instead"
            )));
        }
        (address.trim(), register)
    } else {
        (symbolic.trim(), 0)
    };
    if address.is_empty() {
        return Err(Diagnostic::new(format!(
            "invalid TMS9900 symbolic operand `{text}`"
        )));
    }
    let address = word_value(address, labels, resolve)?;
    Ok(Operand {
        field: 0x20 | register,
        extensions: word(address),
    })
}

fn split_instruction(text: &str) -> Result<(&str, &str), Diagnostic> {
    let text = text.trim();
    let mnemonic_end = text.find(char::is_whitespace).unwrap_or(text.len());
    let mnemonic = &text[..mnemonic_end];
    if mnemonic.is_empty() {
        return Err(Diagnostic::new("empty TMS9900 instruction"));
    }
    Ok((mnemonic, text[mnemonic_end..].trim()))
}

fn required_operand<'a>(operands: &'a str, mnemonic: &str) -> Result<&'a str, Diagnostic> {
    if operands.is_empty() {
        Err(Diagnostic::new(format!(
            "TMS9900 {mnemonic} requires an operand"
        )))
    } else if operands.contains(',') {
        Err(Diagnostic::new(format!(
            "TMS9900 {mnemonic} has too many operands"
        )))
    } else {
        Ok(operands)
    }
}

fn split_operands(text: &str) -> Result<(&str, &str), Diagnostic> {
    text.split_once(',')
        .map(|(left, right)| (left.trim(), right.trim()))
        .filter(|(left, right)| !left.is_empty() && !right.is_empty() && !right.contains(','))
        .ok_or_else(|| Diagnostic::new(format!("invalid TMS9900 operand list `{text}`")))
}

fn register_number(text: &str) -> Result<u8, Diagnostic> {
    text.trim()
        .strip_prefix(['r', 'R'])
        .and_then(|number| number.parse::<u8>().ok())
        .filter(|register| *register < 16)
        .ok_or_else(|| Diagnostic::new(format!("invalid TMS9900 register `{text}`")))
}

fn word_value(text: &str, labels: &HashMap<String, u32>, resolve: bool) -> Result<u16, Diagnostic> {
    let value = value(text, labels, resolve)?;
    u16::try_from(value).map_err(|_| {
        Diagnostic::new(format!(
            "TMS9900 value `{text}` is outside the 16-bit address/value range"
        ))
    })
}

fn signed_value(
    text: &str,
    labels: &HashMap<String, u32>,
    resolve: bool,
) -> Result<i64, Diagnostic> {
    if !resolve {
        return Ok(0);
    }
    if let Ok(value) = text.trim().parse::<i64>() {
        return Ok(value);
    }
    Ok(value(text, labels, true)? as i64)
}

fn value(text: &str, labels: &HashMap<String, u32>, resolve: bool) -> Result<u32, Diagnostic> {
    if !resolve {
        return Ok(0);
    }
    let text = text.trim();
    if let Some(hex) = text.strip_prefix('>') {
        return u32::from_str_radix(hex, 16)
            .map_err(|_| Diagnostic::new(format!("invalid TMS9900 value `{text}`")));
    }
    if let Some(hex) = text.strip_prefix("0x") {
        return u32::from_str_radix(hex, 16)
            .map_err(|_| Diagnostic::new(format!("invalid TMS9900 value `{text}`")));
    }
    if let Some(hex) = text.strip_suffix('h') {
        return u32::from_str_radix(hex, 16)
            .map_err(|_| Diagnostic::new(format!("invalid TMS9900 value `{text}`")));
    }
    if let Ok(value) = text.parse::<u32>() {
        return Ok(value);
    }
    labels
        .iter()
        .find_map(|(name, value)| name.eq_ignore_ascii_case(text).then_some(*value))
        .ok_or_else(|| Diagnostic::new(format!("unknown TMS9900 symbol `{text}`")))
}

fn word(value: u16) -> Vec<u8> {
    value.to_be_bytes().to_vec()
}

#[cfg(test)]
mod tests {
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
}
