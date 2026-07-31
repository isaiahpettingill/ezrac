use crate::ast::Expr;

use super::*;

fn value(name: &str, width: BitWidth) -> BitOperand {
    BitOperand::new(Expr::Ident(name.to_owned()), BitType::unsigned(width))
}

#[test]
fn widths_are_typed_and_have_width_masks() {
    assert_eq!(BitWidth::W8.bits(), 8);
    assert_eq!(BitWidth::W16.bytes(), 2);
    assert_eq!(BitWidth::W24.mask(), 0x00FF_FFFF);
    assert_eq!(BitWidth::W32.mask(), u32::MAX);
    assert_eq!(BitWidth::new(3).unwrap().mask(), 0b111);
    assert_eq!(BitWidth::new(0), None);
    assert_eq!(BitWidth::new(33), None);
}

#[test]
fn canonicalizes_bit_and_mask_tests_to_the_operand_width() {
    let byte = value("byte", BitWidth::W8);

    let bit = bit_test(byte.clone(), 7).unwrap();
    assert_eq!(bit.kind(), BitOpKind::BitTest);
    assert_eq!(bit.result_type(), BitType::unsigned(BitWidth::W1));
    assert!(bit_test(byte.clone(), 8).is_none());

    let any = mask_test_any(byte.clone(), 0x01FF);
    let all = mask_test_all(byte, 0x01FF);
    assert!(matches!(any, BitOp::MaskTestAny { mask: 0xFF, .. }));
    assert!(matches!(all, BitOp::MaskTestAll { mask: 0xFF, .. }));
}

#[test]
fn validates_typed_extract_and_insert_ranges() {
    let wide = value("wide", BitWidth::W24);
    let field = extract(wide.clone(), 8, BitWidth::W8).unwrap();
    assert_eq!(field.result_type(), BitType::unsigned(BitWidth::W8));
    assert!(extract(wide.clone(), 16, BitWidth::W16).is_none());

    let byte = value("byte", BitWidth::W8);
    let inserted = insert(wide.clone(), byte, 8, BitWidth::W8).unwrap();
    assert_eq!(inserted.result_type(), BitType::unsigned(BitWidth::W24));
    assert!(insert(wide, value("small", BitWidth::W8), 20, BitWidth::W8).is_none());
}

#[test]
fn canonicalizes_rotates_extensions_truncation_and_byte_swap() {
    let byte = value("byte", BitWidth::W8);
    let word = value("word", BitWidth::W16);

    assert!(matches!(
        rotate_left(byte.clone(), 8),
        BitOp::RotateLeft { amount: 0, .. }
    ));
    assert!(matches!(
        rotate_right(value("wide", BitWidth::W24), 25),
        BitOp::RotateRight { amount: 1, .. }
    ));

    let zero = zero_extend(byte.clone(), BitWidth::W24).unwrap();
    assert_eq!(zero.result_type(), BitType::unsigned(BitWidth::W24));
    let sign = sign_extend(byte.clone(), BitWidth::W16).unwrap();
    assert_eq!(sign.result_type(), BitType::signed(BitWidth::W16));
    assert!(zero_extend(byte.clone(), BitWidth::W8).is_none());
    assert!(sign_extend(byte.clone(), BitWidth::W8).is_none());

    let truncation = truncate(word, BitWidth::W8).unwrap();
    assert_eq!(truncation.result_type(), BitType::unsigned(BitWidth::W8));
    assert_eq!(
        byte_swap(byte).result_type(),
        BitType::unsigned(BitWidth::W8)
    );
}

#[test]
fn volatile_operands_are_visible_and_sequence_order_is_stable() {
    let first = bit_test(value("first", BitWidth::W8), 0).unwrap();
    let volatile = byte_swap(BitOperand::volatile(
        Expr::Ident("device".to_owned()),
        BitType::unsigned(BitWidth::W16),
    ));
    let last = mask_test_all(value("last", BitWidth::W8), 0xFF);

    assert!(!first.is_volatile());
    assert!(volatile.is_volatile());
    assert!(!BitOpSequence::may_reorder(&first, &volatile));
    assert!(!BitOpSequence::may_reorder(&volatile, &last));
    assert!(BitOpSequence::may_reorder(&first, &last));

    let sequence = BitOpSequence::from_operations(vec![first, volatile, last]).canonicalize();
    let kinds: Vec<_> = sequence.iter().map(BitOp::kind).collect();
    assert_eq!(
        kinds,
        vec![
            BitOpKind::BitTest,
            BitOpKind::ByteSwap,
            BitOpKind::MaskTestAll
        ]
    );
}
