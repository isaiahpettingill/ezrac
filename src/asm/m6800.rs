use std::collections::HashMap;

use crate::diagnostic::Diagnostic;
use crate::target::Address24;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum AddrMode {
    Inherent,
    Relative,
    Immediate,
    Direct,
    Indexed,
    Extended,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct Opcode {
    mnemonic: &'static str,
    mode: AddrMode,
    opcode: u8,
    imm16: bool,
}

const OPS: &[Opcode] = &[
    op("nop", AddrMode::Inherent, 0x01),
    op("tap", AddrMode::Inherent, 0x06),
    op("tpa", AddrMode::Inherent, 0x07),
    op("inx", AddrMode::Inherent, 0x08),
    op("dex", AddrMode::Inherent, 0x09),
    op("clv", AddrMode::Inherent, 0x0A),
    op("sev", AddrMode::Inherent, 0x0B),
    op("clc", AddrMode::Inherent, 0x0C),
    op("sec", AddrMode::Inherent, 0x0D),
    op("cli", AddrMode::Inherent, 0x0E),
    op("sei", AddrMode::Inherent, 0x0F),
    op("sba", AddrMode::Inherent, 0x10),
    op("cba", AddrMode::Inherent, 0x11),
    op("tab", AddrMode::Inherent, 0x16),
    op("tba", AddrMode::Inherent, 0x17),
    op("daa", AddrMode::Inherent, 0x19),
    op("aba", AddrMode::Inherent, 0x1B),
    op("tsx", AddrMode::Inherent, 0x30),
    op("ins", AddrMode::Inherent, 0x31),
    op("pula", AddrMode::Inherent, 0x32),
    op("pulb", AddrMode::Inherent, 0x33),
    op("des", AddrMode::Inherent, 0x34),
    op("txs", AddrMode::Inherent, 0x35),
    op("psha", AddrMode::Inherent, 0x36),
    op("pshb", AddrMode::Inherent, 0x37),
    op("rts", AddrMode::Inherent, 0x39),
    op("rti", AddrMode::Inherent, 0x3B),
    op("wai", AddrMode::Inherent, 0x3E),
    op("swi", AddrMode::Inherent, 0x3F),
    op("nega", AddrMode::Inherent, 0x40),
    op("coma", AddrMode::Inherent, 0x43),
    op("lsra", AddrMode::Inherent, 0x44),
    op("rora", AddrMode::Inherent, 0x46),
    op("asra", AddrMode::Inherent, 0x47),
    op("asla", AddrMode::Inherent, 0x48),
    op("lsla", AddrMode::Inherent, 0x48),
    op("rola", AddrMode::Inherent, 0x49),
    op("deca", AddrMode::Inherent, 0x4A),
    op("inca", AddrMode::Inherent, 0x4C),
    op("tsta", AddrMode::Inherent, 0x4D),
    op("clra", AddrMode::Inherent, 0x4F),
    op("negb", AddrMode::Inherent, 0x50),
    op("comb", AddrMode::Inherent, 0x53),
    op("lsrb", AddrMode::Inherent, 0x54),
    op("rorb", AddrMode::Inherent, 0x56),
    op("asrb", AddrMode::Inherent, 0x57),
    op("aslb", AddrMode::Inherent, 0x58),
    op("lslb", AddrMode::Inherent, 0x58),
    op("rolb", AddrMode::Inherent, 0x59),
    op("decb", AddrMode::Inherent, 0x5A),
    op("incb", AddrMode::Inherent, 0x5C),
    op("tstb", AddrMode::Inherent, 0x5D),
    op("clrb", AddrMode::Inherent, 0x5F),
    op("bra", AddrMode::Relative, 0x20),
    op("brn", AddrMode::Relative, 0x21),
    op("bhi", AddrMode::Relative, 0x22),
    op("bls", AddrMode::Relative, 0x23),
    op("bcc", AddrMode::Relative, 0x24),
    op("bhs", AddrMode::Relative, 0x24),
    op("bcs", AddrMode::Relative, 0x25),
    op("blo", AddrMode::Relative, 0x25),
    op("bne", AddrMode::Relative, 0x26),
    op("beq", AddrMode::Relative, 0x27),
    op("bvc", AddrMode::Relative, 0x28),
    op("bvs", AddrMode::Relative, 0x29),
    op("bpl", AddrMode::Relative, 0x2A),
    op("bmi", AddrMode::Relative, 0x2B),
    op("bge", AddrMode::Relative, 0x2C),
    op("blt", AddrMode::Relative, 0x2D),
    op("bgt", AddrMode::Relative, 0x2E),
    op("ble", AddrMode::Relative, 0x2F),
    op("bsr", AddrMode::Relative, 0x8D),
];

const fn op(mnemonic: &'static str, mode: AddrMode, opcode: u8) -> Opcode {
    Opcode {
        mnemonic,
        mode,
        opcode,
        imm16: false,
    }
}
fn generated(mnemonic: &str, mode: AddrMode) -> Option<Opcode> {
    let mnemonic = canonical_mnemonic(mnemonic)?;
    if mnemonic == "jsr" {
        return match mode {
            AddrMode::Direct => Some(op(mnemonic, mode, 0x9D)),
            AddrMode::Indexed => Some(op(mnemonic, mode, 0xAD)),
            AddrMode::Extended => Some(op(mnemonic, mode, 0xBD)),
            _ => None,
        };
    }
    OPS.iter()
        .copied()
        .find(|op| op.mnemonic == mnemonic && op.mode == mode)
        .or_else(|| gen_mem(mnemonic, mode))
        .or_else(|| gen_acc(mnemonic, mode))
}

fn canonical_mnemonic(m: &str) -> Option<&'static str> {
    Some(match m {
        "nop" => "nop",
        "tap" => "tap",
        "tpa" => "tpa",
        "inx" => "inx",
        "dex" => "dex",
        "clv" => "clv",
        "sev" => "sev",
        "clc" => "clc",
        "sec" => "sec",
        "cli" => "cli",
        "sei" => "sei",
        "sba" => "sba",
        "cba" => "cba",
        "tab" => "tab",
        "tba" => "tba",
        "daa" => "daa",
        "aba" => "aba",
        "tsx" => "tsx",
        "ins" => "ins",
        "pula" => "pula",
        "pulb" => "pulb",
        "des" => "des",
        "txs" => "txs",
        "psha" => "psha",
        "pshb" => "pshb",
        "rts" => "rts",
        "rti" => "rti",
        "wai" => "wai",
        "swi" => "swi",
        "nega" => "nega",
        "coma" => "coma",
        "lsra" => "lsra",
        "rora" => "rora",
        "asra" => "asra",
        "asla" => "asla",
        "lsla" => "lsla",
        "rola" => "rola",
        "deca" => "deca",
        "inca" => "inca",
        "tsta" => "tsta",
        "clra" => "clra",
        "negb" => "negb",
        "comb" => "comb",
        "lsrb" => "lsrb",
        "rorb" => "rorb",
        "asrb" => "asrb",
        "aslb" => "aslb",
        "lslb" => "lslb",
        "rolb" => "rolb",
        "decb" => "decb",
        "incb" => "incb",
        "tstb" => "tstb",
        "clrb" => "clrb",
        "bra" => "bra",
        "brn" => "brn",
        "bhi" => "bhi",
        "bls" => "bls",
        "bcc" => "bcc",
        "bhs" => "bhs",
        "bcs" => "bcs",
        "blo" => "blo",
        "bne" => "bne",
        "beq" => "beq",
        "bvc" => "bvc",
        "bvs" => "bvs",
        "bpl" => "bpl",
        "bmi" => "bmi",
        "bge" => "bge",
        "blt" => "blt",
        "bgt" => "bgt",
        "ble" => "ble",
        "bsr" => "bsr",
        "neg" => "neg",
        "com" => "com",
        "lsr" => "lsr",
        "ror" => "ror",
        "asr" => "asr",
        "asl" => "asl",
        "lsl" => "lsl",
        "rol" => "rol",
        "dec" => "dec",
        "inc" => "inc",
        "tst" => "tst",
        "jmp" => "jmp",
        "jsr" => "jsr",
        "clr" => "clr",
        "suba" => "suba",
        "cmpa" => "cmpa",
        "sbca" => "sbca",
        "anda" => "anda",
        "bita" => "bita",
        "ldaa" => "ldaa",
        "staa" => "staa",
        "eora" => "eora",
        "adca" => "adca",
        "oraa" => "oraa",
        "adda" => "adda",
        "cpx" => "cpx",
        "lds" => "lds",
        "sts" => "sts",
        "subb" => "subb",
        "cmpb" => "cmpb",
        "sbcb" => "sbcb",
        "andb" => "andb",
        "bitb" => "bitb",
        "ldab" => "ldab",
        "stab" => "stab",
        "eorb" => "eorb",
        "adcb" => "adcb",
        "orab" => "orab",
        "addb" => "addb",
        "ldx" => "ldx",
        "stx" => "stx",
        _ => return None,
    })
}

