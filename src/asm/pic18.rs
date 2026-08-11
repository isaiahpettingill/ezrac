//! Classic PIC18 instruction encoding.
//!
//! This module intentionally implements the baseline PIC18 instruction set
//! only. Extended instruction-set mode is a separate target profile and is not
//! enabled by `AssemblerCpu::Pic18`.

use crate::{asm::frontend::AssemblyExpression, compat::prelude::*, diagnostic::Diagnostic};

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
    let (mnemonic, operands) = split_instruction(text)?;
    let mnemonic = mnemonic.to_ascii_lowercase();
    let operands = operands.trim();

    let fixed = match mnemonic.as_str() {
        "nop" => Some(0x0000),
        "sleep" => Some(0x0003),
        "clrwdt" => Some(0x0004),
        "push" => Some(0x0005),
        "pop" => Some(0x0006),
        "daw" => Some(0x0007),
        "callw" => Some(0x0014),
        "reset" => Some(0x00FF),
        "tblrd*" => Some(0x0008),
        "tblrd*+" => Some(0x0009),
        "tblrd*-" => Some(0x000A),
        "tblrd+*" => Some(0x000B),
        _ => None,
    };
    if let Some(opcode) = fixed {
        no_operands(operands, &mnemonic)?;
        return Ok(word(opcode));
    }

    match mnemonic.as_str() {
        "return" | "retfie" => {
            let fast = fast_operand(operands, &mnemonic)?;
            let opcode = if mnemonic == "return" { 0x0012 } else { 0x0010 } | u16::from(fast);
            Ok(word(opcode))
        }
        "movlb" => {
            let bank = immediate(operands, labels, pc, resolve, 0x0F, "PIC18 bank")?;
            Ok(word(0x0100 | bank as u16))
        }
        "addlw" | "andlw" | "iorlw" | "movlw" | "mullw" | "retlw" | "sublw" | "xorlw" => {
            let value = immediate(operands, labels, pc, resolve, 0xFF, "PIC18 literal")?;
            let base = match mnemonic.as_str() {
                "sublw" => 0x0800,
                "iorlw" => 0x0900,
                "xorlw" => 0x0A00,
                "andlw" => 0x0B00,
                "retlw" => 0x0C00,
                "mullw" => 0x0D00,
                "movlw" => 0x0E00,
                "addlw" => 0x0F00,
                _ => unreachable!(),
            };
            Ok(word(base | value as u16))
        }
        "movff" => {
            let values = operands_array(operands, 2, &mnemonic)?;
            let source = immediate(
                values[0],
                labels,
                pc,
                resolve,
                0x0FFF,
                "MOVFF source address",
            )?;
            let destination = immediate(
                values[1],
                labels,
                pc,
                resolve,
                0x0FFF,
                "MOVFF destination address",
            )?;
            Ok(words(&[
                0xC000 | source as u16,
                0xF000 | destination as u16,
            ]))
        }
        "lfsr" => {
            let values = operands_array(operands, 2, &mnemonic)?;
            let index = immediate(values[0], labels, pc, resolve, 2, "LFSR index")?;
            let value = immediate(values[1], labels, pc, resolve, 0x0FFF, "LFSR address")?;
            Ok(words(&[
                0xEE00 | ((index as u16) << 4) | ((value as u16 >> 8) & 0x0F),
                0xF000 | (value as u16 & 0x00FF),
            ]))
        }
        "call" => {
            let values = split_operands(operands);
            if values.is_empty() || values.len() > 2 {
                return Err(error(
                    "PIC18 CALL requires an address and optional fast selector",
                ));
            }
            let address = program_address(values[0], labels, pc, resolve, "CALL target")?;
            let fast = values
                .get(1)
                .map(|value| fast_selector(value, &mnemonic))
                .transpose()?
                .unwrap_or(false);
            Ok(long_words(0xEC00 | if fast { 0x0100 } else { 0 }, address))
        }
        "goto" => {
            let target = immediate(operands, labels, pc, resolve, 0x1F_FFFF, "GOTO target")?;
            if target & 1 != 0 {
                return Err(error("PIC18 GOTO target must be an even byte address"));
            }
            Ok(long_words(0xEF00, target))
        }
        "bra" => relative(0xD000, 12, operands, labels, pc, resolve, "BRA"),
        "rcall" => relative(0xD800, 11, operands, labels, pc, resolve, "RCALL"),
        "bz" | "bnz" | "bc" | "bnc" | "bov" | "bnov" | "bn" | "bnn" => {
            let base = match mnemonic.as_str() {
                "bz" => 0xE000,
                "bnz" => 0xE100,
                "bc" => 0xE200,
                "bnc" => 0xE300,
                "bov" => 0xE400,
                "bnov" => 0xE500,
                "bn" => 0xE600,
                "bnn" => 0xE700,
                _ => unreachable!(),
            };
            relative(
                base,
                8,
                operands,
                labels,
                pc,
                resolve,
                &mnemonic.to_ascii_uppercase(),
            )
        }
        "addwf" | "addwfc" | "andwf" | "comf" | "decf" | "decfsz" | "dcfsnz" | "incf"
        | "incfsz" | "infsnz" | "iorwf" | "movf" | "movwf" | "mulwf" | "negf" | "rlcf"
        | "rlncf" | "rrcf" | "rrncf" | "setf" | "subfwb" | "subwf" | "subwfb" | "swapf"
        | "tstfsz" | "xorwf" | "clrf" => {
            encode_byte_operation(&mnemonic, operands, labels, pc, resolve)
        }
        "cpfseq" | "cpfslt" | "cpfsgt" => {
            let [file, addressing] = file_and_addressing(operands, &mnemonic)?;
            let file = file_value(file, labels, pc, resolve)?;
            let addressing = addressing_selector(addressing)?;
            let base = match mnemonic.as_str() {
                "cpfseq" => 0x6000,
                "cpfslt" => 0x6200,
                "cpfsgt" => 0x6400,
                _ => unreachable!(),
            };
            Ok(word(base | (u16::from(addressing) << 8) | file as u16))
        }
        "bcf" | "bsf" | "btfsc" | "btfss" | "btg" => {
            encode_bit_operation(&mnemonic, operands, labels, pc, resolve)
        }
        _ => Err(error(format!(
            "assembler does not support classic PIC18 instruction `{text}`"
        ))),
    }
}

