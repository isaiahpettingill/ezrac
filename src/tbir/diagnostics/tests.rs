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

#[test]
fn validates_catalog_scalar_and_paired_calls() {
    let program = parse_program(
        Path::new("intrinsics.ezra"),
        r#"
            global bytes: [u8; 4] = [1, 2, 3, 4]
            fn scalar(value: u8) -> bool {
                return ezra.bits.test(value, 7)
            }
            fn main() {
                let tested: bool = scalar(1u8)
                let quotient: u8, remainder: u8 = ezra.int.divmod(7u8, 3u8)
                let sum: u8, carry: bool = ezra.int.add_carry(0xFFu8, 1u8, false)
                let difference: u8, borrow: bool = ezra.int.sub_borrow(0u8, 1u8, false)
                let low: u8, high: u8 = ezra.int.full_mul(3u8, 4u8)
                let found: ptr<u8>, present: bool = ezra.mem.find_byte(&bytes[0], 4u24, 2u8)
                ezra.mem.poke8(&bytes[0], 0u8)
            }
        "#,
    )
    .unwrap();

    validate_multi_value_returns(&program).unwrap();
}

#[test]
fn rejects_invalid_catalog_calls() {
    for (source, expected) in [
        (
            "fn main() { ezra.bits.set(1u8) }",
            "expects 2 arguments, got 1",
        ),
        (
            "fn main() { let value: u8 = ezra.bits.set(true, 0u8) }",
            "argument 1 must be u8, u16, or u24",
        ),
        (
            "fn bad(index: u8) -> u8 { return ezra.bits.set(1u8, index) } fn main() {}",
            "must be a compile-time constant bit index",
        ),
        (
            "fn main() { let value: u8 = ezra.bits.set(1u8, 8) }",
            "bit index must be within the input width",
        ),
        (
            "fn main() { let value: u8 = ezra.bits.test(1u8, 0) }",
            "type mismatch in function `main` for let binding",
        ),
        (
            "fn main() { let value: u8 = ezra.int.divmod(7u8, 3u8) }",
            "two-result intrinsic",
        ),
        (
            "fn main() { let value: u8 = ezra.mem.poke8(cast<ptr<u8>>(0u24), 1u8) }",
            "zero-result intrinsic",
        ),
        (
            "fn main() { let first: u16, second: u16 = ezra.int.divmod(7u8, 3u8) }",
            "type mismatch in two-result binding",
        ),
    ] {
        let program = parse_program(Path::new("invalid_intrinsic.ezra"), source).unwrap();
        let error = validate_multi_value_returns(&program).unwrap_err();
        assert!(error.message.contains(expected), "{error}");
    }
}
