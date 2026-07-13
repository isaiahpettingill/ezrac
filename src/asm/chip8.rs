use std::collections::HashMap;

use crate::diagnostic::Diagnostic;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Chip8Dialect {
    Chip8,
    SuperChip,
    XoChip,
}

impl Chip8Dialect {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Chip8 => "chip8",
            Self::SuperChip => "schip",
            Self::XoChip => "xochip",
        }
    }

    fn supports_super(self) -> bool {
        matches!(self, Self::SuperChip | Self::XoChip)
    }

    fn supports_xo(self) -> bool {
        self == Self::XoChip
    }
}

pub fn instruction_len(dialect: Chip8Dialect, text: &str) -> Result<usize, Diagnostic> {
    let text = normalize(text);
    if text.starts_with("long i, ") {
        if dialect.supports_xo() {
            return Ok(4);
        }
        return Err(unsupported(dialect, &text));
    }
    Ok(2)
}

pub fn encode_instruction(
    dialect: Chip8Dialect,
    text: &str,
    labels: &HashMap<String, u32>,
    pc: u32,
) -> Result<Vec<u8>, Diagnostic> {
    let text = normalize(text);
    if let Some(addr) = text.strip_prefix("long i, ") {
        if !dialect.supports_xo() {
            return Err(unsupported(dialect, &text));
        }
        let value = value(addr, labels, pc)?;
        if value > 0xFFFF {
            return Err(Diagnostic::new(format!(
                "XO-CHIP long I address 0x{value:X} is outside 16-bit range"
            )));
        }
        return Ok(vec![0xF0, 0x00, (value >> 8) as u8, value as u8]);
    }
    let word = encode_word(dialect, &text, labels, pc)?;
    Ok(vec![(word >> 8) as u8, word as u8])
}

fn encode_word(
    dialect: Chip8Dialect,
    text: &str,
    labels: &HashMap<String, u32>,
    pc: u32,
) -> Result<u16, Diagnostic> {
    match text {
        "cls" => return Ok(0x00E0),
        "ret" => return Ok(0x00EE),
        "scroll-right" | "scr" if dialect.supports_super() => return Ok(0x00FB),
        "scroll-left" | "scl" if dialect.supports_super() => return Ok(0x00FC),
        "exit" if dialect.supports_super() => return Ok(0x00FD),
        "low" if dialect.supports_super() => return Ok(0x00FE),
        "high" if dialect.supports_super() => return Ok(0x00FF),
        "audio" if dialect.supports_xo() => return Ok(0xF002),
        _ => {}
    }

    let Some((op, operands)) = split_mnemonic(text) else {
        return Err(unsupported(dialect, text));
    };
    let args = operands.split(',').map(str::trim).collect::<Vec<_>>();
    match (op, args.as_slice()) {
        ("scroll-down" | "scd", [n]) if dialect.supports_super() => Ok(0x00C0 | nibble(n)?),
        ("scroll-up" | "scu", [n]) if dialect.supports_xo() => Ok(0x00D0 | nibble(n)?),
        ("sys", [addr]) => Ok(0x0000 | addr12(addr, labels, pc)?),
        ("jp", [addr]) | ("jump", [addr]) => Ok(0x1000 | addr12(addr, labels, pc)?),
        ("call", [addr]) => Ok(0x2000 | addr12(addr, labels, pc)?),
        ("se", [x, y]) if register(x).is_some() && register(y).is_some() => {
            Ok(0x5000 | reg_pair(x, y)? << 4)
        }
        ("se", [x, kk]) => Ok(0x3000 | (register_req(x)? << 8) | byte(kk, labels, pc)?),
        ("sne", [x, y]) if register(x).is_some() && register(y).is_some() => {
            Ok(0x9000 | reg_pair(x, y)? << 4)
        }
        ("sne", [x, kk]) => Ok(0x4000 | (register_req(x)? << 8) | byte(kk, labels, pc)?),
        ("ld", [x, y]) if register(x).is_some() && register(y).is_some() => {
            Ok(0x8000 | reg_pair(x, y)?)
        }
        ("ld", [x, kk]) if register(x).is_some() => {
            Ok(0x6000 | (register_req(x)? << 8) | byte(kk, labels, pc)?)
        }
        ("add", [x, kk]) if register(x).is_some() => {
            Ok(0x7000 | (register_req(x)? << 8) | byte(kk, labels, pc)?)
        }
        ("or", [x, y]) => Ok(0x8001 | reg_pair(x, y)?),
        ("and", [x, y]) => Ok(0x8002 | reg_pair(x, y)?),
        ("xor", [x, y]) => Ok(0x8003 | reg_pair(x, y)?),
        ("sub", [x, y]) => Ok(0x8005 | reg_pair(x, y)?),
        ("shr", [x]) => Ok(0x8006 | (register_req(x)? << 8)),
        ("shr", [x, y]) => Ok(0x8006 | reg_pair(x, y)?),
        ("subn", [x, y]) => Ok(0x8007 | reg_pair(x, y)?),
        ("shl", [x]) => Ok(0x800E | (register_req(x)? << 8)),
        ("shl", [x, y]) => Ok(0x800E | reg_pair(x, y)?),
        ("ld", ["i", addr]) => Ok(0xA000 | addr12(addr, labels, pc)?),
        ("jp", ["v0", addr]) => Ok(0xB000 | addr12(addr, labels, pc)?),
        ("rnd", [x, kk]) => Ok(0xC000 | (register_req(x)? << 8) | byte(kk, labels, pc)?),
        ("drw", [x, y, n]) => Ok(0xD000 | reg_pair(x, y)? | nibble(n)?),
        ("skp", [x]) => Ok(0xE09E | (register_req(x)? << 8)),
        ("sknp", [x]) => Ok(0xE0A1 | (register_req(x)? << 8)),
        ("ld", [x, "dt"]) => Ok(0xF007 | (register_req(x)? << 8)),
        ("ld", [x, "k"]) => Ok(0xF00A | (register_req(x)? << 8)),
        ("ld", ["dt", x]) => Ok(0xF015 | (register_req(x)? << 8)),
        ("ld", ["st", x]) => Ok(0xF018 | (register_req(x)? << 8)),
        ("add", ["i", x]) => Ok(0xF01E | (register_req(x)? << 8)),
        ("ld", ["f", x]) => Ok(0xF029 | (register_req(x)? << 8)),
        ("ld", ["hf", x]) if dialect.supports_super() => Ok(0xF030 | (register_req(x)? << 8)),
        ("ld", ["b", x]) => Ok(0xF033 | (register_req(x)? << 8)),
        ("ld", ["r", x]) if dialect.supports_super() => Ok(0xF075 | (register_req(x)? << 8)),
        ("ld", [x, "r"]) if dialect.supports_super() => Ok(0xF085 | (register_req(x)? << 8)),
        ("save", [x]) | ("ld", ["[i]", x]) => Ok(0xF055 | (register_req(x)? << 8)),
        ("load", [x]) | ("ld", [x, "[i]"]) => Ok(0xF065 | (register_req(x)? << 8)),
        ("plane", [x]) if dialect.supports_xo() => Ok(0xF001 | (register_req(x)? << 8)),
        ("audio", []) if dialect.supports_xo() => Ok(0xF002),
        ("pitch", [x]) if dialect.supports_xo() => Ok(0xF03A | (register_req(x)? << 8)),
        _ => Err(unsupported(dialect, text)),
    }
}