fn encode_byte_operation(
    mnemonic: &str,
    operands: &str,
    labels: &HashMap<String, u32>,
    pc: u32,
    resolve: bool,
) -> Result<Vec<u8>, Diagnostic> {
    let values = split_operands(operands);
    if values.is_empty() || values.len() > 3 {
        return Err(error(format!("PIC18 {mnemonic} expects file[,d][,a]")));
    }
    let file = file_value(values[0], labels, pc, resolve)?;
    let destination = if matches!(mnemonic, "movwf" | "clrf" | "setf" | "negf" | "mulwf") {
        if values.len() == 3 {
            return Err(error(format!(
                "PIC18 {mnemonic} has no destination selector"
            )));
        }
        true
    } else {
        values
            .get(1)
            .map(|value| destination_selector(value))
            .transpose()?
            .unwrap_or(true)
    };
    let addressing = values
        .get(
            if matches!(mnemonic, "movwf" | "clrf" | "setf" | "negf" | "mulwf") {
                1
            } else {
                2
            },
        )
        .map(|value| addressing_selector(value))
        .transpose()?
        .unwrap_or(false);

    let base = match mnemonic {
        "decf" => 0x0400,
        "iorwf" => 0x1000,
        "andwf" => 0x1400,
        "xorwf" => 0x1800,
        "comf" => 0x1C00,
        "addwfc" => 0x2000,
        "addwf" => 0x2400,
        "incf" => 0x2800,
        "decfsz" => 0x2C00,
        "rrcf" => 0x3000,
        "rlcf" => 0x3400,
        "swapf" => 0x3800,
        "incfsz" => 0x3C00,
        "rrncf" => 0x4000,
        "rlncf" => 0x4400,
        "infsnz" => 0x4800,
        "dcfsnz" => 0x4C00,
        "movf" => 0x5000,
        "subfwb" => 0x5400,
        "subwfb" => 0x5800,
        "subwf" => 0x5C00,
        "tstfsz" => 0x6600,
        "setf" => 0x6800,
        "clrf" => 0x6A00,
        "negf" => 0x6C00,
        "movwf" => 0x6E00,
        "mulwf" => 0x0200,
        _ => {
            return Err(error(format!(
                "unsupported PIC18 byte operation `{mnemonic}`"
            )));
        }
    };
    let d_bit = if destination { 0x0200 } else { 0 };
    Ok(word(
        base | d_bit | (u16::from(addressing) << 8) | file as u16,
    ))
}

