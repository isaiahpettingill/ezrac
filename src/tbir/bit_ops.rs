//! Canonical, target-independent bit operations.
//!
//! These nodes describe bit operations without selecting target instructions.
//! Operands carry their type and volatile access state. Constructors validate
//! ranges and normalize constants without reordering or duplicating reads.

use crate::{
    ast::{BinaryOp, Expr, Type},
    compat::prelude::*,
};

#[cfg(test)]
mod bit_ops_tests {
    include!("bit_ops_tests.rs");
}

/// A scalar bit width. The named constants cover EZRA scalar widths; smaller
/// widths are also useful for extracted fields.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct BitWidth(u8);

impl BitWidth {
    pub const W1: Self = Self(1);
    pub const W8: Self = Self(8);
    pub const W16: Self = Self(16);
    pub const W24: Self = Self(24);
    pub const W32: Self = Self(32);
    pub const U8: Self = Self::W8;
    pub const U16: Self = Self::W16;
    pub const U24: Self = Self::W24;
    pub const U32: Self = Self::W32;

    pub const fn new(bits: u8) -> Option<Self> {
        if bits == 0 || bits > 32 {
            None
        } else {
            Some(Self(bits))
        }
    }

    pub const fn bits(self) -> u8 {
        self.0
    }

    pub const fn bytes(self) -> u8 {
        (self.0 + 7) / 8
    }

    pub const fn mask(self) -> u32 {
        if self.0 == 32 {
            u32::MAX
        } else {
            (1u32 << self.0) - 1
        }
    }

    pub const fn contains_bit(self, bit: u8) -> bool {
        bit < self.0
    }
}

/// Signedness carried by a bit-operation value.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BitSignedness {
    Unsigned,
    Signed,
}

/// A typed scalar value used by a bit operation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BitType {
    pub width: BitWidth,
    pub signedness: BitSignedness,
}

impl BitType {
    pub const fn new(width: BitWidth, signedness: BitSignedness) -> Self {
        Self { width, signedness }
    }

    pub const fn unsigned(width: BitWidth) -> Self {
        Self::new(width, BitSignedness::Unsigned)
    }

    pub const fn signed(width: BitWidth) -> Self {
        Self::new(width, BitSignedness::Signed)
    }

    pub const fn is_signed(self) -> bool {
        matches!(self.signedness, BitSignedness::Signed)
    }
}

/// Whether evaluating an operand can observe a volatile read.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BitAccess {
    Pure,
    Volatile,
}

/// An expression with the type and access rules needed by a bit operation.
#[derive(Clone, Debug, PartialEq)]
pub struct BitOperand {
    pub expr: Expr,
    pub ty: BitType,
    pub access: BitAccess,
}

impl BitOperand {
    pub fn new(expr: Expr, ty: BitType) -> Self {
        Self {
            expr,
            ty,
            access: BitAccess::Pure,
        }
    }

    pub fn volatile(expr: Expr, ty: BitType) -> Self {
        Self {
            expr,
            ty,
            access: BitAccess::Volatile,
        }
    }

    pub const fn is_volatile(&self) -> bool {
        matches!(self.access, BitAccess::Volatile)
    }
}

/// The direction of a rotate operation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RotateDirection {
    Left,
    Right,
}

/// The extension rule for widening an integer value.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ExtensionKind {
    Zero,
    Sign,
}

/// The kind of canonical operation represented by [`BitOp`].
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BitOpKind {
    BitTest,
    MaskTestAny,
    MaskTestAll,
    SetBit,
    ClearBit,
    ToggleBit,
    Extract,
    Insert,
    RotateLeft,
    RotateRight,
    ZeroExtend,
    SignExtend,
    Truncate,
    ByteSwap,
}

