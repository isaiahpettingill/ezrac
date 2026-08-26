use super::*;

fn named(name: &str) -> Type {
    Type::Named(name.to_owned())
}

fn ptr_u8() -> Type {
    Type::Ptr(alloc::boxed::Box::new(named("u8")))
}

fn arg(ty: &str) -> IntrinsicArgument {
    IntrinsicArgument::new(named(ty))
}

fn constant(ty: &str, value: i64) -> IntrinsicArgument {
    IntrinsicArgument::with_constant(named(ty), value)
}

#[test]
fn catalog_contains_canonical_names_and_compatibility_aliases() {
    assert_eq!(INTRINSIC_DESCRIPTORS.len(), 36);
    assert_eq!(
        canonical_name("bits.rotate_left"),
        Some("ezra.bits.rotate_left")
    );
    assert_eq!(
        canonical_name("ezra.mem.memcpy"),
        Some("ezra.mem.copy_nonoverlapping")
    );
    assert_eq!(canonical_name("mem.memset"), Some("ezra.mem.fill"));
    assert_eq!(canonical_name("ezra.mem.peek8"), Some("ezra.mem.peek8"));
    assert!(lookup("ezra.bits.not_an_intrinsic").is_none());
}

#[test]
fn bit_operations_preserve_width_and_use_exact_result_types() {
    let rotate = CATALOG
        .validate_types("ezra.bits.rotate_left", &[named("u24"), named("u8")])
        .unwrap();
    assert_eq!(rotate.result_types, vec![named("u24")]);

    let test = CATALOG
        .validate("bits.test", &[constant("u16", 15), constant("u8", 3)])
        .unwrap();
    assert_eq!(test.result_types, vec![named("bool")]);

    let insert = CATALOG
        .validate(
            "ezra.bits.insert",
            &[
                constant("u8", 0),
                constant("u8", 0),
                constant("u8", 4),
                constant("u8", 4),
            ],
        )
        .unwrap();
    assert_eq!(insert.result_types, vec![named("u8")]);

    let counts = CATALOG
        .infer_result_types("bits.leading_zeros", &[named("u24")])
        .unwrap();
    assert_eq!(counts, vec![named("u8")]);
}

#[test]
fn bit_index_and_range_checks_are_constant_aware() {
    let dynamic = CATALOG.validate_types("bits.set", &[named("u8"), named("u8")]);
    assert!(dynamic.is_ok());

    let missing = CATALOG.validate("bits.set", &[arg("u8"), arg("u8")]);
    assert!(matches!(
        missing,
        Err(IntrinsicError::ConstantRequired { argument: 1, .. })
    ));

    let out_of_bounds = CATALOG.validate("bits.set", &[constant("u8", 0), constant("u8", 8)]);
    assert!(matches!(
        out_of_bounds,
        Err(IntrinsicError::ConstantOutOfRange { argument: 1, .. })
    ));

    let zero_width = CATALOG.validate(
        "bits.extract",
        &[constant("u16", 0), constant("u8", 2), constant("u8", 0)],
    );
    assert!(matches!(
        zero_width,
        Err(IntrinsicError::ConstantOutOfRange { argument: 1, .. })
    ));

    let crossing_width = CATALOG.validate(
        "bits.extract",
        &[constant("u16", 0), constant("u8", 12), constant("u8", 5)],
    );
    assert!(crossing_width.is_err());
}

#[test]
fn signed_and_wrong_width_bit_values_are_rejected() {
    let signed = CATALOG.validate_types("bits.rotate_right", &[named("i16"), named("u8")]);
    assert!(matches!(signed, Err(IntrinsicError::ArgumentType { .. })));

    let byte_swap_u8 = CATALOG.validate_types("bits.byte_swap", &[named("u8")]);
    assert!(byte_swap_u8.is_err());

    let mismatched_insert = CATALOG.validate(
        "bits.insert",
        &[
            constant("u8", 0),
            constant("u16", 0),
            constant("u8", 0),
            constant("u8", 1),
        ],
    );
    assert!(matches!(
        mismatched_insert,
        Err(IntrinsicError::MismatchedArguments { .. })
    ));
}

