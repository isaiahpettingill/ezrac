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
    let (mnemonic, operands) = split_instruction(text)?;
    let mnemonic = mnemonic.to_ascii_lowercase();

    if let Some(base) = two_operand_base(&mnemonic) {
        let (source, destination) = split_operands(operands)?;
        let source = parse_operand(source, labels, resolve)?;
        let destination = parse_operand(destination, labels, resolve)?;
        let mut bytes = word(base | ((source.field as u16) << 6) | destination.field as u16);
        bytes.extend(source.extensions);
        bytes.extend(destination.extensions);
        return Ok(bytes);
    }

    if let Some(base) = single_operand_base(&mnemonic) {
        let operand = parse_operand(required_operand(operands, &mnemonic)?, labels, resolve)?;
        let mut bytes = word(base | operand.field as u16);
        bytes.extend(operand.extensions);
        return Ok(bytes);
    }

    if let Some(base) = immediate_base(&mnemonic) {
        return encode_immediate(base, &mnemonic, operands, labels, resolve);
    }

    if mnemonic == "xop" {
        let (operand, vector) = split_operands(operands)?;
        let operand = parse_operand(operand, labels, resolve)?;
        let vector = value(vector, labels, resolve)?;
        if vector > 15 {
            return Err(Diagnostic::new(format!(
                "TMS9900 XOP vector `{vector}` is outside 0..15"
            )));
        }
        let mut bytes = word(0x2c00 | ((operand.field as u16) << 6) | vector as u16);
        bytes.extend(operand.extensions);
        return Ok(bytes);
    }

    if let Some(base) = shift_base(&mnemonic) {
        let (count, register) = split_operands(operands)?;
        let count = value(count, labels, resolve)?;
        if count > 15 {
            return Err(Diagnostic::new(format!(
                "TMS9900 shift count `{count}` is outside 0..15"
            )));
        }
        let register = register_number(register)?;
        return Ok(word(base | ((count as u16) << 4) | register as u16));
    }

    if let Some(base) = jump_base(&mnemonic) {
        if !resolve {
            required_operand(operands, &mnemonic)?;
            return Ok(word(base));
        }
        let target = value(required_operand(operands, &mnemonic)?, labels, true)?;
        let next = pc.wrapping_add(2);
        if target & 1 != 0 {
            return Err(Diagnostic::new(format!(
                "TMS9900 {mnemonic} target is not word-aligned"
            )));
        }
        let delta = target as i64 - next as i64;
        if delta % 2 != 0 {
            return Err(Diagnostic::new(format!(
                "TMS9900 {mnemonic} target is not word-aligned"
            )));
        }
        let displacement = delta / 2;
        if !(-128..=127).contains(&displacement) {
            return Err(Diagnostic::new(format!(
                "TMS9900 {mnemonic} target is outside the -128..127 word range"
            )));
        }
        return Ok(word(base | (displacement as i8 as u8) as u16));
    }

    if let Some(base) = cru_bit_base(&mnemonic) {
        let offset = signed_value(required_operand(operands, &mnemonic)?, labels, resolve)?;
        if !(-128..=127).contains(&offset) {
            return Err(Diagnostic::new(format!(
                "TMS9900 {mnemonic} CRU offset is outside -128..127"
            )));
        }
        return Ok(word(base | (offset as i8 as u8) as u16));
    }

    if matches!(mnemonic.as_str(), "ldcr" | "stcr") {
        let (operand, count) = split_operands(operands)?;
        let operand = parse_operand(operand, labels, resolve)?;
        let count = value(count, labels, resolve)?;
        if count > 16 {
            return Err(Diagnostic::new(format!(
                "TMS9900 {mnemonic} count `{count}` is outside 0..16"
            )));
        }
        let base = if mnemonic == "ldcr" { 0x3000 } else { 0x3400 };
        // The ISA encodes a 16-bit transfer as a zero count field.
        let count_field = (count & 0x0f) as u16;
        let mut bytes = word(base | (count_field << 6) | operand.field as u16);
        bytes.extend(operand.extensions);
        return Ok(bytes);
    }

    if matches!(mnemonic.as_str(), "mpy" | "div") {
        let (source, destination) = split_operands(operands)?;
        let source = parse_operand(source, labels, resolve)?;
        let destination = register_number(destination)?;
        let base = if mnemonic == "mpy" { 0x3800 } else { 0x3c00 };
        let mut bytes = word(base | ((source.field as u16) << 6) | destination as u16);
        bytes.extend(source.extensions);
        return Ok(bytes);
    }

    match mnemonic.as_str() {
        "nop" if operands.is_empty() => Ok(word(0x1000)),
        "idle" if operands.is_empty() => Ok(word(0x0340)),
        "rset" if operands.is_empty() => Ok(word(0x0360)),
        "rtwp" if operands.is_empty() => Ok(word(0x0380)),
        "ckon" if operands.is_empty() => Ok(word(0x03a0)),
        "ckof" if operands.is_empty() => Ok(word(0x03c0)),
        "lrex" if operands.is_empty() => Ok(word(0x03e0)),
        _ => Err(Diagnostic::new(format!(
            "assembler does not support TMS9900 instruction `{text}`"
        ))),
    }
}