/// A canonical target-independent bit operation.
#[derive(Clone, Debug, PartialEq)]
pub enum BitOp {
    BitTest {
        value: BitOperand,
        bit: u8,
    },
    MaskTestAny {
        value: BitOperand,
        mask: u32,
    },
    MaskTestAll {
        value: BitOperand,
        mask: u32,
    },
    SetBit {
        value: BitOperand,
        bit: u8,
    },
    ClearBit {
        value: BitOperand,
        bit: u8,
    },
    ToggleBit {
        value: BitOperand,
        bit: u8,
    },
    Extract {
        value: BitOperand,
        lsb: u8,
        width: BitWidth,
    },
    Insert {
        base: BitOperand,
        value: BitOperand,
        lsb: u8,
        width: BitWidth,
    },
    RotateLeft {
        value: BitOperand,
        amount: u8,
    },
    RotateRight {
        value: BitOperand,
        amount: u8,
    },
    ZeroExtend {
        value: BitOperand,
        to: BitType,
    },
    SignExtend {
        value: BitOperand,
        to: BitType,
    },
    Truncate {
        value: BitOperand,
        to: BitType,
    },
    ByteSwap {
        value: BitOperand,
    },
}

impl BitOp {
    pub fn bit_test(value: BitOperand, bit: u8) -> Option<Self> {
        value
            .ty
            .width
            .contains_bit(bit)
            .then_some(Self::BitTest { value, bit })
    }

    pub fn mask_test_any(value: BitOperand, mask: u32) -> Self {
        Self::MaskTestAny {
            mask: mask & value.ty.width.mask(),
            value,
        }
    }

    pub fn mask_test_all(value: BitOperand, mask: u32) -> Self {
        Self::MaskTestAll {
            mask: mask & value.ty.width.mask(),
            value,
        }
    }

    pub fn set_bit(value: BitOperand, bit: u8) -> Option<Self> {
        value
            .ty
            .width
            .contains_bit(bit)
            .then_some(Self::SetBit { value, bit })
    }

    pub fn clear_bit(value: BitOperand, bit: u8) -> Option<Self> {
        value
            .ty
            .width
            .contains_bit(bit)
            .then_some(Self::ClearBit { value, bit })
    }

    pub fn toggle_bit(value: BitOperand, bit: u8) -> Option<Self> {
        value
            .ty
            .width
            .contains_bit(bit)
            .then_some(Self::ToggleBit { value, bit })
    }

    pub fn extract(value: BitOperand, lsb: u8, width: BitWidth) -> Option<Self> {
        valid_range(value.ty.width, lsb, width).then_some(Self::Extract { value, lsb, width })
    }

    pub fn insert(base: BitOperand, value: BitOperand, lsb: u8, width: BitWidth) -> Option<Self> {
        valid_range(base.ty.width, lsb, width).then_some(Self::Insert {
            base,
            value,
            lsb,
            width,
        })
    }

    pub fn rotate(value: BitOperand, amount: u8, direction: RotateDirection) -> Self {
        match direction {
            RotateDirection::Left => Self::rotate_left(value, amount),
            RotateDirection::Right => Self::rotate_right(value, amount),
        }
    }

    pub fn rotate_left(value: BitOperand, amount: u8) -> Self {
        Self::RotateLeft {
            amount: amount % value.ty.width.bits(),
            value,
        }
    }

    pub fn rotate_right(value: BitOperand, amount: u8) -> Self {
        Self::RotateRight {
            amount: amount % value.ty.width.bits(),
            value,
        }
    }

    pub fn extend(value: BitOperand, to: BitType, kind: ExtensionKind) -> Option<Self> {
        match kind {
            ExtensionKind::Zero => Self::zero_extend_to(value, to),
            ExtensionKind::Sign => Self::sign_extend_to(value, to),
        }
    }

    pub fn zero_extend(value: BitOperand, to: BitWidth) -> Option<Self> {
        Self::zero_extend_to(value, BitType::unsigned(to))
    }

    pub fn zero_extend_to(value: BitOperand, to: BitType) -> Option<Self> {
        (to.width.bits() > value.ty.width.bits()).then_some(Self::ZeroExtend { value, to })
    }

