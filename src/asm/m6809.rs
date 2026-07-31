use std::collections::HashMap;

use crate::diagnostic::Diagnostic;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum OperandMode {
    Inherent,
    Immediate(u8),
    Relative(u8),
    Direct,
    Extended,
    Indexed,
    RegisterPair,
    RegisterList,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct Encoding {
    opcode: u16,
    opcode_len: u8,
    mode: OperandMode,
}

impl Encoding {
    const fn one(opcode: u8, mode: OperandMode) -> Self {
        Self {
            opcode: opcode as u16,
            opcode_len: 1,
            mode,
        }
    }

    const fn two(opcode: u16, mode: OperandMode) -> Self {
        Self {
            opcode,
            opcode_len: 2,
            mode,
        }
    }
}

pub fn instruction_len(text: &str) -> Result<Option<usize>, Diagnostic> {
    let Some((encoding, operand)) = analyze(text)? else {
        return Ok(None);
    };
    Ok(Some(
        usize::from(encoding.opcode_len) + operand_len(encoding.mode, &operand)?,
    ))
}

pub fn emit_instruction(
    text: &str,
    labels: &HashMap<String, u32>,
    pc: u32,
) -> Result<Option<Vec<u8>>, Diagnostic> {
    let Some((encoding, operand)) = analyze(text)? else {
        return Ok(None);
    };
    let mut out = opcode_bytes(encoding.opcode, encoding.opcode_len);
    match encoding.mode {
        OperandMode::Inherent => {}
        OperandMode::Immediate(width) => {
            push_signed(&mut out, eval_signed(&operand, labels, pc)?, width)?;
        }
        OperandMode::Relative(width) => {
            let target = eval_address(&operand, labels, pc)?;
            let offset = relative(pc, target, encoding.opcode_len, width)?;
            push_signed(&mut out, offset, width)?;
        }
        OperandMode::Direct => {
            push_address(
                &mut out,
                eval_address(&strip_force(&operand), labels, pc)?,
                1,
            )?;
        }
        OperandMode::Extended => {
            push_address(
                &mut out,
                eval_address(&strip_force(&operand), labels, pc)?,
                2,
            )?;
        }
        OperandMode::Indexed => encode_indexed(&mut out, &operand, labels, pc)?,
        OperandMode::RegisterPair => out.push(register_pair(&operand)?),
        OperandMode::RegisterList => out.push(register_list(&operand)?),
    }
    Ok(Some(out))
}

fn analyze(text: &str) -> Result<Option<(Encoding, String)>, Diagnostic> {
    let text = text.trim().to_ascii_lowercase();
    let (raw_mnemonic, rest) = text
        .split_once(char::is_whitespace)
        .map_or((text.as_str(), ""), |(m, r)| (m, r.trim()));
    let Some(mnemonic) = canonical_mnemonic(raw_mnemonic) else {
        return Ok(None);
    };

    if rest.is_empty()
        && let Some(opcode) = inherent_opcode(mnemonic)
    {
        return Ok(Some((
            Encoding::one(opcode, OperandMode::Inherent),
            String::new(),
        )));
    }
    if rest.is_empty() && matches!(mnemonic, "swi2" | "swi3") {
        let opcode = if mnemonic == "swi2" { 0x103F } else { 0x113F };
        return Ok(Some((
            Encoding::two(opcode, OperandMode::Inherent),
            String::new(),
        )));
    }
    if let Some(opcode) = short_branch_opcode(mnemonic) {
        return Ok(Some((
            Encoding::one(opcode, OperandMode::Relative(1)),
            rest.to_owned(),
        )));
    }
    if let Some(opcode) = long_branch_opcode(mnemonic) {
        return Ok(Some((
            Encoding::two(opcode, OperandMode::Relative(2)),
            rest.to_owned(),
        )));
    }
    if matches!(mnemonic, "lbra" | "lbsr") {
        let opcode = if mnemonic == "lbra" { 0x16 } else { 0x17 };
        return Ok(Some((
            Encoding::one(opcode, OperandMode::Relative(2)),
            rest.to_owned(),
        )));
    }
    if matches!(mnemonic, "orcc" | "andcc" | "cwai") {
        let opcode = match mnemonic {
            "orcc" => 0x1A,
            "andcc" => 0x1C,
            "cwai" => 0x3C,
            _ => unreachable!(),
        };
        return Ok(Some((
            Encoding::one(opcode, OperandMode::Immediate(1)),
            immediate_operand(rest),
        )));
    }
    if matches!(mnemonic, "exg" | "tfr") {
        let opcode = if mnemonic == "exg" { 0x1E } else { 0x1F };
        return Ok(Some((
            Encoding::one(opcode, OperandMode::RegisterPair),
            rest.to_owned(),
        )));
    }
    if matches!(mnemonic, "pshs" | "puls" | "pshu" | "pulu") {
        let opcode = match mnemonic {
            "pshs" => 0x34,
            "puls" => 0x35,
            "pshu" => 0x36,
            "pulu" => 0x37,
            _ => unreachable!(),
        };
        return Ok(Some((
            Encoding::one(opcode, OperandMode::RegisterList),
            rest.to_owned(),
        )));
    }
    if let Some(opcode) = lea_opcode(mnemonic) {
        return Ok(Some((
            Encoding::one(opcode, OperandMode::Indexed),
            rest.to_owned(),
        )));
    }

    let operand = if let Some(immediate) = rest.strip_prefix('#') {
        (
            OperandMode::Immediate(operand_width(mnemonic).unwrap_or(1)),
            immediate.trim(),
        )
    } else if is_indexed_operand(rest) {
        (OperandMode::Indexed, rest)
    } else if rest.strip_prefix('<').is_some() || prefer_direct(rest) {
        (OperandMode::Direct, rest)
    } else {
        (OperandMode::Extended, rest)
    };
    let Some(encoding) = memory_encoding(mnemonic, operand.0) else {
        return Ok(None);
    };
    Ok(Some((encoding, operand.1.to_owned())))
}

fn canonical_mnemonic(mnemonic: &str) -> Option<&str> {
    Some(match mnemonic {
        "nop" | "sync" | "daa" | "sex" | "abx" | "mul" | "rts" | "rti" | "wai" | "swi" | "swi2"
        | "swi3" => mnemonic,
        "ldaa" => "lda",
        "staa" => "sta",
        "ldab" => "ldb",
        "stab" => "stb",
        "asla" | "lsla" | "aslb" | "lslb" | "asra" | "asrb" | "lsra" | "lsrb" | "rola" | "rolb"
        | "rora" | "rorb" | "nega" | "negb" | "coma" | "comb" | "deca" | "decb" | "inca"
        | "incb" | "tsta" | "tstb" | "clra" | "clrb" => mnemonic,
        "bra" | "brn" | "bhi" | "bls" | "bcc" | "bhs" | "bcs" | "blo" | "bne" | "beq" | "bvc"
        | "bvs" | "bpl" | "bmi" | "bge" | "blt" | "bgt" | "ble" | "bsr" | "lbra" | "lbsr"
        | "lbrn" | "lbhi" | "lbls" | "lbcc" | "lbhs" | "lbcs" | "lblo" | "lbne" | "lbeq"
        | "lbvc" | "lbvs" | "lbpl" | "lbmi" | "lbge" | "lblt" | "lbgt" | "lble" => mnemonic,
        "orcc" | "andcc" | "cwai" | "exg" | "tfr" | "pshs" | "puls" | "pshu" | "pulu" => mnemonic,
        "leax" | "leay" | "leas" | "leau" => mnemonic,
        "neg" | "com" | "lsr" | "ror" | "asr" | "asl" | "lsl" | "rol" | "dec" | "inc" | "tst"
        | "jmp" | "jsr" | "clr" => mnemonic,
        "suba" | "cmpa" | "sbca" | "anda" | "bita" | "lda" | "sta" | "eora" | "adca" | "ora"
        | "oraa" | "adda" | "cpx" | "ldx" | "stx" | "subb" | "cmpb" | "sbcb" | "andb" | "bitb"
        | "ldb" | "stb" | "eorb" | "adcb" | "orb" | "orab" | "addb" | "subd" | "addd" | "cmpd"
        | "ldd" | "std" | "ldu" | "stu" | "ldy" | "sty" | "cmpy" | "lds" | "sts" | "cmpu"
        | "cmps" => mnemonic,
        _ => return None,
    })
}

fn inherent_opcode(mnemonic: &str) -> Option<u8> {
    Some(match mnemonic {
        "nop" => 0x12,
        "sync" => 0x13,
        "daa" => 0x19,
        "sex" => 0x1D,
        "nega" => 0x40,
        "coma" => 0x43,
        "lsra" => 0x44,
        "rora" => 0x46,
        "asra" => 0x47,
        "asla" | "lsla" => 0x48,
        "rola" => 0x49,
        "deca" => 0x4A,
        "inca" => 0x4C,
        "tsta" => 0x4D,
        "clra" => 0x4F,
        "negb" => 0x50,
        "comb" => 0x53,
        "lsrb" => 0x54,
        "rorb" => 0x56,
        "asrb" => 0x57,
        "aslb" | "lslb" => 0x58,
        "rolb" => 0x59,
        "decb" => 0x5A,
        "incb" => 0x5C,
        "tstb" => 0x5D,
        "clrb" => 0x5F,
        "exg" => return None,
        "tfr" => return None,
        "abx" => 0x3A,
        "mul" => 0x3D,
        "rts" => 0x39,
        "rti" => 0x3B,
        "wai" => 0x3E,
        "swi" => 0x3F,
        "swi2" => return None,
        "swi3" => return None,
        _ => return None,
    })
}

fn short_branch_opcode(mnemonic: &str) -> Option<u8> {
    Some(match mnemonic {
        "bra" => 0x20,
        "brn" => 0x21,
        "bhi" => 0x22,
        "bls" => 0x23,
        "bcc" | "bhs" => 0x24,
        "bcs" | "blo" => 0x25,
        "bne" => 0x26,
        "beq" => 0x27,
        "bvc" => 0x28,
        "bvs" => 0x29,
        "bpl" => 0x2A,
        "bmi" => 0x2B,
        "bge" => 0x2C,
        "blt" => 0x2D,
        "bgt" => 0x2E,
        "ble" => 0x2F,
        "bsr" => 0x8D,
        _ => return None,
    })
}

fn long_branch_opcode(mnemonic: &str) -> Option<u16> {
    let low = match mnemonic {
        "lbrn" => 0x21,
        "lbhi" => 0x22,
        "lbls" => 0x23,
        "lbcc" | "lbhs" => 0x24,
        "lbcs" | "lblo" => 0x25,
        "lbne" => 0x26,
        "lbeq" => 0x27,
        "lbvc" => 0x28,
        "lbvs" => 0x29,
        "lbpl" => 0x2A,
        "lbmi" => 0x2B,
        "lbge" => 0x2C,
        "lblt" => 0x2D,
        "lbgt" => 0x2E,
        "lble" => 0x2F,
        _ => return None,
    };
    Some(0x1000 | low)
}

fn lea_opcode(mnemonic: &str) -> Option<u8> {
    Some(match mnemonic {
        "leax" => 0x30,
        "leay" => 0x31,
        "leas" => 0x32,
        "leau" => 0x33,
        _ => return None,
    })
}

fn operand_width(mnemonic: &str) -> Option<u8> {
    Some(match mnemonic {
        "cpx" | "ldx" | "subd" | "addd" | "cmpd" | "ldd" | "ldu" | "ldy" | "cmpy" | "lds"
        | "cmpu" | "cmps" => 2,
        "stx" | "std" | "stu" | "sty" | "sts" => 2,
        _ => 1,
    })
}

fn memory_encoding(mnemonic: &str, mode: OperandMode) -> Option<Encoding> {
    let (immediate, direct, indexed, extended, width) = match mnemonic {
        "suba" => (0x80, 0x90, 0xA0, 0xB0, 1),
        "cmpa" => (0x81, 0x91, 0xA1, 0xB1, 1),
        "sbca" => (0x82, 0x92, 0xA2, 0xB2, 1),
        "subd" => (0x83, 0x93, 0xA3, 0xB3, 2),
        "anda" => (0x84, 0x94, 0xA4, 0xB4, 1),
        "bita" => (0x85, 0x95, 0xA5, 0xB5, 1),
        "lda" => (0x86, 0x96, 0xA6, 0xB6, 1),
        "sta" => (0, 0x97, 0xA7, 0xB7, 1),
        "eora" => (0x88, 0x98, 0xA8, 0xB8, 1),
        "adca" => (0x89, 0x99, 0xA9, 0xB9, 1),
        "ora" | "oraa" => (0x8A, 0x9A, 0xAA, 0xBA, 1),
        "adda" => (0x8B, 0x9B, 0xAB, 0xBB, 1),
        "cpx" => (0x8C, 0x9C, 0xAC, 0xBC, 2),
        "ldx" => (0x8E, 0x9E, 0xAE, 0xBE, 2),
        "stx" => (0, 0x9F, 0xAF, 0xBF, 2),
        "subb" => (0xC0, 0xD0, 0xE0, 0xF0, 1),
        "cmpb" => (0xC1, 0xD1, 0xE1, 0xF1, 1),
        "sbcb" => (0xC2, 0xD2, 0xE2, 0xF2, 1),
        "addd" => (0xC3, 0xD3, 0xE3, 0xF3, 2),
        "andb" => (0xC4, 0xD4, 0xE4, 0xF4, 1),
        "bitb" => (0xC5, 0xD5, 0xE5, 0xF5, 1),
        "ldb" => (0xC6, 0xD6, 0xE6, 0xF6, 1),
        "stb" => (0, 0xD7, 0xE7, 0xF7, 1),
        "eorb" => (0xC8, 0xD8, 0xE8, 0xF8, 1),
        "adcb" => (0xC9, 0xD9, 0xE9, 0xF9, 1),
        "orb" | "orab" => (0xCA, 0xDA, 0xEA, 0xFA, 1),
        "addb" => (0xCB, 0xDB, 0xEB, 0xFB, 1),
        "ldd" => (0xCC, 0xDC, 0xEC, 0xFC, 2),
        "ldu" => (0xCE, 0xDE, 0xEE, 0xFE, 2),
        "std" => (0, 0xDD, 0xED, 0xFD, 2),
        "stu" => (0, 0xDF, 0xEF, 0xFF, 2),
        "ldy" => (0x108E, 0x109E, 0x10AE, 0x10BE, 2),
        "sty" => (0, 0x109F, 0x10AF, 0x10BF, 2),
        "lds" => (0x10CE, 0x10DE, 0x10EE, 0x10FE, 2),
        "sts" => (0, 0x10DF, 0x10EF, 0x10FF, 2),
        "cmpd" => (0x1083, 0x1093, 0x10A3, 0x10B3, 2),
        "cmpy" => (0x108C, 0x109C, 0x10AC, 0x10BC, 2),
        "cmpu" => (0x1183, 0x1193, 0x11A3, 0x11B3, 2),
        "cmps" => (0x118C, 0x119C, 0x11AC, 0x11BC, 2),
        "neg" => (0, 0, 0x60, 0x70, 1),
        "com" => (0, 0, 0x63, 0x73, 1),
        "lsr" => (0, 0, 0x64, 0x74, 1),
        "ror" => (0, 0, 0x66, 0x76, 1),
        "asr" => (0, 0, 0x67, 0x77, 1),
        "asl" | "lsl" => (0, 0, 0x68, 0x78, 1),
        "rol" => (0, 0, 0x69, 0x79, 1),
        "dec" => (0, 0, 0x6A, 0x7A, 1),
        "inc" => (0, 0, 0x6C, 0x7C, 1),
        "tst" => (0, 0, 0x6D, 0x7D, 1),
        "jmp" => (0, 0x0E, 0x6E, 0x7E, 0),
        "jsr" => (0, 0x9D, 0xAD, 0xBD, 0),
        "clr" => (0, 0, 0x6F, 0x7F, 1),
        _ => return None,
    };
    let (opcode, mode) = match mode {
        OperandMode::Immediate(_) if immediate != 0 => (immediate, OperandMode::Immediate(width)),
        OperandMode::Immediate(_) => return None,
        OperandMode::Direct if direct != 0 => (direct, OperandMode::Direct),
        OperandMode::Direct => return None,
        OperandMode::Indexed => (indexed, OperandMode::Indexed),
        OperandMode::Extended if extended != 0 => (extended, OperandMode::Extended),
        OperandMode::Extended => return None,
        _ => return None,
    };
    Some(if opcode > 0xFF {
        Encoding::two(opcode, mode)
    } else {
        Encoding::one(opcode as u8, mode)
    })
}

fn is_indexed_operand(operand: &str) -> bool {
    let operand = operand.trim();
    operand.starts_with('[') || operand.contains(',')
}

fn immediate_operand(operand: &str) -> String {
    operand
        .strip_prefix('#')
        .unwrap_or(operand)
        .trim()
        .to_owned()
}

fn prefer_direct(operand: &str) -> bool {
    parse_number(strip_force(operand).trim()).is_ok_and(|value| value <= 0xFF)
}

fn operand_len(mode: OperandMode, operand: &str) -> Result<usize, Diagnostic> {
    Ok(match mode {
        OperandMode::Inherent => 0,
        OperandMode::Immediate(width) | OperandMode::Relative(width) => usize::from(width),
        OperandMode::Direct => 1,
        OperandMode::Extended => 2,
        OperandMode::RegisterPair | OperandMode::RegisterList => 1,
        OperandMode::Indexed => indexed_len(operand)?,
    })
}

fn indexed_len(operand: &str) -> Result<usize, Diagnostic> {
    let (_, inner) = indexed_inner(operand)?;
    let Some((offset, register)) = split_index(inner) else {
        return Ok(3);
    };
    let offset = offset.trim();
    let register = register.trim();
    if is_auto(register) || offset.is_empty() {
        return Ok(1);
    }
    if matches!(offset, "a" | "b") {
        return Ok(1);
    }
    if offset == "d" {
        return Ok(1);
    }
    if register == "pc" || register == "pcr" {
        return Ok(parse_number(strip_force(offset))
            .ok()
            .map(|value| {
                if (-128..=127).contains(&signed_value(value)) {
                    2
                } else {
                    3
                }
            })
            .unwrap_or(3));
    }
    let Some(value) = parse_number(strip_force(offset)).ok() else {
        return Ok(3);
    };
    let value = signed_value(value);
    let length = if (-16..=15).contains(&value) {
        1
    } else if (-128..=127).contains(&value) {
        2
    } else {
        3
    };
    Ok(length)
}

fn encode_indexed(
    out: &mut Vec<u8>,
    operand: &str,
    labels: &HashMap<String, u32>,
    pc: u32,
) -> Result<(), Diagnostic> {
    let (indirect, inner) = indexed_inner(operand)?;
    let Some((offset, register)) = split_index(inner) else {
        let value = eval_address(inner, labels, pc)?;
        out.push(0x9F);
        return push_address(out, value, 2);
    };
    let offset = offset.trim();
    let register = register.trim();
    let (rr, register_is_pc) = index_register(register)?;
    if is_auto(register) {
        let (base, suffix) = auto_register(register)?;
        let (rr, _) = index_register(base)?;
        let post = match suffix {
            "+" => 0x80,
            "++" => 0x81,
            "-" => 0x82,
            "--" => 0x83,
            _ => return Err(Diagnostic::new("invalid M6809 auto-indexed operand")),
        } | (rr << 5);
        if indirect && suffix != "++" && suffix != "--" {
            return Err(Diagnostic::new(
                "M6809 auto-indexed indirection is only valid with ++ or --",
            ));
        }
        out.push(post | if indirect { 0x10 } else { 0 });
        return Ok(());
    }
    if offset.is_empty() {
        if register_is_pc {
            return Err(Diagnostic::new(
                "M6809 indexed operand cannot use an empty PC offset",
            ));
        }
        out.push(0x84 | (rr << 5) | if indirect { 0x10 } else { 0 });
        return Ok(());
    }
    if matches!(offset, "a" | "b" | "d") {
        let base = match offset {
            "b" => 0x85,
            "a" => 0x86,
            "d" => 0x8B,
            _ => unreachable!(),
        };
        out.push(base | (rr << 5) | if indirect { 0x10 } else { 0 });
        return Ok(());
    }

    let result = if register_is_pc {
        let target = eval_address(strip_force(offset), labels, pc)?;
        let width = if parse_number(strip_force(offset)).is_ok() {
            1
        } else {
            2
        };
        let next = pc + u32::from(width) + 2;
        let relative = target as i64 - next as i64;
        if width == 1 {
            if !(-128..=127).contains(&relative) {
                return Err(Diagnostic::new(
                    "M6809 PC-relative 8-bit offset is out of range",
                ));
            }
        }
        if indirect {
            out.push(if width == 1 { 0x9C } else { 0x9D });
        } else {
            out.push(if width == 1 { 0x8C } else { 0x8D });
        }
        push_signed(out, relative, width)
    } else {
        let value = eval_signed(strip_force(offset), labels, pc)?;
        let force_byte = offset.trim_start().starts_with('<');
        let width = if !indirect && (-16..=15).contains(&value) && !force_byte {
            0
        } else if (-128..=127).contains(&value) || force_byte {
            1
        } else {
            2
        };
        let post = match width {
            0 => (value as i8 as u8 & 0x1F) | (rr << 5),
            1 => 0x88 | (rr << 5),
            2 => 0x89 | (rr << 5),
            _ => unreachable!(),
        };
        if indirect && width == 0 {
            return Err(Diagnostic::new(
                "M6809 5-bit indexed offsets cannot be indirect",
            ));
        }
        out.push(post | if indirect { 0x10 } else { 0 });
        let result = if width == 1 {
            push_signed(out, value, 1)
        } else if width == 2 {
            push_signed(out, value, 2)
        } else {
            Ok(())
        };
        result
    };
    result
}

fn indexed_inner(operand: &str) -> Result<(bool, &str), Diagnostic> {
    let operand = operand.trim();
    if let Some(inner) = operand.strip_prefix('[').and_then(|s| s.strip_suffix(']')) {
        Ok((true, inner.trim()))
    } else {
        Ok((false, operand))
    }
}

fn split_index(operand: &str) -> Option<(&str, &str)> {
    operand.rsplit_once(',')
}

fn is_auto(register: &str) -> bool {
    register.ends_with('+')
        || register.ends_with('-')
        || register.starts_with('+')
        || register.starts_with('-')
}

fn auto_register(register: &str) -> Result<(&str, &str), Diagnostic> {
    for suffix in ["++", "--", "+", "-"] {
        if let Some(base) = register.strip_suffix(suffix) {
            if matches!(base, "x" | "y" | "u" | "s") {
                return Ok((base, suffix));
            }
        }
    }
    for prefix in ["--", "-"] {
        if let Some(base) = register.strip_prefix(prefix)
            && matches!(base, "x" | "y" | "u" | "s")
        {
            return Ok((base, prefix));
        }
    }
    Err(Diagnostic::new("invalid M6809 auto-index register"))
}

fn index_register(register: &str) -> Result<(u8, bool), Diagnostic> {
    match register.trim() {
        "x" => Ok((0, false)),
        "y" => Ok((1, false)),
        "u" => Ok((2, false)),
        "s" => Ok((3, false)),
        "pc" | "pcr" => Ok((0, true)),
        _ => Err(Diagnostic::new(format!(
            "invalid M6809 index register `{register}`"
        ))),
    }
}

fn register_pair(operand: &str) -> Result<u8, Diagnostic> {
    let mut registers = operand.split(',').map(str::trim);
    let source = register_code(registers.next().unwrap_or_default())?;
    let destination = register_code(registers.next().unwrap_or_default())?;
    if registers.next().is_some() {
        return Err(Diagnostic::new("M6809 EXG/TFR requires two registers"));
    }
    Ok((source << 4) | destination)
}

fn register_code(register: &str) -> Result<u8, Diagnostic> {
    match register {
        "d" => Ok(0),
        "x" => Ok(1),
        "y" => Ok(2),
        "u" => Ok(3),
        "s" => Ok(4),
        "pc" => Ok(5),
        "a" => Ok(8),
        "b" => Ok(9),
        "cc" => Ok(10),
        "dp" => Ok(11),
        _ => Err(Diagnostic::new(format!(
            "invalid M6809 register `{register}`"
        ))),
    }
}

fn register_list(operand: &str) -> Result<u8, Diagnostic> {
    let mut mask = 0u8;
    for register in operand.split(',').map(str::trim) {
        let bit = match register {
            "pc" => 0x80,
            "s" | "u" => 0x40,
            "y" => 0x20,
            "x" => 0x10,
            "dp" => 0x08,
            "b" => 0x04,
            "a" => 0x02,
            "cc" => 0x01,
            "d" => 0x06,
            _ => {
                return Err(Diagnostic::new(format!(
                    "invalid M6809 stack register `{register}`"
                )));
            }
        };
        mask |= bit;
    }
    Ok(mask)
}

fn opcode_bytes(opcode: u16, len: u8) -> Vec<u8> {
    if len == 1 {
        vec![opcode as u8]
    } else {
        vec![(opcode >> 8) as u8, opcode as u8]
    }
}

fn relative(pc: u32, target: u32, opcode_len: u8, width: u8) -> Result<i64, Diagnostic> {
    let next = i64::from(pc) + i64::from(opcode_len) + i64::from(width);
    let offset = i64::from(target) - next;
    let (min, max) = if width == 1 {
        (-128, 127)
    } else {
        (-32768, 32767)
    };
    if !(min..=max).contains(&offset) {
        return Err(Diagnostic::new(format!(
            "M6809 relative branch target 0x{target:04X} is out of range from 0x{pc:04X}"
        )));
    }
    Ok(offset)
}

fn push_address(out: &mut Vec<u8>, value: u32, width: u8) -> Result<(), Diagnostic> {
    if value > 0xFFFF {
        return Err(Diagnostic::new(format!(
            "M6809 address 0x{value:X} is outside the 16-bit address space"
        )));
    }
    push_signed(out, i64::from(value), width)
}

fn push_signed(out: &mut Vec<u8>, value: i64, width: u8) -> Result<(), Diagnostic> {
    let (min, max, mask) = match width {
        1 => (-128, 255, 0xFF),
        2 => (-32768, 65535, 0xFFFF),
        _ => return Err(Diagnostic::new("invalid M6809 operand width")),
    };
    if !(min..=max).contains(&value) {
        return Err(Diagnostic::new(format!(
            "M6809 operand {value} is outside the {width}-byte range"
        )));
    }
    let value = (value & mask) as u64;
    if width == 1 {
        out.push(value as u8);
    } else {
        out.extend([(value >> 8) as u8, value as u8]);
    }
    Ok(())
}

fn eval_address(expr: &str, labels: &HashMap<String, u32>, pc: u32) -> Result<u32, Diagnostic> {
    let value = eval_signed(expr, labels, pc)?;
    if !(0..=0xFFFF).contains(&value) {
        return Err(Diagnostic::new(format!(
            "M6809 expression `{expr}` is outside the 16-bit address space"
        )));
    }
    Ok(value as u32)
}

fn eval_signed(expr: &str, labels: &HashMap<String, u32>, pc: u32) -> Result<i64, Diagnostic> {
    let expr = expr.trim();
    if expr.is_empty() {
        return Err(Diagnostic::new("empty M6809 expression"));
    }
    let mut total = 0i64;
    let mut start = 0usize;
    let mut sign = 1i64;
    if let Some(first) = expr.as_bytes().first().copied()
        && (first == b'+' || first == b'-')
    {
        sign = if first == b'+' { 1 } else { -1 };
        start = 1;
    }
    for (index, ch) in expr.char_indices() {
        if !matches!(ch, '+' | '-') || index <= start {
            continue;
        }
        let term = expr[start..index].trim();
        if term.is_empty() {
            return Err(Diagnostic::new(format!(
                "missing M6809 expression term before `{ch}`"
            )));
        }
        total += sign * i64::from(atom(term, labels, pc)?);
        sign = if ch == '+' { 1 } else { -1 };
        start = index + 1;
    }
    let term = expr[start..].trim();
    if term.is_empty() {
        return Err(Diagnostic::new("missing M6809 expression term"));
    }
    let first = atom(term, labels, pc)?;
    Ok(total + sign * i64::from(first))
}

fn atom(text: &str, labels: &HashMap<String, u32>, pc: u32) -> Result<u32, Diagnostic> {
    let text = text.trim();
    if text == "$" {
        return Ok(pc);
    }
    if let Some(value) = labels.get(text).copied().or_else(|| {
        labels
            .iter()
            .find_map(|(name, value)| name.eq_ignore_ascii_case(text).then_some(*value))
    }) {
        return Ok(value);
    }
    parse_number(text)
}

fn parse_number(text: &str) -> Result<u32, Diagnostic> {
    let text = text.trim();
    let value = if let Some(hex) = text.strip_suffix('h') {
        u32::from_str_radix(hex, 16)
    } else if let Some(hex) = text.strip_prefix("0x") {
        u32::from_str_radix(hex, 16)
    } else if let Some(hex) = text.strip_prefix('$') {
        u32::from_str_radix(hex, 16)
    } else if let Some(binary) = text.strip_prefix('%') {
        u32::from_str_radix(binary, 2)
    } else {
        text.parse()
    };
    value.map_err(|_| Diagnostic::new(format!("unknown M6809 symbol or number `{text}`")))
}

fn signed_value(value: u32) -> i64 {
    if value <= 0x7FFF {
        i64::from(value)
    } else {
        i64::from(value as i32)
    }
}

fn strip_force(operand: &str) -> &str {
    operand
        .trim()
        .strip_prefix('<')
        .or_else(|| operand.trim().strip_prefix('>'))
        .unwrap_or(operand.trim())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn labels() -> HashMap<String, u32> {
        HashMap::new()
    }

    #[test]
    fn encodes_native_m6809_instructions() {
        for (source, expected) in [
            ("nop", vec![0x12]),
            ("mul", vec![0x3D]),
            ("lbra 1000h", vec![0x16, 0x0F, 0xFD]),
            ("lbsr 1000h", vec![0x17, 0x0F, 0xFD]),
            ("lda #12h", vec![0x86, 0x12]),
            ("ldd #1234h", vec![0xCC, 0x12, 0x34]),
            ("ldy #1234h", vec![0x10, 0x8E, 0x12, 0x34]),
            ("lda ,x", vec![0xA6, 0x84]),
            ("lda 1,x", vec![0xA6, 0x01]),
            ("lda 100,x", vec![0xA6, 0x88, 0x64]),
            ("lda [8,s]", vec![0xA6, 0xF8, 0x08]),
            ("lda >1234h", vec![0xB6, 0x12, 0x34]),
            ("lda [$1234]", vec![0xA6, 0x9F, 0x12, 0x34]),
            ("leax 5,y", vec![0x30, 0x25]),
            ("exg d,x", vec![0x1E, 0x01]),
            ("pshs y,x,b,a", vec![0x34, 0x36]),
        ] {
            assert_eq!(
                emit_instruction(source, &labels(), 0x0).unwrap(),
                Some(expected),
                "{source}"
            );
        }
    }

    #[test]
    fn keeps_m6800_accumulator_aliases() {
        assert_eq!(
            emit_instruction("ldaa #12h", &labels(), 0).unwrap(),
            Some(vec![0x86, 0x12])
        );
        assert_eq!(
            emit_instruction("staa >1234h", &labels(), 0).unwrap(),
            Some(vec![0xB7, 0x12, 0x34])
        );
    }
}