fn encode_bit_operation(
    mnemonic: &str,
    operands: &str,
    labels: &HashMap<String, u32>,
    pc: u32,
    resolve: bool,
) -> Result<Vec<u8>, Diagnostic> {
    let values = split_operands(operands);
    if values.len() < 3 || values.len() > 4 {
        return Err(error(format!("PIC18 {mnemonic} expects file,bit[,a]")));
    }
    let file = file_value(values[0], labels, pc, resolve)?;
    let bit = immediate(values[1], labels, pc, resolve, 7, "PIC18 bit")?;
    let addressing = values
        .get(2)
        .map(|value| addressing_selector(value))
        .transpose()?
        .unwrap_or(false);
    let base = match mnemonic {
        "btg" => 0x7000,
        "bsf" => 0x8000,
        "bcf" => 0x9000,
        "btfss" => 0xA000,
        "btfsc" => 0xB000,
        _ => unreachable!(),
    };
    Ok(word(
        base | ((bit as u16) << 9) | (u16::from(addressing) << 8) | file as u16,
    ))
}

fn file_and_addressing<'a>(operands: &'a str, mnemonic: &str) -> Result<[&'a str; 2], Diagnostic> {
    let values = split_operands(operands);
    if values.is_empty() || values.len() > 2 {
        return Err(error(format!("PIC18 {mnemonic} expects file[,a]")));
    }
    Ok([values[0], values.get(1).copied().unwrap_or("access")])
}

fn file_value(
    text: &str,
    labels: &HashMap<String, u32>,
    pc: u32,
    resolve: bool,
) -> Result<u32, Diagnostic> {
    let value = immediate(text, labels, pc, resolve, 0xFF, "PIC18 file address")?;
    Ok(value)
}

fn destination_selector(text: &str) -> Result<bool, Diagnostic> {
    match text.trim().to_ascii_lowercase().as_str() {
        "f" | "file" | "1" => Ok(true),
        "w" | "wreg" | "0" => Ok(false),
        _ => Err(error(format!(
            "invalid PIC18 destination selector `{text}`"
        ))),
    }
}

fn addressing_selector(text: &str) -> Result<bool, Diagnostic> {
    match text.trim().to_ascii_lowercase().as_str() {
        "a" | "access" | "0" => Ok(false),
        "b" | "banked" | "1" => Ok(true),
        _ => Err(error(format!("invalid PIC18 addressing selector `{text}`"))),
    }
}

fn fast_operand(text: &str, mnemonic: &str) -> Result<bool, Diagnostic> {
    if text.trim().is_empty() {
        return Ok(false);
    }
    let values = split_operands(text);
    if values.len() != 1 {
        return Err(error(format!(
            "PIC18 {mnemonic} accepts only the optional fast selector"
        )));
    }
    fast_selector(values[0], mnemonic)
}

fn fast_selector(text: &str, mnemonic: &str) -> Result<bool, Diagnostic> {
    match text.trim().to_ascii_lowercase().as_str() {
        "fast" | "s" | "1" => Ok(true),
        "normal" | "0" => Ok(false),
        _ => Err(error(format!(
            "invalid PIC18 {mnemonic} fast selector `{text}`"
        ))),
    }
}

fn relative(
    base: u16,
    bits: u8,
    operand: &str,
    labels: &HashMap<String, u32>,
    pc: u32,
    resolve: bool,
    name: &str,
) -> Result<Vec<u8>, Diagnostic> {
    let target = immediate(
        operand,
        labels,
        pc,
        resolve,
        0x1F_FFFF,
        "PIC18 branch target",
    )?;
    let next = pc + 2;
    let delta = i64::from(target) - i64::from(next);
    if delta & 1 != 0 {
        return Err(error(format!(
            "PIC18 {name} target must be an even byte address"
        )));
    }
    let offset = if resolve { delta / 2 } else { 0 };
    let min = -(1i64 << (bits - 1));
    let max = (1i64 << (bits - 1)) - 1;
    if resolve && !(min..=max).contains(&offset) {
        return Err(error(format!("PIC18 {name} target is out of range")));
    }
    Ok(word(base | (offset as u16 & ((1u16 << bits) - 1))))
}