#[test]
fn integer_intrinsics_model_zero_one_and_two_results() {
    let widening = CATALOG
        .infer_result_types("int.widening_mul", &[named("u16"), named("u8")])
        .unwrap();
    assert_eq!(widening, vec![named("u24")]);

    let signed_widening = CATALOG
        .infer_result_types("int.widening_mul", &[named("i16"), named("i16")])
        .unwrap();
    assert_eq!(signed_widening, vec![named("i32")]);

    let divmod = CATALOG
        .infer_result_types("int.divmod", &[named("u16"), named("u16")])
        .unwrap();
    assert_eq!(divmod, vec![named("u16"), named("u16")]);

    let carry = CATALOG
        .infer_result_types(
            "int.add_carry",
            &[named("u24"), named("u24"), named("bool")],
        )
        .unwrap();
    assert_eq!(carry, vec![named("u24"), named("bool")]);

    let full = CATALOG
        .infer_result_types("int.full_mul", &[named("i8"), named("i8")])
        .unwrap();
    assert_eq!(full, vec![named("i8"), named("i8")]);

    let store = CATALOG
        .infer_result_types("mem.store_be24", &[ptr_u8(), named("u24")])
        .unwrap();
    assert!(store.is_empty());
}

#[test]
fn integer_intrinsics_require_exact_operand_types() {
    let mixed_signedness =
        CATALOG.infer_result_types("int.widening_mul", &[named("u8"), named("i8")]);
    assert!(matches!(
        mixed_signedness,
        Err(IntrinsicError::UnsupportedOperandWidths { .. })
    ));

    let mixed_width = CATALOG.infer_result_types("int.divmod", &[named("u8"), named("u16")]);
    assert!(matches!(
        mixed_width,
        Err(IntrinsicError::MismatchedArguments { .. })
    ));

    let bad_carry =
        CATALOG.infer_result_types("int.add_carry", &[named("u8"), named("u8"), named("u8")]);
    assert!(matches!(
        bad_carry,
        Err(IntrinsicError::ArgumentType { .. })
    ));
}

#[test]
fn memory_intrinsics_have_aliases_effects_and_overlap_rules() {
    let memcpy = lookup("mem.memcpy").unwrap();
    assert_eq!(memcpy.canonical_name, "ezra.mem.copy_nonoverlapping");
    assert_eq!(memcpy.effects.memory, MemoryEffect::ReadWrite);
    assert_eq!(memcpy.overlap, OverlapRule::MustNotOverlap);

    let move_descriptor = lookup("ezra.mem.move").unwrap();
    assert_eq!(move_descriptor.overlap, OverlapRule::MayOverlap);

    let memset = lookup("ezra.mem.memset").unwrap();
    assert_eq!(memset.canonical_name, "ezra.mem.fill");
    assert_eq!(memset.effects.memory, MemoryEffect::Write);
}

#[test]
fn memory_intrinsics_validate_pointers_lengths_and_results() {
    let find = CATALOG
        .infer_result_types("ezra.mem.find_byte", &[ptr_u8(), named("u24"), named("u8")])
        .unwrap();
    assert_eq!(find, vec![ptr_u8(), named("bool")]);

    let compare = CATALOG
        .infer_result_types("mem.compare", &[ptr_u8(), ptr_u8(), named("u24")])
        .unwrap();
    assert_eq!(compare, vec![named("i8")]);

    let load = CATALOG
        .infer_result_types("mem.load_le16", &[ptr_u8()])
        .unwrap();
    assert_eq!(load, vec![named("u16")]);

    let bad_pointer = CATALOG.infer_result_types("mem.load_be24", &[Type::Named("ptr".to_owned())]);
    assert!(matches!(
        bad_pointer,
        Err(IntrinsicError::ArgumentType { .. })
    ));

    let bad_length = CATALOG.infer_result_types(
        "mem.copy_nonoverlapping",
        &[ptr_u8(), ptr_u8(), named("i16")],
    );
    assert!(matches!(
        bad_length,
        Err(IntrinsicError::ArgumentType { .. })
    ));

    let peek = lookup("ezra.mem.peek8").unwrap();
    assert_eq!(peek.effects.volatile, VolatilePolicy::PreservesAccess);
    let poke = CATALOG
        .infer_result_types("mem.poke8", &[ptr_u8(), named("u8")])
        .unwrap();
    assert!(poke.is_empty());
}

#[test]
fn argument_count_errors_are_reported_before_type_validation() {
    let error = CATALOG.infer_result_types("int.divmod", &[named("u8")]);
    assert!(matches!(
        error,
        Err(IntrinsicError::WrongArgumentCount {
            expected: 2,
            actual: 1,
            ..
        })
    ));
}