fn normalize(text: &str) -> String {
    text.trim().to_ascii_lowercase().replace("[ i ]", "[i]")
}

fn split_mnemonic(text: &str) -> Option<(&str, &str)> {
    text.split_once(char::is_whitespace)
        .map(|(op, rest)| (op, rest.trim()))
}

fn register(value: &str) -> Option<u16> {
    let rest = value.strip_prefix('v')?;
    u16::from_str_radix(rest, 16).ok().filter(|v| *v <= 0xF)
}

fn register_req(value: &str) -> Result<u16, Diagnostic> {
    register(value).ok_or_else(|| Diagnostic::new(format!("invalid CHIP-8 register `{value}`")))
}

fn reg_pair(x: &str, y: &str) -> Result<u16, Diagnostic> {
    Ok((register_req(x)? << 8) | (register_req(y)? << 4))
}

fn nibble(value_text: &str) -> Result<u16, Diagnostic> {
    let value = number(value_text)?;
    if value > 0xF {
        return Err(Diagnostic::new(format!(
            "nibble value 0x{value:X} is outside 0..0xF"
        )));
    }
    Ok(value as u16)
}

fn byte(text: &str, labels: &HashMap<String, u32>, pc: u32) -> Result<u16, Diagnostic> {
    let value = value(text, labels, pc)?;
    if value > 0xFF {
        return Err(Diagnostic::new(format!(
            "byte value 0x{value:X} is outside 0..0xFF"
        )));
    }
    Ok(value as u16)
}

fn addr12(text: &str, labels: &HashMap<String, u32>, pc: u32) -> Result<u16, Diagnostic> {
    let value = value(text, labels, pc)?;
    if value > 0xFFF {
        return Err(Diagnostic::new(format!(
            "CHIP-8 address 0x{value:X} is outside 12-bit range"
        )));
    }
    Ok(value as u16)
}

fn value(text: &str, labels: &HashMap<String, u32>, pc: u32) -> Result<u32, Diagnostic> {
    let text = text.trim();
    if text == "$" {
        return Ok(pc);
    }
    if let Some(value) = labels.get(text).copied() {
        return Ok(value);
    }
    if let Some(value) = labels
        .iter()
        .find_map(|(name, value)| name.eq_ignore_ascii_case(text).then_some(*value))
    {
        return Ok(value);
    }
    number(text)
}

fn number(text: &str) -> Result<u32, Diagnostic> {
    let text = text.trim();
    let parsed = if let Some(hex) = text.strip_prefix("0x") {
        u32::from_str_radix(hex, 16)
    } else if let Some(hex) = text.strip_suffix('h') {
        u32::from_str_radix(hex, 16)
    } else {
        text.parse()
    };
    parsed.map_err(|_| Diagnostic::new(format!("invalid CHIP-8 numeric operand `{text}`")))
}

fn unsupported(dialect: Chip8Dialect, text: &str) -> Diagnostic {
    Diagnostic::new(format!(
        "{} assembler does not support instruction `{text}`",
        dialect.as_str()
    ))
}