fn program_address(
    operand: &str,
    labels: &HashMap<String, u32>,
    pc: u32,
    resolve: bool,
    description: &str,
) -> Result<u32, Diagnostic> {
    let address = immediate(operand, labels, pc, resolve, 0x1F_FFFF, description)?;
    if address & 1 != 0 {
        return Err(error(format!(
            "PIC18 {description} must be an even byte address"
        )));
    }
    Ok(address)
}

fn immediate(
    text: &str,
    labels: &HashMap<String, u32>,
    pc: u32,
    resolve: bool,
    max: u32,
    description: &str,
) -> Result<u32, Diagnostic> {
    let expression = crate::asm::parse_assembly_expression(text.trim())
        .map_err(|error| Diagnostic::new(format!("invalid {description}: {}", error.message)))?;
    let value = eval_expression(&expression, labels, pc, resolve)?;
    if value > i128::from(max) {
        return Err(error(format!(
            "{description} `{text}` is outside 0..0x{max:X}"
        )));
    }
    Ok(value as u32)
}

fn eval_expression(
    expression: &AssemblyExpression,
    labels: &HashMap<String, u32>,
    pc: u32,
    resolve: bool,
) -> Result<i128, Diagnostic> {
    match expression {
        AssemblyExpression::Symbol(name) => pic18_symbol(name)
            .or_else(|| {
                labels.get(name).copied().or_else(|| {
                    labels.iter().find_map(|(known, value)| {
                        known.eq_ignore_ascii_case(name).then_some(*value)
                    })
                })
            })
            .map(i128::from)
            .or_else(|| (!resolve).then_some(0))
            .ok_or_else(|| error(format!("unknown PIC18 assembly symbol `{name}`"))),
        AssemblyExpression::Current => Ok(i128::from(pc)),
        AssemblyExpression::Number(value) => Ok(i128::from(*value)),
        AssemblyExpression::Unary {
            operator,
            expression,
        } => {
            let value = eval_expression(expression, labels, pc, resolve)?;
            match operator {
                crate::asm::AssemblyUnaryOperator::Plus => Ok(value),
                crate::asm::AssemblyUnaryOperator::Negate => Ok(-value),
                crate::asm::AssemblyUnaryOperator::BitNot => Ok(!value),
            }
        }
        AssemblyExpression::Binary {
            operator,
            left,
            right,
        } => {
            let left = eval_expression(left, labels, pc, resolve)?;
            let right = eval_expression(right, labels, pc, resolve)?;
            let value = match operator {
                crate::asm::AssemblyBinaryOperator::Add => left + right,
                crate::asm::AssemblyBinaryOperator::Subtract => left - right,
                crate::asm::AssemblyBinaryOperator::Multiply => left * right,
                crate::asm::AssemblyBinaryOperator::Divide => {
                    if right == 0 {
                        return Err(error("division by zero in PIC18 assembly expression"));
                    }
                    left / right
                }
                crate::asm::AssemblyBinaryOperator::ShiftLeft => left << right,
                crate::asm::AssemblyBinaryOperator::ShiftRight => left >> right,
                crate::asm::AssemblyBinaryOperator::BitAnd => left & right,
                crate::asm::AssemblyBinaryOperator::BitOr => left | right,
                crate::asm::AssemblyBinaryOperator::BitXor => left ^ right,
            };
            Ok(value)
        }
    }
}

fn operands_array<'a>(
    operands: &'a str,
    count: usize,
    mnemonic: &str,
) -> Result<Vec<&'a str>, Diagnostic> {
    let values = split_operands(operands);
    if values.len() != count {
        return Err(error(format!("PIC18 {mnemonic} expects {count} operands")));
    }
    Ok(values)
}

