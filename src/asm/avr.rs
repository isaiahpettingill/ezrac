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
    let text = text.trim().to_ascii_lowercase();
    let text = text.as_str();
    let word = match text {
        "nop" => Some(0x0000),
        "ret" => Some(0x9508),
        "reti" => Some(0x9518),
        "ijmp" => Some(0x9409),
        "eijmp" => Some(0x9419),
        "icall" => Some(0x9509),
        "eicall" => Some(0x9519),
        "break" => Some(0x9598),
        "sleep" => Some(0x9588),
        "wdr" => Some(0x95A8),
        "lpm" => Some(0x95C8),
        "spm" => Some(0x95E8),
        "cli" => Some(0x94F8),
        "sei" => Some(0x9478),
        "clc" => Some(0x9488),
        "sec" => Some(0x9408),
        "clz" => Some(0x9498),
        "sez" => Some(0x9418),
        "cln" => Some(0x94A8),
        "sen" => Some(0x9428),
        "clv" => Some(0x94B8),
        "sev" => Some(0x9438),
        "cls" => Some(0x94C8),
        "ses" => Some(0x9448),
        "clh" => Some(0x94D8),
        "seh" => Some(0x9458),
        "clt" => Some(0x94E8),
        "set" => Some(0x9468),
        _ => None,
    };
    if let Some(word) = word {
        return Ok(word_bytes(word));
    }
    let Some((op, rest)) = split_mnemonic(text) else {
        return Err(unsupported(text));
    };
    match op {
        "ldi" => {
            let (dst, imm) = split_operands(rest)?;
            let reg = avr_reg(dst)?;
            if reg < 16 {
                return Err(Diagnostic::new("AVR ldi destination must be r16..r31"));
            }
            let k = imm8(imm, labels, pc, resolve)?;
            let d = reg - 16;
            Ok(word_bytes(
                0xE000 | ((k as u16 & 0xF0) << 4) | ((d as u16) << 4) | (k as u16 & 0x0F),
            ))
        }
        "mov" | "add" | "adc" | "sub" | "sbc" | "and" | "or" | "eor" | "cp" | "cpc" => {
            let (dst, src) = split_operands(rest)?;
            let d = avr_reg(dst)? as u16;
            let r = avr_reg(src)? as u16;
            let base = match op {
                "mov" => 0x2C00,
                "add" => 0x0C00,
                "adc" => 0x1C00,
                "sub" => 0x1800,
                "sbc" => 0x0800,
                "and" => 0x2000,
                "or" => 0x2800,
                "eor" => 0x2400,
                "cp" => 0x1400,
                "cpc" => 0x0400,
                _ => unreachable!(),
            };
            Ok(word_bytes(
                base | ((d & 0x1F) << 4) | (r & 0x0F) | ((r & 0x10) << 5),
            ))
        }
        "inc" | "dec" | "clr" | "lsl" | "tst" => {
            let r = avr_reg(rest)? as u16;
            let word = match op {
                "inc" => 0x9403 | (r << 4),
                "dec" => 0x940A | (r << 4),
                "clr" | "lsl" | "tst" => 0x2400 | (r << 4) | (r & 0x0F) | ((r & 0x10) << 5),
                _ => unreachable!(),
            };
            Ok(word_bytes(word))
        }
        "out" | "in" => {
            let (left, right) = split_operands(rest)?;
            let (a, r) = if op == "out" {
                (io_addr(left, labels, pc, resolve)?, avr_reg(right)?)
            } else {
                (io_addr(right, labels, pc, resolve)?, avr_reg(left)?)
            };
            let base = if op == "out" { 0xB800 } else { 0xB000 };
            Ok(word_bytes(
                base | (((a as u16) & 0x30) << 5) | ((r as u16) << 4) | ((a as u16) & 0x0F),
            ))
        }
        "sbi" | "cbi" => {
            let (addr, bit) = split_operands(rest)?;
            let a = io_addr(addr, labels, pc, resolve)?;
            let b = imm3(bit, labels, pc, resolve)?;
            Ok(word_bytes(
                (if op == "sbi" { 0x9A00 } else { 0x9800 }) | ((a as u16) << 3) | b as u16,
            ))
        }
        "rjmp" | "rcall" => {
            let target = value(rest, labels, pc, resolve)?;
            let next = pc.wrapping_add(2);
            let offset = ((target as i64 - next as i64) / 2) as i64;
            if !(-2048..=2047).contains(&offset) {
                return Err(Diagnostic::new(format!(
                    "AVR {op} target `{rest}` is out of range"
                )));
            }
            Ok(word_bytes(
                (if op == "rjmp" { 0xC000 } else { 0xD000 }) | ((offset as i16 as u16) & 0x0FFF),
            ))
        }
        _ => Err(unsupported(text)),
    }
}

