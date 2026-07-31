use crate::ast::{BinaryOp, Expr, Type};

use super::*;

fn value(name: &str, width: BitWidth) -> BitOperand {
    BitOperand::new(Expr::Ident(name.to_owned()), BitType::unsigned(width))
}

fn binary(left: Expr, op: BinaryOp, right: Expr) -> Expr {
    Expr::Binary {
        left: Box::new(left),
        op,
        right: Box::new(right),
    }
}

fn int(value: i64) -> Expr {
    Expr::Int(value)
}

fn test_operand(expr: &Expr) -> Option<BitOperand> {
    let Expr::Ident(name) = expr else {
        return None;
    };
    let ty = match name.as_str() {
        "byte" => BitType::unsigned(BitWidth::W8),
        "signed_byte" => BitType::signed(BitWidth::W8),
        "word" => BitType::unsigned(BitWidth::W16),
        "wide" => BitType::unsigned(BitWidth::W24),
        "long" => BitType::unsigned(BitWidth::W32),
        "device" => {
            return Some(BitOperand::volatile(
                expr.clone(),
                BitType::unsigned(BitWidth::W16),
            ));
        }
        _ => return None,
    };
    Some(BitOperand::new(expr.clone(), ty))
}

fn test_constant(expr: &Expr) -> Option<i64> {
    match expr {
        Expr::Int(value) | Expr::TypedInt(value, _) => Some(*value),
        _ => None,
    }
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
fn recognizes_bit_updates_and_mask_tests_with_polarity() {
    let byte = Expr::Ident("byte".to_owned());
    let set = recognize_expr(
        &binary(byte.clone(), BinaryOp::BitOr, int(0x20)),
        test_operand,
        test_constant,
    )
    .unwrap();
    assert!(matches!(set.operation, BitOp::SetBit { bit: 5, .. }));
    assert!(!set.inverted);

    let clear = recognize_expr(
        &binary(byte.clone(), BinaryOp::BitAnd, int(0xDF)),
        test_operand,
        test_constant,
    )
    .unwrap();
    assert!(matches!(clear.operation, BitOp::ClearBit { bit: 5, .. }));
    let toggle = recognize_expr(
        &binary(byte, BinaryOp::BitXor, int(0x20)),
        test_operand,
        test_constant,
    )
    .unwrap();
    assert!(matches!(toggle.operation, BitOp::ToggleBit { bit: 5, .. }));

    let masked = binary(
        Expr::Ident("word".to_owned()),
        BinaryOp::BitAnd,
        int(0x0300),
    );
    let any = recognize_expr(
        &binary(masked.clone(), BinaryOp::Ne, int(0)),
        test_operand,
        test_constant,
    )
    .unwrap();
    assert!(matches!(
        any.operation,
        BitOp::MaskTestAny { mask: 0x0300, .. }
    ));
    assert!(!any.inverted);
    let all = recognize_expr(
        &binary(masked, BinaryOp::Ne, int(0x0300)),
        test_operand,
        test_constant,
    )
    .unwrap();
    assert!(matches!(
        all.operation,
        BitOp::MaskTestAll { mask: 0x0300, .. }
    ));
    assert!(all.inverted);

    let volatile_test = recognize_expr(
        &binary(
            binary(
                Expr::Ident("device".to_owned()),
                BinaryOp::BitAnd,
                int(0x20),
            ),
            BinaryOp::Eq,
            int(0),
        ),
        test_operand,
        test_constant,
    )
    .unwrap();
    assert!(matches!(
        volatile_test.operation,
        BitOp::BitTest { bit: 5, .. }
    ));
    assert!(volatile_test.inverted);
    assert!(volatile_test.operation.is_volatile());
}

#[test]
fn recognizes_extract_rotate_swap_and_cast_patterns() {
    let extract_expr = binary(
        binary(Expr::Ident("wide".to_owned()), BinaryOp::Shr, int(8)),
        BinaryOp::BitAnd,
        int(0xFF),
    );
    let extracted = recognize_expr(&extract_expr, test_operand, test_constant).unwrap();
    assert!(matches!(
        extracted.operation,
        BitOp::Extract {
            lsb: 8,
            width: BitWidth::W8,
            ..
        }
    ));

    let rotate_expr = binary(
        binary(Expr::Ident("long".to_owned()), BinaryOp::Shl, int(8)),
        BinaryOp::BitOr,
        binary(Expr::Ident("long".to_owned()), BinaryOp::Shr, int(24)),
    );
    let rotated = recognize_expr(&rotate_expr, test_operand, test_constant).unwrap();
    assert!(matches!(
        rotated.operation,
        BitOp::RotateLeft { amount: 8, .. }
    ));

    let swap_expr = binary(
        binary(Expr::Ident("word".to_owned()), BinaryOp::Shl, int(8)),
        BinaryOp::BitOr,
        binary(Expr::Ident("word".to_owned()), BinaryOp::Shr, int(8)),
    );
    let swapped = recognize_expr(&swap_expr, test_operand, test_constant).unwrap();
    assert!(matches!(swapped.operation, BitOp::ByteSwap { .. }));

    let sign_cast = Expr::Cast {
        ty: Type::Named("i24".to_owned()),
        expr: Box::new(Expr::Ident("signed_byte".to_owned())),
    };
    assert!(matches!(
        recognize_expr(&sign_cast, test_operand, test_constant)
            .unwrap()
            .operation,
        BitOp::SignExtend { .. }
    ));
    let zero_cast = Expr::Cast {
        ty: Type::Named("u24".to_owned()),
        expr: Box::new(Expr::Ident("byte".to_owned())),
    };
    assert!(matches!(
        recognize_expr(&zero_cast, test_operand, test_constant)
            .unwrap()
            .operation,
        BitOp::ZeroExtend { .. }
    ));
    let truncate_cast = Expr::Cast {
        ty: Type::Named("i8".to_owned()),
        expr: Box::new(Expr::Ident("word".to_owned())),
    };
    assert!(matches!(
        recognize_expr(&truncate_cast, test_operand, test_constant)
            .unwrap()
            .operation,
        BitOp::Truncate {
            to: BitType {
                width: BitWidth::W8,
                signedness: BitSignedness::Signed
            },
            ..
        }
    ));
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