    pub fn sign_extend(value: BitOperand, to: BitWidth) -> Option<Self> {
        Self::sign_extend_to(value, BitType::signed(to))
    }

    pub fn sign_extend_to(value: BitOperand, to: BitType) -> Option<Self> {
        (to.width.bits() > value.ty.width.bits()).then_some(Self::SignExtend { value, to })
    }

    pub fn truncate(value: BitOperand, to: BitWidth) -> Option<Self> {
        let to = BitType::new(to, value.ty.signedness);
        (to.width.bits() < value.ty.width.bits()).then_some(Self::Truncate { value, to })
    }

    pub fn byte_swap(value: BitOperand) -> Self {
        Self::ByteSwap { value }
    }

    pub const fn kind(&self) -> BitOpKind {
        match self {
            Self::BitTest { .. } => BitOpKind::BitTest,
            Self::MaskTestAny { .. } => BitOpKind::MaskTestAny,
            Self::MaskTestAll { .. } => BitOpKind::MaskTestAll,
            Self::SetBit { .. } => BitOpKind::SetBit,
            Self::ClearBit { .. } => BitOpKind::ClearBit,
            Self::ToggleBit { .. } => BitOpKind::ToggleBit,
            Self::Extract { .. } => BitOpKind::Extract,
            Self::Insert { .. } => BitOpKind::Insert,
            Self::RotateLeft { .. } => BitOpKind::RotateLeft,
            Self::RotateRight { .. } => BitOpKind::RotateRight,
            Self::ZeroExtend { .. } => BitOpKind::ZeroExtend,
            Self::SignExtend { .. } => BitOpKind::SignExtend,
            Self::Truncate { .. } => BitOpKind::Truncate,
            Self::ByteSwap { .. } => BitOpKind::ByteSwap,
        }
    }

    pub const fn is_volatile(&self) -> bool {
        match self {
            Self::BitTest { value, .. }
            | Self::MaskTestAny { value, .. }
            | Self::MaskTestAll { value, .. }
            | Self::SetBit { value, .. }
            | Self::ClearBit { value, .. }
            | Self::ToggleBit { value, .. }
            | Self::Extract { value, .. }
            | Self::RotateLeft { value, .. }
            | Self::RotateRight { value, .. }
            | Self::ZeroExtend { value, .. }
            | Self::SignExtend { value, .. }
            | Self::Truncate { value, .. }
            | Self::ByteSwap { value } => value.is_volatile(),
            Self::Insert { base, value, .. } => base.is_volatile() || value.is_volatile(),
        }
    }

    pub const fn result_type(&self) -> BitType {
        match self {
            Self::BitTest { .. } | Self::MaskTestAny { .. } | Self::MaskTestAll { .. } => {
                BitType::unsigned(BitWidth::W1)
            }
            Self::Extract { width, .. } => BitType::unsigned(*width),
            Self::Insert { base, .. }
            | Self::SetBit { value: base, .. }
            | Self::ClearBit { value: base, .. }
            | Self::ToggleBit { value: base, .. }
            | Self::RotateLeft { value: base, .. }
            | Self::RotateRight { value: base, .. }
            | Self::ByteSwap { value: base } => base.ty,
            Self::ZeroExtend { to, .. }
            | Self::SignExtend { to, .. }
            | Self::Truncate { to, .. } => *to,
        }
    }
}

/// An ordered list of canonical operations. It has no sorting or commuting
/// operation, so volatile operations remain barriers to later target passes.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct BitOpSequence {
    operations: Vec<BitOp>,
}

impl BitOpSequence {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn from_operations(operations: Vec<BitOp>) -> Self {
        Self { operations }
    }

    pub fn push(&mut self, operation: BitOp) {
        self.operations.push(operation);
    }

    pub fn as_slice(&self) -> &[BitOp] {
        &self.operations
    }

    pub fn iter(&self) -> impl Iterator<Item = &BitOp> {
        self.operations.iter()
    }

