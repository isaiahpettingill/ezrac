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
    let source = text.trim().to_ascii_lowercase();
    let (op, args) = split_mnemonic(&source);

    let fixed = match op {
        "nop" => Some(0x0000),
        "ret" => Some(0x9508),
        "reti" => Some(0x9518),
        "ijmp" => Some(0x9409),
        "eijmp" => Some(0x9419),
        "icall" => Some(0x9509),
        "eicall" => Some(0x9519),
        "break" => Some(0x9598),
        "sleep" => Some(0x9588),
        "wdr" => Some(0x95a8),
        "lpm" if args.is_empty() => Some(0x95c8),
        "elpm" if args.is_empty() => Some(0x95d8),
        "spm" if args.is_empty() => Some(0x95e8),
        "spm" if args == "z+" => Some(0x95f8),
        _ => None,
    };
    if let Some(word) = fixed {
        return Ok(word_bytes(word));
    }

    match op {
        "ldi" | "ori" | "andi" | "cpi" | "subi" | "sbci" | "sbr" | "cbr" => {
            let (d, k) = operands2(args)?;
            let d = high_reg(d, op)?;
            let mut k = immediate(k, labels, pc, resolve, 0xff, "8-bit immediate")? as u16;
            let base = match op {
                "ldi" => 0xe000,
                "ori" | "sbr" => 0x6000,
                "andi" => 0x7000,
                "cpi" => 0x3000,
                "subi" => 0x5000,
                "sbci" => 0x4000,
                "cbr" => {
                    k = (!k) & 0xff;
                    0x7000
                }
                _ => unreachable!(),
            };
            Ok(word_bytes(
                base | ((k & 0xf0) << 4) | ((d as u16 - 16) << 4) | (k & 0x0f),
            ))
        }
        "ser" => {
            let d = high_reg(args, op)? as u16 - 16;
            Ok(word_bytes(0xef0f | (d << 4)))
        }
        "mov" | "add" | "adc" | "sub" | "sbc" | "and" | "or" | "eor" | "cp" | "cpc" | "cpse"
        | "mul" => {
            let (d, r) = operands2(args)?;
            let d = reg(d)? as u16;
            let r = reg(r)? as u16;
            let base = match op {
                "mov" => 0x2c00,
                "add" => 0x0c00,
                "adc" => 0x1c00,
                "sub" => 0x1800,
                "sbc" => 0x0800,
                "and" => 0x2000,
                "or" => 0x2800,
                "eor" => 0x2400,
                "cp" => 0x1400,
                "cpc" => 0x0400,
                "cpse" => 0x1000,
                "mul" => 0x9c00,
                _ => unreachable!(),
            };
            Ok(word_bytes(rr_word(base, d, r)))
        }
        "clr" | "lsl" | "rol" | "tst" => {
            let r = reg(args)? as u16;
            let base = match op {
                "clr" => 0x2400,
                "lsl" => 0x0c00,
                "rol" => 0x1c00,
                "tst" => 0x2000,
                _ => unreachable!(),
            };
            Ok(word_bytes(rr_word(base, r, r)))
        }
        "com" | "neg" | "swap" | "inc" | "asr" | "lsr" | "ror" | "dec" | "push" | "pop" => {
            let r = reg(args)? as u16;
            let base = match op {
                "com" => 0x9400,
                "neg" => 0x9401,
                "swap" => 0x9402,
                "inc" => 0x9403,
                "asr" => 0x9405,
                "lsr" => 0x9406,
                "ror" => 0x9407,
                "dec" => 0x940a,
                "push" => 0x920f,
                "pop" => 0x900f,
                _ => unreachable!(),
            };
            Ok(word_bytes(base | (r << 4)))
        }
        "movw" => {
            let (d, r) = operands2(args)?;
            let d = even_reg(d, op)?;
            let r = even_reg(r, op)?;
            Ok(word_bytes(0x0100 | ((d as u16 / 2) << 4) | (r as u16 / 2)))
        }
        "muls" => multiply_restricted(args, op, 16, 31, 0x0200, 4),
        "mulsu" => multiply_restricted(args, op, 16, 23, 0x0300, 3),
        "fmul" => multiply_restricted(args, op, 16, 23, 0x0308, 3),
        "fmuls" => multiply_restricted(args, op, 16, 23, 0x0380, 3),
        "fmulsu" => multiply_restricted(args, op, 16, 23, 0x0388, 3),
        "adiw" | "sbiw" => {
            let (d, k) = operands2(args)?;
            let d = reg(d)?;
            if !matches!(d, 24 | 26 | 28 | 30) {
                return Err(error(format!(
                    "AVR {op} register pair must start at r24, r26, r28, or r30"
                )));
            }
            let k = immediate(k, labels, pc, resolve, 63, "6-bit immediate")? as u16;
            let base = if op == "adiw" { 0x9600 } else { 0x9700 };
            Ok(word_bytes(
                base | ((k & 0x30) << 2) | (((d - 24) as u16 / 2) << 4) | (k & 0x0f),
            ))
        }
        "in" | "out" => {
            let (a, r) = operands2(args)?;
            let (a, r) = if op == "in" { (r, a) } else { (a, r) };
            let a = immediate(a, labels, pc, resolve, 63, "I/O address")? as u16;
            let r = reg(r)? as u16;
            let base = if op == "in" { 0xb000 } else { 0xb800 };
            Ok(word_bytes(base | ((a & 0x30) << 5) | (r << 4) | (a & 0x0f)))
        }
        "sbi" | "cbi" | "sbic" | "sbis" => {
            let (a, b) = operands2(args)?;
            let a = immediate(a, labels, pc, resolve, 31, "I/O address")? as u16;
            let b = immediate(b, labels, pc, resolve, 7, "bit")? as u16;
            let base = match op {
                "cbi" => 0x9800,
                "sbic" => 0x9900,
                "sbi" => 0x9a00,
                "sbis" => 0x9b00,
                _ => unreachable!(),
            };
            Ok(word_bytes(base | (a << 3) | b))
        }
        "bld" | "bst" | "sbrc" | "sbrs" => {
            let (r, b) = operands2(args)?;
            let r = reg(r)? as u16;
            let b = immediate(b, labels, pc, resolve, 7, "bit")? as u16;
            let base = match op {
                "bld" => 0xf800,
                "bst" => 0xfa00,
                "sbrc" => 0xfc00,
                "sbrs" => 0xfe00,
                _ => unreachable!(),
            };
            Ok(word_bytes(base | (r << 4) | b))
        }
        "bset" | "bclr" => {
            let s = immediate(args, labels, pc, resolve, 7, "status bit")? as u16;
            Ok(word_bytes(
                (if op == "bset" { 0x9408 } else { 0x9488 }) | (s << 4),
            ))
        }
        "sec" | "sez" | "sen" | "sev" | "ses" | "seh" | "set" | "sei" | "clc" | "clz" | "cln"
        | "clv" | "cls" | "clh" | "clt" | "cli" => {
            let (clear, bit) = match op {
                "sec" => (false, 0),
                "sez" => (false, 1),
                "sen" => (false, 2),
                "sev" => (false, 3),
                "ses" => (false, 4),
                "seh" => (false, 5),
                "set" => (false, 6),
                "sei" => (false, 7),
                "clc" => (true, 0),
                "clz" => (true, 1),
                "cln" => (true, 2),
                "clv" => (true, 3),
                "cls" => (true, 4),
                "clh" => (true, 5),
                "clt" => (true, 6),
                "cli" => (true, 7),
                _ => unreachable!(),
            };
            Ok(word_bytes(
                (if clear { 0x9488 } else { 0x9408 }) | (bit << 4),
            ))
        }
        "brbs" | "brbc" => {
            let (s, target) = operands2(args)?;
            let s = immediate(s, labels, pc, resolve, 7, "status bit")? as u16;
            relative_branch(
                op,
                target,
                labels,
                pc,
                resolve,
                if op == "brbs" { 0xf000 } else { 0xf400 },
                7,
                s,
            )
        }
        "breq" | "brne" | "brcs" | "brlo" | "brcc" | "brsh" | "brmi" | "brpl" | "brvs" | "brvc"
        | "brlt" | "brge" | "brhs" | "brhc" | "brts" | "brtc" | "brie" | "brid" => {
            let (set, bit) = match op {
                "breq" => (true, 1),
                "brne" => (false, 1),
                "brcs" | "brlo" => (true, 0),
                "brcc" | "brsh" => (false, 0),
                "brmi" => (true, 2),
                "brpl" => (false, 2),
                "brvs" => (true, 3),
                "brvc" => (false, 3),
                "brlt" => (true, 4),
                "brge" => (false, 4),
                "brhs" => (true, 5),
                "brhc" => (false, 5),
                "brts" => (true, 6),
                "brtc" => (false, 6),
                "brie" => (true, 7),
                "brid" => (false, 7),
                _ => unreachable!(),
            };
            relative_branch(
                op,
                args,
                labels,
                pc,
                resolve,
                if set { 0xf000 } else { 0xf400 },
                7,
                bit,
            )
        }
        "rjmp" | "rcall" => relative_branch(
            op,
            args,
            labels,
            pc,
            resolve,
            if op == "rjmp" { 0xc000 } else { 0xd000 },
            12,
            0,
        ),
        "jmp" | "call" => absolute_program(op, args, labels, pc, resolve),
        "ld" | "st" => encode_indirect(op, args),
        "ldd" | "std" => encode_displaced(op, args, labels, pc, resolve),
        "lds" | "sts" => encode_direct(op, args, labels, pc, resolve),
        "lpm" | "elpm" => encode_program_load(op, args),
        "xch" | "las" | "lac" | "lat" => {
            let (z, r) = operands2(args)?;
            if z != "z" {
                return Err(error(format!("AVR {op} first operand must be Z")));
            }
            let base = match op {
                "xch" => 0x9204,
                "las" => 0x9205,
                "lac" => 0x9206,
                "lat" => 0x9207,
                _ => unreachable!(),
            };
            Ok(word_bytes(base | ((reg(r)? as u16) << 4)))
        }
        "des" => {
            let k = immediate(args, labels, pc, resolve, 15, "DES round")? as u16;
            Ok(word_bytes(0x940b | (k << 4)))
        }
        _ => Err(error(format!(
            "assembler does not support AVR instruction `{source}`"
        ))),
    }
}

