use crate::ast::{BinaryOp, Expr, Type};

use super::*;

fn ty(name: &str) -> Type {
    Type::Named(name.to_owned())
}

fn interval(min: i64, max: i64) -> Interval {
    Interval::new(min, max).unwrap()
}

fn binary(left: Expr, op: BinaryOp, right: Expr) -> Expr {
    Expr::Binary {
        left: Box::new(left),
        op,
        right: Box::new(right),
    }
}

#[test]
fn reports_unsigned_and_signed_type_ranges() {
    let u8_facts = ValueFacts::for_type(&ty("u8")).unwrap();
    assert_eq!(u8_facts.unsigned_range, Some(interval(0, 255)));
    assert_eq!(u8_facts.signed_range, Some(interval(-128, 127)));

    let i8_facts = ValueFacts::for_type(&ty("i8")).unwrap();
    assert_eq!(i8_facts.unsigned_range, Some(interval(0, 255)));
    assert_eq!(i8_facts.signed_range, Some(interval(-128, 127)));

    let u24_facts = ValueFacts::for_type(&ty("u24")).unwrap();
    assert_eq!(u24_facts.unsigned_range, Some(interval(0, 0xFF_FFFF)));
    assert_eq!(
        u24_facts.signed_range,
        Some(interval(-0x80_0000, 0x7F_FFFF))
    );
}

#[test]
fn wraps_unsigned_and_signed_constants_at_the_declared_width() {
    let unsigned = analyze_expr(
        &binary(
            Expr::TypedInt(255, ty("u8")),
            BinaryOp::Add,
            Expr::TypedInt(1, ty("u8")),
        ),
        &ty("u8"),
    );
    assert_eq!(unsigned.unsigned_range, Some(interval(0, 0)));
    assert_eq!(unsigned.signed_range, Some(interval(0, 0)));

    let signed = analyze_expr(
        &binary(
            Expr::TypedInt(-128, ty("i8")),
            BinaryOp::Add,
            Expr::TypedInt(-1, ty("i8")),
        ),
        &ty("i8"),
    );
    assert_eq!(signed.unsigned_range, Some(interval(127, 127)));
    assert_eq!(signed.signed_range, Some(interval(127, 127)));
}

#[test]
fn propagates_alignment_through_shift_and_low_bit_masks() {
    let value = Expr::Ident("value".to_owned());
    let shifted = analyze_expr(
        &binary(value.clone(), BinaryOp::Shl, Expr::Int(2)),
        &ty("u16"),
    );
    assert_eq!(shifted.alignment.modulus, 4);

    let aligned = analyze_expr(
        &binary(value, BinaryOp::BitAnd, Expr::TypedInt(0xFFFC, ty("u16"))),
        &ty("u16"),
    );
    assert_eq!(aligned.alignment.modulus, 4);
    let proof = aligned.mask_proof(3).unwrap();
    assert_eq!(proof.low_bits, Some(2));
    assert!(proof.proves_zero);
}

#[test]
fn proves_mask_shape_and_identity_without_overclaiming() {
    let unknown = ValueFacts::for_type(&ty("u8")).unwrap();
    let all_bits = unknown.mask_proof(0xFF).unwrap();
    assert_eq!(all_bits.low_bits, Some(8));
    assert!(all_bits.proves_identity);

    let one_bit = unknown.mask_proof(0x08).unwrap();
    assert_eq!(one_bit.single_bit, Some(3));
    assert!(!one_bit.proves_zero);

    let aligned = ValueFacts::from_known_bits(&ty("u8"), 0x03, 0).unwrap();
    assert!(aligned.mask_proof(0x03).unwrap().proves_zero);
}

#[test]
fn classifies_safe_maybe_overshift_and_definite_overshift() {
    let value = ValueFacts::for_type(&ty("u8")).unwrap();
    let safe_count = ValueFacts::from_unsigned_range(&ty("u8"), interval(0, 7)).unwrap();
    let safe = shift_proof(&value, &safe_count);
    assert!(safe.definitely_in_range);
    assert!(!safe.may_overshift);

    let variable_count = ValueFacts::for_type(&ty("u8")).unwrap();
    let maybe = shift_proof(&value, &variable_count);
    assert!(!maybe.definitely_in_range);
    assert!(maybe.may_overshift);
    assert!(!maybe.definitely_overshift);

    let overshift_count = ValueFacts::from_unsigned_range(&ty("u8"), interval(8, 8)).unwrap();
    let overshift = shift_proof(&value, &overshift_count);
    assert!(overshift.definitely_overshift);
    assert!(overshift.is_definitely_invalid());

    let negative_count = ValueFacts::from_signed_range(&ty("i8"), interval(-1, -1)).unwrap();
    assert!(shift_proof(&value, &negative_count).may_be_negative);
}

#[test]
fn keeps_alignment_conservative_when_shift_count_may_overshift() {
    let facts = analyze_expr(
        &binary(
            Expr::Ident("value".to_owned()),
            BinaryOp::Shl,
            Expr::Ident("count".to_owned()),
        ),
        &ty("u8"),
    );
    assert_eq!(facts.alignment.modulus, 1);
    assert_eq!(facts.known_zero, 0);
}

#[test]
fn casts_preserve_known_sign_extension() {
    let facts = analyze_expr(
        &Expr::Cast {
            ty: ty("u16"),
            expr: Box::new(Expr::TypedInt(-1, ty("i8"))),
        },
        &ty("u16"),
    );
    assert_eq!(facts.unsigned_range, Some(interval(0xFFFF, 0xFFFF)));
    assert_eq!(facts.known_one, 0xFFFF);
}

#[test]
fn carries_effects_without_using_them_as_range_facts() {
    let call_and_zero = binary(
        Expr::Call {
            path: vec!["read".to_owned()],
            args: Vec::new(),
        },
        BinaryOp::BitAnd,
        Expr::TypedInt(0, ty("u8")),
    );
    let facts = analyze_expr(&call_and_zero, &ty("u8"));
    assert!(facts.is_known_zero());
    assert!(facts.effects.may_call);
    assert!(!facts.effects.is_pure());

    let port = analyze_expr(&Expr::In("STATUS".to_owned()), &ty("u8"));
    assert!(port.effects.may_port_io);
    assert!(port.unsigned_range.is_some());
}

#[test]
fn unsupported_types_are_unknown_and_non_diagnostic() {
    let facts = analyze_expr(&Expr::Ident("pointer".to_owned()), &ty("ptr"));
    assert_eq!(facts.bit_width, 0);
    assert!(facts.unsigned_range.is_none());
    assert!(facts.signed_range.is_none());
}
