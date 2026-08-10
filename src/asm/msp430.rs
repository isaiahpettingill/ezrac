use crate::{compat::prelude::*, diagnostic::Diagnostic, target::AssemblerCpu};

pub fn instruction_len(cpu: AssemblerCpu, text: &str) -> Result<usize, Diagnostic> {
    Ok(encode(cpu, text, &HashMap::new(), 0, false)?.len())
}

pub fn encode_instruction(
    cpu: AssemblerCpu,
    text: &str,
    labels: &HashMap<String, u32>,
    pc: u32,
) -> Result<Vec<u8>, Diagnostic> {
    encode(cpu, text, labels, pc, true)
}

fn encode(
    cpu: AssemblerCpu,
    text: &str,
    labels: &HashMap<String, u32>,
    pc: u32,
    resolve: bool,
) -> Result<Vec<u8>, Diagnostic> {
    let (mnemonic, operands) = split_instruction(text)?;
    let mnemonic = mnemonic.to_ascii_lowercase();
    let (mnemonic, byte) = mnemonic
        .strip_suffix(".b")
        .map_or((mnemonic.as_str(), false), |m| (m, true));
    let (mnemonic, address) = mnemonic
        .strip_suffix(".a")
        .map_or((mnemonic, false), |m| (m, true));
    if address && cpu == AssemblerCpu::Msp430 {
        return Err(Diagnostic::new(
            "MSP430 address instructions require MSP430X or MSP430X2",
        ));
    }

    match mnemonic {
        "ret" => return encode(cpu, "mov @r1+,r0", labels, pc, resolve),
        "br" => return encode(cpu, &format!("mov {operands},r0"), labels, pc, resolve),
        "clr" => return encode(cpu, &format!("mov #0,{operands}"), labels, pc, resolve),
        "inc" => return encode(cpu, &format!("add #1,{operands}"), labels, pc, resolve),
        "dec" => return encode(cpu, &format!("sub #1,{operands}"), labels, pc, resolve),
        "tst" => return encode(cpu, &format!("cmp #0,{operands}"), labels, pc, resolve),
        "nop" => return Ok(words(&[0x4303])),
        "reti" => return no_operands(operands, mnemonic).map(|_| words(&[0x1300])),
        _ => {}
    }

    if let Some(base) = jump_opcode(mnemonic) {
        let target = value_for_cpu(cpu, operands, labels, resolve)?;
        let offset = if resolve {
            (i64::from(target) - i64::from(pc + 2)) / 2
        } else {
            0
        };
        if resolve && !(-512..=511).contains(&offset) {
            return Err(Diagnostic::new(format!(
                "MSP430 jump target `{operands}` is out of range"
            )));
        }
        return Ok(words(&[base | ((offset as u16) & 0x03ff)]));
    }

    if let Some(base) = single_opcode(mnemonic) {
        let operand = parse_source(cpu, operands, labels, pc + 2, resolve)?;
        let mut encoded = vec![
            base | u16::from(byte) << 6 | u16::from(operand.mode) << 4 | u16::from(operand.reg),
        ];
        encoded.extend(operand.extra);
        return Ok(words(&encoded));
    }

    let opcode = double_opcode(mnemonic).ok_or_else(|| {
        Diagnostic::new(format!(
            "assembler does not support MSP430 instruction `{text}`"
        ))
    })?;
    let (source, destination) = split_operands(operands)?;
    let source = parse_source(cpu, source, labels, pc + 2, resolve)?;
    let destination_pc = pc + 2 + (source.extra.len() as u32 * 2);
    let destination = parse_destination(cpu, destination, labels, destination_pc, resolve)?;
    let word = opcode << 12
        | u16::from(source.reg) << 8
        | u16::from(destination.mode) << 7
        | u16::from(byte) << 6
        | u16::from(source.mode) << 4
        | u16::from(destination.reg);
    let mut encoded = vec![word];
    encoded.extend(source.extra);
    encoded.extend(destination.extra);
    Ok(words(&encoded))
}

struct Operand {
    reg: u8,
    mode: u8,
    extra: Vec<u16>,
}