fn encode_indirect(op: &str, args: &str) -> Result<Vec<u8>, Diagnostic> {
    let (left, right) = operands2(args)?;
    let (r, pointer) = if op == "ld" {
        (reg(left)?, right)
    } else {
        (reg(right)?, left)
    };
    let base = match (op, pointer) {
        ("ld", "x") => 0x900c,
        ("ld", "x+") => 0x900d,
        ("ld", "-x") => 0x900e,
        ("ld", "y") => 0x8008,
        ("ld", "y+") => 0x9009,
        ("ld", "-y") => 0x900a,
        ("ld", "z") => 0x8000,
        ("ld", "z+") => 0x9001,
        ("ld", "-z") => 0x9002,
        ("st", "x") => 0x920c,
        ("st", "x+") => 0x920d,
        ("st", "-x") => 0x920e,
        ("st", "y") => 0x8208,
        ("st", "y+") => 0x9209,
        ("st", "-y") => 0x920a,
        ("st", "z") => 0x8200,
        ("st", "z+") => 0x9201,
        ("st", "-z") => 0x9202,
        _ => return Err(error(format!("invalid AVR {op} pointer mode `{pointer}`"))),
    };
    Ok(word_bytes(base | ((r as u16) << 4)))
}

fn encode_displaced(
    op: &str,
    args: &str,
    labels: &HashMap<String, u32>,
    pc: u32,
    resolve: bool,
) -> Result<Vec<u8>, Diagnostic> {
    let (left, right) = operands2(args)?;
    let (r, pointer) = if op == "ldd" {
        (reg(left)?, right)
    } else {
        (reg(right)?, left)
    };
    let (base, qtext) = if let Some(q) = pointer.strip_prefix("y+") {
        (if op == "ldd" { 0x8008 } else { 0x8208 }, q)
    } else if let Some(q) = pointer.strip_prefix("z+") {
        (if op == "ldd" { 0x8000 } else { 0x8200 }, q)
    } else {
        return Err(error(format!("invalid AVR {op} displacement `{pointer}`")));
    };
    let q = immediate(qtext, labels, pc, resolve, 63, "displacement")? as u16;
    Ok(word_bytes(
        base | ((r as u16) << 4) | ((q & 0x20) << 8) | ((q & 0x18) << 7) | (q & 7),
    ))
}

