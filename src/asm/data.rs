use crate::compat::prelude::*;

#[allow(dead_code)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ByteLiteralStyle {
    HexPrefix,
    HexSuffix,
}

/// Formats byte records for assemblers whose data directives accept comma-separated bytes.
pub fn byte_data_lines(
    directive: &str,
    bytes: &[u8],
    style: ByteLiteralStyle,
    bytes_per_line: usize,
) -> Vec<String> {
    let bytes_per_line = bytes_per_line.max(1);
    bytes
        .chunks(bytes_per_line)
        .map(|chunk| {
            let values = chunk
                .iter()
                .map(|byte| match style {
                    ByteLiteralStyle::HexPrefix => format!("0x{byte:02X}"),
                    ByteLiteralStyle::HexSuffix => format!("{byte:02X}h"),
                })
                .collect::<Vec<_>>()
                .join(", ");
            format!("    {directive} {values}")
        })
        .collect()
}

/// Escapes UTF-8 text for the common double-quoted assembly string syntax.
pub fn escaped_data_string(value: &str) -> String {
    let mut output = String::from("\"");
    for character in value.chars() {
        match character {
            '\\' => output.push_str("\\\\"),
            '\"' => output.push_str("\\\""),
            '\n' => output.push_str("\\n"),
            '\r' => output.push_str("\\r"),
            '\t' => output.push_str("\\t"),
            '\0' => output.push_str("\\0"),
            character if character.is_control() => {
                for byte in character.to_string().bytes() {
                    output.push_str(&format!("\\x{byte:02X}"));
                }
            }
            character => output.push(character),
        }
    }
    output.push('\"');
    output
}

/// Formats a text-data record and its explicit byte terminator.
pub fn terminated_text_data_line(directive: &str, value: &str, terminator: &str) -> String {
    format!(
        "    {directive} {}, {terminator}",
        escaped_data_string(value)
    )
}

#[cfg(test)]
mod tests;
