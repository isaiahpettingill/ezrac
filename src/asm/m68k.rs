use std::{
    collections::HashMap,
    panic::{AssertUnwindSafe, catch_unwind},
};

use m68000::{
    addressing_modes::{AddressingMode as AM, BriefExtensionWord},
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
    if let Some(token) = text.split_whitespace().next()
        && let Some((_, suffix)) = token.rsplit_once('.')
        && !matches!(suffix, "b" | "w" | "l")
    {
        return Err(Diagnostic::new(format!(
            "invalid 68000 size suffix `.{suffix}`"
        )));
    }
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
            signed_val(&ops[1], labels, resolve)? as i16,
        )),
        "unlk" => w1(a::unlk(areg(&ops[0])?)),
        "swap" => w1(a::swap(dreg(&ops[0])?)),
        "ext" => w1(a::ext(matches!(sz, Some(Size::Long)), dreg(&ops[0])?)),
        "moveq" => w1(a::moveq(
            dreg(&ops[1])?,
            imm_u16(&ops, 0, labels, pc, resolve)? as i8,
        )),
        "abcd" | "sbcd" => bcd(op.as_str(), &ops),
        "addx" | "subx" => add_sub_x(op.as_str(), sz.unwrap_or(Size::Word), &ops),
        "cmpm" => w1(a::cmpm(
            areg_post(&ops[0])?,
            sz.unwrap_or(Size::Word),
            areg_post(&ops[1])?,
        )),
        "exg" => exg(&ops),
        "movep" => movep(sz.unwrap_or(Size::Word), &ops, labels, pc, resolve),
        "movem" => movem(sz.unwrap_or(Size::Word), &ops, labels, pc, resolve),
        "moveusp" => moveusp(&ops),
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
        "andi" if ops.get(1).is_some_and(|x| x != "ccr" && x != "sr") => Ok(a::andi(
            sz.unwrap_or(Size::Word),
            ea(&ops[1], labels, pc, resolve)?,
            imm(&ops[0], labels, pc, resolve)?,
        )),
        "ori" if ops.get(1).is_some_and(|x| x != "ccr" && x != "sr") => Ok(a::ori(
            sz.unwrap_or(Size::Word),
            ea(&ops[1], labels, pc, resolve)?,
            imm(&ops[0], labels, pc, resolve)?,
        )),
        "eori" if ops.get(1).is_some_and(|x| x == "ccr") => {
            w2(a::eoriccr(imm_u16(&ops, 0, labels, pc, resolve)?))
        }
        "eori" if ops.get(1).is_some_and(|x| x == "sr") => {
            w2(a::eorisr(imm_u16(&ops, 0, labels, pc, resolve)?))
        }
        "eori" if ops.get(1).is_some_and(|x| x != "ccr" && x != "sr") => Ok(a::eori(
            sz.unwrap_or(Size::Word),
            ea(&ops[1], labels, pc, resolve)?,
            imm(&ops[0], labels, pc, resolve)?,
        )),
        "ori" if ops.get(1).is_some_and(|x| x == "ccr") => {
            w2(a::oriccr(imm_u16(&ops, 0, labels, pc, resolve)?))
        }
        "ori" if ops.get(1).is_some_and(|x| x == "sr") => {
            w2(a::orisr(imm_u16(&ops, 0, labels, pc, resolve)?))
        }
        "andi" if ops.get(1).is_some_and(|x| x == "ccr") => {
            w2(a::andiccr(imm_u16(&ops, 0, labels, pc, resolve)?))
        }
        "andi" if ops.get(1).is_some_and(|x| x == "sr") => {
            w2(a::andisr(imm_u16(&ops, 0, labels, pc, resolve)?))
        }
        "btst" | "bchg" | "bclr" | "bset" => bit_op(op.as_str(), &ops, labels, pc, resolve),
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
        "lsl" | "lsr" | "asl" | "asr" | "rol" | "ror" | "roxl" | "roxr" => {
            shift_or_rotate(op.as_str(), sz, &ops, labels, pc, resolve)
        }

        _ if op.starts_with('b') => Ok(a::bcc(
            cond(&op[1..])?,
            branch_disp(&ops, pc, labels, resolve)?,
        )),
        _ if op.starts_with("db") => w2(a::dbcc(
            cond(&op[2..])?,
            dreg(&ops[0])?,
            val(&ops[1], labels, pc, resolve)? as i16,
        )),
        _ if op.starts_with('s')
            && op != "stop"
            && op != "swap"
            && op != "sub"
            && op != "suba"
            && op != "subi"
            && op != "subq"
            && op != "subx"
            && op != "sbcd" =>
        {
            Ok(a::scc(cond(&op[1..])?, ea(&ops[0], labels, pc, resolve)?))
        }
        _ if op.starts_with('s') => Ok(a::scc(cond(&op[1..])?, ea(&ops[0], labels, pc, resolve)?)),
        _ => Err(Diagnostic::new(format!(
            "test assembler does not support 68000 instruction `{text}`"
        ))),
    }
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
    let mut out = Vec::new();
    let mut start = 0;
    let mut depth = 0i32;
    for (i, c) in s.char_indices() {
        match c {
            '(' => depth += 1,
            ')' => depth -= 1,
            ',' if depth == 0 => {
                out.push(s[start..i].trim().to_string());
                start = i + 1;
            }
            _ => {}
        }
    }
    if !s[start..].trim().is_empty() {
        out.push(s[start..].trim().to_string());
    }
    out
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
    if let Some(x) = s.strip_prefix("-(").and_then(|x| x.strip_suffix(')')) {
        return Ok(AM::Ariwpr(areg(x)?));
    }
    if let Some(x) = s.strip_prefix('(').and_then(|x| x.strip_suffix(")+")) {
        return Ok(AM::Ariwpo(areg(x)?));
    }
    let explicit = s
        .strip_suffix(".w")
        .map(|x| (x, false))
        .or_else(|| s.strip_suffix(".l").map(|x| (x, true)));
    if let Some((x, long)) = explicit {
        let v = val(x.trim_matches(['(', ')']), l, pc, r)?;
        return Ok(if long {
            AM::AbsLong(v)
        } else {
            AM::AbsShort(v as u16)
        });
    }
    if let Some(inner) = s.strip_prefix('(').and_then(|x| x.strip_suffix(')')) {
        let parts = split_operands(inner);
        if parts.len() == 1 {
            return Ok(AM::Ari(areg(&parts[0])?));
        }
        return indexed_ea(&parts, l, pc, r);
    }
    if let Some((disp, inner)) = s.rsplit_once('(') {
        if let Some(inner) = inner.strip_suffix(')') {
            let mut parts = vec![disp.trim().to_owned()];
            parts.extend(split_operands(inner));
            return indexed_ea(&parts, l, pc, r);
        }
    }
    let v = val(s, l, pc, r)?;
    Ok(if v <= 0xffff {
        AM::AbsShort(v as u16)
    } else {
        AM::AbsLong(v)
    })
}