fn parse_source(
    cpu: AssemblerCpu,
    text: &str,
    labels: &HashMap<String, u32>,
    extension_pc: u32,
    resolve: bool,
) -> Result<Operand, Diagnostic> {
    let text = text.trim();
    if let Ok(reg) = register(text) {
        return Ok(Operand {
            reg,
            mode: 0,
            extra: vec![],
        });
    }
    if let Some(reg) = text.strip_prefix('@').and_then(|s| s.strip_suffix('+')) {
        return Ok(Operand {
            reg: register(reg)?,
            mode: 3,
            extra: vec![],
        });
    }
    if let Some(reg) = text.strip_prefix('@') {
        return Ok(Operand {
            reg: register(reg)?,
            mode: 2,
            extra: vec![],
        });
    }
    if let Some(immediate) = text.strip_prefix('#') {
        let v = value_for_cpu(cpu, immediate, labels, resolve)?;
        let known_without_resolution =
            labels.contains_key(immediate.trim()) || is_numeric_value(immediate) || resolve;
        if known_without_resolution && let Some((reg, mode)) = constant_generator(v) {
            return Ok(Operand {
                reg,
                mode,
                extra: vec![],
            });
        }
        return Ok(Operand {
            reg: 0,
            mode: 3,
            extra: vec![low_word(v)],
        });
    }
    if let Some(absolute) = text.strip_prefix('&') {
        let v = value_for_cpu(cpu, absolute, labels, resolve)?;
        return Ok(Operand {
            reg: 2,
            mode: 1,
            extra: vec![low_word(v)],
        });
    }
    if let Some((offset, reg)) = indexed(text) {
        return Ok(Operand {
            reg: register(reg)?,
            mode: 1,
            extra: vec![low_word(value_for_cpu(cpu, offset, labels, resolve)?)],
        });
    }
    let target = value_for_cpu(cpu, text, labels, resolve)?;
    let displacement = if resolve {
        target.wrapping_sub(extension_pc)
    } else {
        0
    };
    Ok(Operand {
        reg: 0,
        mode: 1,
        extra: vec![low_word(displacement)],
    })
}

fn parse_destination(
    cpu: AssemblerCpu,
    text: &str,
    labels: &HashMap<String, u32>,
    extension_pc: u32,
    resolve: bool,
) -> Result<Operand, Diagnostic> {
    let text = text.trim();
    if let Ok(reg) = register(text) {
        return Ok(Operand {
            reg,
            mode: 0,
            extra: vec![],
        });
    }
    if let Some(absolute) = text.strip_prefix('&') {
        return Ok(Operand {
            reg: 2,
            mode: 1,
            extra: vec![low_word(value_for_cpu(cpu, absolute, labels, resolve)?)],
        });
    }
    if let Some((offset, reg)) = indexed(text) {
        return Ok(Operand {
            reg: register(reg)?,
            mode: 1,
            extra: vec![low_word(value_for_cpu(cpu, offset, labels, resolve)?)],
        });
    }
    let target = value_for_cpu(cpu, text, labels, resolve)?;
    let displacement = if resolve {
        target.wrapping_sub(extension_pc)
    } else {
        0
    };
    Ok(Operand {
        reg: 0,
        mode: 1,
        extra: vec![low_word(displacement)],
    })
}

fn double_opcode(m: &str) -> Option<u16> {
    Some(match m {
        "mov" => 4,
        "add" => 5,
        "addc" => 6,
        "subc" => 7,
        "sub" => 8,
        "cmp" => 9,
        "dadd" => 10,
        "bit" => 11,
        "bic" => 12,
        "bis" => 13,
        "xor" => 14,
        "and" => 15,
        _ => return None,
    })
}
fn single_opcode(m: &str) -> Option<u16> {
    Some(match m {
        "rrc" => 0x1000,
        "swpb" => 0x1080,
        "rra" => 0x1100,
        "sxt" => 0x1180,
        "push" => 0x1200,
        "call" => 0x1280,
        _ => return None,
    })
}
fn jump_opcode(m: &str) -> Option<u16> {
    Some(match m {
        "jne" | "jnz" => 0x2000,
        "jeq" | "jz" => 0x2400,
        "jnc" | "jlo" => 0x2800,
        "jc" | "jhs" => 0x2c00,
        "jn" => 0x3000,
        "jge" => 0x3400,
        "jl" => 0x3800,
        "jmp" => 0x3c00,
        _ => return None,
    })
}
fn is_numeric_value(text: &str) -> bool {
    let text = text.trim();
    text.strip_prefix("0x")
        .or_else(|| text.strip_prefix('$'))
        .is_some_and(|digits| u32::from_str_radix(digits, 16).is_ok())
        || text.parse::<u32>().is_ok()
}