fn encode_direct(
    op: &str,
    args: &str,
    labels: &HashMap<String, u32>,
    pc: u32,
    resolve: bool,
) -> Result<Vec<u8>, Diagnostic> {
    let (left, right) = operands2(args)?;
    let (r, address) = if op == "lds" {
        (reg(left)?, right)
    } else {
        (reg(right)?, left)
    };
    let address = immediate(address, labels, pc, resolve, 0xffff, "data address")? as u16;
    let mut out = word_bytes((if op == "lds" { 0x9000 } else { 0x9200 }) | ((r as u16) << 4));
    out.extend(word_bytes(address));
    Ok(out)
}

fn encode_program_load(op: &str, args: &str) -> Result<Vec<u8>, Diagnostic> {
    let (r, pointer) = operands2(args)?;
    let base = match (op, pointer) {
        ("lpm", "z") => 0x9004,
        ("lpm", "z+") => 0x9005,
        ("elpm", "z") => 0x9006,
        ("elpm", "z+") => 0x9007,
        _ => return Err(error(format!("invalid AVR {op} pointer mode `{pointer}`"))),
    };
    Ok(word_bytes(base | ((reg(r)? as u16) << 4)))
}

fn absolute_program(
    op: &str,
    arg: &str,
    labels: &HashMap<String, u32>,
    pc: u32,
    resolve: bool,
) -> Result<Vec<u8>, Diagnostic> {
    let target = value(arg, labels, pc, resolve)?;
    if target & 1 != 0 {
        return Err(error(format!(
            "AVR {op} target `{arg}` must be word-aligned"
        )));
    }
    let address = target / 2;
    if address > 0x3f_ffff {
        return Err(error(format!(
            "AVR {op} target `{arg}` is outside the 22-bit program address range"
        )));
    }
    let high = (address >> 16) as u16;
    let first = (if op == "jmp" { 0x940c } else { 0x940e }) | ((high & 0x3e) << 3) | (high & 1);
    let mut out = word_bytes(first);
    out.extend(word_bytes(address as u16));
    Ok(out)
}