    pub fn into_operations(self) -> Vec<BitOp> {
        self.operations
    }

    /// Returns the operations in their original order.
    pub fn canonicalize(self) -> Self {
        self
    }

    pub const fn may_reorder(left: &BitOp, right: &BitOp) -> bool {
        !left.is_volatile() && !right.is_volatile()
    }
}

impl IntoIterator for BitOpSequence {
    type Item = BitOp;
    type IntoIter = alloc::vec::IntoIter<BitOp>;

    fn into_iter(self) -> Self::IntoIter {
        self.operations.into_iter()
    }
}

pub type BitOperation = BitOp;
pub type BitValue = BitOperand;

pub fn bit_test(value: BitOperand, bit: u8) -> Option<BitOp> {
    BitOp::bit_test(value, bit)
}

pub fn mask_test_any(value: BitOperand, mask: u32) -> BitOp {
    BitOp::mask_test_any(value, mask)
}

pub fn mask_test_all(value: BitOperand, mask: u32) -> BitOp {
    BitOp::mask_test_all(value, mask)
}

pub fn set_bit(value: BitOperand, bit: u8) -> Option<BitOp> {
    BitOp::set_bit(value, bit)
}

pub fn clear_bit(value: BitOperand, bit: u8) -> Option<BitOp> {
    BitOp::clear_bit(value, bit)
}

pub fn toggle_bit(value: BitOperand, bit: u8) -> Option<BitOp> {
    BitOp::toggle_bit(value, bit)
}

pub fn extract(value: BitOperand, lsb: u8, width: BitWidth) -> Option<BitOp> {
    BitOp::extract(value, lsb, width)
}

pub fn insert(base: BitOperand, value: BitOperand, lsb: u8, width: BitWidth) -> Option<BitOp> {
    BitOp::insert(base, value, lsb, width)
}

pub fn rotate(value: BitOperand, amount: u8, direction: RotateDirection) -> BitOp {
    BitOp::rotate(value, amount, direction)
}

pub fn rotate_left(value: BitOperand, amount: u8) -> BitOp {
    BitOp::rotate_left(value, amount)
}

pub fn rotate_right(value: BitOperand, amount: u8) -> BitOp {
    BitOp::rotate_right(value, amount)
}

pub fn extend(value: BitOperand, to: BitType, kind: ExtensionKind) -> Option<BitOp> {
    BitOp::extend(value, to, kind)
}

pub fn zero_extend(value: BitOperand, to: BitWidth) -> Option<BitOp> {
    BitOp::zero_extend(value, to)
}

pub fn sign_extend(value: BitOperand, to: BitWidth) -> Option<BitOp> {
    BitOp::sign_extend(value, to)
}

pub fn truncate(value: BitOperand, to: BitWidth) -> Option<BitOp> {
    BitOp::truncate(value, to)
}

pub fn byte_swap(value: BitOperand) -> BitOp {
    BitOp::byte_swap(value)
}

/// A canonical operation recognized from an AST expression. `inverted` keeps
/// the polarity of comparisons such as `(value & mask) == 0` without changing
/// or re-evaluating the operand.
#[derive(Clone, Debug, PartialEq)]
pub struct BitOpMatch {
    pub operation: BitOp,
    pub inverted: bool,
}

impl BitOpMatch {
    fn direct(operation: BitOp) -> Self {
        Self {
            operation,
            inverted: false,
        }
    }
}

/// Recognize common source-level bit patterns without rewriting the AST.
///
/// `operand` supplies type and volatile-access facts for a source expression.
/// `constant` supplies a value only for expressions proven constant by the
/// caller. The recognizer never evaluates, duplicates, or reorders operands.
pub fn recognize_expr<F, C>(expr: &Expr, operand: F, constant: C) -> Option<BitOpMatch>
where
    F: Fn(&Expr) -> Option<BitOperand> + Copy,
    C: Fn(&Expr) -> Option<i64> + Copy,
{
    if let Some(operation) = recognize_bit_update(expr, operand, constant) {
        return Some(BitOpMatch::direct(operation));
    }
    if let Some(matched) = recognize_mask_test(expr, operand, constant) {
        return Some(matched);
    }
    if let Some(operation) = recognize_extract(expr, operand, constant) {
        return Some(BitOpMatch::direct(operation));
    }
    if let Some(operation) = recognize_rotate_or_swap(expr, operand, constant) {
        return Some(BitOpMatch::direct(operation));
    }
    recognize_cast(expr, operand).map(BitOpMatch::direct)
}

