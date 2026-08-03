use std::collections::HashMap;

use crate::{
    asm::{
        AssemblyBinaryOperator, AssemblyExpression, AssemblyUnaryOperator,
        parse_assembly_expression,
    },
    diagnostic::Diagnostic,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Position {
    A,
    B,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct Operand {
    code: u16,
    extra: Option<u16>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum SymbolUnits {
    ByteAddresses,
    WordAddresses,
}

pub fn instruction_len(text: &str) -> Result<usize, Diagnostic> {
    Ok(encode(text, &HashMap::new(), 0, false, SymbolUnits::ByteAddresses)?.len())
}

pub fn encode_instruction(
    text: &str,
    labels: &HashMap<String, u32>,
    pc: u32,
) -> Result<Vec<u8>, Diagnostic> {
    encode(text, labels, pc, true, SymbolUnits::ByteAddresses)
}

pub(crate) fn encode_instruction_with_word_symbols(
    text: &str,
    symbols: &HashMap<String, u32>,
    pc_words: u32,
) -> Result<Vec<u8>, Diagnostic> {
    encode(text, symbols, pc_words, true, SymbolUnits::WordAddresses)
}

fn encode(
    text: &str,
    labels: &HashMap<String, u32>,
    pc: u32,
    resolve_labels: bool,
    symbol_units: SymbolUnits,
) -> Result<Vec<u8>, Diagnostic> {
    let lowered = normalize(text);
    let (mnemonic, operands) = parse_instruction(&lowered)?;
    let mut words = Vec::new();

    if let Some(opcode) = special_opcode(mnemonic) {
        let operand = parse_single_operand(mnemonic, operands)?;
        let operand = parse_operand(
            operand,
            Position::A,
            labels,
            pc,
            resolve_labels,
            symbol_units,
        )?;
        words.push((operand.code << 10) | (opcode << 5));
        if let Some(extra) = operand.extra {
            words.push(extra);
        }
        return Ok(words_to_bytes(&words));
    }

    let opcode = basic_opcode(mnemonic).ok_or_else(|| {
        Diagnostic::new(format!(
            "assembler does not support DCPU instruction `{text}`"
        ))
    })?;
    let (b_text, a_text) = split_operands(operands).ok_or_else(|| {
        Diagnostic::new(format!(
            "DCPU instruction `{mnemonic}` expects exactly two operands"
        ))
    })?;
    let b = parse_operand(
        b_text,
        Position::B,
        labels,
        pc,
        resolve_labels,
        symbol_units,
    )?;
    let a = parse_operand(
        a_text,
        Position::A,
        labels,
        pc,
        resolve_labels,
        symbol_units,
    )?;
    words.push(opcode | (b.code << 5) | (a.code << 10));
    if let Some(extra) = b.extra {
        words.push(extra);
    }
    if let Some(extra) = a.extra {
        words.push(extra);
    }
    Ok(words_to_bytes(&words))
}

fn normalize(text: &str) -> String {
    text.trim().to_ascii_lowercase()
}

fn parse_instruction(text: &str) -> Result<(&str, &str), Diagnostic> {
    let (mnemonic, operands) = text
        .split_once(char::is_whitespace)
        .map(|(mnemonic, operands)| (mnemonic, operands.trim()))
        .unwrap_or((text, ""));
    if mnemonic.is_empty() {
        return Err(Diagnostic::new("empty DCPU instruction"));
    }
    Ok((mnemonic, operands))
}

fn parse_single_operand<'a>(mnemonic: &str, operands: &'a str) -> Result<&'a str, Diagnostic> {
    let operand = operands.trim();
    if operand.is_empty() || operand.contains(',') {
        return Err(Diagnostic::new(format!(
            "DCPU special instruction `{mnemonic}` expects exactly one operand"
        )));
    }
    Ok(operand)
}

fn split_operands(operands: &str) -> Option<(&str, &str)> {
    let (left, right) = operands.split_once(',')?;
    if left.trim().is_empty() || right.trim().is_empty() || right.contains(',') {
        return None;
    }
    Some((left.trim(), right.trim()))
}

fn special_opcode(mnemonic: &str) -> Option<u16> {
    Some(match mnemonic {
        "jsr" => 0x01,
        "int" => 0x08,
        "iag" => 0x09,
        "ias" => 0x0a,
        "rfi" => 0x0b,
        "iaq" => 0x0c,
        "hwn" => 0x10,
        "hwq" => 0x11,
        "hwi" => 0x12,
        _ => return None,
    })
}

fn basic_opcode(mnemonic: &str) -> Option<u16> {
    Some(match mnemonic {
        "set" => 0x01,
        "add" => 0x02,
        "sub" => 0x03,
        "mul" => 0x04,
        "mli" => 0x05,
        "div" => 0x06,
        "dvi" => 0x07,
        "mod" => 0x08,
        "mdi" => 0x09,
        "and" => 0x0a,
        "bor" => 0x0b,
        "xor" => 0x0c,
        "shr" => 0x0d,
        "asr" => 0x0e,
        "shl" => 0x0f,
        "ifb" => 0x10,
        "ifc" => 0x11,
        "ife" => 0x12,
        "ifn" => 0x13,
        "ifg" => 0x14,
        "ifa" => 0x15,
        "ifl" => 0x16,
        "ifu" => 0x17,
        "adx" => 0x1a,
        "sbx" => 0x1b,
        "sti" => 0x1e,
        "std" => 0x1f,
        _ => return None,
    })
}

fn parse_operand(
    operand: &str,
    position: Position,
    labels: &HashMap<String, u32>,
    pc: u32,
    resolve_labels: bool,
    symbol_units: SymbolUnits,
) -> Result<Operand, Diagnostic> {
    let registers = ["a", "b", "c", "x", "y", "z", "i", "j"];
    if let Some(index) = registers.iter().position(|register| *register == operand) {
        return Ok(Operand {
            code: index as u16,
            extra: None,
        });
    }
    if let Some(inner) = operand.strip_prefix('[').and_then(|s| s.strip_suffix(']')) {
        let inner = inner.trim();
        if let Some(index) = registers.iter().position(|register| *register == inner) {
            return Ok(Operand {
                code: 0x08 + index as u16,
                extra: None,
            });
        }
        if inner == "sp" {
            return Ok(Operand {
                code: 0x19,
                extra: None,
            });
        }
        let expression = parse_value_expression(inner)?;
        let mut terms = Vec::new();
        collect_add_terms(&expression, &mut terms);
        let register_terms = terms
            .iter()
            .filter_map(|term| expression_register(term))
            .collect::<Vec<_>>();
        if register_terms.len() > 1 {
            return Err(Diagnostic::new(format!(
                "DCPU address `{operand}` contains more than one register"
            )));
        }
        if let Some(register) = register_terms.first().copied() {
            if terms.iter().any(|term| {
                expression_register(term).is_none() && expression_mentions_register(term)
            }) {
                return Err(Diagnostic::new(format!(
                    "DCPU address `{operand}` has an unsupported register expression"
                )));
            }
            let mut offset = 0u16;
            for term in terms
                .iter()
                .filter(|term| expression_register(term).is_none())
            {
                offset = offset.wrapping_add(eval_value16(
                    term,
                    labels,
                    pc,
                    resolve_labels,
                    symbol_units,
                )?);
            }
            if register == "sp" {
                return Ok(Operand {
                    code: 0x1a,
                    extra: Some(offset),
                });
            }
            let index = registers
                .iter()
                .position(|candidate| *candidate == register)
                .expect("register was checked above");
            return Ok(Operand {
                code: 0x10 + index as u16,
                extra: Some(offset),
            });
        }
        if expression_mentions_register(&expression) {
            return Err(Diagnostic::new(format!(
                "DCPU address `{operand}` has an unsupported register expression"
            )));
        }
        return Ok(Operand {
            code: 0x1e,
            extra: Some(eval_value16(
                &expression,
                labels,
                pc,
                resolve_labels,
                symbol_units,
            )?),
        });
    }
    if let Some(value) = operand.strip_prefix("pick ") {
        return Ok(Operand {
            code: 0x1a,
            extra: Some(value16(
                value.trim(),
                labels,
                pc,
                resolve_labels,
                symbol_units,
            )?),
        });
    }
    if operand == "push" && position != Position::B {
        return Err(Diagnostic::new(
            "DCPU `push` is only valid in the B position",
        ));
    }
    if operand == "pop" && position != Position::A {
        return Err(Diagnostic::new(
            "DCPU `pop` is only valid in the A position",
        ));
    }
    let code = match operand {
        "push" => Some(0x18),
        "pop" => Some(0x18),
        "peek" => Some(0x19),
        "sp" => Some(0x1b),
        "pc" => Some(0x1c),
        "ex" => Some(0x1d),
        _ => None,
    };
    let parsed = if let Some(code) = code {
        Operand { code, extra: None }
    } else {
        let expression = parse_value_expression(operand)?;
        let force_next_word = expression_has_symbol_or_current(&expression);
        let value = eval_value16(&expression, labels, pc, resolve_labels, symbol_units)?;
        if force_next_word {
            Operand {
                code: 0x1f,
                extra: Some(value),
            }
        } else if value <= 30 {
            Operand {
                code: 0x21 + value,
                extra: None,
            }
        } else if value == 0xffff {
            Operand {
                code: 0x20,
                extra: None,
            }
        } else {
            Operand {
                code: 0x1f,
                extra: Some(value),
            }
        }
    };
    if position == Position::B && parsed.code >= 0x20 {
        return Err(Diagnostic::new(
            "DCPU literal operands are only valid in the A position",
        ));
    }
    Ok(parsed)
}

fn value16(
    text: &str,
    labels: &HashMap<String, u32>,
    pc: u32,
    resolve_labels: bool,
    symbol_units: SymbolUnits,
) -> Result<u16, Diagnostic> {
    let expression = parse_value_expression(text)?;
    eval_value16(&expression, labels, pc, resolve_labels, symbol_units)
}

fn parse_value_expression(text: &str) -> Result<AssemblyExpression, Diagnostic> {
    parse_assembly_expression(text)
        .map_err(|_| Diagnostic::new(format!("invalid DCPU expression `{text}`")))
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Eval {
    Known(i128),
    Unknown,
}

fn eval_value16(
    expression: &AssemblyExpression,
    symbols: &HashMap<String, u32>,
    pc: u32,
    resolve_symbols: bool,
    symbol_units: SymbolUnits,
) -> Result<u16, Diagnostic> {
    match eval_expression(expression, symbols, pc, resolve_symbols, symbol_units)? {
        Eval::Known(value) => Ok(value as u16),
        Eval::Unknown => Ok(0),
    }
}

fn eval_expression(
    expression: &AssemblyExpression,
    symbols: &HashMap<String, u32>,
    pc: u32,
    resolve_symbols: bool,
    symbol_units: SymbolUnits,
) -> Result<Eval, Diagnostic> {
    match expression {
        AssemblyExpression::Symbol(name) => symbol_value(name, symbols)
            .map(|value| match symbol_units {
                SymbolUnits::ByteAddresses => value / 2,
                SymbolUnits::WordAddresses => value,
            })
            .map(|value| Eval::Known(i128::from(value)))
            .map_or_else(
                || {
                    if resolve_symbols {
                        Err(Diagnostic::new(format!("unknown DCPU symbol `{name}`")))
                    } else {
                        Ok(Eval::Unknown)
                    }
                },
                Ok,
            ),
        AssemblyExpression::Current => Ok(Eval::Known(i128::from(match symbol_units {
            SymbolUnits::ByteAddresses => pc / 2,
            SymbolUnits::WordAddresses => pc,
        }))),
        AssemblyExpression::Number(value) => Ok(Eval::Known(i128::from(*value))),
        AssemblyExpression::Unary {
            operator,
            expression,
        } => match eval_expression(expression, symbols, pc, resolve_symbols, symbol_units)? {
            Eval::Unknown => Ok(Eval::Unknown),
            Eval::Known(value) => Ok(Eval::Known(match operator {
                AssemblyUnaryOperator::Plus => value,
                AssemblyUnaryOperator::Negate => -value,
                AssemblyUnaryOperator::BitNot => !value,
            })),
        },
        AssemblyExpression::Binary {
            operator,
            left,
            right,
        } => match (
            eval_expression(left, symbols, pc, resolve_symbols, symbol_units)?,
            eval_expression(right, symbols, pc, resolve_symbols, symbol_units)?,
        ) {
            (Eval::Known(left), Eval::Known(right)) => {
                let value = match operator {
                    AssemblyBinaryOperator::Add => left
                        .checked_add(right)
                        .ok_or_else(|| expression_overflow(expression))?,
                    AssemblyBinaryOperator::Subtract => left
                        .checked_sub(right)
                        .ok_or_else(|| expression_overflow(expression))?,
                    AssemblyBinaryOperator::Multiply => left
                        .checked_mul(right)
                        .ok_or_else(|| expression_overflow(expression))?,
                    AssemblyBinaryOperator::Divide if right == 0 => {
                        return Err(Diagnostic::new("division by zero in DCPU expression"));
                    }
                    AssemblyBinaryOperator::Divide => left
                        .checked_div(right)
                        .ok_or_else(|| expression_overflow(expression))?,
                    AssemblyBinaryOperator::ShiftLeft | AssemblyBinaryOperator::ShiftRight
                        if !(0..=127).contains(&right) =>
                    {
                        return Err(Diagnostic::new(format!(
                            "DCPU expression shift count `{right}` is outside 0 through 127"
                        )));
                    }
                    AssemblyBinaryOperator::ShiftLeft => left
                        .checked_shl(right as u32)
                        .ok_or_else(|| expression_overflow(expression))?,
                    AssemblyBinaryOperator::ShiftRight => left
                        .checked_shr(right as u32)
                        .ok_or_else(|| expression_overflow(expression))?,
                    AssemblyBinaryOperator::BitAnd => left & right,
                    AssemblyBinaryOperator::BitOr => left | right,
                    AssemblyBinaryOperator::BitXor => left ^ right,
                };
                Ok(Eval::Known(value))
            }
            _ => Ok(Eval::Unknown),
        },
    }
}

fn expression_overflow(expression: &AssemblyExpression) -> Diagnostic {
    Diagnostic::new(format!("DCPU expression `{expression:?}` overflows"))
}

fn symbol_value(name: &str, symbols: &HashMap<String, u32>) -> Option<u32> {
    symbols.get(name).copied().or_else(|| {
        symbols
            .iter()
            .find_map(|(known, value)| known.eq_ignore_ascii_case(name).then_some(*value))
    })
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

fn expression_register(expression: &AssemblyExpression) -> Option<&str> {
    let AssemblyExpression::Symbol(name) = expression else {
        return None;
    };
    matches!(
        name.as_str(),
        "a" | "b" | "c" | "x" | "y" | "z" | "i" | "j" | "sp"
    )
    .then_some(name.as_str())
}

fn expression_mentions_register(expression: &AssemblyExpression) -> bool {
    if expression_register(expression).is_some() {
        return true;
    }
    match expression {
        AssemblyExpression::Symbol(_)
        | AssemblyExpression::Current
        | AssemblyExpression::Number(_) => false,
        AssemblyExpression::Unary { expression, .. } => expression_mentions_register(expression),
        AssemblyExpression::Binary { left, right, .. } => {
            expression_mentions_register(left) || expression_mentions_register(right)
        }
    }
}

fn collect_add_terms<'a>(
    expression: &'a AssemblyExpression,
    terms: &mut Vec<&'a AssemblyExpression>,
) {
    if let AssemblyExpression::Binary {
        operator: AssemblyBinaryOperator::Add,
        left,
        right,
    } = expression
    {
        collect_add_terms(left, terms);
        collect_add_terms(right, terms);
    } else {
        terms.push(expression);
    }
}

fn words_to_bytes(words: &[u16]) -> Vec<u8> {
    words.iter().flat_map(|word| word.to_le_bytes()).collect()
}

#[cfg(test)]
mod tests;