fn gen_mem(m: &'static str, mode: AddrMode) -> Option<Opcode> {
    let off = match m {
        "neg" => 0x00,
        "com" => 0x03,
        "lsr" => 0x04,
        "ror" => 0x06,
        "asr" => 0x07,
        "asl" | "lsl" => 0x08,
        "rol" => 0x09,
        "dec" => 0x0A,
        "inc" => 0x0C,
        "tst" => 0x0D,
        "jmp" => 0x0E,
        "clr" => 0x0F,
        _ => return None,
    };
    match mode {
        AddrMode::Indexed => Some(op(m, mode, 0x60 + off)),
        AddrMode::Extended => Some(op(m, mode, 0x70 + off)),
        _ => None,
    }
}

fn gen_acc(m: &'static str, mode: AddrMode) -> Option<Opcode> {
    let (base, imm16) = match m {
        "suba" => (0x80, false),
        "cmpa" => (0x81, false),
        "sbca" => (0x82, false),
        "anda" => (0x84, false),
        "bita" => (0x85, false),
        "ldaa" => (0x86, false),
        "staa" => (0x87, false),
        "eora" => (0x88, false),
        "adca" => (0x89, false),
        "oraa" => (0x8A, false),
        "adda" => (0x8B, false),
        "cpx" => (0x8C, true),
        "lds" => (0x8E, true),
        "sts" => (0x8F, true),
        "subb" => (0xC0, false),
        "cmpb" => (0xC1, false),
        "sbcb" => (0xC2, false),
        "andb" => (0xC4, false),
        "bitb" => (0xC5, false),
        "ldab" => (0xC6, false),
        "stab" => (0xC7, false),
        "eorb" => (0xC8, false),
        "adcb" => (0xC9, false),
        "orab" => (0xCA, false),
        "addb" => (0xCB, false),
        "ldx" => (0xCE, true),
        "stx" => (0xCF, true),
        _ => return None,
    };
    let add = match mode {
        AddrMode::Immediate if !m.starts_with("st") => 0x00,
        AddrMode::Direct => 0x10,
        AddrMode::Indexed => 0x20,
        AddrMode::Extended => 0x30,
        _ => return None,
    };
    Some(Opcode {
        mnemonic: m,
        mode,
        opcode: base + add,
        imm16,
    })
}