#[allow(clippy::too_many_arguments)]
fn relative_branch(
    op: &str,
    arg: &str,
    labels: &HashMap<String, u32>,
    pc: u32,
    resolve: bool,
    base: u16,
    bits: u8,
    low: u16,
) -> Result<Vec<u8>, Diagnostic> {
    let target = value(arg, labels, pc, resolve)?;
    let delta = target as i64 - (pc as i64 + 2);
    if delta & 1 != 0 {
        return Err(error(format!(
            "AVR {op} target `{arg}` must be word-aligned relative to the instruction"
        )));
    }
    let offset = delta / 2;
    let min = -(1_i64 << (bits - 1));
    let max = (1_i64 << (bits - 1)) - 1;
    if offset < min || offset > max {
        return Err(error(format!("AVR {op} target `{arg}` is out of range")));
    }
    let mask = (1_u16 << bits) - 1;
    Ok(word_bytes(
        base | (((offset as i16 as u16) & mask) << if bits == 7 { 3 } else { 0 }) | low,
    ))
}

fn multiply_restricted(
    args: &str,
    op: &str,
    min: u8,
    max: u8,
    base: u16,
    field_bits: u8,
) -> Result<Vec<u8>, Diagnostic> {
    let (d, r) = operands2(args)?;
    let d = reg(d)?;
    let r = reg(r)?;
    if !(min..=max).contains(&d) || !(min..=max).contains(&r) {
        return Err(error(format!("AVR {op} registers must be r{min}..r{max}")));
    }
    let mask = (1_u16 << field_bits) - 1;
    Ok(word_bytes(
        base | ((((d - min) as u16) & mask) << 4) | (((r - min) as u16) & mask),
    ))
}