fn two_operand_base(mnemonic: &str) -> Option<u16> {
    Some(match mnemonic {
        "szc" => 0x4000,
        "szcb" => 0x5000,
        "s" => 0x6000,
        "sb" => 0x7000,
        "c" => 0x8000,
        "cb" => 0x9000,
        "a" => 0xa000,
        "ab" => 0xb000,
        "mov" => 0xc000,
        "movb" => 0xd000,
        "soc" => 0xe000,
        "socb" => 0xf000,
        _ => return None,
    })
}

fn single_operand_base(mnemonic: &str) -> Option<u16> {
    Some(match mnemonic {
        "blwp" => 0x0400,
        "b" => 0x0440,
        "x" => 0x0480,
        "clr" => 0x04c0,
        "neg" => 0x0500,
        "inv" => 0x0540,
        "inc" => 0x0580,
        "inct" => 0x05c0,
        "dec" => 0x0600,
        "dect" => 0x0640,
        "bl" => 0x0680,
        "swpb" => 0x06c0,
        "seto" => 0x0700,
        "abs" => 0x0740,
        _ => return None,
    })
}

fn immediate_base(mnemonic: &str) -> Option<u16> {
    Some(match mnemonic {
        "li" => 0x0200,
        "ai" => 0x0220,
        "andi" => 0x0240,
        "ori" => 0x0260,
        "ci" => 0x0280,
        "stwp" => 0x02a0,
        "stst" => 0x02c0,
        "lwpi" => 0x02e0,
        "limi" => 0x0300,
        _ => return None,
    })
}

fn shift_base(mnemonic: &str) -> Option<u16> {
    Some(match mnemonic {
        "sra" => 0x0800,
        "srl" => 0x0900,
        "sla" => 0x0a00,
        "src" => 0x0b00,
        _ => return None,
    })
}

fn jump_base(mnemonic: &str) -> Option<u16> {
    Some(match mnemonic {
        "jmp" => 0x1000,
        "jlt" => 0x1100,
        "jle" => 0x1200,
        "jeq" => 0x1300,
        "jhe" => 0x1400,
        "jgt" => 0x1500,
        "jne" => 0x1600,
        "jnc" => 0x1700,
        "joc" => 0x1800,
        "jno" => 0x1900,
        "jl" => 0x1a00,
        "jh" => 0x1b00,
        "jop" => 0x1c00,
        _ => return None,
    })
}

fn cru_bit_base(mnemonic: &str) -> Option<u16> {
    Some(match mnemonic {
        "sbo" => 0x1d00,
        "sbz" => 0x1e00,
        "tb" => 0x1f00,
        _ => return None,
    })
}

fn encode_immediate(
    base: u16,
    mnemonic: &str,
    operands: &str,
    labels: &HashMap<String, u32>,
    resolve: bool,
) -> Result<Vec<u8>, Diagnostic> {
    match mnemonic {
        "stwp" | "stst" => {
            let register = register_number(required_operand(operands, mnemonic)?)?;
            Ok(word(base | register as u16))
        }
        "lwpi" => {
            let immediate = word_value(required_operand(operands, mnemonic)?, labels, resolve)?;
            let mut bytes = word(base);
            bytes.extend(word(immediate));
            Ok(bytes)
        }
        "limi" => {
            let mask = value(required_operand(operands, mnemonic)?, labels, resolve)?;
            if mask > 15 {
                return Err(Diagnostic::new(format!(
                    "TMS9900 LIMI mask `{mask}` is outside 0..15"
                )));
            }
            let mut bytes = word(base);
            bytes.extend(word(mask as u16));
            Ok(bytes)
        }
        _ => {
            let (register, immediate) = split_operands(operands)?;
            let register = register_number(register)?;
            let immediate = word_value(immediate, labels, resolve)?;
            let mut bytes = word(base | register as u16);
            bytes.extend(word(immediate));
            Ok(bytes)
        }
    }
}

struct Operand {
    field: u8,
    extensions: Vec<u8>,
}