fn constant_generator(v: u32) -> Option<(u8, u8)> {
    Some(match v {
        0 => (3, 0),
        1 => (3, 1),
        2 => (3, 2),
        4 => (2, 2),
        8 => (2, 3),
        0xffff | 0xffff_ffff => (3, 3),
        _ => return None,
    })
}
fn register(text: &str) -> Result<u8, Diagnostic> {
    match text.trim().to_ascii_lowercase().as_str() {
        "pc" => Ok(0),
        "sp" => Ok(1),
        "sr" => Ok(2),
        "cg" => Ok(3),
        s => s
            .strip_prefix('r')
            .and_then(|n| n.parse().ok())
            .filter(|r| *r < 16)
            .ok_or_else(|| Diagnostic::new(format!("invalid MSP430 register `{text}`"))),
    }
}
fn indexed(text: &str) -> Option<(&str, &str)> {
    let (offset, rest) = text.rsplit_once('(')?;
    Some((offset.trim(), rest.strip_suffix(')')?.trim()))
}
fn split_instruction(text: &str) -> Result<(&str, &str), Diagnostic> {
    let text = text.trim();
    let end = text.find(char::is_whitespace).unwrap_or(text.len());
    if end == 0 {
        return Err(Diagnostic::new("empty MSP430 instruction"));
    }
    Ok((&text[..end], text[end..].trim()))
}
fn split_operands(text: &str) -> Result<(&str, &str), Diagnostic> {
    text.split_once(',')
        .map(|(a, b)| (a.trim(), b.trim()))
        .filter(|(a, b)| !a.is_empty() && !b.is_empty() && !b.contains(','))
        .ok_or_else(|| Diagnostic::new(format!("invalid MSP430 operand list `{text}`")))
}
fn no_operands(text: &str, m: &str) -> Result<(), Diagnostic> {
    if text.is_empty() {
        Ok(())
    } else {
        Err(Diagnostic::new(format!("MSP430 {m} takes no operands")))
    }
}
fn value(text: &str, labels: &HashMap<String, u32>, resolve: bool) -> Result<u32, Diagnostic> {
    let t = text.trim();
    if let Some(v) = labels.get(t) {
        return Ok(*v);
    };
    if let Some(h) = t.strip_prefix("0x").or_else(|| t.strip_prefix('$')) {
        return u32::from_str_radix(h, 16)
            .map_err(|_| Diagnostic::new(format!("invalid MSP430 value `{text}`")));
    }
    if let Ok(value) = t.parse() {
        return Ok(value);
    }
    if !resolve {
        return Ok(0);
    }
    Err(Diagnostic::new(format!(
        "unknown MSP430 symbol or value `{text}`"
    )))
}

fn value_for_cpu(
    cpu: AssemblerCpu,
    text: &str,
    labels: &HashMap<String, u32>,
    resolve: bool,
) -> Result<u32, Diagnostic> {
    let value = value(text, labels, resolve)?;
    let max = if cpu == AssemblerCpu::Msp430 {
        0xFFFF
    } else {
        0xFFFFF
    };
    if value > max && !(cpu == AssemblerCpu::Msp430 && value == u32::MAX) {
        return Err(Diagnostic::new(format!(
            "MSP430 value `{text}` is outside the {}-bit address/value range",
            if cpu == AssemblerCpu::Msp430 { 16 } else { 20 }
        )));
    }
    Ok(if value == u32::MAX { 0xFFFF } else { value })
}

fn low_word(v: u32) -> u16 {
    v as u16
}
fn words(values: &[u16]) -> Vec<u8> {
    values.iter().flat_map(|v| v.to_le_bytes()).collect()
}

#[cfg(test)]
mod tests;