pub fn instruction_len(text: &str) -> Result<Option<usize>, Diagnostic> {
    Ok(analyze(text)?.map(|(op, _)| 1 + operand_len(op)))
}

pub fn emit_instruction(
    text: &str,
    labels: &HashMap<String, u32>,
    pc: u32,
) -> Result<Option<Vec<u8>>, Diagnostic> {
    let Some((op, operand)) = analyze(text)? else {
        return Ok(None);
    };
    let mut out = vec![op.opcode];
    match op.mode {
        AddrMode::Inherent => {}
        AddrMode::Relative => out.push(rel8(pc, eval(&operand, labels, pc)?)?),
        AddrMode::Immediate => push_operand(
            &mut out,
            eval(&operand, labels, pc)?,
            if op.imm16 { 2 } else { 1 },
        )?,
        AddrMode::Direct => push_operand(&mut out, eval(&operand, labels, pc)?, 1)?,
        AddrMode::Indexed => push_operand(&mut out, eval(index_expr(&operand), labels, pc)?, 1)?,
        AddrMode::Extended => push_operand(&mut out, eval(strip_force(&operand), labels, pc)?, 2)?,
    }
    Ok(Some(out))
}

fn analyze(text: &str) -> Result<Option<(Opcode, String)>, Diagnostic> {
    let text = text.trim().to_ascii_lowercase();
    let (mnemonic, rest) = text
        .split_once(char::is_whitespace)
        .map_or((text.as_str(), ""), |(m, r)| (m, r.trim()));
    if rest.is_empty() {
        return Ok(generated(mnemonic, AddrMode::Inherent).map(|op| (op, String::new())));
    }
    if let Some(op) = generated(mnemonic, AddrMode::Relative) {
        return Ok(Some((op, rest.to_owned())));
    }
    let (mode, operand) = if let Some(imm) = rest.strip_prefix('#') {
        (AddrMode::Immediate, imm.trim())
    } else if let Some((offset, index)) = rest.rsplit_once(',')
        && index.trim() == "x"
    {
        let offset = offset.trim();
        if offset.is_empty() {
            return Err(Diagnostic::new(
                "M6800 indexed operand is missing its offset",
            ));
        }
        (AddrMode::Indexed, offset)
    } else if let Some(operand) = rest.strip_prefix('<') {
        (AddrMode::Direct, operand.trim())
    } else if let Some(operand) = rest.strip_prefix('>') {
        (AddrMode::Extended, operand.trim())
    } else if prefer_direct(mnemonic, rest) {
        (AddrMode::Direct, rest)
    } else {
        (AddrMode::Extended, rest)
    };
    Ok(generated(mnemonic, mode).map(|op| (op, operand.to_owned())))
}