fn word_bytes(word: u16) -> Vec<u8> {
    vec![(word & 0xFF) as u8, (word >> 8) as u8]
}
fn split_mnemonic(text: &str) -> Option<(&str, &str)> {
    text.split_once(char::is_whitespace)
        .map(|(op, rest)| (op, rest.trim()))
}
fn split_operands(text: &str) -> Result<(&str, &str), Diagnostic> {
    text.split_once(',')
        .map(|(a, b)| (a.trim(), b.trim()))
        .ok_or_else(|| Diagnostic::new(format!("invalid AVR operand list `{text}`")))
}
fn avr_reg(text: &str) -> Result<u8, Diagnostic> {
    text.strip_prefix('r')
        .and_then(|n| n.parse::<u8>().ok())
        .filter(|r| *r < 32)
        .ok_or_else(|| Diagnostic::new(format!("invalid AVR register `{text}`")))
}
fn imm8(
    text: &str,
    labels: &HashMap<String, u32>,
    pc: u32,
    resolve: bool,
) -> Result<u8, Diagnostic> {
    u8::try_from(value(text, labels, pc, resolve)?)
        .map_err(|_| Diagnostic::new(format!("AVR immediate `{text}` is outside 8-bit range")))
}
fn imm3(
    text: &str,
    labels: &HashMap<String, u32>,
    pc: u32,
    resolve: bool,
) -> Result<u8, Diagnostic> {
    let v = value(text, labels, pc, resolve)?;
    if v < 8 {
        Ok(v as u8)
    } else {
        Err(Diagnostic::new(format!("AVR bit `{text}` is outside 0..7")))
    }
}
fn io_addr(
    text: &str,
    labels: &HashMap<String, u32>,
    pc: u32,
    resolve: bool,
) -> Result<u8, Diagnostic> {
    let v = value(text, labels, pc, resolve)?;
    if v < 64 {
        Ok(v as u8)
    } else {
        Err(Diagnostic::new(format!(
            "AVR I/O address `{text}` is outside 0..63"
        )))
    }
}
fn value(
    text: &str,
    labels: &HashMap<String, u32>,
    _pc: u32,
    resolve: bool,
) -> Result<u32, Diagnostic> {
    if !resolve {
        return Ok(0);
    }
    let t = text.trim().trim_start_matches('#');
    if let Some(hex) = t.strip_suffix('h') {
        return u32::from_str_radix(hex, 16)
            .map_err(|_| Diagnostic::new(format!("invalid AVR value `{text}`")));
    }
    if let Some(hex) = t.strip_prefix("0x") {
        return u32::from_str_radix(hex, 16)
            .map_err(|_| Diagnostic::new(format!("invalid AVR value `{text}`")));
    }
    if let Ok(v) = t.parse::<u32>() {
        return Ok(v);
    }
    labels
        .get(t)
        .copied()
        .ok_or_else(|| Diagnostic::new(format!("unknown AVR symbol `{text}`")))
}
fn unsupported(text: &str) -> Diagnostic {
    Diagnostic::new(format!(
        "assembler does not support AVR instruction `{text}`"
    ))
}