fn rr_word(base: u16, d: u16, r: u16) -> u16 {
    base | (d << 4) | (r & 0x0f) | ((r & 0x10) << 5)
}
fn word_bytes(word: u16) -> Vec<u8> {
    vec![word as u8, (word >> 8) as u8]
}
fn split_mnemonic(text: &str) -> (&str, &str) {
    text.split_once(char::is_whitespace)
        .map_or((text, ""), |(a, b)| (a, b.trim()))
}
fn operands2(text: &str) -> Result<(&str, &str), Diagnostic> {
    text.split_once(',')
        .map(|(a, b)| (a.trim(), b.trim()))
        .filter(|(a, b)| !a.is_empty() && !b.is_empty() && !b.contains(','))
        .ok_or_else(|| error(format!("invalid AVR operand list `{text}`")))
}
fn reg(text: &str) -> Result<u8, Diagnostic> {
    text.strip_prefix('r')
        .and_then(|n| n.parse().ok())
        .filter(|r| *r < 32)
        .ok_or_else(|| error(format!("invalid AVR register `{text}`")))
}
fn high_reg(text: &str, op: &str) -> Result<u8, Diagnostic> {
    let r = reg(text)?;
    if r >= 16 {
        Ok(r)
    } else {
        Err(error(format!("AVR {op} destination must be r16..r31")))
    }
}
fn even_reg(text: &str, op: &str) -> Result<u8, Diagnostic> {
    let r = reg(text)?;
    if r & 1 == 0 {
        Ok(r)
    } else {
        Err(error(format!(
            "AVR {op} register pairs must start at an even register"
        )))
    }
}
fn immediate(
    text: &str,
    labels: &HashMap<String, u32>,
    pc: u32,
    resolve: bool,
    max: u32,
    kind: &str,
) -> Result<u32, Diagnostic> {
    let v = value(text, labels, pc, resolve)?;
    if v <= max {
        Ok(v)
    } else {
        Err(error(format!("AVR {kind} `{text}` is outside 0..{max}")))
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
            .map_err(|_| error(format!("invalid AVR value `{text}`")));
    }
    if let Some(hex) = t.strip_prefix("0x") {
        return u32::from_str_radix(hex, 16)
            .map_err(|_| error(format!("invalid AVR value `{text}`")));
    }
    if let Ok(v) = t.parse() {
        return Ok(v);
    }
    labels
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case(t))
        .map(|(_, value)| *value)
        .ok_or_else(|| error(format!("unknown AVR symbol `{text}`")))
}
fn error(message: String) -> Diagnostic {
    Diagnostic::new(message)
}

#[cfg(test)]
mod tests {
    use super::{encode_instruction, instruction_len};
    use std::collections::HashMap;

    fn word(value: u16) -> Vec<u8> {
        vec![value as u8, (value >> 8) as u8]
    }