fn prefer_direct(_mnemonic: &str, operand: &str) -> bool {
    if operand.starts_with("$+") || operand.starts_with("$-") {
        return false;
    }
    parse_number(operand).is_ok_and(|value| value <= 0xFF)
}
fn operand_len(op: Opcode) -> usize {
    match op.mode {
        AddrMode::Inherent => 0,
        AddrMode::Relative | AddrMode::Direct | AddrMode::Indexed => 1,
        AddrMode::Immediate => {
            if op.imm16 {
                2
            } else {
                1
            }
        }
        AddrMode::Extended => 2,
    }
}
fn index_expr(operand: &str) -> &str {
    operand
        .trim()
        .strip_prefix('<')
        .or_else(|| operand.trim().strip_prefix('>'))
        .unwrap_or(operand.trim())
}
fn strip_force(operand: &str) -> &str {
    operand
        .trim()
        .strip_prefix('<')
        .or_else(|| operand.trim().strip_prefix('>'))
        .unwrap_or(operand.trim())
}

fn validate_address16(kind: &str, value: u32) -> Result<(), Diagnostic> {
    if value > 0xFFFF {
        return Err(Diagnostic::new(format!(
            "M6800 {kind} 0x{value:X} is outside the 16-bit address space"
        )));
    }
    Ok(())
}

