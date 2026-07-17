//! Strict Intel 8086 instruction encoder.
//!
//! This module intentionally excludes undocumented opcodes and every 80186+
//! extension. Bare targets use a single 64 KiB code/data segment; explicit far
//! operands still encode their complete 16:16 pointer.

use crate::asm::frontend::{
    AssemblyBinaryOperator, AssemblyExpression, AssemblyUnaryOperator, parse_assembly_expression,
};
use crate::compat::prelude::*;
use crate::diagnostic::Diagnostic;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Width {
    Byte,
    Word,
}

impl Width {
    const fn bit(self) -> u8 {
        match self {
            Self::Byte => 0,
            Self::Word => 1,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct Register {
    width: Width,
    code: u8,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Distance {
    Short,
    Near,
    Far,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct Memory {
    width: Option<Width>,
    far: bool,
    segment: Option<u8>,
    registers: Vec<&'static str>,
    displacement: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum OperandKind {
    Register(Register),
    Segment(u8),
    Memory(Memory),
    Immediate(String),
    FarImmediate { segment: String, offset: String },
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct Operand {
    distance: Option<Distance>,
    kind: OperandKind,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum RepeatPrefix {
    Rep,
    Repne,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct ParsedInstruction {
    mnemonic: String,
    operands: Vec<Operand>,
    lock: bool,
    repeat: Option<RepeatPrefix>,
    leading_segment: Option<u8>,
}

#[derive(Clone, Debug)]
struct CoreEncoding {
    bytes: Vec<u8>,
    segment: Option<u8>,
    memory_access: bool,
    memory_write: bool,
}

impl CoreEncoding {
    fn plain(bytes: impl Into<Vec<u8>>) -> Self {
        Self {
            bytes: bytes.into(),
            segment: None,
            memory_access: false,
            memory_write: false,
        }
    }

    fn with_rm(bytes: Vec<u8>, rm: &RmEncoding, memory_write: bool) -> Self {
        Self {
            bytes,
            segment: rm.segment,
            memory_access: rm.is_memory,
            memory_write: rm.is_memory && memory_write,
        }
    }
}

#[derive(Clone, Debug)]
struct RmEncoding {
    bytes: Vec<u8>,
    segment: Option<u8>,
    is_memory: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Eval {
    Known(i128),
    Unknown,
}

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
    if resolve && pc > 0xFFFF {
        return Err(error(format!(
            "8086 instruction address 0x{pc:X} is outside the current 64 KiB segment"
        )));
    }
    let instruction = parse_instruction(text)?;
    let core = encode_core(&instruction, labels, pc, resolve)?;
    validate_prefixes(&instruction, &core)?;

    let segment = merge_segment_overrides(instruction.leading_segment, core.segment)?;
    let mut bytes = Vec::new();
    if instruction.lock {
        bytes.push(0xF0);
    } else if let Some(repeat) = instruction.repeat {
        bytes.push(match repeat {
            RepeatPrefix::Rep => 0xF3,
            RepeatPrefix::Repne => 0xF2,
        });
    }
    if let Some(segment) = segment {
        bytes.push(segment_prefix(segment));
    }
    bytes.extend(core.bytes);
    Ok(bytes)
}

fn parse_instruction(text: &str) -> Result<ParsedInstruction, Diagnostic> {
    let source = text.trim().to_ascii_lowercase();
    if source.is_empty() {
        return Err(error("empty 8086 instruction"));
    }

    let mut rest = source.as_str();
    let mut lock = false;
    let mut repeat = None;
    let mut leading_segment = None;
    loop {
        if let Some((segment, tail)) = ["es", "cs", "ss", "ds"].into_iter().find_map(|segment| {
            rest.strip_prefix(segment)
                .and_then(|tail| tail.strip_prefix(':'))
                .map(|tail| (segment, tail))
        }) {
            if leading_segment.replace(segment_code(segment)?).is_some() {
                return Err(error("multiple segment override prefixes"));
            }
            rest = tail.trim_start();
            continue;
        }
        let (word, tail) = take_word(rest);
        match word {
            "lock" => {
                if lock {
                    return Err(error("duplicate LOCK prefix"));
                }
                lock = true;
                rest = tail;
            }
            "rep" | "repe" | "repz" => {
                if repeat.replace(RepeatPrefix::Rep).is_some() {
                    return Err(error("duplicate or conflicting repeat prefix"));
                }
                rest = tail;
            }
            "repne" | "repnz" => {
                if repeat.replace(RepeatPrefix::Repne).is_some() {
                    return Err(error("duplicate or conflicting repeat prefix"));
                }
                rest = tail;
            }
            "es:" | "cs:" | "ss:" | "ds:" => {
                if leading_segment.replace(segment_code(&word[..2])?).is_some() {
                    return Err(error("multiple segment override prefixes"));
                }
                rest = tail;
            }
            _ => break,
        }
    }

    let (mnemonic, operand_text) = take_word(rest);
    if mnemonic.is_empty() {
        return Err(error("8086 prefix is missing an instruction"));
    }
    let operands = split_operands(operand_text)?
        .into_iter()
        .map(parse_operand)
        .collect::<Result<Vec<_>, _>>()?;
    Ok(ParsedInstruction {
        mnemonic: mnemonic.to_owned(),
        operands,
        lock,
        repeat,
        leading_segment,
    })
}

fn take_word(text: &str) -> (&str, &str) {
    let text = text.trim_start();
    text.find(char::is_whitespace).map_or((text, ""), |index| {
        (&text[..index], text[index..].trim_start())
    })
}

fn split_operands(text: &str) -> Result<Vec<&str>, Diagnostic> {
    let text = text.trim();
    if text.is_empty() {
        return Ok(Vec::new());
    }
    let mut result = Vec::new();
    let mut start = 0;
    let mut square = 0usize;
    let mut round = 0usize;
    for (index, ch) in text.char_indices() {
        match ch {
            '[' => square += 1,
            ']' => square = square.saturating_sub(1),
            '(' => round += 1,
            ')' => round = round.saturating_sub(1),
            ',' if square == 0 && round == 0 => {
                let operand = text[start..index].trim();
                if operand.is_empty() {
                    return Err(error("empty 8086 operand"));
                }
                result.push(operand);
                start = index + 1;
            }
            _ => {}
        }
    }
    let operand = text[start..].trim();
    if operand.is_empty() {
        return Err(error("empty 8086 operand"));
    }
    result.push(operand);
    Ok(result)
}

fn parse_operand(text: &str) -> Result<Operand, Diagnostic> {
    let mut text = text.trim();
    let mut distance = None;
    for (word, value) in [
        ("short", Distance::Short),
        ("near", Distance::Near),
        ("far", Distance::Far),
    ] {
        if let Some(tail) = strip_word_prefix(text, word) {
            distance = Some(value);
            text = tail;
            break;
        }
    }

    let mut width = None;
    let mut far_ptr = distance == Some(Distance::Far);
    if distance.is_some()
        && let Some(tail) = strip_word_prefix(text, "ptr")
    {
        text = tail;
    }
    if let Some(tail) = strip_word_prefix(text, "byte") {
        width = Some(Width::Byte);
        text = tail;
        if let Some(tail) = strip_word_prefix(text, "ptr") {
            text = tail;
        }
    } else if let Some(tail) = strip_word_prefix(text, "word") {
        width = Some(Width::Word);
        text = tail;
        if let Some(tail) = strip_word_prefix(text, "ptr") {
            text = tail;
        }
    } else if let Some(tail) = strip_word_prefix(text, "far") {
        if distance.is_some() {
            return Err(error("conflicting 8086 distance qualifiers"));
        }
        far_ptr = true;
        distance = Some(Distance::Far);
        text = tail;
        if let Some(tail) = strip_word_prefix(text, "ptr") {
            text = tail;
        }
    }

    if let Some(register) = register(text) {
        if width.is_some() || far_ptr {
            return Err(error(
                "register operand cannot have a memory size qualifier",
            ));
        }
        return Ok(Operand {
            distance,
            kind: OperandKind::Register(register),
        });
    }
    if let Ok(segment) = segment_code(text) {
        if width.is_some() || far_ptr {
            return Err(error(
                "segment register cannot have a memory size qualifier",
            ));
        }
        return Ok(Operand {
            distance,
            kind: OperandKind::Segment(segment),
        });
    }

    let (segment, memory_text) = parse_operand_segment(text)?;
    if memory_text.starts_with('[') && memory_text.ends_with(']') {
        let inner = &memory_text[1..memory_text.len() - 1];
        let (registers, displacement) = parse_effective_address(inner)?;
        return Ok(Operand {
            distance,
            kind: OperandKind::Memory(Memory {
                width,
                far: far_ptr,
                segment,
                registers,
                displacement,
            }),
        });
    }
    if segment.is_some() {
        return Err(error(
            "segment override must prefix a bracketed memory operand",
        ));
    }
    if width.is_some() || far_ptr && !text.contains(':') {
        return Err(error("PTR qualifier requires a bracketed memory operand"));
    }
    if let Some(index) = find_top_level_colon(text) {
        if distance.is_some_and(|distance| distance != Distance::Far) {
            return Err(error("far pointer conflicts with SHORT or NEAR qualifier"));
        }
        let segment = text[..index].trim();
        let offset = text[index + 1..].trim();
        if segment.is_empty() || offset.is_empty() {
            return Err(error("far pointer requires `segment:offset`"));
        }
        return Ok(Operand {
            distance: Some(Distance::Far),
            kind: OperandKind::FarImmediate {
                segment: segment.to_owned(),
                offset: offset.to_owned(),
            },
        });
    }
    Ok(Operand {
        distance,
        kind: OperandKind::Immediate(text.to_owned()),
    })
}

fn strip_word_prefix<'a>(text: &'a str, word: &str) -> Option<&'a str> {
    let tail = text.strip_prefix(word)?;
    if tail.is_empty() || tail.starts_with(char::is_whitespace) {
        Some(tail.trim_start())
    } else {
        None
    }
}

fn parse_operand_segment(text: &str) -> Result<(Option<u8>, &str), Diagnostic> {
    for name in ["es", "cs", "ss", "ds"] {
        if let Some(tail) = text
            .strip_prefix(name)
            .and_then(|tail| tail.strip_prefix(':'))
        {
            return Ok((Some(segment_code(name)?), tail.trim_start()));
        }
    }
    Ok((None, text))
}

fn find_top_level_colon(text: &str) -> Option<usize> {
    let mut square = 0usize;
    let mut round = 0usize;
    for (index, ch) in text.char_indices() {
        match ch {
            '[' => square += 1,
            ']' => square = square.saturating_sub(1),
            '(' => round += 1,
            ')' => round = round.saturating_sub(1),
            ':' if square == 0 && round == 0 => return Some(index),
            _ => {}
        }
    }
    None
}

fn parse_effective_address(text: &str) -> Result<(Vec<&'static str>, Option<String>), Diagnostic> {
    let text = text.trim();
    if text.is_empty() {
        return Err(error("empty 8086 memory address"));
    }
    let terms = split_additive_terms(text)?;
    let mut registers = Vec::new();
    let mut displacement = String::new();
    for (sign, term) in terms {
        let normalized = term.trim().to_ascii_lowercase();
        let register_name = match normalized.as_str() {
            "bx" => Some("bx"),
            "bp" => Some("bp"),
            "si" => Some("si"),
            "di" => Some("di"),
            _ => None,
        };
        if register_name.is_none() && register(&normalized).is_some() {
            return Err(error(format!(
                "8086 register `{normalized}` cannot be used in an effective address"
            )));
        }
        if let Some(register) = register_name {
            if sign == '-' {
                return Err(error("8086 address registers cannot be subtracted"));
            }
            if registers.contains(&register) {
                return Err(error(format!(
                    "duplicate 8086 address register `{register}`"
                )));
            }
            registers.push(register);
        } else {
            if expression_mentions_register(term)? {
                return Err(error(format!(
                    "8086 address register is used in an unsupported expression `{term}`"
                )));
            }
            if !displacement.is_empty() {
                displacement.push(sign);
            } else if sign == '-' {
                displacement.push('-');
            }
            displacement.push_str(term.trim());
        }
    }
    validate_ea_registers(&registers)?;
    Ok((
        registers,
        (!displacement.is_empty()).then_some(displacement),
    ))
}

fn split_additive_terms(text: &str) -> Result<Vec<(char, &str)>, Diagnostic> {
    let mut terms = Vec::new();
    let mut start = 0usize;
    let mut sign = '+';
    let mut depth = 0usize;
    for (index, ch) in text.char_indices() {
        match ch {
            '(' => depth += 1,
            ')' => depth = depth.saturating_sub(1),
            '+' | '-' if depth == 0 => {
                if index == start {
                    sign = ch;
                    start = index + 1;
                    continue;
                }
                let term = text[start..index].trim();
                if term.is_empty() {
                    return Err(error("missing term in 8086 effective address"));
                }
                terms.push((sign, term));
                sign = ch;
                start = index + 1;
            }
            _ => {}
        }
    }
    let term = text[start..].trim();
    if term.is_empty() {
        return Err(error("missing final term in 8086 effective address"));
    }
    terms.push((sign, term));
    Ok(terms)
}

fn validate_ea_registers(registers: &[&str]) -> Result<(), Diagnostic> {
    let valid = matches!(
        registers,
        [] | ["bx"]
            | ["bp"]
            | ["si"]
            | ["di"]
            | ["bx", "si"]
            | ["si", "bx"]
            | ["bx", "di"]
            | ["di", "bx"]
            | ["bp", "si"]
            | ["si", "bp"]
            | ["bp", "di"]
            | ["di", "bp"]
    );
    if valid {
        Ok(())
    } else {
        Err(error(format!(
            "invalid 8086 effective-address register combination `{}`",
            registers.join("+")
        )))
    }
}

fn encode_core(
    instruction: &ParsedInstruction,
    labels: &HashMap<String, u32>,
    pc: u32,
    resolve: bool,
) -> Result<CoreEncoding, Diagnostic> {
    let op = instruction.mnemonic.as_str();
    validate_operand_qualifiers(instruction)?;
    if let Some(opcode) = fixed_opcode(op) {
        require_operands(op, &instruction.operands, 0)?;
        let bytes = if matches!(op, "aam" | "aad") {
            vec![opcode, 0x0A]
        } else {
            vec![opcode]
        };
        return Ok(CoreEncoding::plain(bytes));
    }
    match op {
        "movsb" | "movsw" | "cmpsb" | "cmpsw" | "stosb" | "stosw" | "lodsb" | "lodsw" | "scasb"
        | "scasw" => encode_string(instruction),
        "add" | "or" | "adc" | "sbb" | "and" | "sub" | "xor" | "cmp" => {
            encode_alu(op, &instruction.operands, labels, pc, resolve)
        }
        "inc" | "dec" => encode_inc_dec(op, &instruction.operands, labels, pc, resolve),
        "not" | "neg" | "mul" | "imul" | "div" | "idiv" => {
            encode_group3(op, &instruction.operands, labels, pc, resolve)
        }
        "rol" | "ror" | "rcl" | "rcr" | "shl" | "sal" | "shr" | "sar" => {
            encode_shift(op, &instruction.operands, labels, pc, resolve)
        }
        "mov" => encode_mov(&instruction.operands, labels, pc, resolve),
        "xchg" => encode_xchg(&instruction.operands, labels, pc, resolve),
        "test" => encode_test(&instruction.operands, labels, pc, resolve),
        "lea" | "les" | "lds" => {
            encode_address_load(op, &instruction.operands, labels, pc, resolve)
        }
        "xlat" | "xlatb" => {
            require_operands(op, &instruction.operands, 0)?;
            Ok(CoreEncoding {
                bytes: vec![0xD7],
                segment: None,
                memory_access: true,
                memory_write: false,
            })
        }
        "push" | "pop" => encode_stack(op, &instruction.operands, labels, pc, resolve),
        "ret" | "retn" | "retf" => encode_return(op, &instruction.operands, labels, pc, resolve),
        "call" | "jmp" => encode_call_jump(op, &instruction.operands, labels, pc, resolve),
        "jo" | "jno" | "jb" | "jc" | "jnae" | "jae" | "jnb" | "jnc" | "je" | "jz" | "jne"
        | "jnz" | "jbe" | "jna" | "ja" | "jnbe" | "js" | "jns" | "jp" | "jpe" | "jnp" | "jpo"
        | "jl" | "jnge" | "jge" | "jnl" | "jle" | "jng" | "jg" | "jnle" => encode_short_branch(
            op,
            jcc_opcode(op).expect("matched conditional jump"),
            &instruction.operands,
            labels,
            pc,
            resolve,
        ),
        "loopne" | "loopnz" | "loope" | "loopz" | "loop" | "jcxz" => encode_short_branch(
            op,
            match op {
                "loopne" | "loopnz" => 0xE0,
                "loope" | "loopz" => 0xE1,
                "loop" => 0xE2,
                "jcxz" => 0xE3,
                _ => unreachable!(),
            },
            &instruction.operands,
            labels,
            pc,
            resolve,
        ),
        "int" => encode_int(&instruction.operands, labels, pc, resolve),
        "int3" => {
            require_operands(op, &instruction.operands, 0)?;
            Ok(CoreEncoding::plain(vec![0xCC]))
        }
        "in" | "out" => encode_io(op, &instruction.operands, labels, pc, resolve),
        "esc" => encode_esc(&instruction.operands, labels, pc, resolve),
        _ => Err(error(format!(
            "assembler does not support 8086 instruction `{op}`"
        ))),
    }
}

fn fixed_opcode(op: &str) -> Option<u8> {
    Some(match op {
        "daa" => 0x27,
        "das" => 0x2F,
        "aaa" => 0x37,
        "aas" => 0x3F,
        "nop" => 0x90,
        "cbw" => 0x98,
        "cwd" => 0x99,
        "wait" | "fwait" => 0x9B,
        "pushf" => 0x9C,
        "popf" => 0x9D,
        "sahf" => 0x9E,
        "lahf" => 0x9F,
        "aam" => 0xD4,
        "aad" => 0xD5,
        "into" => 0xCE,
        "iret" => 0xCF,
        "hlt" => 0xF4,
        "cmc" => 0xF5,
        "clc" => 0xF8,
        "stc" => 0xF9,
        "cli" => 0xFA,
        "sti" => 0xFB,
        "cld" => 0xFC,
        "std" => 0xFD,
        _ => return None,
    })
}

fn encode_string(instruction: &ParsedInstruction) -> Result<CoreEncoding, Diagnostic> {
    require_operands(&instruction.mnemonic, &instruction.operands, 0)?;
    let opcode = match instruction.mnemonic.as_str() {
        "movsb" => 0xA4,
        "movsw" => 0xA5,
        "cmpsb" => 0xA6,
        "cmpsw" => 0xA7,
        "stosb" => 0xAA,
        "stosw" => 0xAB,
        "lodsb" => 0xAC,
        "lodsw" => 0xAD,
        "scasb" => 0xAE,
        "scasw" => 0xAF,
        _ => unreachable!(),
    };
    Ok(CoreEncoding {
        bytes: vec![opcode],
        segment: None,
        memory_access: true,
        memory_write: instruction.mnemonic.starts_with("movs")
            || instruction.mnemonic.starts_with("stos"),
    })
}

fn encode_alu(
    op: &str,
    operands: &[Operand],
    labels: &HashMap<String, u32>,
    pc: u32,
    resolve: bool,
) -> Result<CoreEncoding, Diagnostic> {
    require_operands(op, operands, 2)?;
    let base = match op {
        "add" => 0x00,
        "or" => 0x08,
        "adc" => 0x10,
        "sbb" => 0x18,
        "and" => 0x20,
        "sub" => 0x28,
        "xor" => 0x30,
        "cmp" => 0x38,
        _ => unreachable!(),
    };
    let group = (base >> 3) & 7;
    let writes = op != "cmp";
    match (&operands[0].kind, &operands[1].kind) {
        (OperandKind::Register(dst), OperandKind::Register(_) | OperandKind::Memory(_)) => {
            let rm = encode_rm(&operands[1], dst.width, dst.code, labels, pc, resolve)?;
            let mut bytes = vec![base + 2 + dst.width.bit()];
            bytes.extend(&rm.bytes);
            Ok(CoreEncoding::with_rm(bytes, &rm, false))
        }
        (OperandKind::Memory(_), OperandKind::Register(src)) => {
            let rm = encode_rm(&operands[0], src.width, src.code, labels, pc, resolve)?;
            let mut bytes = vec![base + src.width.bit()];
            bytes.extend(&rm.bytes);
            Ok(CoreEncoding::with_rm(bytes, &rm, writes))
        }
        (OperandKind::Register(dst), OperandKind::Immediate(expression)) => {
            let value = eval_text(expression, labels, pc, resolve)?;
            if dst.code == 0 {
                let mut bytes = vec![base + 4 + dst.width.bit()];
                push_immediate(&mut bytes, value, dst.width, op)?;
                return Ok(CoreEncoding::plain(bytes));
            }
            encode_alu_immediate(
                op,
                group,
                &operands[0],
                dst.width,
                expression,
                labels,
                pc,
                resolve,
            )
        }
        (OperandKind::Memory(memory), OperandKind::Immediate(expression)) => {
            let width = memory.width.ok_or_else(|| {
                error(format!(
                    "8086 {op} memory-immediate operand requires BYTE PTR or WORD PTR"
                ))
            })?;
            encode_alu_immediate(
                op,
                group,
                &operands[0],
                width,
                expression,
                labels,
                pc,
                resolve,
            )
        }
        _ => Err(error(format!("invalid 8086 {op} operand combination"))),
    }
}

fn encode_alu_immediate(
    op: &str,
    group: u8,
    destination: &Operand,
    width: Width,
    expression: &str,
    labels: &HashMap<String, u32>,
    pc: u32,
    resolve: bool,
) -> Result<CoreEncoding, Diagnostic> {
    let value = eval_text(expression, labels, pc, resolve)?;
    let use_sign_extended = width == Width::Word
        && expression_is_absolute(expression)?
        && matches!(value, Eval::Known(value) if (-128..=127).contains(&value) || (0xFF80..=0xFFFF).contains(&value));
    let opcode = match (width, use_sign_extended) {
        (Width::Byte, _) => 0x80,
        (Width::Word, false) => 0x81,
        (Width::Word, true) => 0x83,
    };
    let rm = encode_rm(destination, width, group, labels, pc, resolve)?;
    let mut bytes = vec![opcode];
    bytes.extend(&rm.bytes);
    if use_sign_extended {
        push_signed_byte(&mut bytes, value, "sign-extended immediate")?;
    } else {
        push_immediate(&mut bytes, value, width, op)?;
    }
    Ok(CoreEncoding::with_rm(bytes, &rm, op != "cmp"))
}

fn encode_inc_dec(
    op: &str,
    operands: &[Operand],
    labels: &HashMap<String, u32>,
    pc: u32,
    resolve: bool,
) -> Result<CoreEncoding, Diagnostic> {
    require_operands(op, operands, 1)?;
    let extension = u8::from(op == "dec");
    if let OperandKind::Register(register) = operands[0].kind
        && register.width == Width::Word
    {
        return Ok(CoreEncoding::plain(vec![
            if op == "inc" { 0x40 } else { 0x48 } + register.code,
        ]));
    }
    let width = operand_width(&operands[0], op)?;
    let rm = encode_rm(&operands[0], width, extension, labels, pc, resolve)?;
    let mut bytes = vec![if width == Width::Byte { 0xFE } else { 0xFF }];
    bytes.extend(&rm.bytes);
    Ok(CoreEncoding::with_rm(bytes, &rm, true))
}

fn encode_group3(
    op: &str,
    operands: &[Operand],
    labels: &HashMap<String, u32>,
    pc: u32,
    resolve: bool,
) -> Result<CoreEncoding, Diagnostic> {
    require_operands(op, operands, 1)?;
    let width = operand_width(&operands[0], op)?;
    let extension = match op {
        "not" => 2,
        "neg" => 3,
        "mul" => 4,
        "imul" => 5,
        "div" => 6,
        "idiv" => 7,
        _ => unreachable!(),
    };
    let rm = encode_rm(&operands[0], width, extension, labels, pc, resolve)?;
    let mut bytes = vec![if width == Width::Byte { 0xF6 } else { 0xF7 }];
    bytes.extend(&rm.bytes);
    Ok(CoreEncoding::with_rm(
        bytes,
        &rm,
        matches!(op, "not" | "neg"),
    ))
}

fn encode_shift(
    op: &str,
    operands: &[Operand],
    labels: &HashMap<String, u32>,
    pc: u32,
    resolve: bool,
) -> Result<CoreEncoding, Diagnostic> {
    require_operands(op, operands, 2)?;
    let width = operand_width(&operands[0], op)?;
    let extension = match op {
        "rol" => 0,
        "ror" => 1,
        "rcl" => 2,
        "rcr" => 3,
        "shl" | "sal" => 4,
        "shr" => 5,
        "sar" => 7,
        _ => unreachable!(),
    };
    let by_cl = match &operands[1].kind {
        OperandKind::Register(count) if *count == register("cl").unwrap() => true,
        OperandKind::Immediate(expression) => {
            let value = eval_text(expression, labels, pc, resolve)?;
            match value {
                Eval::Known(1) | Eval::Unknown => false,
                Eval::Known(_) => {
                    return Err(error(
                        "8086 shift count must be exactly 1 or the CL register",
                    ));
                }
            }
        }
        _ => {
            return Err(error(
                "8086 shift count must be exactly 1 or the CL register",
            ));
        }
    };
    let rm = encode_rm(&operands[0], width, extension, labels, pc, resolve)?;
    let mut bytes = vec![0xD0 + width.bit() + if by_cl { 2 } else { 0 }];
    bytes.extend(&rm.bytes);
    Ok(CoreEncoding::with_rm(bytes, &rm, true))
}

fn encode_mov(
    operands: &[Operand],
    labels: &HashMap<String, u32>,
    pc: u32,
    resolve: bool,
) -> Result<CoreEncoding, Diagnostic> {
    require_operands("mov", operands, 2)?;
    match (&operands[0].kind, &operands[1].kind) {
        (OperandKind::Register(dst), OperandKind::Immediate(expression)) => {
            let mut bytes = vec![if dst.width == Width::Byte { 0xB0 } else { 0xB8 } + dst.code];
            push_immediate(
                &mut bytes,
                eval_text(expression, labels, pc, resolve)?,
                dst.width,
                "MOV",
            )?;
            Ok(CoreEncoding::plain(bytes))
        }
        (OperandKind::Register(dst), OperandKind::Register(_) | OperandKind::Memory(_)) => {
            if dst.code == 0
                && let OperandKind::Memory(memory) = &operands[1].kind
                && memory.registers.is_empty()
            {
                validate_memory_width(memory, dst.width)?;
                let mut bytes = vec![if dst.width == Width::Byte { 0xA0 } else { 0xA1 }];
                push_memory_offset(&mut bytes, memory, labels, pc, resolve)?;
                return Ok(CoreEncoding {
                    bytes,
                    segment: memory.segment,
                    memory_access: true,
                    memory_write: false,
                });
            }
            let rm = encode_rm(&operands[1], dst.width, dst.code, labels, pc, resolve)?;
            let mut bytes = vec![0x8A + dst.width.bit()];
            bytes.extend(&rm.bytes);
            Ok(CoreEncoding::with_rm(bytes, &rm, false))
        }
        (OperandKind::Memory(memory), OperandKind::Register(src)) => {
            if src.code == 0 && memory.registers.is_empty() {
                validate_memory_width(memory, src.width)?;
                let mut bytes = vec![if src.width == Width::Byte { 0xA2 } else { 0xA3 }];
                push_memory_offset(&mut bytes, memory, labels, pc, resolve)?;
                return Ok(CoreEncoding {
                    bytes,
                    segment: memory.segment,
                    memory_access: true,
                    memory_write: true,
                });
            }
            let rm = encode_rm(&operands[0], src.width, src.code, labels, pc, resolve)?;
            let mut bytes = vec![0x88 + src.width.bit()];
            bytes.extend(&rm.bytes);
            Ok(CoreEncoding::with_rm(bytes, &rm, true))
        }
        (OperandKind::Memory(memory), OperandKind::Immediate(expression)) => {
            let width = memory.width.ok_or_else(|| {
                error("8086 MOV memory-immediate operand requires BYTE PTR or WORD PTR")
            })?;
            let rm = encode_rm(&operands[0], width, 0, labels, pc, resolve)?;
            let mut bytes = vec![if width == Width::Byte { 0xC6 } else { 0xC7 }];
            bytes.extend(&rm.bytes);
            push_immediate(
                &mut bytes,
                eval_text(expression, labels, pc, resolve)?,
                width,
                "MOV",
            )?;
            Ok(CoreEncoding::with_rm(bytes, &rm, true))
        }
        (OperandKind::Register(dst), OperandKind::Segment(segment)) if dst.width == Width::Word => {
            let rm = encode_rm(&operands[0], Width::Word, *segment, labels, pc, resolve)?;
            let mut bytes = vec![0x8C];
            bytes.extend(&rm.bytes);
            Ok(CoreEncoding::plain(bytes))
        }
        (OperandKind::Memory(_), OperandKind::Segment(segment)) => {
            let rm = encode_rm(&operands[0], Width::Word, *segment, labels, pc, resolve)?;
            let mut bytes = vec![0x8C];
            bytes.extend(&rm.bytes);
            Ok(CoreEncoding::with_rm(bytes, &rm, true))
        }
        (OperandKind::Segment(segment), OperandKind::Register(_) | OperandKind::Memory(_)) => {
            if *segment == 1 {
                return Err(error("8086 MOV cannot load CS"));
            }
            let rm = encode_rm(&operands[1], Width::Word, *segment, labels, pc, resolve)?;
            let mut bytes = vec![0x8E];
            bytes.extend(&rm.bytes);
            Ok(CoreEncoding::with_rm(bytes, &rm, false))
        }
        _ => Err(error("invalid 8086 MOV operand combination")),
    }
}

fn encode_xchg(
    operands: &[Operand],
    labels: &HashMap<String, u32>,
    pc: u32,
    resolve: bool,
) -> Result<CoreEncoding, Diagnostic> {
    require_operands("xchg", operands, 2)?;
    let (register_operand, rm_operand) = match (&operands[0].kind, &operands[1].kind) {
        (OperandKind::Register(register), OperandKind::Register(_) | OperandKind::Memory(_)) => {
            (*register, &operands[1])
        }
        (OperandKind::Memory(_), OperandKind::Register(register)) => (*register, &operands[0]),
        _ => return Err(error("8086 XCHG requires one register operand")),
    };
    if register_operand.width == Width::Word {
        if register_operand.code == 0
            && let OperandKind::Register(other) = rm_operand.kind
            && other.width == Width::Word
        {
            return Ok(CoreEncoding::plain(vec![0x90 + other.code]));
        }
        if let OperandKind::Register(other) = rm_operand.kind
            && other.width == Width::Word
            && other.code == 0
        {
            return Ok(CoreEncoding::plain(vec![0x90 + register_operand.code]));
        }
    }
    let rm = encode_rm(
        rm_operand,
        register_operand.width,
        register_operand.code,
        labels,
        pc,
        resolve,
    )?;
    let mut bytes = vec![if register_operand.width == Width::Byte {
        0x86
    } else {
        0x87
    }];
    bytes.extend(&rm.bytes);
    Ok(CoreEncoding::with_rm(bytes, &rm, true))
}

fn encode_test(
    operands: &[Operand],
    labels: &HashMap<String, u32>,
    pc: u32,
    resolve: bool,
) -> Result<CoreEncoding, Diagnostic> {
    require_operands("test", operands, 2)?;
    match (&operands[0].kind, &operands[1].kind) {
        (OperandKind::Register(dst), OperandKind::Immediate(expression)) => {
            if dst.code == 0 {
                let mut bytes = vec![if dst.width == Width::Byte { 0xA8 } else { 0xA9 }];
                push_immediate(
                    &mut bytes,
                    eval_text(expression, labels, pc, resolve)?,
                    dst.width,
                    "TEST",
                )?;
                return Ok(CoreEncoding::plain(bytes));
            }
            encode_test_immediate(&operands[0], dst.width, expression, labels, pc, resolve)
        }
        (OperandKind::Memory(memory), OperandKind::Immediate(expression)) => {
            let width = memory.width.ok_or_else(|| {
                error("8086 TEST memory-immediate operand requires BYTE PTR or WORD PTR")
            })?;
            encode_test_immediate(&operands[0], width, expression, labels, pc, resolve)
        }
        (OperandKind::Register(register), OperandKind::Register(_) | OperandKind::Memory(_)) => {
            let rm = encode_rm(
                &operands[1],
                register.width,
                register.code,
                labels,
                pc,
                resolve,
            )?;
            let mut bytes = vec![if register.width == Width::Byte {
                0x84
            } else {
                0x85
            }];
            bytes.extend(&rm.bytes);
            Ok(CoreEncoding::with_rm(bytes, &rm, false))
        }
        (OperandKind::Memory(_), OperandKind::Register(register)) => {
            let rm = encode_rm(
                &operands[0],
                register.width,
                register.code,
                labels,
                pc,
                resolve,
            )?;
            let mut bytes = vec![if register.width == Width::Byte {
                0x84
            } else {
                0x85
            }];
            bytes.extend(&rm.bytes);
            Ok(CoreEncoding::with_rm(bytes, &rm, false))
        }
        _ => Err(error("invalid 8086 TEST operand combination")),
    }
}

fn encode_test_immediate(
    destination: &Operand,
    width: Width,
    expression: &str,
    labels: &HashMap<String, u32>,
    pc: u32,
    resolve: bool,
) -> Result<CoreEncoding, Diagnostic> {
    let rm = encode_rm(destination, width, 0, labels, pc, resolve)?;
    let mut bytes = vec![if width == Width::Byte { 0xF6 } else { 0xF7 }];
    bytes.extend(&rm.bytes);
    push_immediate(
        &mut bytes,
        eval_text(expression, labels, pc, resolve)?,
        width,
        "TEST",
    )?;
    Ok(CoreEncoding::with_rm(bytes, &rm, false))
}

fn encode_address_load(
    op: &str,
    operands: &[Operand],
    labels: &HashMap<String, u32>,
    pc: u32,
    resolve: bool,
) -> Result<CoreEncoding, Diagnostic> {
    require_operands(op, operands, 2)?;
    let OperandKind::Register(destination) = operands[0].kind else {
        return Err(error(format!(
            "8086 {op} destination must be a word register"
        )));
    };
    if destination.width != Width::Word {
        return Err(error(format!(
            "8086 {op} destination must be a word register"
        )));
    }
    let OperandKind::Memory(memory) = &operands[1].kind else {
        return Err(error(format!("8086 {op} source must be memory")));
    };
    if op == "lea" && memory.segment.is_some() {
        return Err(error("8086 LEA does not accept a segment override"));
    }
    if matches!(op, "les" | "lds") && memory.width == Some(Width::Byte) {
        return Err(error(format!("8086 {op} requires a 16:16 memory pointer")));
    }
    let rm = encode_rm(
        &operands[1],
        Width::Word,
        destination.code,
        labels,
        pc,
        resolve,
    )?;
    let mut bytes = vec![match op {
        "lea" => 0x8D,
        "les" => 0xC4,
        "lds" => 0xC5,
        _ => unreachable!(),
    }];
    bytes.extend(&rm.bytes);
    Ok(CoreEncoding::with_rm(bytes, &rm, false))
}

fn encode_stack(
    op: &str,
    operands: &[Operand],
    labels: &HashMap<String, u32>,
    pc: u32,
    resolve: bool,
) -> Result<CoreEncoding, Diagnostic> {
    require_operands(op, operands, 1)?;
    match operands[0].kind {
        OperandKind::Register(register) if register.width == Width::Word => {
            Ok(CoreEncoding::plain(vec![
                if op == "push" { 0x50 } else { 0x58 } + register.code,
            ]))
        }
        OperandKind::Segment(segment) => {
            if op == "pop" && segment == 1 {
                return Err(error("8086 POP CS is not a documented instruction"));
            }
            let opcode = match (op, segment) {
                ("push", 0) => 0x06,
                ("push", 1) => 0x0E,
                ("push", 2) => 0x16,
                ("push", 3) => 0x1E,
                ("pop", 0) => 0x07,
                ("pop", 2) => 0x17,
                ("pop", 3) => 0x1F,
                _ => unreachable!(),
            };
            Ok(CoreEncoding::plain(vec![opcode]))
        }
        OperandKind::Memory(_) | OperandKind::Register(_) => {
            if op == "pop" {
                let rm = encode_rm(&operands[0], Width::Word, 0, labels, pc, resolve)?;
                let mut bytes = vec![0x8F];
                bytes.extend(&rm.bytes);
                Ok(CoreEncoding::with_rm(bytes, &rm, true))
            } else {
                let rm = encode_rm(&operands[0], Width::Word, 6, labels, pc, resolve)?;
                let mut bytes = vec![0xFF];
                bytes.extend(&rm.bytes);
                Ok(CoreEncoding::with_rm(bytes, &rm, false))
            }
        }
        _ => Err(error(format!("invalid 8086 {op} operand"))),
    }
}

fn encode_return(
    op: &str,
    operands: &[Operand],
    labels: &HashMap<String, u32>,
    pc: u32,
    resolve: bool,
) -> Result<CoreEncoding, Diagnostic> {
    if operands.len() > 1 {
        return Err(error(format!("8086 {op} expects zero or one operand")));
    }
    let far = op == "retf";
    if operands.is_empty() {
        return Ok(CoreEncoding::plain(vec![if far { 0xCB } else { 0xC3 }]));
    }
    let OperandKind::Immediate(expression) = &operands[0].kind else {
        return Err(error(format!(
            "8086 {op} stack adjustment must be immediate"
        )));
    };
    let mut bytes = vec![if far { 0xCA } else { 0xC2 }];
    push_unsigned_word(
        &mut bytes,
        eval_text(expression, labels, pc, resolve)?,
        "return stack adjustment",
    )?;
    Ok(CoreEncoding::plain(bytes))
}

fn encode_call_jump(
    op: &str,
    operands: &[Operand],
    labels: &HashMap<String, u32>,
    pc: u32,
    resolve: bool,
) -> Result<CoreEncoding, Diagnostic> {
    require_operands(op, operands, 1)?;
    let operand = &operands[0];
    match &operand.kind {
        OperandKind::FarImmediate { segment, offset } => {
            let mut bytes = vec![if op == "call" { 0x9A } else { 0xEA }];
            push_unsigned_word(
                &mut bytes,
                eval_text(offset, labels, pc, resolve)?,
                "far offset",
            )?;
            push_unsigned_word(
                &mut bytes,
                eval_text(segment, labels, pc, resolve)?,
                "far segment",
            )?;
            Ok(CoreEncoding::plain(bytes))
        }
        OperandKind::Register(_) | OperandKind::Memory(_) => {
            if operand.distance == Some(Distance::Short) {
                return Err(error(format!("8086 indirect {op} has no short form")));
            }
            let far = operand.distance == Some(Distance::Far)
                || matches!(&operand.kind, OperandKind::Memory(memory) if memory.far);
            if far && matches!(operand.kind, OperandKind::Register(_)) {
                return Err(error(format!("8086 far indirect {op} requires memory")));
            }
            let extension = match (op, far) {
                ("call", false) => 2,
                ("call", true) => 3,
                ("jmp", false) => 4,
                ("jmp", true) => 5,
                _ => unreachable!(),
            };
            let rm = encode_rm(operand, Width::Word, extension, labels, pc, resolve)?;
            let mut bytes = vec![0xFF];
            bytes.extend(&rm.bytes);
            Ok(CoreEncoding::with_rm(bytes, &rm, false))
        }
        OperandKind::Immediate(expression) => {
            if operand.distance == Some(Distance::Far) {
                return Err(error(format!(
                    "8086 far direct {op} requires `segment:offset`"
                )));
            }
            if op == "call" && operand.distance == Some(Distance::Short) {
                return Err(error("8086 CALL has no short form"));
            }
            let short = op == "jmp" && operand.distance == Some(Distance::Short);
            let length = if short { 2 } else { 3 };
            let opcode = if op == "call" {
                0xE8
            } else if short {
                0xEB
            } else {
                0xE9
            };
            let mut bytes = vec![opcode];
            let target = eval_text(expression, labels, pc, resolve)?;
            push_relative(
                &mut bytes,
                if resolve { target } else { Eval::Unknown },
                pc,
                length,
                if short { Width::Byte } else { Width::Word },
                op,
            )?;
            Ok(CoreEncoding::plain(bytes))
        }
        _ => Err(error(format!("invalid 8086 {op} operand"))),
    }
}

fn encode_short_branch(
    op: &str,
    opcode: u8,
    operands: &[Operand],
    labels: &HashMap<String, u32>,
    pc: u32,
    resolve: bool,
) -> Result<CoreEncoding, Diagnostic> {
    require_operands(op, operands, 1)?;
    if operands[0].distance == Some(Distance::Near) || operands[0].distance == Some(Distance::Far) {
        return Err(error(format!("8086 {op} only has a short form")));
    }
    let OperandKind::Immediate(expression) = &operands[0].kind else {
        return Err(error(format!(
            "8086 {op} target must be a label or expression"
        )));
    };
    let mut bytes = vec![opcode];
    let target = eval_text(expression, labels, pc, resolve)?;
    push_relative(
        &mut bytes,
        if resolve { target } else { Eval::Unknown },
        pc,
        2,
        Width::Byte,
        op,
    )?;
    Ok(CoreEncoding::plain(bytes))
}

fn encode_int(
    operands: &[Operand],
    labels: &HashMap<String, u32>,
    pc: u32,
    resolve: bool,
) -> Result<CoreEncoding, Diagnostic> {
    require_operands("int", operands, 1)?;
    let OperandKind::Immediate(expression) = &operands[0].kind else {
        return Err(error("8086 INT vector must be immediate"));
    };
    let value = eval_text(expression, labels, pc, resolve)?;
    if expression_is_absolute(expression)? && value == Eval::Known(3) {
        return Ok(CoreEncoding::plain(vec![0xCC]));
    }
    let mut bytes = vec![0xCD];
    push_unsigned_byte(&mut bytes, value, "interrupt vector")?;
    Ok(CoreEncoding::plain(bytes))
}

fn encode_io(
    op: &str,
    operands: &[Operand],
    labels: &HashMap<String, u32>,
    pc: u32,
    resolve: bool,
) -> Result<CoreEncoding, Diagnostic> {
    require_operands(op, operands, 2)?;
    let (accumulator, port) = if op == "in" {
        (&operands[0], &operands[1])
    } else {
        (&operands[1], &operands[0])
    };
    let OperandKind::Register(accumulator) = accumulator.kind else {
        return Err(error(format!("8086 {op} data register must be AL or AX")));
    };
    if accumulator.code != 0 {
        return Err(error(format!("8086 {op} data register must be AL or AX")));
    }
    match &port.kind {
        OperandKind::Register(register)
            if *register
                == (Register {
                    width: Width::Word,
                    code: 2,
                }) =>
        {
            Ok(CoreEncoding::plain(vec![match (op, accumulator.width) {
                ("in", Width::Byte) => 0xEC,
                ("in", Width::Word) => 0xED,
                ("out", Width::Byte) => 0xEE,
                ("out", Width::Word) => 0xEF,
                _ => unreachable!(),
            }]))
        }
        OperandKind::Immediate(expression) => {
            let mut bytes = vec![match (op, accumulator.width) {
                ("in", Width::Byte) => 0xE4,
                ("in", Width::Word) => 0xE5,
                ("out", Width::Byte) => 0xE6,
                ("out", Width::Word) => 0xE7,
                _ => unreachable!(),
            }];
            push_unsigned_byte(
                &mut bytes,
                eval_text(expression, labels, pc, resolve)?,
                "I/O port",
            )?;
            Ok(CoreEncoding::plain(bytes))
        }
        _ => Err(error(format!(
            "8086 {op} port must be an immediate byte or DX"
        ))),
    }
}

fn encode_esc(
    operands: &[Operand],
    labels: &HashMap<String, u32>,
    pc: u32,
    resolve: bool,
) -> Result<CoreEncoding, Diagnostic> {
    require_operands("esc", operands, 2)?;
    let OperandKind::Immediate(escape) = &operands[0].kind else {
        return Err(error(
            "8086 ESC opcode must be an immediate value from 0 through 63",
        ));
    };
    let escape = eval_text(escape, labels, pc, resolve)?;
    let escape = known_unsigned(escape, 63, "ESC opcode")? as u8;
    let opcode = 0xD8 + (escape >> 3);
    let extension = escape & 7;
    match &operands[1].kind {
        OperandKind::Memory(_) => {
            let rm = encode_rm(&operands[1], Width::Word, extension, labels, pc, resolve)?;
            let mut bytes = vec![opcode];
            bytes.extend(&rm.bytes);
            Ok(CoreEncoding::with_rm(bytes, &rm, false))
        }
        OperandKind::Immediate(selector) => {
            let selector = known_unsigned(
                eval_text(selector, labels, pc, resolve)?,
                7,
                "ESC register selector",
            )? as u8;
            Ok(CoreEncoding::plain(vec![
                opcode,
                0xC0 | (extension << 3) | selector,
            ]))
        }
        _ => Err(error(
            "8086 ESC selector must be memory or an integer from 0 through 7",
        )),
    }
}

fn encode_rm(
    operand: &Operand,
    width: Width,
    reg_field: u8,
    labels: &HashMap<String, u32>,
    pc: u32,
    resolve: bool,
) -> Result<RmEncoding, Diagnostic> {
    match &operand.kind {
        OperandKind::Register(register) => {
            if register.width != width {
                return Err(error("8086 operand size mismatch"));
            }
            Ok(RmEncoding {
                bytes: vec![0xC0 | ((reg_field & 7) << 3) | register.code],
                segment: None,
                is_memory: false,
            })
        }
        OperandKind::Memory(memory) => {
            encode_memory_rm(memory, width, reg_field, labels, pc, resolve)
        }
        _ => Err(error("8086 ModR/M operand must be a register or memory")),
    }
}

fn encode_memory_rm(
    memory: &Memory,
    width: Width,
    reg_field: u8,
    labels: &HashMap<String, u32>,
    pc: u32,
    resolve: bool,
) -> Result<RmEncoding, Diagnostic> {
    validate_memory_width(memory, width)?;
    let rm = ea_rm_code(&memory.registers)?;
    if memory.registers.is_empty() {
        let expression = memory
            .displacement
            .as_deref()
            .ok_or_else(|| error("direct 8086 memory address requires an offset"))?;
        let mut bytes = vec![((reg_field & 7) << 3) | 6];
        push_unsigned_word(
            &mut bytes,
            eval_text(expression, labels, pc, resolve)?,
            "direct memory offset",
        )?;
        return Ok(RmEncoding {
            bytes,
            segment: memory.segment,
            is_memory: true,
        });
    }

    let symbolic = memory
        .displacement
        .as_deref()
        .is_some_and(|expression| !expression_is_absolute(expression).unwrap_or(false));
    let value = memory
        .displacement
        .as_deref()
        .map(|expression| eval_text(expression, labels, pc, resolve))
        .transpose()?
        .unwrap_or(Eval::Known(0));
    let is_bp_only = matches!(memory.registers.as_slice(), ["bp"]);
    let (mode, displacement_width) = if memory.displacement.is_none() && !is_bp_only {
        (0, None)
    } else if symbolic {
        (2, Some(Width::Word))
    } else if matches!(value, Eval::Known(value) if (-128..=127).contains(&value)) {
        (1, Some(Width::Byte))
    } else {
        (2, Some(Width::Word))
    };
    let mut bytes = vec![(mode << 6) | ((reg_field & 7) << 3) | rm];
    if let Some(displacement_width) = displacement_width {
        push_displacement(&mut bytes, value, displacement_width)?;
    }
    Ok(RmEncoding {
        bytes,
        segment: memory.segment,
        is_memory: true,
    })
}

fn ea_rm_code(registers: &[&str]) -> Result<u8, Diagnostic> {
    Ok(match registers {
        ["bx", "si"] | ["si", "bx"] => 0,
        ["bx", "di"] | ["di", "bx"] => 1,
        ["bp", "si"] | ["si", "bp"] => 2,
        ["bp", "di"] | ["di", "bp"] => 3,
        ["si"] => 4,
        ["di"] => 5,
        ["bp"] => 6,
        ["bx"] => 7,
        [] => 6,
        _ => return Err(error("invalid 8086 effective address")),
    })
}

fn push_memory_offset(
    bytes: &mut Vec<u8>,
    memory: &Memory,
    labels: &HashMap<String, u32>,
    pc: u32,
    resolve: bool,
) -> Result<(), Diagnostic> {
    let expression = memory
        .displacement
        .as_deref()
        .ok_or_else(|| error("direct 8086 memory address requires an offset"))?;
    push_unsigned_word(
        bytes,
        eval_text(expression, labels, pc, resolve)?,
        "direct memory offset",
    )
}

fn operand_width(operand: &Operand, op: &str) -> Result<Width, Diagnostic> {
    match &operand.kind {
        OperandKind::Register(register) => Ok(register.width),
        OperandKind::Memory(memory) => memory.width.ok_or_else(|| {
            error(format!(
                "8086 {op} memory operand requires BYTE PTR or WORD PTR"
            ))
        }),
        _ => Err(error(format!("invalid 8086 {op} operand"))),
    }
}

fn register(name: &str) -> Option<Register> {
    Some(match name.trim() {
        "al" => Register {
            width: Width::Byte,
            code: 0,
        },
        "cl" => Register {
            width: Width::Byte,
            code: 1,
        },
        "dl" => Register {
            width: Width::Byte,
            code: 2,
        },
        "bl" => Register {
            width: Width::Byte,
            code: 3,
        },
        "ah" => Register {
            width: Width::Byte,
            code: 4,
        },
        "ch" => Register {
            width: Width::Byte,
            code: 5,
        },
        "dh" => Register {
            width: Width::Byte,
            code: 6,
        },
        "bh" => Register {
            width: Width::Byte,
            code: 7,
        },
        "ax" => Register {
            width: Width::Word,
            code: 0,
        },
        "cx" => Register {
            width: Width::Word,
            code: 1,
        },
        "dx" => Register {
            width: Width::Word,
            code: 2,
        },
        "bx" => Register {
            width: Width::Word,
            code: 3,
        },
        "sp" => Register {
            width: Width::Word,
            code: 4,
        },
        "bp" => Register {
            width: Width::Word,
            code: 5,
        },
        "si" => Register {
            width: Width::Word,
            code: 6,
        },
        "di" => Register {
            width: Width::Word,
            code: 7,
        },
        _ => return None,
    })
}

fn segment_code(name: &str) -> Result<u8, Diagnostic> {
    match name.trim().trim_end_matches(':') {
        "es" => Ok(0),
        "cs" => Ok(1),
        "ss" => Ok(2),
        "ds" => Ok(3),
        _ => Err(error(format!("unknown 8086 segment register `{name}`"))),
    }
}

fn segment_prefix(segment: u8) -> u8 {
    match segment {
        0 => 0x26,
        1 => 0x2E,
        2 => 0x36,
        3 => 0x3E,
        _ => unreachable!(),
    }
}

fn validate_memory_width(memory: &Memory, width: Width) -> Result<(), Diagnostic> {
    if let Some(memory_width) = memory.width
        && memory_width != width
    {
        return Err(error(
            "8086 memory operand size does not match register size",
        ));
    }
    Ok(())
}

fn merge_segment_overrides(a: Option<u8>, b: Option<u8>) -> Result<Option<u8>, Diagnostic> {
    match (a, b) {
        (Some(a), Some(b)) if a != b => Err(error("multiple segment override prefixes")),
        (Some(a), _) | (_, Some(a)) => Ok(Some(a)),
        (None, None) => Ok(None),
    }
}

fn validate_operand_qualifiers(instruction: &ParsedInstruction) -> Result<(), Diagnostic> {
    let op = instruction.mnemonic.as_str();
    if matches!(op, "call" | "jmp") {
        return Ok(());
    }
    if jcc_opcode(op).is_some()
        || matches!(
            op,
            "loopne" | "loopnz" | "loope" | "loopz" | "loop" | "jcxz"
        )
    {
        if instruction
            .operands
            .iter()
            .all(|operand| matches!(operand.distance, None | Some(Distance::Short)))
        {
            return Ok(());
        }
        return Err(error(format!("8086 {op} only accepts a SHORT qualifier")));
    }
    if matches!(op, "les" | "lds") {
        let valid = instruction
            .operands
            .iter()
            .enumerate()
            .all(|(index, operand)| {
                operand.distance.is_none() || index == 1 && operand.distance == Some(Distance::Far)
            });
        if valid {
            return Ok(());
        }
    } else if instruction
        .operands
        .iter()
        .all(|operand| operand.distance.is_none())
    {
        return Ok(());
    }
    Err(error(format!(
        "8086 {op} does not accept SHORT, NEAR, or FAR operand qualifiers"
    )))
}

fn validate_prefixes(
    instruction: &ParsedInstruction,
    core: &CoreEncoding,
) -> Result<(), Diagnostic> {
    if instruction.lock && instruction.repeat.is_some() {
        return Err(error("8086 LOCK cannot be combined with a repeat prefix"));
    }
    if instruction.lock {
        let lockable = matches!(
            instruction.mnemonic.as_str(),
            "add"
                | "or"
                | "adc"
                | "sbb"
                | "and"
                | "sub"
                | "xor"
                | "inc"
                | "dec"
                | "not"
                | "neg"
                | "xchg"
        );
        if !lockable || !core.memory_write {
            return Err(error("8086 LOCK requires a lockable memory destination"));
        }
    }
    if let Some(repeat) = instruction.repeat {
        let mnemonic = instruction.mnemonic.as_str();
        let compare = matches!(mnemonic, "cmpsb" | "cmpsw" | "scasb" | "scasw");
        let unconditional = matches!(
            mnemonic,
            "movsb" | "movsw" | "lodsb" | "lodsw" | "stosb" | "stosw"
        );
        if !compare && !unconditional {
            return Err(error("8086 repeat prefix requires a string instruction"));
        }
        if repeat == RepeatPrefix::Repne && !compare {
            return Err(error("8086 REPNE is only valid with CMPS or SCAS"));
        }
    }
    if instruction.leading_segment.is_some() {
        let fixed_destination_string = matches!(
            instruction.mnemonic.as_str(),
            "stosb" | "stosw" | "scasb" | "scasw"
        );
        if !core.memory_access || fixed_destination_string || instruction.mnemonic.as_str() == "lea"
        {
            return Err(error(
                "8086 segment override requires an overridable memory access",
            ));
        }
    }
    if core.segment.is_some() && !core.memory_access {
        return Err(error("8086 segment override requires a memory access"));
    }
    Ok(())
}

fn require_operands(op: &str, operands: &[Operand], count: usize) -> Result<(), Diagnostic> {
    if operands.len() == count {
        Ok(())
    } else {
        Err(error(format!(
            "8086 {op} expects {count} operand{}",
            if count == 1 { "" } else { "s" }
        )))
    }
}

fn jcc_opcode(op: &str) -> Option<u8> {
    Some(match op {
        "jo" => 0x70,
        "jno" => 0x71,
        "jb" | "jc" | "jnae" => 0x72,
        "jae" | "jnb" | "jnc" => 0x73,
        "je" | "jz" => 0x74,
        "jne" | "jnz" => 0x75,
        "jbe" | "jna" => 0x76,
        "ja" | "jnbe" => 0x77,
        "js" => 0x78,
        "jns" => 0x79,
        "jp" | "jpe" => 0x7A,
        "jnp" | "jpo" => 0x7B,
        "jl" | "jnge" => 0x7C,
        "jge" | "jnl" => 0x7D,
        "jle" | "jng" => 0x7E,
        "jg" | "jnle" => 0x7F,
        _ => return None,
    })
}

fn eval_text(
    text: &str,
    labels: &HashMap<String, u32>,
    pc: u32,
    resolve: bool,
) -> Result<Eval, Diagnostic> {
    let expression = parse_assembly_expression(text)
        .map_err(|_| error(format!("invalid 8086 expression `{text}`")))?;
    eval_expression(&expression, labels, pc, resolve)
}

fn eval_expression(
    expression: &AssemblyExpression,
    labels: &HashMap<String, u32>,
    pc: u32,
    resolve: bool,
) -> Result<Eval, Diagnostic> {
    match expression {
        AssemblyExpression::Symbol(name) => labels
            .get(name)
            .copied()
            .or_else(|| {
                labels
                    .iter()
                    .find_map(|(known, value)| known.eq_ignore_ascii_case(name).then_some(*value))
            })
            .map(|value| Eval::Known(i128::from(value)))
            .map_or_else(
                || {
                    if resolve {
                        Err(error(format!("unknown 8086 symbol `{name}`")))
                    } else {
                        Ok(Eval::Unknown)
                    }
                },
                Ok,
            ),
        AssemblyExpression::Current => Ok(Eval::Known(i128::from(pc))),
        AssemblyExpression::Number(value) => Ok(Eval::Known(i128::from(*value))),
        AssemblyExpression::Unary {
            operator,
            expression,
        } => match eval_expression(expression, labels, pc, resolve)? {
            Eval::Unknown => Ok(Eval::Unknown),
            Eval::Known(value) => Ok(Eval::Known(match operator {
                AssemblyUnaryOperator::Plus => value,
                AssemblyUnaryOperator::Negate => -value,
            })),
        },
        AssemblyExpression::Binary {
            operator,
            left,
            right,
        } => match (
            eval_expression(left, labels, pc, resolve)?,
            eval_expression(right, labels, pc, resolve)?,
        ) {
            (Eval::Known(left), Eval::Known(right)) => Ok(Eval::Known(match operator {
                AssemblyBinaryOperator::Add => left + right,
                AssemblyBinaryOperator::Subtract => left - right,
                AssemblyBinaryOperator::Multiply => left * right,
                AssemblyBinaryOperator::BitAnd => left & right,
                AssemblyBinaryOperator::BitOr => left | right,
                AssemblyBinaryOperator::BitXor => left ^ right,
            })),
            _ => Ok(Eval::Unknown),
        },
    }
}

fn expression_is_absolute(text: &str) -> Result<bool, Diagnostic> {
    let expression = parse_assembly_expression(text)
        .map_err(|_| error(format!("invalid 8086 expression `{text}`")))?;
    Ok(!expression_has_symbol_or_current(&expression))
}

fn expression_has_symbol_or_current(expression: &AssemblyExpression) -> bool {
    match expression {
        AssemblyExpression::Symbol(_) | AssemblyExpression::Current => true,
        AssemblyExpression::Number(_) => false,
        AssemblyExpression::Unary { expression, .. } => {
            expression_has_symbol_or_current(expression)
        }
        AssemblyExpression::Binary { left, right, .. } => {
            expression_has_symbol_or_current(left) || expression_has_symbol_or_current(right)
        }
    }
}

fn expression_mentions_register(text: &str) -> Result<bool, Diagnostic> {
    fn visit(expression: &AssemblyExpression) -> bool {
        match expression {
            AssemblyExpression::Symbol(name) => {
                register(name).is_some() || segment_code(name).is_ok()
            }
            AssemblyExpression::Number(_) | AssemblyExpression::Current => false,
            AssemblyExpression::Unary { expression, .. } => visit(expression),
            AssemblyExpression::Binary { left, right, .. } => visit(left) || visit(right),
        }
    }

    let expression = parse_assembly_expression(text)
        .map_err(|_| error(format!("invalid 8086 address expression `{text}`")))?;
    Ok(visit(&expression))
}

fn push_immediate(
    bytes: &mut Vec<u8>,
    value: Eval,
    width: Width,
    kind: &str,
) -> Result<(), Diagnostic> {
    match width {
        Width::Byte => push_byte_pattern(bytes, value, kind),
        Width::Word => push_word_pattern(bytes, value, kind),
    }
}

fn push_byte_pattern(bytes: &mut Vec<u8>, value: Eval, kind: &str) -> Result<(), Diagnostic> {
    match value {
        Eval::Unknown => bytes.push(0),
        Eval::Known(value) if (-128..=255).contains(&value) => bytes.push(value as u8),
        Eval::Known(value) => {
            return Err(error(format!(
                "8086 {kind} value {value} is outside 8-bit range"
            )));
        }
    }
    Ok(())
}

fn push_word_pattern(bytes: &mut Vec<u8>, value: Eval, kind: &str) -> Result<(), Diagnostic> {
    let value = match value {
        Eval::Unknown => 0,
        Eval::Known(value) if (-32768..=65535).contains(&value) => value as u16,
        Eval::Known(value) => {
            return Err(error(format!(
                "8086 {kind} value {value} is outside 16-bit range"
            )));
        }
    };
    bytes.extend(value.to_le_bytes());
    Ok(())
}

fn push_unsigned_byte(bytes: &mut Vec<u8>, value: Eval, kind: &str) -> Result<(), Diagnostic> {
    let value = known_or_zero(value);
    if !(0..=255).contains(&value) {
        return Err(error(format!(
            "8086 {kind} value {value} is outside 0..255"
        )));
    }
    bytes.push(value as u8);
    Ok(())
}

fn push_unsigned_word(bytes: &mut Vec<u8>, value: Eval, kind: &str) -> Result<(), Diagnostic> {
    let value = known_or_zero(value);
    if !(0..=65535).contains(&value) {
        return Err(error(format!(
            "8086 {kind} value {value} is outside 0..65535"
        )));
    }
    bytes.extend((value as u16).to_le_bytes());
    Ok(())
}

fn push_signed_byte(bytes: &mut Vec<u8>, value: Eval, kind: &str) -> Result<(), Diagnostic> {
    let value = known_or_zero(value);
    let signed = if (-128..=127).contains(&value) {
        value
    } else if (0xFF80..=0xFFFF).contains(&value) {
        value - 0x1_0000
    } else {
        return Err(error(format!(
            "8086 {kind} value {value} is not sign-extendable"
        )));
    };
    bytes.push(signed as i8 as u8);
    Ok(())
}

fn push_displacement(bytes: &mut Vec<u8>, value: Eval, width: Width) -> Result<(), Diagnostic> {
    match width {
        Width::Byte => {
            let value = known_or_zero(value);
            if !(-128..=127).contains(&value) {
                return Err(error(format!(
                    "8086 displacement {value} is outside signed 8-bit range"
                )));
            }
            bytes.push(value as i8 as u8);
        }
        Width::Word => push_word_pattern(bytes, value, "displacement")?,
    }
    Ok(())
}

fn push_relative(
    bytes: &mut Vec<u8>,
    target: Eval,
    pc: u32,
    instruction_len: u32,
    width: Width,
    op: &str,
) -> Result<(), Diagnostic> {
    let Eval::Known(target) = target else {
        bytes.extend(core::iter::repeat_n(
            0,
            if width == Width::Byte { 1 } else { 2 },
        ));
        return Ok(());
    };
    if pc > 0xFFFF || !(0..=0xFFFF).contains(&target) {
        return Err(error(format!(
            "8086 {op} target must be within the current 64 KiB segment"
        )));
    }
    let next = (pc as u16).wrapping_add(instruction_len as u16);
    let displacement = (target as u16).wrapping_sub(next);
    match width {
        Width::Byte if displacement <= 0x7F || displacement >= 0xFF80 => {
            bytes.push(displacement as u8)
        }
        Width::Word => bytes.extend(displacement.to_le_bytes()),
        Width::Byte => {
            return Err(error(format!("8086 {op} short target is out of range")));
        }
    }
    Ok(())
}

fn known_unsigned(value: Eval, max: i128, kind: &str) -> Result<i128, Diagnostic> {
    let value = known_or_zero(value);
    if !(0..=max).contains(&value) {
        Err(error(format!(
            "8086 {kind} value {value} is outside 0..{max}"
        )))
    } else {
        Ok(value)
    }
}

fn known_or_zero(value: Eval) -> i128 {
    match value {
        Eval::Known(value) => value,
        Eval::Unknown => 0,
    }
}

fn error(message: impl Into<String>) -> Diagnostic {
    Diagnostic::new(message)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::target::AssemblerCpu;
    use crate::vm::assemble_subset_with_symbols_at;

    fn encode_one(source: &str) -> Vec<u8> {
        encode_instruction(source, &HashMap::new(), 0x1000).unwrap()
    }

    #[test]
    fn golden_fixed_and_implicit_instructions() {
        for (source, expected) in [
            ("aaa", vec![0x37]),
            ("aad", vec![0xD5, 0x0A]),
            ("aam", vec![0xD4, 0x0A]),
            ("aas", vec![0x3F]),
            ("cbw", vec![0x98]),
            ("cwd", vec![0x99]),
            ("daa", vec![0x27]),
            ("das", vec![0x2F]),
            ("iret", vec![0xCF]),
            ("lahf", vec![0x9F]),
            ("sahf", vec![0x9E]),
            ("pushf", vec![0x9C]),
            ("popf", vec![0x9D]),
            ("wait", vec![0x9B]),
            ("hlt", vec![0xF4]),
            ("cmc", vec![0xF5]),
            ("clc", vec![0xF8]),
            ("stc", vec![0xF9]),
            ("cli", vec![0xFA]),
            ("sti", vec![0xFB]),
            ("cld", vec![0xFC]),
            ("std", vec![0xFD]),
        ] {
            assert_eq!(encode_one(source), expected, "{source}");
        }
    }

    #[test]
    fn golden_covers_alu_data_stack_and_io_forms() {
        let cases: &[(&str, &[u8])] = &[
            ("add al,7fh", &[0x04, 0x7F]),
            ("or ax,1234h", &[0x0D, 0x34, 0x12]),
            ("adc [bx+si],cl", &[0x10, 0x08]),
            ("sbb dx,[bp+di-2]", &[0x1B, 0x53, 0xFE]),
            ("and word ptr [bx],-1", &[0x83, 0x27, 0xFF]),
            (
                "sub word ptr [1234h],128",
                &[0x81, 0x2E, 0x34, 0x12, 0x80, 0x00],
            ),
            ("xor bh,byte ptr es:[di+4]", &[0x26, 0x32, 0x7D, 0x04]),
            ("cmp word ptr [bp],1", &[0x83, 0x7E, 0x00, 0x01]),
            ("mov ax,[1234h]", &[0xA1, 0x34, 0x12]),
            ("mov [1234h],al", &[0xA2, 0x34, 0x12]),
            ("mov ds,ax", &[0x8E, 0xD8]),
            ("mov ax,cs", &[0x8C, 0xC8]),
            ("xchg ax,di", &[0x97]),
            ("test byte ptr [si],80h", &[0xF6, 0x04, 0x80]),
            ("lea bx,[bp+si+1234h]", &[0x8D, 0x9A, 0x34, 0x12]),
            ("les di,far ptr [bx]", &[0xC4, 0x3F]),
            ("push ds", &[0x1E]),
            ("pop word ptr [bx]", &[0x8F, 0x07]),
            ("in al,20h", &[0xE4, 0x20]),
            ("out dx,ax", &[0xEF]),
        ];
        for (source, expected) in cases {
            assert_eq!(encode_one(source), *expected, "{source}");
        }
    }

    #[test]
    fn golden_covers_unary_shift_control_string_and_escape_forms() {
        let cases: &[(&str, &[u8])] = &[
            ("inc ax", &[0x40]),
            ("dec byte ptr [bx]", &[0xFE, 0x0F]),
            ("not word ptr [si]", &[0xF7, 0x14]),
            ("neg bl", &[0xF6, 0xDB]),
            ("mul word ptr [di]", &[0xF7, 0x25]),
            ("idiv ch", &[0xF6, 0xFD]),
            ("rol byte ptr [bx],1", &[0xD0, 0x07]),
            ("sar ax,cl", &[0xD3, 0xF8]),
            ("call 1100h", &[0xE8, 0xFD, 0x00]),
            ("jmp short 0ff0h", &[0xEB, 0xEE]),
            ("call far 1234h:5678h", &[0x9A, 0x78, 0x56, 0x34, 0x12]),
            ("jmp far ptr [bx]", &[0xFF, 0x2F]),
            ("ret 4", &[0xC2, 0x04, 0x00]),
            ("retf", &[0xCB]),
            ("int 3", &[0xCC]),
            ("rep movsw", &[0xF3, 0xA5]),
            ("repne scasb", &[0xF2, 0xAE]),
            ("rep ds: movsb", &[0xF3, 0x3E, 0xA4]),
            ("lock add word ptr [bx],1", &[0xF0, 0x83, 0x07, 0x01]),
            ("esc 63,[bp+di]", &[0xDF, 0x3B]),
            ("esc 0,7", &[0xD8, 0xC7]),
        ];
        for (source, expected) in cases {
            assert_eq!(encode_one(source), *expected, "{source}");
        }
    }

    #[test]
    fn all_effective_address_modes_and_displacement_sizes_encode() {
        for (address, modrm) in [
            ("[bx+si]", 0x00),
            ("[bx+di]", 0x01),
            ("[bp+si]", 0x02),
            ("[bp+di]", 0x03),
            ("[si]", 0x04),
            ("[di]", 0x05),
            ("[bx]", 0x07),
        ] {
            assert_eq!(encode_one(&format!("mov ax,{address}")), [0x8B, modrm]);
        }
        assert_eq!(encode_one("mov ax,[bp]"), [0x8B, 0x46, 0x00]);
        assert_eq!(encode_one("mov ax,[bx+127]"), [0x8B, 0x47, 0x7F]);
        assert_eq!(encode_one("mov ax,[bx+128]"), [0x8B, 0x87, 0x80, 0x00]);
        assert_eq!(instruction_len("mov ax,[bx+symbol]").unwrap(), 4);
    }

    #[test]
    fn labels_and_every_short_branch_alias_resolve() {
        let source = "start:\n jz next\n loop start\nnext:\n jmp near start\n";
        let assembled =
            assemble_subset_with_symbols_at(AssemblerCpu::I8086, source, 0x1000).unwrap();
        assert_eq!(assembled.bytes, [0x74, 0x02, 0xE2, 0xFC, 0xE9, 0xF9, 0xFF]);
        for alias in [
            "jo", "jno", "jb", "jc", "jnae", "jae", "jnb", "jnc", "je", "jz", "jne", "jnz", "jbe",
            "jna", "ja", "jnbe", "js", "jns", "jp", "jpe", "jnp", "jpo", "jl", "jnge", "jge",
            "jnl", "jle", "jng", "jg", "jnle",
        ] {
            assert_eq!(
                instruction_len(&format!("{alias} 0")).unwrap(),
                2,
                "{alias}"
            );
        }
    }

    #[test]
    fn documented_mnemonic_matrix_is_complete() {
        let forms = [
            "aaa",
            "aad",
            "aam",
            "aas",
            "adc ax,bx",
            "add ax,bx",
            "and ax,bx",
            "call 1003h",
            "call far 1:2",
            "call bx",
            "call far ptr [bx]",
            "cbw",
            "clc",
            "cld",
            "cli",
            "cmc",
            "cmp ax,bx",
            "cmpsb",
            "cmpsw",
            "cwd",
            "daa",
            "das",
            "dec ax",
            "div ax",
            "hlt",
            "idiv ax",
            "imul ax",
            "in al,1",
            "in ax,dx",
            "inc ax",
            "int 4",
            "int3",
            "into",
            "iret",
            "jcxz 2",
            "jmp 1003h",
            "jmp short 2",
            "jmp far 1:2",
            "jmp bx",
            "jmp far ptr [bx]",
            "lahf",
            "lds ax,[bx]",
            "lea ax,[bx]",
            "les ax,[bx]",
            "lodsb",
            "lodsw",
            "loop 2",
            "loope 2",
            "loopne 2",
            "loopnz 2",
            "loopz 2",
            "mov ax,bx",
            "mov ax,1",
            "mov ax,[1]",
            "mov [1],ax",
            "mov ds,ax",
            "mov ax,ds",
            "movsb",
            "movsw",
            "mul ax",
            "neg ax",
            "nop",
            "not ax",
            "or ax,bx",
            "out 1,al",
            "out dx,ax",
            "pop ax",
            "pop ds",
            "pop word ptr [bx]",
            "popf",
            "push ax",
            "push cs",
            "push word ptr [bx]",
            "pushf",
            "rcl ax,1",
            "rcr ax,cl",
            "ret",
            "ret 2",
            "retn",
            "retf",
            "rol ax,1",
            "ror ax,cl",
            "sahf",
            "sal ax,1",
            "sar ax,cl",
            "sbb ax,bx",
            "scasb",
            "scasw",
            "shl ax,1",
            "shr ax,cl",
            "stc",
            "std",
            "sti",
            "stosb",
            "stosw",
            "sub ax,bx",
            "test ax,bx",
            "test ax,1",
            "wait",
            "xchg ax,bx",
            "xlat",
            "xor ax,bx",
            "esc 0,[bx]",
        ];
        for form in forms {
            instruction_len(form).unwrap_or_else(|error| panic!("{form}: {error}"));
        }

        for (mnemonic, opcode) in [
            ("jo", 0x70),
            ("jno", 0x71),
            ("jb", 0x72),
            ("jae", 0x73),
            ("je", 0x74),
            ("jne", 0x75),
            ("jbe", 0x76),
            ("ja", 0x77),
            ("js", 0x78),
            ("jns", 0x79),
            ("jp", 0x7A),
            ("jnp", 0x7B),
            ("jl", 0x7C),
            ("jge", 0x7D),
            ("jle", 0x7E),
            ("jg", 0x7F),
        ] {
            assert_eq!(encode_one(&format!("{mnemonic} 1002h")), [opcode, 0]);
        }
        for (mnemonic, opcode) in [
            ("movsb", 0xA4),
            ("movsw", 0xA5),
            ("cmpsb", 0xA6),
            ("cmpsw", 0xA7),
            ("stosb", 0xAA),
            ("stosw", 0xAB),
            ("lodsb", 0xAC),
            ("lodsw", 0xAD),
            ("scasb", 0xAE),
            ("scasw", 0xAF),
        ] {
            assert_eq!(encode_one(mnemonic), [opcode]);
        }
    }

    #[test]
    fn prefixed_segments_and_numeric_branches_work_through_both_passes() {
        let assembled = assemble_subset_with_symbols_at(
            AssemblerCpu::I8086,
            "rep ds: movsb\njmp short 1000h\n",
            0x1000,
        )
        .unwrap();
        assert_eq!(assembled.bytes, [0xF3, 0x3E, 0xA4, 0xEB, 0xFB]);

        assert_eq!(
            encode_instruction("jmp near 9000h", &HashMap::new(), 0,).unwrap(),
            [0xE9, 0xFD, 0x8F]
        );
        assert_eq!(
            encode_instruction("jmp short 0", &HashMap::new(), 0xFFFE).unwrap(),
            [0xEB, 0x00]
        );
    }

    #[test]
    fn symbolic_sizes_remain_stable_between_assembly_passes() {
        let source = "vector equ 3\nint vector\nmov ax,[bx+target]\ntarget:\nnop\n";
        let assembled =
            assemble_subset_with_symbols_at(AssemblerCpu::I8086, source, 0x1000).unwrap();
        assert_eq!(assembled.bytes, [0xCD, 0x03, 0x8B, 0x87, 0x06, 0x10, 0x90]);
    }

    #[test]
    fn rejects_post_8086_reserved_and_invalid_forms() {
        for source in [
            "pusha",
            "push 1",
            "imul ax,bx",
            "shl ax,2",
            "enter 4,0",
            "leave",
            "insb",
            "mov eax,1",
            "mov cs,ax",
            "pop cs",
            "lea ax,bx",
            "jmp far ax",
            "lock cmp word ptr [bx],1",
            "lock add ax,1",
            "lock shl word ptr [bx],1",
            "repne movsb",
            "rep nop",
            "xchg ax,al",
            "xchg al,ax",
            "mov ax,byte ptr [1234h]",
            "mov al,word ptr [1234h]",
            "mov byte ptr [1234h],ax",
            "mov word ptr [1234h],al",
            "mov short ax,bx",
            "add ax,far ptr [bx]",
            "call short bx",
            "jmp short bx",
            "jmp short 1234h:5678h",
            "call near 1234h:5678h",
            "jmp short far ptr [bx]",
            "mov ax,[sp]",
            "mov ax,[cs]",
            "mov ax,[bx*2]",
            "mov ax,[bx+bp]",
            "inc [bx]",
        ] {
            assert!(
                instruction_len(source).is_err(),
                "unexpectedly accepted `{source}`"
            );
        }
    }
}