    #[test]
    fn encodes_all_documented_mnemonics_and_aliases() {
        let labels = HashMap::new();
        let cases: &[(&str, u16)] = &[
            ("nop", 0x0000),
            ("add r1,r2", 0x0c12),
            ("adc r1,r2", 0x1c12),
            ("adiw r24,63", 0x96cf),
            ("sub r1,r2", 0x1812),
            ("subi r16,255", 0x5f0f),
            ("sbc r1,r2", 0x0812),
            ("sbci r16,255", 0x4f0f),
            ("sbiw r30,63", 0x97ff),
            ("and r1,r2", 0x2012),
            ("andi r16,255", 0x7f0f),
            ("or r1,r2", 0x2812),
            ("ori r16,255", 0x6f0f),
            ("eor r1,r2", 0x2412),
            ("com r1", 0x9410),
            ("neg r1", 0x9411),
            ("inc r1", 0x9413),
            ("dec r1", 0x941a),
            ("asr r1", 0x9415),
            ("lsr r1", 0x9416),
            ("ror r1", 0x9417),
            ("swap r1", 0x9412),
            ("lsl r1", 0x0c11),
            ("rol r1", 0x1c11),
            ("clr r1", 0x2411),
            ("tst r1", 0x2011),
            ("ser r16", 0xef0f),
            ("sbr r16,1", 0x6001),
            ("cbr r16,1", 0x7f0e),
            ("cp r1,r2", 0x1412),
            ("cpc r1,r2", 0x0412),
            ("cpi r16,255", 0x3f0f),
            ("cpse r1,r2", 0x1012),
            ("mov r1,r2", 0x2c12),
            ("movw r2,r4", 0x0112),
            ("mul r1,r2", 0x9c12),
            ("muls r16,r17", 0x0201),
            ("mulsu r16,r17", 0x0301),
            ("fmul r16,r17", 0x0309),
            ("fmuls r16,r17", 0x0381),
            ("fmulsu r16,r17", 0x0389),
            ("bld r1,7", 0xf817),
            ("bst r1,7", 0xfa17),
            ("sbrc r1,7", 0xfc17),
            ("sbrs r1,7", 0xfe17),
            ("sbi 31,7", 0x9aff),
            ("cbi 31,7", 0x98ff),
            ("sbic 31,7", 0x99ff),
            ("sbis 31,7", 0x9bff),
            ("in r1,63", 0xb61f),
            ("out 63,r1", 0xbe1f),
            ("bset 7", 0x9478),
            ("bclr 7", 0x94f8),
            ("sec", 0x9408),
            ("sez", 0x9418),
            ("sen", 0x9428),
            ("sev", 0x9438),
            ("ses", 0x9448),
            ("seh", 0x9458),
            ("set", 0x9468),
            ("sei", 0x9478),
            ("clc", 0x9488),
            ("clz", 0x9498),
            ("cln", 0x94a8),
            ("clv", 0x94b8),
            ("cls", 0x94c8),
            ("clh", 0x94d8),
            ("clt", 0x94e8),
            ("cli", 0x94f8),
            ("rjmp 2", 0xc000),
            ("rcall 2", 0xd000),
            ("brbs 0,2", 0xf000),
            ("brbc 0,2", 0xf400),
            ("breq 2", 0xf001),
            ("brne 2", 0xf401),
            ("brcs 2", 0xf000),
            ("brlo 2", 0xf000),
            ("brcc 2", 0xf400),
            ("brsh 2", 0xf400),
            ("brmi 2", 0xf002),
            ("brpl 2", 0xf402),
            ("brvs 2", 0xf003),
            ("brvc 2", 0xf403),
            ("brlt 2", 0xf004),
            ("brge 2", 0xf404),
            ("brhs 2", 0xf005),
            ("brhc 2", 0xf405),
            ("brts 2", 0xf006),
            ("brtc 2", 0xf406),
            ("brie 2", 0xf007),
            ("brid 2", 0xf407),
            ("push r1", 0x921f),
            ("pop r1", 0x901f),
            ("ret", 0x9508),
            ("reti", 0x9518),
            ("ijmp", 0x9409),
            ("eijmp", 0x9419),
            ("icall", 0x9509),
            ("eicall", 0x9519),
            ("lpm", 0x95c8),
            ("elpm", 0x95d8),
            ("spm", 0x95e8),
            ("spm z+", 0x95f8),
            ("break", 0x9598),
            ("sleep", 0x9588),
            ("wdr", 0x95a8),
            ("lpm r1,z", 0x9014),
            ("lpm r1,z+", 0x9015),
            ("elpm r1,z", 0x9016),
            ("elpm r1,z+", 0x9017),
            ("xch z,r1", 0x9214),
            ("las z,r1", 0x9215),
            ("lac z,r1", 0x9216),
            ("lat z,r1", 0x9217),
            ("des 15", 0x94fb),
        ];
        for &(source, expected) in cases {
            assert_eq!(
                encode_instruction(source, &labels, 0).unwrap(),
                word(expected),
                "{source}"
            );
        }
    }

