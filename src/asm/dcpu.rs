use std::collections::HashMap;

use crate::diagnostic::Diagnostic;

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
    resolve_labels: bool,
) -> Result<Vec<u8>, Diagnostic> {
    let lowered = normalize(text);
    let (mnemonic, operands) = parse_instruction(&lowered)?;
    let mut words = Vec::new();

    if let Some(opcode) = special_opcode(mnemonic) {
        let operand = parse_single_operand(mnemonic, operands)?;
        let operand = parse_operand(operand, Position::A, labels, pc, resolve_labels)?;
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
    let b = parse_operand(b_text, Position::B, labels, pc, resolve_labels)?;
    let a = parse_operand(a_text, Position::A, labels, pc, resolve_labels)?;
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
        if let Some((left, right)) = inner.split_once('+') {
            let left = left.trim();
            let right = right.trim();
            let (register, value) = if registers.contains(&left) {
                (left, right)
            } else if registers.contains(&right) {
                (right, left)
            } else if left == "sp" {
                return Ok(Operand {
                    code: 0x1a,
                    extra: Some(value16(right, labels, pc, resolve_labels)?),
                });
            } else if right == "sp" {
                return Ok(Operand {
                    code: 0x1a,
                    extra: Some(value16(left, labels, pc, resolve_labels)?),
                });
            } else {
                return Ok(Operand {
                    code: 0x1e,
                    extra: Some(value16(inner, labels, pc, resolve_labels)?),
                });
            };
            let index = registers
                .iter()
                .position(|candidate| *candidate == register)
                .expect("register was checked above");
            return Ok(Operand {
                code: 0x10 + index as u16,
                extra: Some(value16(value, labels, pc, resolve_labels)?),
            });
        }
        return Ok(Operand {
            code: 0x1e,
            extra: Some(value16(inner, labels, pc, resolve_labels)?),
        });
    }
    if let Some(value) = operand.strip_prefix("pick ") {
        return Ok(Operand {
            code: 0x1a,
            extra: Some(value16(value.trim(), labels, pc, resolve_labels)?),
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
        let force_next_word = label_value(operand, labels).is_some()
            || (!resolve_labels && !is_numeric_literal(operand));
        let value = value16(operand, labels, pc, resolve_labels)?;
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
    _pc: u32,
    resolve_labels: bool,
) -> Result<u16, Diagnostic> {
    let text = text.trim();
    if let Some(value) = label_value(text, labels) {
        return Ok((value / 2) as u16);
    }
    match parse_numeric_literal(text) {
        Ok(value) => Ok(value as u16),
        Err(_) if !resolve_labels => Ok(0),
        Err(_) => Err(Diagnostic::new(format!("unknown DCPU operand `{text}`"))),
    }
}

fn label_value(text: &str, labels: &HashMap<String, u32>) -> Option<u32> {
    labels.get(text).copied().or_else(|| {
        labels
            .iter()
            .find_map(|(name, value)| name.eq_ignore_ascii_case(text).then_some(*value))
    })
}

fn is_numeric_literal(text: &str) -> bool {
    parse_numeric_literal(text).is_ok()
}

fn parse_numeric_literal(text: &str) -> Result<u32, std::num::ParseIntError> {
    if text == "-1" {
        return Ok(0xffff);
    }
    if let Some(hex) = text.strip_prefix("0x") {
        u32::from_str_radix(hex, 16)
    } else if let Some(hex) = text.strip_suffix('h') {
        u32::from_str_radix(hex, 16)
    } else {
        text.parse::<u32>()
    }
}

fn words_to_bytes(words: &[u16]) -> Vec<u8> {
    words.iter().flat_map(|word| word.to_le_bytes()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn words(bytes: &[u8]) -> Vec<u16> {
        bytes
            .chunks_exact(2)
            .map(|word| u16::from_le_bytes([word[0], word[1]]))
            .collect()
    }

    #[test]
    fn encodes_every_basic_opcode_form() {
        let labels = HashMap::new();
        let cases = [
            ("set", 0x01),
            ("add", 0x02),
            ("sub", 0x03),
            ("mul", 0x04),
            ("mli", 0x05),
            ("div", 0x06),
            ("dvi", 0x07),
            ("mod", 0x08),
            ("mdi", 0x09),
            ("and", 0x0a),
            ("bor", 0x0b),
            ("xor", 0x0c),
            ("shr", 0x0d),
            ("asr", 0x0e),
            ("shl", 0x0f),
            ("ifb", 0x10),
            ("ifc", 0x11),
            ("ife", 0x12),
            ("ifn", 0x13),
            ("ifg", 0x14),
            ("ifa", 0x15),
            ("ifl", 0x16),
            ("ifu", 0x17),
            ("adx", 0x1a),
            ("sbx", 0x1b),
            ("sti", 0x1e),
            ("std", 0x1f),
        ];

        for (mnemonic, opcode) in cases {
            let source = format!("{mnemonic} b, a");
            assert_eq!(
                words(&encode_instruction(&source, &labels, 0).unwrap()),
                [opcode | 0x20]
            );
        }
    }

    #[test]
    fn encodes_every_special_opcode_form() {
        let labels = HashMap::new();
        for (mnemonic, opcode) in [
            ("jsr", 0x01),
            ("int", 0x08),
            ("iag", 0x09),
            ("ias", 0x0a),
            ("rfi", 0x0b),
            ("iaq", 0x0c),
            ("hwn", 0x10),
            ("hwq", 0x11),
            ("hwi", 0x12),
        ] {
            let source = format!("{mnemonic} a");
            assert_eq!(
                words(&encode_instruction(&source, &labels, 0).unwrap()),
                [opcode << 5]
            );
        }
    }

    #[test]
    fn encodes_all_a_operand_encodings() {
        let labels = HashMap::new();
        let cases = [
            ("a", 0x00, None),
            ("b", 0x01, None),
            ("c", 0x02, None),
            ("x", 0x03, None),
            ("y", 0x04, None),
            ("z", 0x05, None),
            ("i", 0x06, None),
            ("j", 0x07, None),
            ("[a]", 0x08, None),
            ("[b]", 0x09, None),
            ("[c]", 0x0a, None),
            ("[x]", 0x0b, None),
            ("[y]", 0x0c, None),
            ("[z]", 0x0d, None),
            ("[i]", 0x0e, None),
            ("[j]", 0x0f, None),
            ("[0x1111+a]", 0x10, Some(0x1111)),
            ("[b + 0x1112]", 0x11, Some(0x1112)),
            ("[0x1113 + c]", 0x12, Some(0x1113)),
            ("[x+0x1114]", 0x13, Some(0x1114)),
            ("[0x1115+y]", 0x14, Some(0x1115)),
            ("[z+0x1116]", 0x15, Some(0x1116)),
            ("[0x1117+i]", 0x16, Some(0x1117)),
            ("[j+0x1118]", 0x17, Some(0x1118)),
            ("pop", 0x18, None),
            ("peek", 0x19, None),
            ("pick 0x1119", 0x1a, Some(0x1119)),
            ("[sp + 0x111a]", 0x1a, Some(0x111a)),
            ("sp", 0x1b, None),
            ("pc", 0x1c, None),
            ("ex", 0x1d, None),
            ("[0x111e]", 0x1e, Some(0x111e)),
            ("0x111f", 0x1f, Some(0x111f)),
            ("0xffff", 0x20, None),
        ];

        for (operand, code, extra) in cases {
            let source = format!("set b, {operand}");
            let encoded = words(&encode_instruction(&source, &labels, 0).unwrap());
            assert_eq!(encoded[0], 0x01 | (0x01 << 5) | (code << 10), "{source}");
            assert_eq!(encoded.get(1).copied(), extra, "{source}");
        }
        for value in 0..=30 {
            let encoded =
                words(&encode_instruction(&format!("set b, {value}"), &labels, 0).unwrap());
            assert_eq!(encoded, [0x01 | (0x01 << 5) | ((0x21 + value) << 10)]);
        }
        assert_eq!(
            words(&encode_instruction("set b, -1", &labels, 0).unwrap()),
            [0x01 | (0x01 << 5) | (0x20 << 10)],
        );
    }

    #[test]
    fn encodes_all_b_operand_encodings() {
        let labels = HashMap::new();
        let cases = [
            ("a", 0x00),
            ("b", 0x01),
            ("c", 0x02),
            ("x", 0x03),
            ("y", 0x04),
            ("z", 0x05),
            ("i", 0x06),
            ("j", 0x07),
            ("[a]", 0x08),
            ("[b]", 0x09),
            ("[c]", 0x0a),
            ("[x]", 0x0b),
            ("[y]", 0x0c),
            ("[z]", 0x0d),
            ("[i]", 0x0e),
            ("[j]", 0x0f),
            ("[0x1000+a]", 0x10),
            ("[0x1000+b]", 0x11),
            ("[0x1000+c]", 0x12),
            ("[0x1000+x]", 0x13),
            ("[0x1000+y]", 0x14),
            ("[0x1000+z]", 0x15),
            ("[0x1000+i]", 0x16),
            ("[0x1000+j]", 0x17),
            ("push", 0x18),
            ("peek", 0x19),
            ("pick 1", 0x1a),
            ("sp", 0x1b),
            ("pc", 0x1c),
            ("ex", 0x1d),
            ("[0x1000]", 0x1e),
            ("0x1000", 0x1f),
        ];

        for (operand, code) in cases {
            let source = format!("set {operand}, a");
            let encoded = words(&encode_instruction(&source, &labels, 0).unwrap());
            assert_eq!(encoded[0], 0x01 | (code << 5), "{source}");
        }
    }

    #[test]
    fn preserves_extra_word_order_and_label_word_addresses() {
        let labels = HashMap::from([("Destination".to_owned(), 0x1234)]);
        assert_eq!(
            words(&encode_instruction("set [0x1111], 0x2222", &labels, 0).unwrap()),
            [0x7fc1, 0x1111, 0x2222],
        );
        assert_eq!(
            words(&encode_instruction("JSR destination", &labels, 0).unwrap()),
            [0x7c20, 0x091a],
        );
        assert_eq!(instruction_len("set [symbol], symbol").unwrap(), 6);
        assert_eq!(instruction_len("hwi symbol").unwrap(), 4);
    }

    #[test]
    fn rejects_invalid_operand_positions_and_arity() {
        let labels = HashMap::new();
        for source in [
            "set 0, a",
            "set 0xffff, a",
            "set pop, a",
            "set a, push",
            "set a",
            "set a, b, c",
            "int",
            "int a, b",
            "rfi ,",
            "unknown a",
        ] {
            assert!(encode_instruction(source, &labels, 0).is_err(), "{source}");
            assert!(instruction_len(source).is_err(), "{source}");
        }
    }
}