fn indexed_ea(
    parts: &[String],
    l: &HashMap<String, u32>,
    pc: u32,
    r: bool,
) -> Result<AM, Diagnostic> {
    if !(2..=3).contains(&parts.len()) {
        return Err(Diagnostic::new(
            "invalid 68000 parenthesized effective address",
        ));
    }
    let disp_text = &parts[0];
    let base = parts[1].trim();
    let pc_base = base.eq_ignore_ascii_case("pc");
    let disp = if pc_base
        && r
        && l.iter()
            .any(|(name, _)| name.eq_ignore_ascii_case(disp_text))
    {
        (val(disp_text, l, pc, r)? as i64 - (pc as i64 + 2)) as i16
    } else {
        signed_val(disp_text, l, r)? as i16
    };
    if parts.len() == 2 {
        return if pc_base {
            Ok(AM::Pciwd(pc, disp))
        } else {
            Ok(AM::Ariwd(areg(base)?, disp))
        };
    }
    let index = parts[2].trim();
    let (register, long) = if let Some(x) = index.strip_suffix(".w") {
        (x, false)
    } else if let Some(x) = index.strip_suffix(".l") {
        (x, true)
    } else {
        return Err(Diagnostic::new("68000 index register requires .w or .l"));
    };
    let reg = if let Ok(n) = dreg(register) {
        n
    } else {
        8 + areg(register)?
    };
    if !(-128..=127).contains(&(disp as i32)) {
        return Err(Diagnostic::new(
            "68000 indexed displacement is outside -128..127",
        ));
    }
    let ext =
        BriefExtensionWord((u16::from(reg) << 12) | (u16::from(long) << 11) | (disp as u8 as u16));
    if pc_base {
        Ok(AM::Pciwi8(pc, ext))
    } else {
        Ok(AM::Ariwi8(areg(base)?, ext))
    }
}
fn signed_val(s: &str, l: &HashMap<String, u32>, r: bool) -> Result<i64, Diagnostic> {
    let s = s.trim().trim_start_matches('#');
    if let Ok(v) = s.parse::<i64>() {
        return Ok(v);
    }
    Ok(val(s, l, 0, r)? as i64)
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
        "f" | "ra" => Condition::F,
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
    if o[0].as_str() == "usp" || o[1].as_str() == "usp" {
        moveusp(o)
    } else if o[1].as_str() == "ccr" {
        Ok(a::moveccr(ea(&o[0], l, pc, r)?))
    } else if o[1].as_str() == "sr" {
        Ok(a::movesr(ea(&o[0], l, pc, r)?))
    } else if o[0].as_str() == "sr" {
        Ok(a::movefsr(ea(&o[1], l, pc, r)?))
    } else if o[0].as_str() == "ccr" {
        let am = ea(&o[1], l, pc, r)?;
        let (field, ext) = am.assemble(false);
        let mut words = vec![0x42c0 | field];
        words.extend(ext.iter());
        Ok(words)
    } else if let Ok(ar) = areg(&o[1]) {
        Ok(a::movea(sz, ar, ea(&o[0], l, pc, r)?))
    } else {
        let source = ea(&o[0], l, pc, r)?;
        if sz == Size::Byte && matches!(source, AM::Ard(_)) {
            return Err(Diagnostic::new(
                "68000 MOVE.B cannot use an address-register source",
            ));
        }
        let (dop, dext) = ea(&o[1], l, pc, r)?.assemble_move_dst();
        let (sop, sext) = source.assemble(sz.is_long());
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
fn bcd(op: &str, o: &[String]) -> Result<Vec<u16>, Diagnostic> {
    if o.len() != 2 {
        return Err(Diagnostic::new(format!("68000 {op} requires two operands")));
    }
    let (src, dst, memory) = match (dreg(&o[0]), dreg(&o[1])) {
        (Ok(s), Ok(d)) => (s, d, false),
        _ => (areg_pre(&o[0])?, areg_pre(&o[1])?, true),
    };
    Ok(vec![if op == "abcd" {
        a::abcd(
            dst,
            if memory {
                Direction::MemoryToMemory
            } else {
                Direction::RegisterToRegister
            },
            src,
        )
    } else {
        a::sbcd(
            dst,
            if memory {
                Direction::MemoryToMemory
            } else {
                Direction::RegisterToRegister
            },
            src,
        )
    }])
}
fn add_sub_x(op: &str, size: Size, o: &[String]) -> Result<Vec<u16>, Diagnostic> {
    if o.len() != 2 {
        return Err(Diagnostic::new(format!("68000 {op} requires two operands")));
    }
    let (src, dst, memory) = match (dreg(&o[0]), dreg(&o[1])) {
        (Ok(s), Ok(d)) => (s, d, false),
        _ => (areg_pre(&o[0])?, areg_pre(&o[1])?, true),
    };
    Ok(vec![if op == "addx" {
        a::addx(
            dst,
            size,
            if memory {
                Direction::MemoryToMemory
            } else {
                Direction::RegisterToRegister
            },
            src,
        )
    } else {
        a::subx(
            dst,
            size,
            if memory {
                Direction::MemoryToMemory
            } else {
                Direction::RegisterToRegister
            },
            src,
        )
    }])
}
fn areg_pre(s: &str) -> Result<u8, Diagnostic> {
    s.trim()
        .strip_prefix("-(")
        .and_then(|x| x.strip_suffix(')'))
        .ok_or_else(|| Diagnostic::new("expected 68000 predecrement address register"))
        .and_then(areg)
}
fn areg_post(s: &str) -> Result<u8, Diagnostic> {
    s.trim()
        .strip_prefix('(')
        .and_then(|x| x.strip_suffix(")+"))
        .ok_or_else(|| Diagnostic::new("expected 68000 postincrement address register"))
        .and_then(areg)
}
fn exg(o: &[String]) -> Result<Vec<u16>, Diagnostic> {
    if o.len() != 2 {
        return Err(Diagnostic::new("68000 exg requires two registers"));
    }
    let w = match (dreg(&o[0]), dreg(&o[1]), areg(&o[0]), areg(&o[1])) {
        (Ok(x), Ok(y), _, _) => a::exg(x, Direction::ExchangeData, y),
        (_, _, Ok(x), Ok(y)) => a::exg(x, Direction::ExchangeAddress, y),
        (Ok(x), _, _, Ok(y)) => a::exg(x, Direction::ExchangeDataAddress, y),
        (_, Ok(y), Ok(x), _) => a::exg(y, Direction::ExchangeDataAddress, x),
        _ => return Err(Diagnostic::new("invalid 68000 exg registers")),
    };
    Ok(vec![w])
}
fn moveusp(o: &[String]) -> Result<Vec<u16>, Diagnostic> {
    if o.len() != 2 {
        return Err(Diagnostic::new("68000 moveusp requires two operands"));
    }
    if o[0] == "usp" {
        Ok(vec![a::moveusp(Direction::UspToRegister, areg(&o[1])?)])
    } else if o[1] == "usp" {
        Ok(vec![a::moveusp(Direction::RegisterToUsp, areg(&o[0])?)])
    } else {
        Err(Diagnostic::new("68000 moveusp requires usp and An"))
    }
}
fn movep(
    size: Size,
    o: &[String],
    l: &HashMap<String, u32>,
    _pc: u32,
    r: bool,
) -> Result<Vec<u16>, Diagnostic> {
    if o.len() != 2 {
        return Err(Diagnostic::new("68000 movep requires two operands"));
    }
    let parse_mem = |s: &str| -> Result<(u8, i16), Diagnostic> {
        let (d, a) = s
            .trim()
            .rsplit_once('(')
            .ok_or_else(|| Diagnostic::new("MOVEP memory operand must be d16(An)"))?;
        Ok((areg(a.trim_end_matches(')'))?, signed_val(d, l, r)? as i16))
    };
    let w = if let Ok(d) = dreg(&o[0]) {
        let (a0, disp) = parse_mem(&o[1])?;
        a::movep(d, Direction::RegisterToMemory, size, a0, disp)
    } else {
        let (a0, disp) = parse_mem(&o[0])?;
        a::movep(dreg(&o[1])?, Direction::MemoryToRegister, size, a0, disp)
    };
    Ok(w.to_vec())
}
fn movem(
    size: Size,
    o: &[String],
    l: &HashMap<String, u32>,
    pc: u32,
    r: bool,
) -> Result<Vec<u16>, Diagnostic> {
    if o.len() != 2 {
        return Err(Diagnostic::new(
            "68000 movem requires a register list and an effective address",
        ));
    }
    if looks_like_reg_list(&o[0]) {
        Ok(a::movem(
            Direction::RegisterToMemory,
            size,
            ea(&o[1], l, pc, r)?,
            register_mask(&o[0])?,
        ))
    } else {
        Ok(a::movem(
            Direction::MemoryToRegister,
            size,
            ea(&o[0], l, pc, r)?,
            register_mask(&o[1])?,
        ))
    }
}
fn looks_like_reg_list(s: &str) -> bool {
    s.contains('/') || s.contains('-') || dreg(s).is_ok() || areg(s).is_ok()
}
fn register_mask(s: &str) -> Result<u16, Diagnostic> {
    let mut mask = 0u16;
    for item in s.split('/') {
        let item = item.trim();
        let (first, last) = item.split_once('-').unwrap_or((item, item));
        let reg_num = |x: &str| -> Result<u8, Diagnostic> {
            if let Ok(n) = dreg(x) {
                Ok(n)
            } else {
                Ok(8 + areg(x)?)
            }
        };
        let a = reg_num(first)?;
        let b = reg_num(last)?;
        if a > b {
            return Err(Diagnostic::new("68000 register-list ranges must ascend"));
        }
        for n in a..=b {
            mask |= 1 << n;
        }
    }
    Ok(mask)
}
fn bit_op(
    op: &str,
    o: &[String],
    l: &HashMap<String, u32>,
    pc: u32,
    r: bool,
) -> Result<Vec<u16>, Diagnostic> {
    if o.len() != 2 {
        return Err(Diagnostic::new(format!("68000 {op} requires two operands")));
    }
    let am = ea(&o[1], l, pc, r)?;
    let result = if let Some(x) = o[0].strip_prefix('#') {
        let bit = val(x, l, pc, r)? as u8;
        match op {
            "btst" => a::btst_static(am, bit),
            "bchg" => a::bchg_static(am, bit),
            "bclr" => a::bclr_static(am, bit),
            _ => a::bset_static(am, bit),
        }
    } else {
        let bit = dreg(&o[0])?;
        match op {
            "btst" => a::btst_dynamic(bit, am),
            "bchg" => a::bchg_dynamic(bit, am),
            "bclr" => a::bclr_dynamic(bit, am),
            _ => a::bset_dynamic(bit, am),
        }
    };
    Ok(result)
}
fn shift_or_rotate(
    op: &str,
    size: Option<Size>,
    o: &[String],
    l: &HashMap<String, u32>,
    pc: u32,
    r: bool,
) -> Result<Vec<u16>, Diagnostic> {
    if o.len() == 1 {
        let am = ea(&o[0], l, pc, r)?;
        let left = op.ends_with('l');
        return Ok(match &op[..2] {
            "as" => a::asm(
                if left {
                    Direction::Left
                } else {
                    Direction::Right
                },
                am,
            ),
            "ls" => a::lsm(
                if left {
                    Direction::Left
                } else {
                    Direction::Right
                },
                am,
            ),
            "ro" if op.starts_with("rox") => a::roxm(
                if left {
                    Direction::Left
                } else {
                    Direction::Right
                },
                am,
            ),
            "ro" => a::rom(
                if left {
                    Direction::Left
                } else {
                    Direction::Right
                },
                am,
            ),
            _ => unreachable!(),
        });
    }
    let size =
        size.ok_or_else(|| Diagnostic::new("68000 register shift/rotate requires .b, .w, or .l"))?;
    if o.len() != 2 {
        return Err(Diagnostic::new("invalid 68000 shift/rotate operands"));
    }
    let count_text = &o[0];
    let (count, reg_count) = if let Some(x) = count_text.strip_prefix('#') {
        let n = val(x, l, pc, r)?;
        if !(1..=8).contains(&n) {
            return Err(Diagnostic::new("68000 shift count must be in 1..8"));
        }
        ((n & 7) as u16, false)
    } else {
        (u16::from(dreg(count_text)?), true)
    };
    let reg = u16::from(dreg(&o[1])?);
    let left = op.ends_with('l');
    let dir = if left {
        Direction::Left
    } else {
        Direction::Right
    };
    Ok(vec![match &op[..2] {
        "as" => a::asr(count, dir, size, reg_count, reg),
        "ls" => a::lsr(count, dir, size, reg_count, reg),
        "ro" if op.starts_with("rox") => a::roxr(count, dir, size, reg_count, reg),
        "ro" => a::ror(count, dir, size, reg_count, reg),
        _ => unreachable!(),
    }])
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

#[cfg(test)]
mod tests {
    use super::*;

    fn labels() -> HashMap<String, u32> {
        HashMap::from([
            ("target".to_owned(), 0x1010),
            ("long".to_owned(), 0x1234_5678),
        ])
    }
    #[test]
    fn golden_instruction_and_effective_address_encodings() {
        let cases: &[(&str, &[u8])] = &[
            ("move.b d0,(a1)+", &[0x12, 0xc0]),
            (
                "move.l #$12345678,d0",
                &[0x20, 0x3c, 0x12, 0x34, 0x56, 0x78],
            ),
            ("move.w (4,a0,d1.w),d2", &[0x34, 0x30, 0x10, 0x04]),
            ("move.w (4,pc,d1.l),d2", &[0x34, 0x3b, 0x18, 0x04]),
            ("move.w $1234.w,d0", &[0x30, 0x38, 0x12, 0x34]),
            (
                "move.l $12345678.l,d0",
                &[0x20, 0x39, 0x12, 0x34, 0x56, 0x78],
            ),
            ("addx.w d1,d2", &[0xd5, 0x41]),
            ("subx.b -(a1),-(a2)", &[0x95, 0x09]),
            ("abcd d1,d2", &[0xc5, 0x01]),
            ("movep.l d1,4(a2)", &[0x03, 0xca, 0x00, 0x04]),
            ("movem.l d0-d2/a1,-(sp)", &[0x48, 0xe7, 0x02, 0x07]),
            ("exg d1,a2", &[0xc3, 0x8a]),
            ("rol.w #8,d0", &[0xe1, 0x58]),
            ("move usp,a3", &[0x4e, 0x6b]),
            ("move ccr,(a0)", &[0x42, 0xd0]),
            ("move sr,(a0)", &[0x40, 0xd0]),
            ("move (a0),sr", &[0x46, 0xd0]),
        ];
        for (source, expected) in cases {
            assert_eq!(
                encode(source, &labels(), 0x1000, true).unwrap(),
                *expected,
                "{source}"
            );
            assert_eq!(instruction_len(source).unwrap(), expected.len(), "{source}");
        }
    }
    #[test]
    fn every_official_family_has_a_table_driven_smoke_case() {
        let cases = [
            "abcd d0,d1",
            "add.b d0,d1",
            "adda.w d0,a1",
            "addi.b #1,d0",
            "addq.w #8,d0",
            "addx.l d0,d1",
            "and.w d0,d1",
            "andi.w #1,d0",
            "asl.w #1,d0",
            "asl (a0)",
            "bra target",
            "bsr target",
            "bne target",
            "bchg #1,d0",
            "bclr d0,(a0)",
            "bset #1,(a0)",
            "btst d0,(a0)",
            "chk (a0),d0",
            "clr.w d0",
            "cmp.w d0,d1",
            "cmpa.w d0,a1",
            "cmpi.w #1,d0",
            "cmpm.w (a0)+,(a1)+",
            "dbra d0,target",
            "divs (a0),d0",
            "divu (a0),d0",
            "eor.w d0,d1",
            "eori.w #1,d0",
            "exg d0,d1",
            "ext.w d0",
            "illegal",
            "jmp (a0)",
            "jsr (a0)",
            "lea (a0),a1",
            "link a0,#-4",
            "lsl.w #1,d0",
            "lsr (a0)",
            "move.w d0,d1",
            "movea.w d0,a1",
            "move ccr,(a0)",
            "move (a0),ccr",
            "move sr,(a0)",
            "move (a0),sr",
            "moveusp a0,usp",
            "movem.w d0-d1,(a0)",
            "movep.w d0,4(a0)",
            "moveq #1,d0",
            "muls (a0),d0",
            "mulu (a0),d0",
            "nbcd d0",
            "neg.w d0",
            "negx.w d0",
            "nop",
            "not.w d0",
            "or.w d0,d1",
            "ori.w #1,d0",
            "pea (a0)",
            "reset",
            "rol.w #1,d0",
            "ror (a0)",
            "roxl.w #1,d0",
            "roxr (a0)",
            "rte",
            "rtr",
            "rts",
            "sbcd d0,d1",
            "seq d0",
            "stop #$2700",
            "sub.w d0,d1",
            "suba.w d0,a1",
            "subi.w #1,d0",
            "subq.w #1,d0",
            "subx.w d0,d1",
            "swap d0",
            "tas d0",
            "trap #15",
            "trapv",
            "tst.w d0",
            "unlk a0",
        ];
        for source in cases {
            assert!(encode(source, &labels(), 0x1000, true).is_ok(), "{source}");
        }
    }
    #[test]
    fn rejects_invalid_forms_and_boundaries() {
        for source in [
            "move.x d0,d1",
            "move.b a0,d0",
            "movea.b d0,a0",
            "addq.w #0,d0",
            "addq.w #9,d0",
            "trap #16",
            "asl.w #0,d0",
            "asl.w #9,d0",
            "movem.w d2-d0,(a0)",
            "movep.b d0,0(a0)",
            "move.w (128,a0,d0.w),d1",
            "move.w (0,a0,d0),d1",
            "exg d0,(a0)",
        ] {
            assert!(encode(source, &labels(), 0x1000, true).is_err(), "{source}");
        }
    }
}