    #[test]
    fn encodes_every_pointer_mode_and_long_instruction() {
        let labels = HashMap::new();
        for (source, expected) in [
            ("ld r1,x", 0x901c),
            ("ld r1,x+", 0x901d),
            ("ld r1,-x", 0x901e),
            ("ld r1,y", 0x8018),
            ("ld r1,y+", 0x9019),
            ("ld r1,-y", 0x901a),
            ("ld r1,z", 0x8010),
            ("ld r1,z+", 0x9011),
            ("ld r1,-z", 0x9012),
            ("st x,r1", 0x921c),
            ("st x+,r1", 0x921d),
            ("st -x,r1", 0x921e),
            ("st y,r1", 0x8218),
            ("st y+,r1", 0x9219),
            ("st -y,r1", 0x921a),
            ("st z,r1", 0x8210),
            ("st z+,r1", 0x9211),
            ("st -z,r1", 0x9212),
            ("ldd r1,y+63", 0xac1f),
            ("ldd r1,z+63", 0xac17),
            ("std y+63,r1", 0xae1f),
            ("std z+63,r1", 0xae17),
        ] {
            assert_eq!(
                encode_instruction(source, &labels, 0).unwrap(),
                word(expected),
                "{source}"
            );
        }
        assert_eq!(
            encode_instruction("lds r31,0xffff", &labels, 0).unwrap(),
            vec![0xf0, 0x91, 0xff, 0xff]
        );
        assert_eq!(
            encode_instruction("sts 0xffff,r31", &labels, 0).unwrap(),
            vec![0xf0, 0x93, 0xff, 0xff]
        );
        assert_eq!(
            encode_instruction("jmp 0x7ffffe", &labels, 0).unwrap(),
            vec![0xfd, 0x95, 0xff, 0xff]
        );
        assert_eq!(
            encode_instruction("call 0x7ffffe", &labels, 0).unwrap(),
            vec![0xff, 0x95, 0xff, 0xff]
        );
        assert_eq!(instruction_len("lds r1, symbol").unwrap(), 4);
    }

    #[test]
    fn validates_boundaries_alignment_and_case_insensitive_labels() {
        let labels = HashMap::from([("MiXeD".to_string(), 4096)]);
        assert_eq!(
            encode_instruction("RJMP mixed", &labels, 4094).unwrap(),
            word(0xc000)
        );
        let empty = HashMap::new();
        for source in ["rjmp 4097", "rcall 4097", "breq 3", "jmp 3", "call 3"] {
            assert!(encode_instruction(source, &empty, 0).is_err(), "{source}");
        }
        assert!(encode_instruction("rjmp 4096", &empty, 0).is_ok());
        assert!(encode_instruction("rjmp 4294960000", &empty, u32::MAX - 1).is_err());
        assert!(encode_instruction("breq 128", &empty, 0).is_ok());
        assert!(encode_instruction("breq 130", &empty, 0).is_err());
        for source in [
            "ldi r15,0",
            "adiw r22,0",
            "adiw r24,64",
            "muls r15,r16",
            "mulsu r24,r16",
            "movw r1,r2",
            "sbi 32,0",
            "sbi 0,8",
            "in r0,64",
            "ldd r0,y+64",
            "lds r0,65536",
            "des 16",
            "jmp 0x800000",
        ] {
            assert!(encode_instruction(source, &empty, 0).is_err(), "{source}");
        }
    }
}
