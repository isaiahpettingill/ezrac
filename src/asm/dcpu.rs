use std::collections::HashMap;

use crate::diagnostic::Diagnostic;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Position {
    A,
    B,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct Operand {
    code: u16,
    extra: Option<u16>,
}

pub fn instruction_len(text: &str) -> Result<usize, Diagnostic> {
    let lowered = normalize(text);
    let (mnemonic, operands) = parse_instruction(&lowered)?;
    if mnemonic == "jsr" {
        let operand = parse_operand(operands.trim(), Position::A, &HashMap::new(), 0, false)?;
        return Ok(2 + usize::from(operand.extra.is_some()) * 2);
    }
    let opcode = basic_opcode(mnemonic).ok_or_else(|| {
        Diagnostic::new(format!(
            "assembler does not support DCPU instruction `{text}`"
        ))
    })?;
    let (b_text, a_text) = split_operands(operands).ok_or_else(|| {
        Diagnostic::new(format!(
            "DCPU instruction `{mnemonic}` expects two operands"
        ))
    })?;
    let b = parse_operand(b_text, Position::B, &HashMap::new(), 0, false)?;
    let a = parse_operand(a_text, Position::A, &HashMap::new(), 0, false)?;
    let _ = opcode;
    Ok((1 + usize::from(b.extra.is_some()) + usize::from(a.extra.is_some())) * 2)
}

pub fn encode_instruction(
    text: &str,
    labels: &HashMap<String, u32>,
    pc: u32,
) -> Result<Vec<u8>, Diagnostic> {
    let lowered = normalize(text);
    let (mnemonic, operands) = parse_instruction(&lowered)?;
    let mut words = Vec::new();
    if mnemonic == "jsr" {
        let operand = parse_operand(operands.trim(), Position::A, labels, pc, true)?;
        words.push((operand.code << 10) | (0x01 << 5));
        if let Some(extra) = operand.extra {
            words.push(extra);
        }
        return Ok(words_to_bytes(&words));
    }
    let opcode = basic_opcode(mnemonic).ok_or_else(|| {
        Diagnostic::new(format!(
            "assembler does not support DCPU instruction `{text}`"
        ))
    })?;
    let (b_text, a_text) = split_operands(operands).ok_or_else(|| {
        Diagnostic::new(format!(
            "DCPU instruction `{mnemonic}` expects two operands"
        ))
    })?;
    let b = parse_operand(b_text, Position::B, labels, pc, true)?;
    let a = parse_operand(a_text, Position::A, labels, pc, true)?;
    words.push(opcode | (b.code << 5) | (a.code << 10));
    if let Some(extra) = b.extra {
        words.push(extra);
    }
    if let Some(extra) = a.extra {
        words.push(extra);
    }
    Ok(words_to_bytes(&words))
}

fn normalize(text: &str) -> String {
    text.trim().to_ascii_lowercase()
}

fn parse_instruction(text: &str) -> Result<(&str, &str), Diagnostic> {
    let (mnemonic, operands) = text
        .split_once(char::is_whitespace)
        .map(|(mnemonic, operands)| (mnemonic, operands.trim()))
        .unwrap_or((text, ""));
    if mnemonic.is_empty() {
        return Err(Diagnostic::new("empty DCPU instruction"));
    }
    Ok((mnemonic, operands))
}

fn split_operands(operands: &str) -> Option<(&str, &str)> {
    let (left, right) = operands.split_once(',')?;
    Some((left.trim(), right.trim()))
}

fn basic_opcode(mnemonic: &str) -> Option<u16> {
    Some(match mnemonic {
        "set" => 0x01,
        "add" => 0x02,
        "sub" => 0x03,
        "mul" => 0x04,
        "mli" => 0x05,
        "div" => 0x06,
        "dvi" => 0x07,
        "mod" => 0x08,
        "mdi" => 0x09,
        "and" => 0x0a,
        "bor" => 0x0b,
        "xor" => 0x0c,
        "shr" => 0x0d,
        "asr" => 0x0e,
        "shl" => 0x0f,
        "ifb" => 0x10,
        "ifc" => 0x11,
        "ife" => 0x12,
        "ifn" => 0x13,
        "ifg" => 0x14,
        "ifa" => 0x15,
        "ifl" => 0x16,
        "ifu" => 0x17,
        "adx" => 0x1a,
        "sbx" => 0x1b,
        "sti" => 0x1e,
        "std" => 0x1f,
        _ => return None,
    })
}

fn parse_operand(
    operand: &str,
    position: Position,
    labels: &HashMap<String, u32>,
    pc: u32,
    resolve_labels: bool,
) -> Result<Operand, Diagnostic> {
    let registers = ["a", "b", "c", "x", "y", "z", "i", "j"];
    if let Some(index) = registers.iter().position(|register| *register == operand) {
        return Ok(Operand {
            code: index as u16,
            extra: None,
        });
    }
    if let Some(inner) = operand.strip_prefix('[').and_then(|s| s.strip_suffix(']')) {
        let inner = inner.trim();
        if let Some(index) = registers.iter().position(|register| *register == inner) {
            return Ok(Operand {
                code: 0x08 + index as u16,
                extra: None,
            });
        }
        for (index, register) in registers.iter().enumerate() {
            let plus_reg = format!("+{register}");
            if let Some(value) = inner.strip_suffix(&plus_reg) {
                return Ok(Operand {
                    code: 0x10 + index as u16,
                    extra: Some(value16(value.trim(), labels, pc, resolve_labels)?),
                });
            }
            let reg_plus = format!("{register}+");
            if let Some(value) = inner.strip_prefix(&reg_plus) {
                return Ok(Operand {
                    code: 0x10 + index as u16,
                    extra: Some(value16(value.trim(), labels, pc, resolve_labels)?),
                });
            }
        }
        return Ok(Operand {
            code: 0x1e,
            extra: Some(value16(inner, labels, pc, resolve_labels)?),
        });
    }
    let code = match operand {
        "push" if position == Position::B => Some(0x18),
        "pop" if position == Position::A => Some(0x18),
        "peek" => Some(0x19),
        "sp" => Some(0x1b),
        "pc" => Some(0x1c),
        "ex" => Some(0x1d),
        _ => None,
    };
    if let Some(code) = code {
        return Ok(Operand { code, extra: None });
    }
    let force_next_word =
        labels.contains_key(operand) || (!resolve_labels && !is_numeric_literal(operand));
    let value = value16(operand, labels, pc, resolve_labels)?;
    if force_next_word {
        Ok(Operand {
            code: 0x1f,
            extra: Some(value),
        })
    } else if value <= 30 {
        Ok(Operand {
            code: 0x21 + value,
            extra: None,
        })
    } else if value == 0xffff {
        Ok(Operand {
            code: 0x20,
            extra: None,
        })
    } else {
        Ok(Operand {
            code: 0x1f,
            extra: Some(value),
        })
    }
}

fn value16(
    text: &str,
    labels: &HashMap<String, u32>,
    _pc: u32,
    resolve_labels: bool,
) -> Result<u16, Diagnostic> {
    let text = text.trim();
    if let Some(value) = labels.get(text) {
        return Ok((*value / 2) as u16);
    }
    match parse_numeric_literal(text) {
        Ok(value) => Ok(value as u16),
        Err(_) if !resolve_labels => Ok(0),
        Err(_) => Err(Diagnostic::new(format!("unknown DCPU operand `{text}`"))),
    }
}

fn is_numeric_literal(text: &str) -> bool {
    parse_numeric_literal(text).is_ok()
}

fn parse_numeric_literal(text: &str) -> Result<u32, std::num::ParseIntError> {
    if let Some(hex) = text.strip_prefix("0x") {
        u32::from_str_radix(hex, 16)
    } else if let Some(hex) = text.strip_suffix('h') {
        u32::from_str_radix(hex, 16)
    } else {
        text.parse::<u32>()
    }
}

fn words_to_bytes(words: &[u16]) -> Vec<u8> {
    words.iter().flat_map(|word| word.to_le_bytes()).collect()
}