fn pic18_symbol(name: &str) -> Option<u32> {
    Some(match name.to_ascii_uppercase().as_str() {
        "STATUS" => 0xD8,
        "FSR2L" => 0xD9,
        "FSR2H" => 0xDA,
        "PLUSW2" => 0xDB,
        "PREINC2" => 0xDC,
        "POSTDEC2" => 0xDD,
        "POSTINC2" => 0xDE,
        "INDF2" => 0xDF,
        "BSR" => 0xE0,
        "FSR1L" => 0xE1,
        "FSR1H" => 0xE2,
        "PLUSW1" => 0xE3,
        "PREINC1" => 0xE4,
        "POSTDEC1" => 0xE5,
        "POSTINC1" => 0xE6,
        "INDF1" => 0xE7,
        "WREG" => 0xE8,
        "FSR0L" => 0xE9,
        "FSR0H" => 0xEA,
        "PLUSW0" => 0xEB,
        "PREINC0" => 0xEC,
        "POSTDEC0" => 0xED,
        "POSTINC0" => 0xEE,
        "INDF0" => 0xEF,
        "TABLAT" => 0xF5,
        "TBLPTRL" => 0xF6,
        "TBLPTRH" => 0xF7,
        "TBLPTRU" => 0xF8,
        "PCL" => 0xF9,
        "PCLATH" => 0xFA,
        "PCLATU" => 0xFB,
        "STKPTR" => 0xFC,
        "TOSL" => 0xFD,
        "TOSH" => 0xFE,
        "TOSU" => 0xFF,
        _ => return None,
    })
}

fn split_instruction(text: &str) -> Result<(&str, &str), Diagnostic> {
    let text = text.trim();
    let Some((mnemonic, operands)) = text.split_once(char::is_whitespace) else {
        return Ok((text, ""));
    };
    if mnemonic.is_empty() {
        return Err(error("PIC18 instruction has no mnemonic"));
    }
    Ok((mnemonic, operands.trim()))
}

fn split_operands(text: &str) -> Vec<&str> {
    text.split(',')
        .map(str::trim)
        .filter(|part| !part.is_empty())
        .collect()
}

fn no_operands(operands: &str, mnemonic: &str) -> Result<(), Diagnostic> {
    if operands.is_empty() {
        Ok(())
    } else {
        Err(error(format!("PIC18 {mnemonic} takes no operands")))
    }
}

fn word(opcode: u16) -> Vec<u8> {
    opcode.to_le_bytes().to_vec()
}

fn words(opcodes: &[u16]) -> Vec<u8> {
    opcodes
        .iter()
        .flat_map(|opcode| opcode.to_le_bytes())
        .collect()
}

fn long_words(base: u16, address: u32) -> Vec<u8> {
    let literal = (address >> 1) & 0x0F_FFFF;
    words(&[
        base | (literal as u16 & 0x00FF),
        0xF000 | ((literal >> 8) as u16 & 0x0FFF),
    ])
}

fn error(message: impl Into<String>) -> Diagnostic {
    Diagnostic::new(message)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn encode_one(text: &str) -> Vec<u8> {
        encode_instruction(text, &HashMap::new(), 0).unwrap()
    }

    #[test]
    fn encodes_classic_fixed_and_literal_forms() {
        assert_eq!(encode_one("nop"), vec![0x00, 0x00]);
        assert_eq!(encode_one("return fast"), vec![0x13, 0x00]);
        assert_eq!(encode_one("movlw 55h"), vec![0x55, 0x0E]);
        assert_eq!(encode_one("movlb 3"), vec![0x03, 0x01]);
        assert_eq!(encode_one("tblrd*+"), vec![0x09, 0x00]);
    }

    #[test]
    fn encodes_file_bit_and_two_word_forms() {
        assert_eq!(encode_one("addwf 20h, f, a"), vec![0x20, 0x26]);
        assert_eq!(encode_one("btfss 20h, 3, b"), vec![0x20, 0xA7]);
        assert_eq!(encode_one("movff 120h, 345h"), vec![0x20, 0xC1, 0x45, 0xF3]);
        assert_eq!(encode_one("lfsr 1, 0ABCh"), vec![0x1A, 0xEE, 0xBC, 0xF0]);
    }

    #[test]
    fn resolves_word_addressed_calls_and_branches() {
        let mut labels = HashMap::new();
        labels.insert("callee".to_owned(), 0x0120);
        labels.insert("near".to_owned(), 0x000A);
        assert_eq!(
            encode_instruction("call callee", &labels, 0).unwrap(),
            vec![0x90, 0xEC, 0x00, 0xF0]
        );
        assert_eq!(
            encode_instruction("bra near", &labels, 0).unwrap(),
            vec![0x04, 0xD0]
        );
    }

    #[test]
    fn rejects_extended_or_invalid_forms() {
        assert!(encode_instruction("movlw 100h", &HashMap::new(), 0).is_err());
        assert!(encode_instruction("addwf 20h, q", &HashMap::new(), 0).is_err());
        assert!(encode_instruction("goto 3", &HashMap::new(), 0).is_err());
    }
}