fn parse_operand(
    text: &str,
    labels: &HashMap<String, u32>,
    resolve: bool,
) -> Result<Operand, Diagnostic> {
    let text = text.trim();
    if let Ok(register) = register_number(text) {
        return Ok(Operand {
            field: register,
            extensions: Vec::new(),
        });
    }
    if let Some(register) = text
        .strip_prefix('*')
        .and_then(|value| value.strip_suffix('+'))
    {
        return Ok(Operand {
            field: 0x30 | register_number(register)?,
            extensions: Vec::new(),
        });
    }
    if let Some(register) = text.strip_prefix('*') {
        return Ok(Operand {
            field: 0x10 | register_number(register)?,
            extensions: Vec::new(),
        });
    }
    let symbolic = text.strip_prefix('@').ok_or_else(|| {
        Diagnostic::new(format!(
            "invalid TMS9900 operand `{text}`; expected R0..R15, *Rn, *Rn+, or @address[(Rn)]"
        ))
    })?;
    let (address, register) = if let Some((address, index)) = symbolic.rsplit_once('(') {
        let index = index
            .strip_suffix(')')
            .ok_or_else(|| Diagnostic::new(format!("invalid TMS9900 indexed operand `{text}`")))?;
        (address.trim(), register_number(index.trim())?)
    } else {
        (symbolic.trim(), 0)
    };
    if address.is_empty() {
        return Err(Diagnostic::new(format!(
            "invalid TMS9900 symbolic operand `{text}`"
        )));
    }
    let address = word_value(address, labels, resolve)?;
    Ok(Operand {
        field: 0x20 | register,
        extensions: word(address),
    })
}

fn split_instruction(text: &str) -> Result<(&str, &str), Diagnostic> {
    let text = text.trim();
    let mnemonic_end = text.find(char::is_whitespace).unwrap_or(text.len());
    let mnemonic = &text[..mnemonic_end];
    if mnemonic.is_empty() {
        return Err(Diagnostic::new("empty TMS9900 instruction"));
    }
    Ok((mnemonic, text[mnemonic_end..].trim()))
}

fn required_operand<'a>(operands: &'a str, mnemonic: &str) -> Result<&'a str, Diagnostic> {
    if operands.is_empty() {
        Err(Diagnostic::new(format!(
            "TMS9900 {mnemonic} requires an operand"
        )))
    } else if operands.contains(',') {
        Err(Diagnostic::new(format!(
            "TMS9900 {mnemonic} has too many operands"
        )))
    } else {
        Ok(operands)
    }
}

fn split_operands(text: &str) -> Result<(&str, &str), Diagnostic> {
    text.split_once(',')
        .map(|(left, right)| (left.trim(), right.trim()))
        .filter(|(left, right)| !left.is_empty() && !right.is_empty() && !right.contains(','))
        .ok_or_else(|| Diagnostic::new(format!("invalid TMS9900 operand list `{text}`")))
}

fn register_number(text: &str) -> Result<u8, Diagnostic> {
    text.trim()
        .strip_prefix(['r', 'R'])
        .and_then(|number| number.parse::<u8>().ok())
        .filter(|register| *register < 16)
        .ok_or_else(|| Diagnostic::new(format!("invalid TMS9900 register `{text}`")))
}

fn word_value(text: &str, labels: &HashMap<String, u32>, resolve: bool) -> Result<u16, Diagnostic> {
    let value = value(text, labels, resolve)?;
    u16::try_from(value).map_err(|_| {
        Diagnostic::new(format!(
            "TMS9900 value `{text}` is outside the 16-bit address/value range"
        ))
    })
}

fn signed_value(
    text: &str,
    labels: &HashMap<String, u32>,
    resolve: bool,
) -> Result<i64, Diagnostic> {
    if !resolve {
        return Ok(0);
    }
    if let Ok(value) = text.trim().parse::<i64>() {
        return Ok(value);
    }
    Ok(value(text, labels, true)? as i64)
}

fn value(text: &str, labels: &HashMap<String, u32>, resolve: bool) -> Result<u32, Diagnostic> {
    if !resolve {
        return Ok(0);
    }
    let text = text.trim();
    if let Some(hex) = text.strip_prefix('>') {
        return u32::from_str_radix(hex, 16)
            .map_err(|_| Diagnostic::new(format!("invalid TMS9900 value `{text}`")));
    }
    if let Some(hex) = text.strip_prefix("0x") {
        return u32::from_str_radix(hex, 16)
            .map_err(|_| Diagnostic::new(format!("invalid TMS9900 value `{text}`")));
    }
    if let Some(hex) = text.strip_suffix(['h', 'H']) {
        return u32::from_str_radix(hex, 16)
            .map_err(|_| Diagnostic::new(format!("invalid TMS9900 value `{text}`")));
    }
    if let Ok(value) = text.parse::<u32>() {
        return Ok(value);
    }
    labels
        .iter()
        .find_map(|(name, value)| name.eq_ignore_ascii_case(text).then_some(*value))
        .ok_or_else(|| Diagnostic::new(format!("unknown TMS9900 symbol `{text}`")))
}