fn push_operand(out: &mut Vec<u8>, value: u32, width: usize) -> Result<(), Diagnostic> {
    match width {
        1 => {
            if value > 0xFF {
                return Err(Diagnostic::new(format!(
                    "M6800 operand 0x{value:X} is outside u8 range"
                )));
            }
            out.push(value as u8);
        }
        2 => {
            if value > 0xFFFF {
                return Err(Diagnostic::new(format!(
                    "M6800 operand 0x{value:X} is outside u16 range"
                )));
            }
            out.push((value >> 8) as u8);
            out.push(value as u8);
        }
        _ => unreachable!(),
    }
    Ok(())
}
fn rel8(pc: u32, target: u32) -> Result<u8, Diagnostic> {
    validate_address16("branch address", pc)?;
    validate_address16("branch target", target)?;
    let next = pc + 2;
    let off = target as i64 - next as i64;
    if !(-128..=127).contains(&off) {
        return Err(Diagnostic::new(format!(
            "M6800 relative branch target 0x{target:04X} is out of range from 0x{pc:04X}"
        )));
    }
    Ok((off as i8) as u8)
}
fn eval(expr: &str, labels: &HashMap<String, u32>, pc: u32) -> Result<u32, Diagnostic> {
    let expr = expr.trim();
    if expr.is_empty() {
        return Err(Diagnostic::new("empty M6800 expression"));
    }

    let mut value = None;
    let mut operator = None;
    let mut term_start = 0;
    for (index, ch) in expr.char_indices() {
        if !matches!(ch, '+' | '-') {
            continue;
        }
        let term = expr[term_start..index].trim();
        if term.is_empty() {
            return Err(Diagnostic::new(format!("missing operand before `{ch}`")));
        }
        let rhs = atom(term, labels, pc)? as i64;
        value = Some(match (value, operator) {
            (None, None) => rhs,
            (Some(lhs), Some('+')) => lhs + rhs,
            (Some(lhs), Some('-')) => lhs - rhs,
            _ => unreachable!("M6800 expression operators are tracked with their left operand"),
        });
        operator = Some(ch);
        term_start = index + ch.len_utf8();
    }

    let term = expr[term_start..].trim();
    if term.is_empty() {
        let operator = operator.expect("a missing final term follows an operator");
        return Err(Diagnostic::new(format!(
            "missing operand after `{operator}`"
        )));
    }
    let rhs = atom(term, labels, pc)? as i64;
    let value = match (value, operator) {
        (None, None) => rhs,
        (Some(lhs), Some('+')) => lhs + rhs,
        (Some(lhs), Some('-')) => lhs - rhs,
        _ => unreachable!("M6800 expression operators are tracked with their left operand"),
    };
    if !(0..=Address24::MAX as i64).contains(&value) {
        return Err(Diagnostic::new(format!(
            "M6800 expression `{expr}` is outside the address space"
        )));
    }
    Ok(value as u32)
}
fn atom(text: &str, labels: &HashMap<String, u32>, pc: u32) -> Result<u32, Diagnostic> {
    let text = text.trim().trim_end_matches(',');
    if text == "$" {
        return Ok(pc);
    }
    if let Some(v) = labels.get(text).copied().or_else(|| {
        labels
            .iter()
            .find_map(|(n, v)| n.eq_ignore_ascii_case(text).then_some(*v))
    }) {
        return Ok(v);
    }
    parse_number(text)
}
fn parse_number(text: &str) -> Result<u32, Diagnostic> {
    let text = text.trim();
    if let Some(hex) = text.strip_suffix('h') {
        u32::from_str_radix(hex, 16)
    } else if let Some(hex) = text.strip_prefix("0x") {
        u32::from_str_radix(hex, 16)
    } else if let Some(hex) = text.strip_prefix('$') {
        u32::from_str_radix(hex, 16)
    } else if let Some(bin) = text.strip_prefix('%') {
        u32::from_str_radix(bin, 2)
    } else {
        text.parse()
    }
    .map_err(|_| Diagnostic::new(format!("unknown M6800 symbol or number `{text}`")))
}

#[cfg(test)]
mod tests;
