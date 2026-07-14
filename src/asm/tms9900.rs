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
        let mut bytes = word(base | ((source.field as u16) << 6) | destination.field as u16);
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
        let (count, register) = split_operands(operands)?;
        let count = value(count, labels, resolve)?;
        if count > 15 {
            return Err(Diagnostic::new(format!(
                "TMS9900 shift count `{count}` is outside 0..15"
            )));
        }
        let register = register_number(register)?;
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

    if matches!(mnemonic.as_str(), "mpy" | "div") {
        let (source, destination) = split_operands(operands)?;
        let source = parse_operand(source, labels, resolve)?;
        let destination = register_number(destination)?;
        let base = if mnemonic == "mpy" { 0x3800 } else { 0x3c00 };
        let mut bytes = word(base | ((source.field as u16) << 6) | destination as u16);
        bytes.extend(source.extensions);
        return Ok(bytes);
    }

    match mnemonic.as_str() {
        "nop" if operands.is_empty() => Ok(word(0x1000)),
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
        (address.trim(), register_number(index.trim())?)
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

    #[test]
    fn encodes_core_instruction_formats() {
        assert_eq!(
            encode_instruction("li r1, >1234", &HashMap::new(), 0).unwrap(),
            [0x02, 0x01, 0x12, 0x34]
        );
        assert_eq!(
            encode_instruction("mov r1, *r2+", &HashMap::new(), 0).unwrap(),
            [0xc0, 0x72]
        );
        assert_eq!(
            encode_instruction("a @>8300(r4), r5", &HashMap::new(), 0).unwrap(),
            [0xa9, 0x05, 0x83, 0x00]
        );
        assert_eq!(
            encode_instruction("sra 4, r6", &HashMap::new(), 0).unwrap(),
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
}