fn word(value: u16) -> Vec<u8> {
    value.to_be_bytes().to_vec()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bytes(value: u16) -> Vec<u8> {
        value.to_be_bytes().to_vec()
    }

    fn general_addresses() -> [(&'static str, u8, Vec<u8>); 5] {
        [
            ("r3", 0x03, vec![]),
            ("*r3", 0x13, vec![]),
            ("*r3+", 0x33, vec![]),
            ("@>1234", 0x20, vec![0x12, 0x34]),
            ("@>1234(r3)", 0x23, vec![0x12, 0x34]),
        ]
    }

    #[test]
    fn encodes_every_general_address_form_in_every_family() {
        let labels = HashMap::new();
        let general_addresses = general_addresses();

        for (mnemonic, base) in [
            ("szc", 0x4000),
            ("szcb", 0x5000),
            ("s", 0x6000),
            ("sb", 0x7000),
            ("c", 0x8000),
            ("cb", 0x9000),
            ("a", 0xa000),
            ("ab", 0xb000),
            ("mov", 0xc000),
            ("movb", 0xd000),
            ("soc", 0xe000),
            ("socb", 0xf000),
        ] {
            for (source, source_field, source_extension) in &general_addresses {
                for (destination, destination_field, destination_extension) in &general_addresses {
                    let text = format!("{mnemonic} {source}, {destination}");
                    let mut expected =
                        bytes(base | ((*source_field as u16) << 6) | *destination_field as u16);
                    expected.extend(source_extension);
                    expected.extend(destination_extension);
                    assert_eq!(
                        encode_instruction(&text, &labels, 0).unwrap(),
                        expected,
                        "{text}"
                    );
                    assert_eq!(instruction_len(&text).unwrap(), expected.len(), "{text}");
                }
            }
        }

        for (mnemonic, base) in [
            ("blwp", 0x0400),
            ("b", 0x0440),
            ("x", 0x0480),
            ("clr", 0x04c0),
            ("neg", 0x0500),
            ("inv", 0x0540),
            ("inc", 0x0580),
            ("inct", 0x05c0),
            ("dec", 0x0600),
            ("dect", 0x0640),
            ("bl", 0x0680),
            ("swpb", 0x06c0),
            ("seto", 0x0700),
            ("abs", 0x0740),
        ] {
            for (operand, field, extension) in &general_addresses {
                let text = format!("{mnemonic} {operand}");
                let mut expected = bytes(base | *field as u16);
                expected.extend(extension);
                assert_eq!(
                    encode_instruction(&text, &labels, 0).unwrap(),
                    expected,
                    "{text}"
                );
            }
        }

        for (mnemonic, base) in [("ldcr", 0x3000), ("stcr", 0x3400)] {
            for (operand, field, extension) in &general_addresses {
                let text = format!("{mnemonic} {operand}, 7");
                let mut expected = bytes(base | (7 << 6) | *field as u16);
                expected.extend(extension);
                assert_eq!(
                    encode_instruction(&text, &labels, 0).unwrap(),
                    expected,
                    "{text}"
                );
            }
        }

        for (mnemonic, base) in [("mpy", 0x3800), ("div", 0x3c00)] {
            for (operand, field, extension) in &general_addresses {
                let text = format!("{mnemonic} {operand}, r4");
                let mut expected = bytes(base | ((*field as u16) << 6) | 4);
                expected.extend(extension);
                assert_eq!(
                    encode_instruction(&text, &labels, 0).unwrap(),
                    expected,
                    "{text}"
                );
            }
        }

        for (operand, field, extension) in &general_addresses {
            let text = format!("xop {operand}, 5");
            let mut expected = bytes(0x2c00 | ((*field as u16) << 6) | 5);
            expected.extend(extension);
            assert_eq!(
                encode_instruction(&text, &labels, 0).unwrap(),
                expected,
                "{text}"
            );
        }
    }

    #[test]
    fn encodes_every_non_general_address_opcode_family() {
        let labels = HashMap::new();
        for (text, expected) in [
            ("li r1, >1234", vec![0x02, 0x01, 0x12, 0x34]),
            ("ai r1, >1234", vec![0x02, 0x21, 0x12, 0x34]),
            ("andi r1, >1234", vec![0x02, 0x41, 0x12, 0x34]),
            ("ori r1, >1234", vec![0x02, 0x61, 0x12, 0x34]),
            ("ci r1, >1234", vec![0x02, 0x81, 0x12, 0x34]),
            ("stwp r1", vec![0x02, 0xa1]),
            ("stst r1", vec![0x02, 0xc1]),
            ("lwpi >1234", vec![0x02, 0xe0, 0x12, 0x34]),
            ("limi 15", vec![0x03, 0x00, 0x00, 0x0f]),
            ("idle", vec![0x03, 0x40]),
            ("rset", vec![0x03, 0x60]),
            ("rtwp", vec![0x03, 0x80]),
            ("ckon", vec![0x03, 0xa0]),
            ("ckof", vec![0x03, 0xc0]),
            ("lrex", vec![0x03, 0xe0]),
            ("sra 4, r6", vec![0x08, 0x46]),
            ("srl 4, r6", vec![0x09, 0x46]),
            ("sla 4, r6", vec![0x0a, 0x46]),
            ("src 4, r6", vec![0x0b, 0x46]),
            ("nop", vec![0x10, 0x00]),
        ] {
            assert_eq!(
                encode_instruction(text, &labels, 0).unwrap(),
                expected,
                "{text}"
            );
        }
    }

    #[test]
    fn encodes_jumps_cru_ranges_and_case_insensitive_labels() {
        let labels = HashMap::from([("Loop".to_owned(), 0x1000), ("VALUE".to_owned(), 0x1234)]);
        for (mnemonic, base) in [
            ("jmp", 0x1000),
            ("jlt", 0x1100),
            ("jle", 0x1200),
            ("jeq", 0x1300),
            ("jhe", 0x1400),
            ("jgt", 0x1500),
            ("jne", 0x1600),
            ("jnc", 0x1700),
            ("joc", 0x1800),
            ("jno", 0x1900),
            ("jl", 0x1a00),
            ("jh", 0x1b00),
            ("jop", 0x1c00),
        ] {
            let text = format!("{mnemonic} loop");
            assert_eq!(
                encode_instruction(&text, &labels, 0x1004).unwrap(),
                bytes(base | 0xfd),
                "{text}"
            );
        }
        for (mnemonic, base) in [("sbo", 0x1d00), ("sbz", 0x1e00), ("tb", 0x1f00)] {
            for (offset, field) in [(-128, 0x80), (-1, 0xff), (0, 0), (127, 0x7f)] {
                let text = format!("{mnemonic} {offset}");
                assert_eq!(
                    encode_instruction(&text, &labels, 0).unwrap(),
                    bytes(base | field),
                    "{text}"
                );
            }
        }
        assert_eq!(
            encode_instruction("mov @value, @VALUE(r3)", &labels, 0).unwrap(),
            [0xc8, 0x23, 0x12, 0x34, 0x12, 0x34]
        );
        assert_eq!(
            encode_instruction("ldcr r1, 16", &labels, 0).unwrap(),
            [0x30, 0x01]
        );
        assert_eq!(
            encode_instruction("stcr r1, 0", &labels, 0).unwrap(),
            [0x34, 0x01]
        );
        assert_eq!(
            encode_instruction("li r1, FFH", &labels, 0).unwrap(),
            [0x02, 0x01, 0x00, 0xff]
        );
    }

    #[test]
    fn rejects_out_of_range_and_invalid_forms() {
        let labels = HashMap::new();
        for text in [
            "limi 16",
            "xop r1, 16",
            "sra 16, r1",
            "ldcr r1, 17",
            "stcr r1, 17",
            "sbo -129",
            "sbz 128",
            "tb >ff",
            "jmp 1",
            "jmp >200",
            "jmp >0ff",
            "jmp >10102",
            "mov r1",
            "mov r1, r2, r3",
            "blwp",
            "idle r1",
            "li r16, 1",
            "mpy r1, *r2",
            "@>1234",
            "mov @, r1",
            "mov @>1234(r1, r2), r3",
            "unknown r1",
            "nop r1",
        ] {
            assert!(encode_instruction(text, &labels, 0).is_err(), "{text}");
        }
        for text in [
            "mov r1",
            "mov r1, r2, r3",
            "blwp",
            "idle r1",
            "li r16, 1",
            "mpy r1, *r2",
            "@>1234",
            "mov @, r1",
            "mov @>1234(r1, r2), r3",
            "unknown r1",
            "nop r1",
        ] {
            assert!(instruction_len(text).is_err(), "{text}");
        }
    }
}