fn recognize_bit_update<F, C>(expr: &Expr, operand: F, constant: C) -> Option<BitOp>
where
    F: Fn(&Expr) -> Option<BitOperand> + Copy,
    C: Fn(&Expr) -> Option<i64> + Copy,
{
    let Expr::Binary { left, op, right } = expr else {
        return None;
    };
    let (value_expr, raw_mask) = source_and_constant(left, right, constant)?;
    let value = operand(value_expr)?;
    let width_mask = value.ty.width.mask();
    let mask = raw_mask as u32 & width_mask;
    match op {
        BinaryOp::BitOr if mask.is_power_of_two() => {
            BitOp::set_bit(value, mask.trailing_zeros() as u8)
        }
        BinaryOp::BitXor if mask.is_power_of_two() => {
            BitOp::toggle_bit(value, mask.trailing_zeros() as u8)
        }
        BinaryOp::BitAnd => {
            let cleared = !mask & width_mask;
            cleared
                .is_power_of_two()
                .then(|| BitOp::clear_bit(value, cleared.trailing_zeros() as u8))?
        }
        _ => None,
    }
}

fn recognize_mask_test<F, C>(expr: &Expr, operand: F, constant: C) -> Option<BitOpMatch>
where
    F: Fn(&Expr) -> Option<BitOperand> + Copy,
    C: Fn(&Expr) -> Option<i64> + Copy,
{
    let Expr::Binary { left, op, right } = expr else {
        return None;
    };
    if !matches!(op, BinaryOp::Eq | BinaryOp::Ne) {
        return None;
    }
    let Expr::Binary {
        left: source,
        op: BinaryOp::BitAnd,
        right: mask_expr,
    } = left.as_ref()
    else {
        return None;
    };
    let value = operand(source)?;
    let mask = constant(mask_expr)? as u32 & value.ty.width.mask();
    let expected = constant(right)? as u32 & value.ty.width.mask();
    let (operation, inverted) = if expected == 0 {
        let operation = if mask.is_power_of_two() {
            BitOp::bit_test(value, mask.trailing_zeros() as u8)?
        } else {
            BitOp::mask_test_any(value, mask)
        };
        (operation, *op == BinaryOp::Eq)
    } else if expected == mask {
        (BitOp::mask_test_all(value, mask), *op == BinaryOp::Ne)
    } else {
        return None;
    };
    Some(BitOpMatch {
        operation,
        inverted,
    })
}

fn recognize_extract<F, C>(expr: &Expr, operand: F, constant: C) -> Option<BitOp>
where
    F: Fn(&Expr) -> Option<BitOperand> + Copy,
    C: Fn(&Expr) -> Option<i64> + Copy,
{
    let Expr::Binary {
        left,
        op: BinaryOp::BitAnd,
        right,
    } = expr
    else {
        return None;
    };
    let Expr::Binary {
        left: source,
        op: BinaryOp::Shr,
        right: shift,
    } = left.as_ref()
    else {
        return None;
    };
    let value = operand(source)?;
    let lsb = u8::try_from(constant(shift)?).ok()?;
    let mask = constant(right)? as u32 & value.ty.width.mask();
    if mask == 0 || mask & mask.wrapping_add(1) != 0 {
        return None;
    }
    let width = BitWidth::new(mask.count_ones() as u8)?;
    if width == BitWidth::W1 {
        BitOp::bit_test(value, lsb)
    } else {
        BitOp::extract(value, lsb, width)
    }
}

