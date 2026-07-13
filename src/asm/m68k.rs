use std::{
    collections::HashMap,
    panic::{AssertUnwindSafe, catch_unwind},
};

use m68000::{
    addressing_modes::AddressingMode as AM,
    assembler as a,
    assembler::Condition,
    instruction::{Direction, Size},
};

use crate::diagnostic::Diagnostic;

pub fn instruction_len(text: &str) -> Result<usize, Diagnostic> {
    Ok(encode(text, &HashMap::new(), 0, false)?.len())
}

pub fn encode(
    text: &str,
    labels: &HashMap<String, u32>,
    pc: u32,
    resolve: bool,
) -> Result<Vec<u8>, Diagnostic> {
    let words = catch_unwind(AssertUnwindSafe(|| encode_words(text, labels, pc, resolve)))
        .map_err(|_| {
            Diagnostic::new(format!(
                "invalid 68000 instruction or addressing mode `{text}`"
            ))
        })??;
    let mut out = Vec::with_capacity(words.len() * 2);
    for w in words {
        out.push((w >> 8) as u8);
        out.push(w as u8);
    }
    Ok(out)
}

fn encode_words(
    text: &str,
    labels: &HashMap<String, u32>,
    pc: u32,
    resolve: bool,
) -> Result<Vec<u16>, Diagnostic> {
    let text = text.trim().to_ascii_lowercase();
    let (op, size) = split_size(&text);
    let ops = split_operands(size.rest.trim());
    let sz = size.size;
    let w1 = |x: u16| Ok(vec![x]);
    let w2 = |x: [u16; 2]| Ok(x.to_vec());
    match op.as_str() {
        "nop" => w1(a::nop()),
        "reset" => w1(a::reset()),
        "rte" => w1(a::rte()),
        "rtr" => w1(a::rtr()),
        "rts" => w1(a::rts()),
        "trapv" => w1(a::trapv()),
        "illegal" => w1(a::illegal()),
        "stop" => w2(a::stop(imm_u16(&ops, 0, labels, pc, resolve)?)),
        "trap" => w1(a::trap(imm_u16(&ops, 0, labels, pc, resolve)? as u8)),
        "bra" => Ok(a::bra(branch_disp(&ops, pc, labels, resolve)?)),
        "bsr" => Ok(a::bsr(branch_disp(&ops, pc, labels, resolve)?)),
        "jmp" => Ok(a::jmp(ea(&ops[0], labels, pc, resolve)?)),
        "jsr" => Ok(a::jsr(ea(&ops[0], labels, pc, resolve)?)),
        "pea" => Ok(a::pea(ea(&ops[0], labels, pc, resolve)?)),
        "link" => w2(a::link(
            areg(&ops[0])?,
            val(&ops[1], labels, pc, resolve)? as i16,
        )),
        "unlk" => w1(a::unlk(areg(&ops[0])?)),
        "swap" => w1(a::swap(dreg(&ops[0])?)),
        "ext" => w1(a::ext(matches!(sz, Some(Size::Long)), dreg(&ops[0])?)),
        "moveq" => w1(a::moveq(
            dreg(&ops[1])?,
            imm_u16(&ops, 0, labels, pc, resolve)? as i8,
        )),
        "lea" => Ok(a::lea(areg(&ops[1])?, ea(&ops[0], labels, pc, resolve)?)),
        "clr" => Ok(a::clr(
            sz.unwrap_or(Size::Word),
            ea(&ops[0], labels, pc, resolve)?,
        )),
        "neg" => Ok(a::neg(
            sz.unwrap_or(Size::Word),
            ea(&ops[0], labels, pc, resolve)?,
        )),
        "negx" => Ok(a::negx(
            sz.unwrap_or(Size::Word),
            ea(&ops[0], labels, pc, resolve)?,
        )),
        "not" => Ok(a::not(
            sz.unwrap_or(Size::Word),
            ea(&ops[0], labels, pc, resolve)?,
        )),
        "tst" => Ok(a::tst(
            sz.unwrap_or(Size::Word),
            ea(&ops[0], labels, pc, resolve)?,
        )),
        "tas" => Ok(a::tas(ea(&ops[0], labels, pc, resolve)?)),
        "nbcd" => Ok(a::nbcd(ea(&ops[0], labels, pc, resolve)?)),
        "move" => move_instr(sz.unwrap_or(Size::Word), &ops, labels, pc, resolve),
        "movea" => Ok(a::movea(
            sz.unwrap_or(Size::Word),
            areg(&ops[1])?,
            ea(&ops[0], labels, pc, resolve)?,
        )),
        "add" | "sub" | "and" | "or" | "cmp" => two_ea(
            op.as_str(),
            sz.unwrap_or(Size::Word),
            &ops,
            labels,
            pc,
            resolve,
        ),
        "adda" => Ok(a::adda(
            areg(&ops[1])?,
            sz.unwrap_or(Size::Word),
            ea(&ops[0], labels, pc, resolve)?,
        )),
        "suba" => Ok(a::suba(
            areg(&ops[1])?,
            sz.unwrap_or(Size::Word),
            ea(&ops[0], labels, pc, resolve)?,
        )),
        "cmpa" => Ok(a::cmpa(
            areg(&ops[1])?,
            sz.unwrap_or(Size::Word),
            ea(&ops[0], labels, pc, resolve)?,
        )),
        "addi" => Ok(a::addi(
            sz.unwrap_or(Size::Word),
            ea(&ops[1], labels, pc, resolve)?,
            imm(&ops[0], labels, pc, resolve)?,
        )),
        "subi" => Ok(a::subi(
            sz.unwrap_or(Size::Word),
            ea(&ops[1], labels, pc, resolve)?,
            imm(&ops[0], labels, pc, resolve)?,
        )),
        "andi" => Ok(a::andi(
            sz.unwrap_or(Size::Word),
            ea(&ops[1], labels, pc, resolve)?,
            imm(&ops[0], labels, pc, resolve)?,
        )),
        "ori" => Ok(a::ori(
            sz.unwrap_or(Size::Word),
            ea(&ops[1], labels, pc, resolve)?,
            imm(&ops[0], labels, pc, resolve)?,
        )),
        "eori" => Ok(a::eori(
            sz.unwrap_or(Size::Word),
            ea(&ops[1], labels, pc, resolve)?,
            imm(&ops[0], labels, pc, resolve)?,
        )),
        "eor" => Ok(a::eor(
            dreg(&ops[0])?,
            sz.unwrap_or(Size::Word),
            ea(&ops[1], labels, pc, resolve)?,
        )),
        "cmpi" => Ok(a::cmpi(
            sz.unwrap_or(Size::Word),
            ea(&ops[1], labels, pc, resolve)?,
            imm(&ops[0], labels, pc, resolve)?,
        )),
        "addq" => Ok(a::addq(
            imm(&ops[0], labels, pc, resolve)? as u8,
            sz.unwrap_or(Size::Word),
            ea(&ops[1], labels, pc, resolve)?,
        )),
        "subq" => Ok(a::subq(
            imm(&ops[0], labels, pc, resolve)? as u8,
            sz.unwrap_or(Size::Word),
            ea(&ops[1], labels, pc, resolve)?,
        )),
        "mulu" => Ok(a::mulu(dreg(&ops[1])?, ea(&ops[0], labels, pc, resolve)?)),
        "muls" => Ok(a::muls(dreg(&ops[1])?, ea(&ops[0], labels, pc, resolve)?)),
        "divu" => Ok(a::divu(dreg(&ops[1])?, ea(&ops[0], labels, pc, resolve)?)),
        "divs" => Ok(a::divs(dreg(&ops[1])?, ea(&ops[0], labels, pc, resolve)?)),
        "chk" => Ok(a::chk(dreg(&ops[1])?, ea(&ops[0], labels, pc, resolve)?)),
        "lsl" | "lsr" | "asl" | "asr" => w1(shift(
            op.as_str(),
            sz.unwrap_or(Size::Word),
            &ops,
            labels,
            pc,
            resolve,
        )?),
        _ if op.starts_with('b') => Ok(a::bcc(
            cond(&op[1..])?,
            branch_disp(&ops, pc, labels, resolve)?,
        )),
        _ if op.starts_with("db") => w2(a::dbcc(
            cond(&op[2..])?,
            dreg(&ops[0])?,
            val(&ops[1], labels, pc, resolve)? as i16,
        )),
        _ if op.starts_with('s') => Ok(a::scc(cond(&op[1..])?, ea(&ops[0], labels, pc, resolve)?)),
        _ => Err(Diagnostic::new(format!(
            "test assembler does not support 68000 instruction `{text}`"
        ))),
    }
}

