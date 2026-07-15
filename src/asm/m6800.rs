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
    } else if rest.starts_with('<') {
        (AddrMode::Direct, rest[1..].trim())
    } else if rest.starts_with('>') {
        (AddrMode::Extended, rest[1..].trim())
    } else if prefer_direct(mnemonic, rest) {
        (AddrMode::Direct, rest)
    } else {
        (AddrMode::Extended, rest)
    };
    Ok(generated(mnemonic, mode).map(|op| (op, operand.to_owned())))
}

fn prefer_direct(_mnemonic: &str, operand: &str) -> bool {
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
    let mut parts = expr.split_whitespace();
    let Some(first) = parts.next() else {
        return Err(Diagnostic::new("empty M6800 expression"));
    };
    let mut value = atom(first, labels, pc)? as i64;
    while let Some(op) = parts.next() {
        let rhs = atom(
            parts
                .next()
                .ok_or_else(|| Diagnostic::new(format!("missing operand after `{op}`")))?,
            labels,
            pc,
        )? as i64;
        match op {
            "+" => value += rhs,
            "-" => value -= rhs,
            _ => {
                return Err(Diagnostic::new(format!(
                    "unsupported M6800 operator `{op}`"
                )));
            }
        }
    }
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
mod tests {
    use super::*;
    use crate::target::AssemblerCpu;
    use crate::vm::assemble_subset_with_symbols_at;

    fn labels() -> HashMap<String, u32> {
        HashMap::new()
    }

    #[test]
    fn golden_encodes_every_official_mnemonic_and_addressing_form() {
        // Opcode bases are transcribed from the Motorola M6800 programming manual.
        let inherent = [
            ("nop", 0x01),
            ("tap", 0x06),
            ("tpa", 0x07),
            ("inx", 0x08),
            ("dex", 0x09),
            ("clv", 0x0A),
            ("sev", 0x0B),
            ("clc", 0x0C),
            ("sec", 0x0D),
            ("cli", 0x0E),
            ("sei", 0x0F),
            ("sba", 0x10),
            ("cba", 0x11),
            ("tab", 0x16),
            ("tba", 0x17),
            ("daa", 0x19),
            ("aba", 0x1B),
            ("tsx", 0x30),
            ("ins", 0x31),
            ("pula", 0x32),
            ("pulb", 0x33),
            ("des", 0x34),
            ("txs", 0x35),
            ("psha", 0x36),
            ("pshb", 0x37),
            ("rts", 0x39),
            ("rti", 0x3B),
            ("wai", 0x3E),
            ("swi", 0x3F),
            ("nega", 0x40),
            ("coma", 0x43),
            ("lsra", 0x44),
            ("rora", 0x46),
            ("asra", 0x47),
            ("asla", 0x48),
            ("rola", 0x49),
            ("deca", 0x4A),
            ("inca", 0x4C),
            ("tsta", 0x4D),
            ("clra", 0x4F),
            ("negb", 0x50),
            ("comb", 0x53),
            ("lsrb", 0x54),
            ("rorb", 0x56),
            ("asrb", 0x57),
            ("aslb", 0x58),
            ("rolb", 0x59),
            ("decb", 0x5A),
            ("incb", 0x5C),
            ("tstb", 0x5D),
            ("clrb", 0x5F),
        ];
        for (mnemonic, opcode) in inherent {
            assert_eq!(
                emit_instruction(mnemonic, &labels(), 0x1000).unwrap(),
                Some(vec![opcode]),
                "{mnemonic}"
            );
            assert_eq!(instruction_len(mnemonic).unwrap(), Some(1), "{mnemonic}");
        }

        let branches = [
            ("bra", 0x20),
            ("brn", 0x21),
            ("bhi", 0x22),
            ("bls", 0x23),
            ("bcc", 0x24),
            ("bhs", 0x24),
            ("bcs", 0x25),
            ("blo", 0x25),
            ("bne", 0x26),
            ("beq", 0x27),
            ("bvc", 0x28),
            ("bvs", 0x29),
            ("bpl", 0x2A),
            ("bmi", 0x2B),
            ("bge", 0x2C),
            ("blt", 0x2D),
            ("bgt", 0x2E),
            ("ble", 0x2F),
            ("bsr", 0x8D),
        ];
        for (mnemonic, opcode) in branches {
            let source = format!("{mnemonic} 1080h");
            assert_eq!(
                emit_instruction(&source, &labels(), 0x1000).unwrap(),
                Some(vec![opcode, 0x7E]),
                "{source}"
            );
            assert_eq!(instruction_len(&source).unwrap(), Some(2), "{source}");
        }

        for (source, expected) in [
            ("jsr <12h", vec![0x9D, 0x12]),
            ("jsr 12h, x", vec![0xAD, 0x12]),
            ("jsr >1234h", vec![0xBD, 0x12, 0x34]),
        ] {
            assert_eq!(
                emit_instruction(source, &labels(), 0x1000).unwrap(),
                Some(expected),
                "{source}"
            );
        }

        let memory = [
            ("neg", 0x00),
            ("com", 0x03),
            ("lsr", 0x04),
            ("ror", 0x06),
            ("asr", 0x07),
            ("asl", 0x08),
            ("rol", 0x09),
            ("dec", 0x0A),
            ("inc", 0x0C),
            ("tst", 0x0D),
            ("jmp", 0x0E),
            ("clr", 0x0F),
        ];
        for (mnemonic, offset) in memory {
            for (operand, opcode, bytes) in [
                ("12h, x", 0x60 + offset, vec![0x60 + offset, 0x12]),
                (">1234h", 0x70 + offset, vec![0x70 + offset, 0x12, 0x34]),
            ] {
                let source = format!("{mnemonic} {operand}");
                assert_eq!(
                    emit_instruction(&source, &labels(), 0x1000).unwrap(),
                    Some(bytes),
                    "{source}"
                );
                assert_eq!(
                    instruction_len(&source).unwrap(),
                    Some(if opcode < 0x70 { 2 } else { 3 }),
                    "{source}"
                );
            }
        }

        let accumulator = [
            ("suba", 0x80, false, false),
            ("cmpa", 0x81, false, false),
            ("sbca", 0x82, false, false),
            ("anda", 0x84, false, false),
            ("bita", 0x85, false, false),
            ("ldaa", 0x86, false, false),
            ("staa", 0x87, false, true),
            ("eora", 0x88, false, false),
            ("adca", 0x89, false, false),
            ("oraa", 0x8A, false, false),
            ("adda", 0x8B, false, false),
            ("cpx", 0x8C, true, false),
            ("lds", 0x8E, true, false),
            ("sts", 0x8F, true, true),
            ("subb", 0xC0, false, false),
            ("cmpb", 0xC1, false, false),
            ("sbcb", 0xC2, false, false),
            ("andb", 0xC4, false, false),
            ("bitb", 0xC5, false, false),
            ("ldab", 0xC6, false, false),
            ("stab", 0xC7, false, true),
            ("eorb", 0xC8, false, false),
            ("adcb", 0xC9, false, false),
            ("orab", 0xCA, false, false),
            ("addb", 0xCB, false, false),
            ("ldx", 0xCE, true, false),
            ("stx", 0xCF, true, true),
        ];
        for (mnemonic, base, word, store) in accumulator {
            let immediate = if word { "#1234h" } else { "#12h" };
            let forms = if store {
                vec![("<12h", 0x10), ("12h, x", 0x20), (">1234h", 0x30)]
            } else {
                vec![
                    (immediate, 0x00),
                    ("<12h", 0x10),
                    ("12h, x", 0x20),
                    (">1234h", 0x30),
                ]
            };
            for (operand, add) in forms {
                let source = format!("{mnemonic} {operand}");
                let mut expected = vec![base + add];
                if add == 0 && word {
                    expected.extend([0x12, 0x34]);
                } else if add == 0 || add == 0x10 || add == 0x20 {
                    expected.push(0x12);
                } else {
                    expected.extend([0x12, 0x34]);
                }
                assert_eq!(
                    emit_instruction(&source, &labels(), 0x1000).unwrap(),
                    Some(expected.clone()),
                    "{source}"
                );
                assert_eq!(
                    instruction_len(&source).unwrap(),
                    Some(expected.len()),
                    "{source}"
                );
            }
        }

        for (source, expected) in [
            ("lsla", vec![0x48]),
            ("lslb", vec![0x58]),
            ("lsl 12h, x", vec![0x68, 0x12]),
            ("lsl >1234h", vec![0x78, 0x12, 0x34]),
        ] {
            assert_eq!(
                emit_instruction(source, &labels(), 0x1000).unwrap(),
                Some(expected),
                "{source}"
            );
        }
    }

    #[test]
    fn validates_operand_and_branch_boundaries() {
        let labels = labels();
        for (source, expected) in [
            ("ldaa #ffh", vec![0x86, 0xFF]),
            ("cpx #ffffh", vec![0x8C, 0xFF, 0xFF]),
            ("staa <ffh", vec![0x97, 0xFF]),
            ("ldaa ffh, x", vec![0xA6, 0xFF]),
            ("jmp >ffffh", vec![0x7E, 0xFF, 0xFF]),
            ("bra f82h", vec![0x20, 0x80]),
            ("bra 1081h", vec![0x20, 0x7F]),
        ] {
            assert_eq!(
                emit_instruction(source, &labels, 0x1000).unwrap(),
                Some(expected),
                "{source}"
            );
        }
        for source in [
            "ldaa #100h",
            "cpx #10000h",
            "staa <100h",
            "ldaa 100h, x",
            "jmp >10000h",
            "bra f81h",
            "bra 1082h",
            "bra 10000h",
            "ldaa ,x",
        ] {
            assert!(
                assemble_subset_with_symbols_at(AssemblerCpu::M6800, source, 0x1000).is_err(),
                "{source}"
            );
        }
    }

    #[test]
    fn resolves_case_insensitive_labels_equates_and_expressions() {
        let program = assemble_subset_with_symbols_at(
            AssemblerCpu::M6800,
            "origin equ 20h\nnext = origin + 2\nStart:\n ldaa #ORIGIN + 1\n staa <NEXT\n bra start\n",
            0x1000,
        )
        .unwrap();
        assert_eq!(program.bytes, [0x86, 0x21, 0x97, 0x22, 0x20, 0xFA]);

        let labels = HashMap::from([("destination".to_owned(), 0x1234)]);
        assert_eq!(
            emit_instruction("ldx destination", &labels, 0x1000).unwrap(),
            Some(vec![0xFE, 0x12, 0x34])
        );
        assert_eq!(
            emit_instruction("ldaa $ + 2", &labels, 0x1000).unwrap(),
            Some(vec![0xB6, 0x10, 0x02])
        );
    }

    #[test]
    fn rejects_invalid_mnemonics_and_addressing_forms() {
        for source in [
            "ldaa",
            "staa #1",
            "jmp <10h",
            "neg <10h",
            "bra #1000h",
            "inx 1",
            "ldaa 1,y",
            "asld",
            "ld a, 1",
        ] {
            assert!(
                assemble_subset_with_symbols_at(AssemblerCpu::M6800, source, 0x1000).is_err(),
                "{source}"
            );
        }
    }
}