fn recognize_rotate_or_swap<F, C>(expr: &Expr, operand: F, constant: C) -> Option<BitOp>
where
    F: Fn(&Expr) -> Option<BitOperand> + Copy,
    C: Fn(&Expr) -> Option<i64> + Copy,
{
    let Expr::Binary {
        left,
        op: BinaryOp::BitOr,
        right,
    } = expr
    else {
        return None;
    };
    let first = shifted_source(left, constant)?;
    let second = shifted_source(right, constant)?;
    let (left_shift, right_shift) = match (first.1, second.1) {
        (BinaryOp::Shl, BinaryOp::Shr) => (first, second),
        (BinaryOp::Shr, BinaryOp::Shl) => (second, first),
        _ => return None,
    };
    if left_shift.0 != right_shift.0 {
        return None;
    }
    let value = operand(left_shift.0)?;
    let bits = u32::from(value.ty.width.bits());
    if left_shift.2 == 0
        || right_shift.2 == 0
        || left_shift.2 >= bits
        || left_shift.2 + right_shift.2 != bits
    {
        return None;
    }
    if bits == 16 && left_shift.2 == 8 {
        Some(BitOp::byte_swap(value))
    } else {
        Some(BitOp::rotate_left(value, left_shift.2 as u8))
    }
}

fn recognize_cast<F>(expr: &Expr, operand: F) -> Option<BitOp>
where
    F: Fn(&Expr) -> Option<BitOperand>,
{
    let Expr::Cast { expr, ty } = expr else {
        return None;
    };
    let value = operand(expr)?;
    let target = ast_bit_type(ty)?;
    match target.width.bits().cmp(&value.ty.width.bits()) {
        core::cmp::Ordering::Greater if value.ty.is_signed() => {
            BitOp::sign_extend_to(value, target)
        }
        core::cmp::Ordering::Greater => BitOp::zero_extend_to(value, target),
        core::cmp::Ordering::Less => Some(BitOp::Truncate { value, to: target }),
        core::cmp::Ordering::Equal => None,
    }
}

fn source_and_constant<'a, C>(
    left: &'a Expr,
    right: &'a Expr,
    constant: C,
) -> Option<(&'a Expr, i64)>
where
    C: Fn(&Expr) -> Option<i64>,
{
    constant(right)
        .map(|value| (left, value))
        .or_else(|| constant(left).map(|value| (right, value)))
}

fn shifted_source<C>(expr: &Expr, constant: C) -> Option<(&Expr, BinaryOp, u32)>
where
    C: Fn(&Expr) -> Option<i64>,
{
    let Expr::Binary { left, op, right } = expr else {
        return None;
    };
    if !matches!(op, BinaryOp::Shl | BinaryOp::Shr) {
        return None;
    }
    Some((left, *op, u32::try_from(constant(right)?).ok()?))
}

fn ast_bit_type(ty: &Type) -> Option<BitType> {
    let Type::Named(name) = ty else {
        return None;
    };
    let (width, signedness) = match name.as_str() {
        "bool" => (BitWidth::W1, BitSignedness::Unsigned),
        "u8" => (BitWidth::W8, BitSignedness::Unsigned),
        "i8" => (BitWidth::W8, BitSignedness::Signed),
        "u16" => (BitWidth::W16, BitSignedness::Unsigned),
        "i16" => (BitWidth::W16, BitSignedness::Signed),
        "u24" => (BitWidth::W24, BitSignedness::Unsigned),
        "i24" => (BitWidth::W24, BitSignedness::Signed),
        "u32" => (BitWidth::W32, BitSignedness::Unsigned),
        "i32" => (BitWidth::W32, BitSignedness::Signed),
        _ => return None,
    };
    Some(BitType::new(width, signedness))
}

fn valid_range(source: BitWidth, lsb: u8, width: BitWidth) -> bool {
    u16::from(lsb) + u16::from(width.bits()) <= u16::from(source.bits())
}