fn shift(
    op: &str,
    size: Size,
    operands: &[String],
    labels: &HashMap<String, u32>,
    pc: u32,
    resolve: bool,
) -> Result<u16, Diagnostic> {
    if operands.len() != 2 {
        return Err(Diagnostic::new(format!("invalid 68000 {op} operands")));
    }
    let count = operands[0].trim();
    let (count, register_count) = if let Some(value) = count.strip_prefix('#') {
        let value = val(value, labels, pc, resolve)?;
        if !(1..=8).contains(&value) {
            return Err(Diagnostic::new("68000 shift count must be in 1..8"));
        }
        (if value == 8 { 0 } else { value as u16 }, false)
    } else {
        (u16::from(dreg(count)?), true)
    };
    let destination = u16::from(dreg(&operands[1])?);
    let direction = if op.ends_with('l') {
        Direction::Left
    } else {
        Direction::Right
    };
    Ok(if op.starts_with("ls") {
        a::lsr(count, direction, size, register_count, destination)
    } else {
        a::asr(count, direction, size, register_count, destination)
    })
}
struct Split<'a> {
    size: Option<Size>,
    rest: &'a str,
}
fn split_size(s: &str) -> (String, Split<'_>) {
    let mut it = s.splitn(2, char::is_whitespace);
    let mut op = it.next().unwrap_or("");
    let rest = it.next().unwrap_or("");
    let size = if let Some((a, b)) = op.rsplit_once('.') {
        op = a;
        Some(match b {
            "b" => Size::Byte,
            "w" => Size::Word,
            "l" => Size::Long,
            _ => Size::Word,
        })
    } else {
        None
    };
    (op.to_string(), Split { size, rest })
}
fn split_operands(s: &str) -> Vec<String> {
    s.split(',')
        .map(|x| x.trim().to_string())
        .filter(|x| !x.is_empty())
        .collect()
}
fn dreg(s: &str) -> Result<u8, Diagnostic> {
    reg(s, 'd')
}
fn areg(s: &str) -> Result<u8, Diagnostic> {
    if s == "sp" { Ok(7) } else { reg(s, 'a') }
}
fn reg(s: &str, p: char) -> Result<u8, Diagnostic> {
    let b = s.as_bytes();
    if b.len() == 2 && b[0] as char == p && b[1].is_ascii_digit() {
        let n = b[1] - b'0';
        if n < 8 {
            return Ok(n);
        }
    }
    Err(Diagnostic::new(format!("invalid 68000 register `{s}`")))
}
fn ea(s: &str, l: &HashMap<String, u32>, pc: u32, r: bool) -> Result<AM, Diagnostic> {
    let s = s.trim();
    if let Some(x) = s.strip_prefix('#') {
        return Ok(AM::Immediate(val(x, l, pc, r)?));
    }
    if let Ok(n) = dreg(s) {
        return Ok(AM::Drd(n));
    }
    if let Ok(n) = areg(s) {
        return Ok(AM::Ard(n));
    }
    if s.starts_with("(") && s.ends_with(")+") {
        return Ok(AM::Ariwpo(areg(&s[1..s.len() - 2])?));
    }
    if s.starts_with("-(") && s.ends_with(')') {
        return Ok(AM::Ariwpr(areg(&s[2..s.len() - 1])?));
    }
    if s.starts_with('(') && s.ends_with(')') {
        return Ok(AM::Ari(areg(&s[1..s.len() - 1])?));
    }
    if let Some((d, a)) = s.trim_end_matches(')').split_once('(') {
        return Ok(AM::Ariwd(areg(a)?, val(d, l, pc, r)? as i16));
    }
    let v = val(s, l, pc, r)?;
    if v <= 0xffff {
        Ok(AM::AbsShort(v as u16))
    } else {
        Ok(AM::AbsLong(v))
    }
}
fn val(s: &str, l: &HashMap<String, u32>, _pc: u32, r: bool) -> Result<u32, Diagnostic> {
    let s = s.trim().trim_start_matches('#');
    if let Some(v) = l.get(s) {
        return Ok(*v);
    }
    if !r && s.chars().any(|c| c.is_alphabetic() || c == '_') {
        return Ok(0);
    }
    if let Some(h) = s.strip_prefix('$') {
        u32::from_str_radix(h, 16)
    } else if let Some(h) = s.strip_prefix("0x") {
        u32::from_str_radix(h, 16)
    } else if s.ends_with('h') {
        u32::from_str_radix(&s[..s.len() - 1], 16)
    } else {
        s.parse()
    }
    .map_err(|_| Diagnostic::new(format!("invalid 68000 expression `{s}`")))
}
fn imm(s: &str, l: &HashMap<String, u32>, pc: u32, r: bool) -> Result<u32, Diagnostic> {
    val(s.trim_start_matches('#'), l, pc, r)
}
fn imm_u16(
    o: &[String],
    i: usize,
    l: &HashMap<String, u32>,
    pc: u32,
    r: bool,
) -> Result<u16, Diagnostic> {
    Ok(imm(&o[i], l, pc, r)? as u16)
}
fn branch_disp(
    o: &[String],
    pc: u32,
    l: &HashMap<String, u32>,
    r: bool,
) -> Result<i16, Diagnostic> {
    let t = val(&o[0], l, pc, r)? as i64;
    Ok((t - (pc as i64 + 2)) as i16)
}
fn cond(s: &str) -> Result<Condition, Diagnostic> {
    Ok(match s {
        "t" => Condition::T,
        "f" => Condition::F,
        "hi" => Condition::HI,
        "ls" => Condition::LS,
        "cc" | "hs" => Condition::CC,
        "cs" | "lo" => Condition::CS,
        "ne" => Condition::NE,
        "eq" => Condition::EQ,
        "vc" => Condition::VC,
        "vs" => Condition::VS,
        "pl" => Condition::PL,
        "mi" => Condition::MI,
        "ge" => Condition::GE,
        "lt" => Condition::LT,
        "gt" => Condition::GT,
        "le" => Condition::LE,
        _ => return Err(Diagnostic::new(format!("invalid 68000 condition `{s}`"))),
    })
}
fn move_instr(
    sz: Size,
    o: &[String],
    l: &HashMap<String, u32>,
    pc: u32,
    r: bool,
) -> Result<Vec<u16>, Diagnostic> {
    if o[1].as_str() == "ccr" {
        Ok(a::moveccr(ea(&o[0], l, pc, r)?))
    } else if o[0].as_str() == "ccr" {
        Ok(a::movefsr(ea(&o[1], l, pc, r)?))
    } else if let Ok(ar) = areg(&o[1]) {
        Ok(a::movea(sz, ar, ea(&o[0], l, pc, r)?))
    } else {
        let (dop, dext) = ea(&o[1], l, pc, r)?.assemble_move_dst();
        let (sop, sext) = ea(&o[0], l, pc, r)?.assemble(sz.is_long());
        let op = (match sz {
            Size::Byte => 0x1000,
            Size::Long => 0x2000,
            Size::Word => 0x3000,
        }) | dop
            | sop;
        let mut v = vec![op];
        v.extend(sext.iter());
        v.extend(dext.iter());
        Ok(v)
    }
}
fn two_ea(
    op: &str,
    sz: Size,
    o: &[String],
    l: &HashMap<String, u32>,
    pc: u32,
    r: bool,
) -> Result<Vec<u16>, Diagnostic> {
    // m68000-rs rejects register-to-register forms for these otherwise valid
    // instructions. They have a compact, fixed encoding on the 68000.
    if let (Ok(source), Ok(destination)) = (dreg(&o[0]), dreg(&o[1])) {
        let base = match op {
            "add" => 0xD000,
            "sub" => 0x9000,
            "and" => 0xC000,
            "or" => 0x8000,
            "cmp" => 0xB000,
            _ => unreachable!(),
        };
        let size_bits = match sz {
            Size::Byte => 0x0000,
            Size::Word => 0x0040,
            Size::Long => 0x0080,
        };
        return Ok(vec![
            base | (u16::from(destination) << 9) | size_bits | u16::from(source),
        ]);
    }
    if let Ok(d) = dreg(&o[1]) {
        let am = ea(&o[0], l, pc, r)?;
        return Ok(match op {
            "add" => a::add(d, Direction::MemoryToRegister, sz, am),
            "sub" => a::sub(d, Direction::MemoryToRegister, sz, am),
            "and" => a::and(d, Direction::MemoryToRegister, sz, am),
            "or" => a::or(d, Direction::MemoryToRegister, sz, am),
            "cmp" => a::cmp(d, sz, am),
            _ => unreachable!(),
        });
    }
    let d = dreg(&o[0])?;
    let am = ea(&o[1], l, pc, r)?;
    Ok(match op {
        "add" => a::add(d, Direction::RegisterToMemory, sz, am),
        "sub" => a::sub(d, Direction::RegisterToMemory, sz, am),
        "and" => a::and(d, Direction::RegisterToMemory, sz, am),
        "or" => a::or(d, Direction::RegisterToMemory, sz, am),
        _ => return Err(Diagnostic::new("invalid cmp destination")),
    })
}
