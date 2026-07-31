//! Conservative integer range and alignment facts for TBIR expressions.
//!
//! This module computes facts only. It does not rewrite expressions or emit diagnostics. A
//! caller must check [`ValueFacts::effects`] before using a fact for a transformation.

use crate::{
    ast::{AccessPath, AccessSegment, BinaryOp, Expr, Type, UnaryOp},
    compat::prelude::*,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Interval {
    pub min: i64,
    pub max: i64,
}

impl Interval {
    pub const fn new(min: i64, max: i64) -> Option<Self> {
        if min <= max {
            Some(Self { min, max })
        } else {
            None
        }
    }

    pub const fn singleton(value: i64) -> Self {
        Self {
            min: value,
            max: value,
        }
    }

    pub const fn contains(self, value: i64) -> bool {
        self.min <= value && value <= self.max
    }

    pub const fn is_singleton(self) -> bool {
        self.min == self.max
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Alignment {
    /// The value is known to be divisible by this power of two.
    pub modulus: u64,
}

impl Alignment {
    pub const UNKNOWN: Self = Self { modulus: 1 };

    pub fn new(modulus: u64) -> Option<Self> {
        if modulus != 0 && modulus.is_power_of_two() {
            Some(Self { modulus })
        } else {
            None
        }
    }

    pub const fn known_zero_bits(self) -> u8 {
        self.modulus.trailing_zeros() as u8
    }

    pub fn guarantees(self, modulus: u64) -> bool {
        modulus != 0 && modulus.is_power_of_two() && self.modulus >= modulus
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Effects {
    pub may_call: bool,
    pub may_read_memory: bool,
    pub may_write_memory: bool,
    pub may_port_io: bool,
    pub may_inline_asm: bool,
}

impl Effects {
    pub const fn pure() -> Self {
        Self {
            may_call: false,
            may_read_memory: false,
            may_write_memory: false,
            may_port_io: false,
            may_inline_asm: false,
        }
    }

    pub const fn is_pure(self) -> bool {
        !self.may_call
            && !self.may_read_memory
            && !self.may_write_memory
            && !self.may_port_io
            && !self.may_inline_asm
    }

    fn join(self, other: Self) -> Self {
        Self {
            may_call: self.may_call || other.may_call,
            may_read_memory: self.may_read_memory || other.may_read_memory,
            may_write_memory: self.may_write_memory || other.may_write_memory,
            may_port_io: self.may_port_io || other.may_port_io,
            may_inline_asm: self.may_inline_asm || other.may_inline_asm,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ValueFacts {
    /// Zero means that the expression is not a supported scalar integer.
    pub bit_width: u8,
    pub is_signed: bool,
    /// Range of the bit pattern interpreted as unsigned.
    pub unsigned_range: Option<Interval>,
    /// Range of the same bit patterns interpreted as two's-complement signed.
    pub signed_range: Option<Interval>,
    pub known_zero: u64,
    pub known_one: u64,
    pub alignment: Alignment,
    pub effects: Effects,
}

impl ValueFacts {
    pub const fn unknown() -> Self {
        Self {
            bit_width: 0,
            is_signed: false,
            unsigned_range: None,
            signed_range: None,
            known_zero: 0,
            known_one: 0,
            alignment: Alignment::UNKNOWN,
            effects: Effects::pure(),
        }
    }

    pub fn unknown_for_type(ty: &Type) -> Self {
        scalar_type(ty)
            .map(full_facts)
            .unwrap_or_else(Self::unknown)
    }

    pub fn for_type(ty: &Type) -> Option<Self> {
        scalar_type(ty).map(full_facts)
    }

    pub fn from_unsigned_range(ty: &Type, range: Interval) -> Option<Self> {
        let scalar = scalar_type(ty)?;
        let mask = mask_for_width(scalar.width);
        if range.min < 0 || range.max > i64::try_from(mask).ok()? {
            return None;
        }
        Some(facts_from_raw_range(scalar, range, Effects::pure()))
    }

    pub fn from_signed_range(ty: &Type, range: Interval) -> Option<Self> {
        let scalar = scalar_type(ty)?;
        let bounds = signed_bounds(scalar);
        if range.min < bounds.min || range.max > bounds.max {
            return None;
        }
        Some(facts_from_signed_range(scalar, range, Effects::pure()))
    }

    pub fn from_known_bits(ty: &Type, known_zero: u64, known_one: u64) -> Option<Self> {
        let scalar = scalar_type(ty)?;
        Some(facts_from_known_bits(
            scalar,
            known_zero,
            known_one,
            Effects::pure(),
        ))
    }

    pub fn is_known_zero(self) -> bool {
        self.unsigned_range
            .is_some_and(|range| range.min == 0 && range.max == 0)
    }

    pub fn is_known_nonzero(self) -> bool {
        self.unsigned_range.is_some_and(|range| range.min > 0)
    }

    pub fn is_exact(self) -> bool {
        self.unsigned_range
            .is_some_and(|range| range.is_singleton())
    }

    pub fn exact_unsigned(self) -> Option<u64> {
        match self.unsigned_range {
            Some(range) if range.is_singleton() && range.min >= 0 => Some(range.min as u64),
            _ => None,
        }
    }

    /// Prove properties of `self & mask` without changing `self` or `mask`.
    pub fn mask_proof(self, mask: u64) -> Option<MaskProof> {
        if self.bit_width == 0 || mask & !mask_for_width(self.bit_width) != 0 {
            return None;
        }
        let width_mask = mask_for_width(self.bit_width);
        let low_bits = low_bits_mask_bits(mask);
        let single_bit =
            (mask != 0 && mask.is_power_of_two()).then_some(mask.trailing_zeros() as u8);
        let outside = width_mask & !mask;
        let proves_zero = (self.known_zero & mask) == mask
            || low_bits.is_some_and(|bits| {
                self.alignment
                    .guarantees(1u64.checked_shl(u32::from(bits)).unwrap_or(0))
            });
        let proves_identity = (self.known_zero & outside) == outside;
        let result = facts_from_known_bits(
            scalar_from_facts(self),
            self.known_zero | outside,
            self.known_one & mask,
            self.effects,
        );
        Some(MaskProof {
            mask,
            low_bits,
            single_bit,
            result_unsigned_range: result.unsigned_range,
            result_signed_range: result.signed_range,
            result_known_zero: result.known_zero,
            result_known_one: result.known_one,
            proves_zero,
            proves_identity,
        })
    }

    fn with_effects(mut self, effects: Effects) -> Self {
        self.effects = effects;
        self
    }

    fn without_effects(mut self) -> Self {
        self.effects = Effects::pure();
        self
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MaskProof {
    pub mask: u64,
    /// `Some(bits)` means that the mask is `(1 << bits) - 1`.
    pub low_bits: Option<u8>,
    /// `Some(bit)` means that the mask is exactly `1 << bit`.
    pub single_bit: Option<u8>,
    pub result_unsigned_range: Option<Interval>,
    pub result_signed_range: Option<Interval>,
    pub result_known_zero: u64,
    pub result_known_one: u64,
    pub proves_zero: bool,
    pub proves_identity: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ShiftProof {
    pub value_width: u8,
    pub count_range: Option<Interval>,
    pub may_be_negative: bool,
    pub may_overshift: bool,
    pub definitely_overshift: bool,
    pub definitely_in_range: bool,
}

impl ShiftProof {
    pub const fn is_definitely_invalid(self) -> bool {
        self.may_be_negative || self.may_overshift
    }
}

pub fn shift_proof(value: &ValueFacts, count: &ValueFacts) -> ShiftProof {
    let count_range = if count.is_signed {
        count.signed_range
    } else {
        count.unsigned_range
    };
    let Some(count_range) = count_range else {
        return ShiftProof {
            value_width: value.bit_width,
            count_range: None,
            may_be_negative: true,
            may_overshift: true,
            definitely_overshift: false,
            definitely_in_range: false,
        };
    };
    let width = i64::from(value.bit_width);
    let may_be_negative = count_range.min < 0;
    let may_overshift = count_range.max >= width;
    ShiftProof {
        value_width: value.bit_width,
        count_range: Some(count_range),
        may_be_negative,
        may_overshift,
        definitely_overshift: count_range.min >= width,
        definitely_in_range: !may_be_negative && count_range.max < width,
    }
}

#[derive(Clone, Debug, Default)]
pub struct RangeAnalysis {
    bindings: HashMap<String, ValueFacts>,
}

impl RangeAnalysis {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn bind(&mut self, name: impl Into<String>, facts: ValueFacts) {
        self.bindings.insert(name.into(), facts);
    }

    pub fn bind_expr(&mut self, name: impl Into<String>, expr: &Expr, ty: &Type) -> ValueFacts {
        let facts = self.analyze(expr, ty).without_effects();
        self.bind(name, facts);
        facts
    }

    pub fn facts_for(&self, name: &str) -> Option<ValueFacts> {
        self.bindings.get(name).copied()
    }

    pub fn analyze(&self, expr: &Expr, ty: &Type) -> ValueFacts {
        self.analyze_inner(expr, ty)
    }

    fn analyze_inner(&self, expr: &Expr, expected_ty: &Type) -> ValueFacts {
        match expr {
            Expr::Int(value) => scalar_type(expected_ty)
                .map(|scalar| facts_from_raw_value(scalar, *value, Effects::pure()))
                .unwrap_or_else(ValueFacts::unknown),
            Expr::TypedInt(value, ty) => scalar_type(ty)
                .map(|scalar| facts_from_raw_value(scalar, *value, Effects::pure()))
                .unwrap_or_else(ValueFacts::unknown),
            Expr::Bool(value) => facts_from_raw_value(
                ScalarType {
                    width: 1,
                    signed: false,
                },
                i64::from(*value),
                Effects::pure(),
            ),
            Expr::Char(value) => facts_from_raw_value(
                ScalarType {
                    width: 8,
                    signed: false,
                },
                i64::from(*value),
                Effects::pure(),
            ),
            Expr::Ident(name) => self
                .bindings
                .get(name)
                .copied()
                .map(ValueFacts::without_effects)
                .unwrap_or_else(|| ValueFacts::unknown_for_type(expected_ty)),
            Expr::Unary { op, expr } => {
                let value = self.analyze_inner(expr, expected_ty);
                unary_facts(*op, value, expected_ty)
            }
            Expr::Binary { left, op, right } => {
                let left = self.analyze_inner(left, expected_ty);
                let right_ty = if matches!(op, BinaryOp::Shl | BinaryOp::Shr) {
                    Type::Named("u32".to_owned())
                } else {
                    expected_ty.clone()
                };
                let right = self.analyze_inner(right, &right_ty);
                binary_facts(left, *op, right, expected_ty)
            }
            Expr::Cast { ty, expr } => {
                let value = self.analyze_inner(expr, expected_ty);
                cast_facts(value, ty)
            }
            Expr::In(_) => ValueFacts::unknown_for_type(expected_ty).with_effects(Effects {
                may_port_io: true,
                ..Effects::pure()
            }),
            Expr::Call { args, .. } => {
                let effects = args.iter().fold(Effects::pure(), |effects, arg| {
                    effects.join(self.analyze_inner(arg, expected_ty).effects)
                });
                ValueFacts::unknown_for_type(expected_ty).with_effects(effects.join(Effects {
                    may_call: true,
                    ..Effects::pure()
                }))
            }
            Expr::Deref(pointer) => {
                let effects = self
                    .analyze_inner(pointer, &Type::Named("u32".to_owned()))
                    .effects;
                ValueFacts::unknown_for_type(expected_ty).with_effects(effects.join(Effects {
                    may_read_memory: true,
                    ..Effects::pure()
                }))
            }
            Expr::Index { index, .. } => {
                let effects = self
                    .analyze_inner(index, &Type::Named("u32".to_owned()))
                    .effects;
                ValueFacts::unknown_for_type(expected_ty).with_effects(effects.join(Effects {
                    may_read_memory: true,
                    ..Effects::pure()
                }))
            }
            Expr::Field { .. } => ValueFacts::unknown_for_type(expected_ty).with_effects(Effects {
                may_read_memory: true,
                ..Effects::pure()
            }),
            Expr::Access(path) => ValueFacts::unknown_for_type(expected_ty).with_effects(
                effects_for_access(self, path).join(Effects {
                    may_read_memory: true,
                    ..Effects::pure()
                }),
            ),
            Expr::AddressOfIndex { index, .. } => {
                let effects = self
                    .analyze_inner(index, &Type::Named("u32".to_owned()))
                    .effects;
                ValueFacts::unknown_for_type(expected_ty).with_effects(effects)
            }
            Expr::AddressOfField { .. } | Expr::AddressOf(_) => {
                ValueFacts::unknown_for_type(expected_ty)
            }
            Expr::AddressOfAccess(path) => ValueFacts::unknown_for_type(expected_ty)
                .with_effects(effects_for_access(self, path)),
            Expr::BankedPointer { pointer, .. } => self.analyze_inner(pointer, expected_ty),
            Expr::Array(values) => {
                let effects = values.iter().fold(Effects::pure(), |effects, value| {
                    effects.join(self.analyze_inner(value, expected_ty).effects)
                });
                ValueFacts::unknown_for_type(expected_ty).with_effects(effects)
            }
            Expr::StructInit { fields, .. } => {
                let effects = fields.iter().fold(Effects::pure(), |effects, (_, value)| {
                    effects.join(self.analyze_inner(value, expected_ty).effects)
                });
                ValueFacts::unknown_for_type(expected_ty).with_effects(effects)
            }
            Expr::String(_) => ValueFacts::unknown_for_type(expected_ty),
        }
    }
}

pub fn analyze_expr(expr: &Expr, ty: &Type) -> ValueFacts {
    RangeAnalysis::new().analyze(expr, ty)
}

#[derive(Clone, Copy)]
struct ScalarType {
    width: u8,
    signed: bool,
}

fn scalar_type(ty: &Type) -> Option<ScalarType> {
    let Type::Named(name) = ty else {
        return None;
    };
    let (width, signed) = match name.as_str() {
        "bool" => (1, false),
        "u8" => (8, false),
        "i8" => (8, true),
        "u16" => (16, false),
        "i16" => (16, true),
        "u24" => (24, false),
        "i24" => (24, true),
        "u32" => (32, false),
        "i32" => (32, true),
        _ => return None,
    };
    Some(ScalarType { width, signed })
}

fn scalar_from_facts(facts: ValueFacts) -> ScalarType {
    ScalarType {
        width: facts.bit_width,
        signed: facts.is_signed,
    }
}

fn mask_for_width(width: u8) -> u64 {
    if width == 0 {
        0
    } else if width >= 64 {
        u64::MAX
    } else {
        (1u64 << width) - 1
    }
}

fn signed_bounds(scalar: ScalarType) -> Interval {
    if scalar.width == 1 {
        return Interval::new(0, 1).unwrap();
    }
    let sign = 1i64 << (scalar.width - 1);
    Interval::new(-sign, sign - 1).unwrap()
}

fn full_facts(scalar: ScalarType) -> ValueFacts {
    let mask = mask_for_width(scalar.width);
    facts_from_known_bits(scalar, 0, 0, Effects::pure()).with_ranges(
        Interval::new(0, mask as i64).unwrap(),
        signed_bounds(scalar),
    )
}

fn facts_from_raw_value(scalar: ScalarType, value: i64, effects: Effects) -> ValueFacts {
    let raw = (value as u64) & mask_for_width(scalar.width);
    facts_from_known_bits(scalar, !raw, raw, effects)
}

fn facts_from_raw_range(scalar: ScalarType, range: Interval, effects: Effects) -> ValueFacts {
    let (known_zero, known_one) =
        common_bits_for_raw_interval(range.min as u64, range.max as u64, scalar.width);
    facts_from_known_bits(scalar, known_zero, known_one, effects)
        .with_ranges(range, signed_range_for_raw(scalar, range))
}

fn facts_from_signed_range(scalar: ScalarType, range: Interval, effects: Effects) -> ValueFacts {
    let modulus = 1i64 << scalar.width;
    let mask = modulus - 1;
    let raw = if range.max < 0 {
        Interval::new(range.min + modulus, range.max + modulus).unwrap()
    } else if range.min >= 0 {
        range
    } else {
        Interval::new(0, mask).unwrap()
    };
    facts_from_raw_range(scalar, raw, effects).with_signed_range(range)
}

fn signed_range_for_raw(scalar: ScalarType, raw: Interval) -> Interval {
    if scalar.width == 1 {
        return raw;
    }
    let sign = 1i64 << (scalar.width - 1);
    let modulus = 1i64 << scalar.width;
    if raw.max < sign {
        raw
    } else if raw.min >= sign {
        Interval::new(raw.min - modulus, raw.max - modulus).unwrap()
    } else {
        signed_bounds(scalar)
    }
}

fn facts_from_known_bits(
    scalar: ScalarType,
    mut known_zero: u64,
    mut known_one: u64,
    effects: Effects,
) -> ValueFacts {
    let mask = mask_for_width(scalar.width);
    known_zero &= mask;
    known_one &= mask;
    known_zero &= !known_one;
    let unsigned_range = Interval::new(known_one as i64, (mask ^ known_zero) as i64).unwrap();
    let signed_range = signed_range_for_known_bits(scalar, known_zero, known_one);
    let alignment_bits = trailing_known_zero_bits(known_zero, scalar.width);
    ValueFacts {
        bit_width: scalar.width,
        is_signed: scalar.signed,
        unsigned_range: Some(unsigned_range),
        signed_range: Some(signed_range),
        known_zero,
        known_one,
        alignment: Alignment {
            modulus: 1u64 << alignment_bits,
        },
        effects,
    }
}

fn signed_range_for_known_bits(scalar: ScalarType, known_zero: u64, known_one: u64) -> Interval {
    let mask = mask_for_width(scalar.width);
    if scalar.width == 1 {
        return Interval::new(known_one as i64, (mask ^ known_zero) as i64).unwrap();
    }
    let sign = 1u64 << (scalar.width - 1);
    if known_zero & sign != 0 {
        Interval::new(known_one as i64, (mask ^ known_zero) as i64).unwrap()
    } else if known_one & sign != 0 {
        Interval::new(
            known_one as i64 - (1i64 << scalar.width),
            (mask ^ known_zero) as i64 - (1i64 << scalar.width),
        )
        .unwrap()
    } else {
        signed_bounds(scalar)
    }
}

fn common_bits_for_raw_interval(min: u64, max: u64, width: u8) -> (u64, u64) {
    let mut known_zero = 0;
    let mut known_one = 0;
    for bit in 0..width {
        let half = 1u64 << bit;
        let period = half << 1;
        if min / period == max / period {
            if min & half == 0 {
                known_zero |= half;
            } else {
                known_one |= half;
            }
        }
    }
    (known_zero, known_one)
}

fn trailing_known_zero_bits(known_zero: u64, width: u8) -> u8 {
    let mut bits = 0;
    while bits < width && known_zero & (1u64 << bits) != 0 {
        bits += 1;
    }
    bits
}

impl ValueFacts {
    fn with_ranges(mut self, unsigned: Interval, signed: Interval) -> Self {
        self.unsigned_range = Some(unsigned);
        self.signed_range = Some(signed);
        self
    }

    fn with_signed_range(mut self, signed: Interval) -> Self {
        self.signed_range = Some(signed);
        self
    }
}

fn unary_facts(op: UnaryOp, value: ValueFacts, expected_ty: &Type) -> ValueFacts {
    let effects = value.effects;
    let Some(scalar) = scalar_type(expected_ty) else {
        return ValueFacts::unknown_for_type(expected_ty).with_effects(effects);
    };
    match op {
        UnaryOp::BitNot => {
            facts_from_known_bits(scalar, value.known_one, value.known_zero, effects)
        }
        UnaryOp::Neg => negate_facts(value, scalar, effects),
        UnaryOp::Not => {
            if value.is_known_zero() {
                bool_fact(true, effects)
            } else if value.is_known_nonzero() {
                bool_fact(false, effects)
            } else {
                full_bool(effects)
            }
        }
    }
}

fn negate_facts(value: ValueFacts, scalar: ScalarType, effects: Effects) -> ValueFacts {
    if let Some(raw) = value.exact_unsigned() {
        return facts_from_raw_value(
            scalar,
            (0u64.wrapping_sub(raw) & mask_for_width(scalar.width)) as i64,
            effects,
        );
    }
    if scalar.signed {
        if let Some(range) = value.signed_range {
            let min = -(range.max as i128);
            let max = -(range.min as i128);
            let bounds = signed_bounds(scalar);
            if min >= i128::from(bounds.min) && max <= i128::from(bounds.max) {
                return facts_from_signed_range(
                    scalar,
                    Interval::new(min as i64, max as i64).unwrap(),
                    effects,
                );
            }
        }
    } else if let Some(range) = value.unsigned_range {
        let modulus = 1u64 << scalar.width;
        if range.max as u64 <= modulus - range.min as u64 {
            let min = (modulus - range.max as u64) & (modulus - 1);
            let max = (modulus - range.min as u64) & (modulus - 1);
            if min <= max {
                return facts_from_raw_range(
                    scalar,
                    Interval::new(min as i64, max as i64).unwrap(),
                    effects,
                );
            }
        }
    }
    full_facts(scalar).with_effects(effects)
}

fn binary_facts(
    left: ValueFacts,
    op: BinaryOp,
    right: ValueFacts,
    expected_ty: &Type,
) -> ValueFacts {
    let effects = left.effects.join(right.effects);
    if matches!(op, BinaryOp::And | BinaryOp::Or) {
        return logical_facts(left, op, right, effects);
    }
    if matches!(
        op,
        BinaryOp::Lt | BinaryOp::Le | BinaryOp::Gt | BinaryOp::Ge | BinaryOp::Eq | BinaryOp::Ne
    ) {
        return comparison_facts(left, op, right, effects);
    }
    let Some(scalar) = scalar_type(expected_ty) else {
        return ValueFacts::unknown_for_type(expected_ty).with_effects(effects);
    };
    match op {
        BinaryOp::BitAnd | BinaryOp::BitOr | BinaryOp::BitXor => {
            bitwise_facts(left, right, scalar, op, effects)
        }
        BinaryOp::Shl | BinaryOp::Shr => {
            shift_facts(left, right, op == BinaryOp::Shr, scalar, effects)
        }
        BinaryOp::Add | BinaryOp::Sub | BinaryOp::Mul | BinaryOp::Div | BinaryOp::Mod => {
            arithmetic_facts(left, right, op, scalar, effects)
        }
        _ => unreachable!(),
    }
}

fn logical_facts(
    left: ValueFacts,
    op: BinaryOp,
    right: ValueFacts,
    effects: Effects,
) -> ValueFacts {
    let result = match op {
        BinaryOp::And if left.is_known_zero() || right.is_known_zero() => Some(false),
        BinaryOp::And if left.is_known_nonzero() && right.is_known_nonzero() => Some(true),
        BinaryOp::Or if left.is_known_nonzero() || right.is_known_nonzero() => Some(true),
        BinaryOp::Or if left.is_known_zero() && right.is_known_zero() => Some(false),
        _ => None,
    };
    result.map_or_else(|| full_bool(effects), |value| bool_fact(value, effects))
}

fn comparison_facts(
    left: ValueFacts,
    op: BinaryOp,
    right: ValueFacts,
    effects: Effects,
) -> ValueFacts {
    let signed = left.is_signed;
    let Some(a) = active_range(left, signed) else {
        return full_bool(effects);
    };
    let Some(b) = active_range(right, signed) else {
        return full_bool(effects);
    };
    let always_true = match op {
        BinaryOp::Lt => a.max < b.min,
        BinaryOp::Le => a.max <= b.min,
        BinaryOp::Gt => a.min > b.max,
        BinaryOp::Ge => a.min >= b.max,
        BinaryOp::Eq => a.is_singleton() && b.is_singleton() && a.min == b.min,
        BinaryOp::Ne => a.max < b.min || a.min > b.max,
        _ => false,
    };
    let always_false = match op {
        BinaryOp::Lt => a.min >= b.max,
        BinaryOp::Le => a.min > b.max,
        BinaryOp::Gt => a.max <= b.min,
        BinaryOp::Ge => a.max < b.min,
        BinaryOp::Eq => a.max < b.min || a.min > b.max,
        BinaryOp::Ne => a.is_singleton() && b.is_singleton() && a.min == b.min,
        _ => false,
    };
    if always_true {
        bool_fact(true, effects)
    } else if always_false {
        bool_fact(false, effects)
    } else {
        full_bool(effects)
    }
}

fn active_range(facts: ValueFacts, signed: bool) -> Option<Interval> {
    if signed {
        facts.signed_range
    } else {
        facts.unsigned_range
    }
}

fn full_bool(effects: Effects) -> ValueFacts {
    full_facts(ScalarType {
        width: 1,
        signed: false,
    })
    .with_effects(effects)
}

fn bool_fact(value: bool, effects: Effects) -> ValueFacts {
    facts_from_raw_value(
        ScalarType {
            width: 1,
            signed: false,
        },
        i64::from(value),
        effects,
    )
}

fn bitwise_facts(
    left: ValueFacts,
    right: ValueFacts,
    scalar: ScalarType,
    op: BinaryOp,
    effects: Effects,
) -> ValueFacts {
    let (known_zero, known_one) = match op {
        BinaryOp::BitAnd => (
            left.known_zero | right.known_zero,
            left.known_one & right.known_one,
        ),
        BinaryOp::BitOr => (
            left.known_zero & right.known_zero,
            left.known_one | right.known_one,
        ),
        BinaryOp::BitXor => (
            (left.known_zero & right.known_zero) | (left.known_one & right.known_one),
            (left.known_zero & right.known_one) | (left.known_one & right.known_zero),
        ),
        _ => unreachable!(),
    };
    facts_from_known_bits(scalar, known_zero, known_one, effects)
}

fn arithmetic_facts(
    left: ValueFacts,
    right: ValueFacts,
    op: BinaryOp,
    scalar: ScalarType,
    effects: Effects,
) -> ValueFacts {
    if let (Some(a), Some(b)) = (
        active_exact(left, scalar.signed),
        active_exact(right, scalar.signed),
    ) {
        if let Some(result) = exact_arithmetic(a, b, op, scalar) {
            return facts_from_raw_value(scalar, result, effects);
        }
    }
    if scalar.signed {
        signed_arithmetic(left, right, op, scalar, effects)
    } else {
        unsigned_arithmetic(left, right, op, scalar, effects)
    }
}

fn active_exact(facts: ValueFacts, signed: bool) -> Option<i64> {
    active_range(facts, signed)?
        .is_singleton()
        .then_some(active_range(facts, signed).unwrap().min)
}

fn exact_arithmetic(left: i64, right: i64, op: BinaryOp, scalar: ScalarType) -> Option<i64> {
    if scalar.signed {
        let value = match op {
            BinaryOp::Add => i128::from(left) + i128::from(right),
            BinaryOp::Sub => i128::from(left) - i128::from(right),
            BinaryOp::Mul => i128::from(left) * i128::from(right),
            BinaryOp::Div => {
                if right == 0 {
                    0
                } else {
                    i128::from(left) / i128::from(right)
                }
            }
            BinaryOp::Mod => {
                if right == 0 {
                    0
                } else {
                    i128::from(left) % i128::from(right)
                }
            }
            _ => return None,
        };
        Some((value & i128::from(mask_for_width(scalar.width))) as i64)
    } else {
        let left = left as u64;
        let right = right as u64;
        let value = match op {
            BinaryOp::Add => left.wrapping_add(right),
            BinaryOp::Sub => left.wrapping_sub(right),
            BinaryOp::Mul => left.wrapping_mul(right),
            BinaryOp::Div => left.checked_div(right).unwrap_or(0),
            BinaryOp::Mod => left.checked_rem(right).unwrap_or(0),
            _ => return None,
        };
        Some((value & mask_for_width(scalar.width)) as i64)
    }
}

fn unsigned_arithmetic(
    left: ValueFacts,
    right: ValueFacts,
    op: BinaryOp,
    scalar: ScalarType,
    effects: Effects,
) -> ValueFacts {
    let Some(a) = left.unsigned_range else {
        return full_facts(scalar).with_effects(effects);
    };
    let Some(b) = right.unsigned_range else {
        return full_facts(scalar).with_effects(effects);
    };
    let max = mask_for_width(scalar.width);
    let result = match op {
        BinaryOp::Add => bounded_unsigned(a, b, |x, y| x.checked_add(y), max),
        BinaryOp::Sub => bounded_unsigned(a, b, |x, y| x.checked_sub(y), max),
        BinaryOp::Mul => bounded_unsigned(a, b, |x, y| x.checked_mul(y), max),
        BinaryOp::Div if b.min > 0 => Interval::new(a.min / b.max, a.max / b.min),
        BinaryOp::Mod if b.min > 0 => Interval::new(0, b.max - 1),
        BinaryOp::Div | BinaryOp::Mod => None,
        _ => None,
    };
    result
        .map(|range| facts_from_raw_range(scalar, range, effects))
        .unwrap_or_else(|| full_facts(scalar).with_effects(effects))
}

fn bounded_unsigned<F>(a: Interval, b: Interval, operation: F, max: u64) -> Option<Interval>
where
    F: Fn(u64, u64) -> Option<u64>,
{
    let values = [
        operation(a.min as u64, b.min as u64)?,
        operation(a.min as u64, b.max as u64)?,
        operation(a.max as u64, b.min as u64)?,
        operation(a.max as u64, b.max as u64)?,
    ];
    if values.iter().any(|value| *value > max) {
        return None;
    }
    Interval::new(
        values.iter().copied().min()? as i64,
        values.iter().copied().max()? as i64,
    )
}

fn signed_arithmetic(
    left: ValueFacts,
    right: ValueFacts,
    op: BinaryOp,
    scalar: ScalarType,
    effects: Effects,
) -> ValueFacts {
    let Some(a) = left.signed_range else {
        return full_facts(scalar).with_effects(effects);
    };
    let Some(b) = right.signed_range else {
        return full_facts(scalar).with_effects(effects);
    };
    let bounds = signed_bounds(scalar);
    let result = match op {
        BinaryOp::Add => bounded_signed(a, b, |x, y| x + y, bounds),
        BinaryOp::Sub => bounded_signed(a, b, |x, y| x - y, bounds),
        BinaryOp::Mul => bounded_signed(a, b, |x, y| x * y, bounds),
        BinaryOp::Div if !b.contains(0) => bounded_signed(a, b, |x, y| x / y, bounds),
        BinaryOp::Mod if !b.contains(0) => {
            let magnitude = b.min.unsigned_abs().max(b.max.unsigned_abs()) as i64;
            Interval::new(-(magnitude - 1), magnitude - 1)
        }
        BinaryOp::Div | BinaryOp::Mod if b.is_singleton() && b.min == 0 => {
            Some(Interval::singleton(0))
        }
        _ => None,
    };
    result
        .map(|range| facts_from_signed_range(scalar, range, effects))
        .unwrap_or_else(|| full_facts(scalar).with_effects(effects))
}

fn bounded_signed<F>(a: Interval, b: Interval, operation: F, bounds: Interval) -> Option<Interval>
where
    F: Fn(i128, i128) -> i128,
{
    let values = [
        operation(a.min as i128, b.min as i128),
        operation(a.min as i128, b.max as i128),
        operation(a.max as i128, b.min as i128),
        operation(a.max as i128, b.max as i128),
    ];
    if values
        .iter()
        .any(|value| *value < i128::from(bounds.min) || *value > i128::from(bounds.max))
    {
        return None;
    }
    Interval::new(
        values.iter().copied().min()? as i64,
        values.iter().copied().max()? as i64,
    )
}

fn shift_facts(
    value: ValueFacts,
    count: ValueFacts,
    right: bool,
    scalar: ScalarType,
    effects: Effects,
) -> ValueFacts {
    let proof = shift_proof(&value, &count);
    if !proof.definitely_in_range {
        return full_facts(scalar).with_effects(effects);
    }
    let range = proof.count_range.unwrap();
    if range.is_singleton() {
        return fixed_shift(value, range.min as u8, right, scalar, effects);
    }
    if !right {
        let shift = range.min as u8;
        let mask = mask_for_width(scalar.width);
        let low_zero = if shift == 0 { 0 } else { (1u64 << shift) - 1 };
        let known_zero = ((value.known_zero << shift) | low_zero) & mask;
        if value.is_known_zero() {
            return facts_from_known_bits(scalar, mask, 0, effects);
        }
        return facts_from_known_bits(scalar, known_zero, 0, effects);
    }
    if value.is_known_zero() {
        facts_from_known_bits(scalar, mask_for_width(scalar.width), 0, effects)
    } else {
        full_facts(scalar).with_effects(effects)
    }
}

fn fixed_shift(
    value: ValueFacts,
    count: u8,
    right: bool,
    scalar: ScalarType,
    effects: Effects,
) -> ValueFacts {
    let mask = mask_for_width(scalar.width);
    let count = u32::from(count);
    if !right {
        return facts_from_known_bits(
            scalar,
            ((value.known_zero << count) | low_mask(count)) & mask,
            (value.known_one << count) & mask,
            effects,
        );
    }
    let shifted_zero = value.known_zero >> count;
    let shifted_one = value.known_one >> count;
    let high_mask = if count == 0 {
        0
    } else {
        mask ^ (mask >> count)
    };
    if scalar.signed {
        let sign = 1u64 << (scalar.width - 1);
        if value.known_zero & sign != 0 {
            facts_from_known_bits(scalar, shifted_zero | high_mask, shifted_one, effects)
        } else if value.known_one & sign != 0 {
            facts_from_known_bits(scalar, shifted_zero, shifted_one | high_mask, effects)
        } else {
            facts_from_known_bits(scalar, shifted_zero, shifted_one, effects)
        }
    } else {
        facts_from_known_bits(scalar, shifted_zero | high_mask, shifted_one, effects)
    }
}

fn low_mask(bits: u32) -> u64 {
    if bits == 0 {
        0
    } else if bits >= 64 {
        u64::MAX
    } else {
        (1u64 << bits) - 1
    }
}

fn cast_facts(value: ValueFacts, target: &Type) -> ValueFacts {
    let Some(target_scalar) = scalar_type(target) else {
        return ValueFacts::unknown_for_type(target).with_effects(value.effects);
    };
    if value.bit_width == 0 {
        return full_facts(target_scalar).with_effects(value.effects);
    }
    let source_mask = mask_for_width(value.bit_width);
    let target_mask = mask_for_width(target_scalar.width);
    let (mut known_zero, mut known_one) = (value.known_zero, value.known_one);
    if target_scalar.width < value.bit_width {
        known_zero &= target_mask;
        known_one &= target_mask;
    } else if target_scalar.width > value.bit_width {
        let high = target_mask & !source_mask;
        if value.is_signed {
            let sign = 1u64 << (value.bit_width - 1);
            if value.known_zero & sign != 0 {
                known_zero |= high;
            } else if value.known_one & sign != 0 {
                known_one |= high;
            }
        } else {
            known_zero |= high;
        }
    }
    facts_from_known_bits(target_scalar, known_zero, known_one, value.effects)
}

fn low_bits_mask_bits(mask: u64) -> Option<u8> {
    if mask == 0 || !(mask + 1).is_power_of_two() {
        None
    } else {
        Some((mask + 1).trailing_zeros() as u8)
    }
}

fn effects_for_access(analysis: &RangeAnalysis, path: &AccessPath) -> Effects {
    path.segments
        .iter()
        .fold(Effects::pure(), |effects, segment| match segment {
            AccessSegment::Index(index) => effects.join(
                analysis
                    .analyze_inner(index, &Type::Named("u32".to_owned()))
                    .effects,
            ),
            AccessSegment::Field(_) => effects,
        })
}

#[cfg(test)]
mod tests;
