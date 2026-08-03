use std::path::Path;

use crate::parser::parse_program;

use super::*;

#[test]
fn validates_inline_asm_classes_after_alias_resolution() {
    let valid = parse_program(
        Path::new("valid.ezra"),
        "alias Word = u16 fn main() { asm(in value: Word as reg16) { \"nop\" } }",
    )
    .unwrap();
    validate_program(&valid, CpuFamily::I8086).unwrap();

    let invalid = parse_program(
            Path::new("invalid.ezra"),
            "struct Pair { value: u8 } alias PairAlias = Pair fn main() { asm(in value: PairAlias as reg8) { \"nop\" } }",
        )
        .unwrap();
    let error = validate_program(&invalid, CpuFamily::I8086).unwrap_err();
    assert!(
        error
            .message
            .contains("incompatible with type `Named(\"Pair\")`")
    );
}
