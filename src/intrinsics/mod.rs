//! Target-independent intrinsic names, signatures, and semantic metadata.
//!
//! This module deliberately stops at catalog and type-checking concerns. It does
//! not lower an intrinsic, inspect a target, or describe the representation of a
//! multi-value AST expression. Callers provide resolved argument types and, when
//! available, integer constant values.

use alloc::{borrow::ToOwned, string::String, vec, vec::Vec};
use core::fmt;

use crate::ast::Type;

/// The bit-oriented intrinsic operations in `ezra.bits`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BitsIntrinsic {
    RotateLeft,
    RotateRight,
    Test,
    Set,
    Clear,
    Toggle,
    Extract,
    Insert,
    ByteSwap,
    Reverse,
    CountOnes,
    LeadingZeros,
    TrailingZeros,
}

/// The integer-oriented intrinsic operations in `ezra.int`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum IntIntrinsic {
    WideningMul,
    MulHigh,
    SaturatingAdd,
    SaturatingSub,
    Divmod,
    AddCarry,
    SubBorrow,
    FullMul,
}

/// The memory-oriented intrinsic operations in `ezra.mem`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MemIntrinsic {
    CopyNonoverlapping,
    Move,
    Fill,
    FindByte,
    Compare,
    LoadLe16,
    LoadLe24,
    LoadBe16,
    LoadBe24,
    StoreLe16,
    StoreLe24,
    StoreBe16,
    StoreBe24,
    Peek8,
    Poke8,
}

/// The operation represented by one catalog entry.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum IntrinsicOperation {
    Bits(BitsIntrinsic),
    Int(IntIntrinsic),
    Mem(MemIntrinsic),
}

/// The number of scalar results an intrinsic produces.
///
/// The actual result types are kept as `Vec<Type>` on [`IntrinsicResolution`].
/// This is only descriptor metadata and does not create a tuple or aggregate
/// type.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ResultCount {
    Zero,
    One,
    Two,
}

impl ResultCount {
    pub const fn as_usize(self) -> usize {
        match self {
            Self::Zero => 0,
            Self::One => 1,
            Self::Two => 2,
        }
    }

    fn from_len(len: usize) -> Self {
        match len {
            0 => Self::Zero,
            1 => Self::One,
            2 => Self::Two,
            _ => panic!("intrinsic result lists may contain at most two values"),
        }
    }
}

/// Whether an intrinsic accesses memory.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MemoryEffect {
    None,
    Read,
    Write,
    ReadWrite,
}

impl MemoryEffect {
    pub const fn reads(self) -> bool {
        matches!(self, Self::Read | Self::ReadWrite)
    }

    pub const fn writes(self) -> bool {
        matches!(self, Self::Write | Self::ReadWrite)
    }
}

/// How an intrinsic treats volatile memory.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum VolatilePolicy {
    /// The intrinsic does not access memory.
    NotApplicable,
    /// The intrinsic is defined for ordinary, nonvolatile memory only.
    NonVolatileOnly,
    /// The intrinsic performs the stated scalar access without combining it
    /// with another access. This is the policy used by the existing byte
    /// peek/poke operations.
    PreservesAccess,
}

/// Overlap semantics for memory ranges used by an intrinsic.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OverlapRule {
    /// The operation has no source/destination ranges.
    NotApplicable,
    /// Source and destination ranges must not overlap.
    MustNotOverlap,
    /// Source and destination ranges may overlap, with memmove semantics.
    MayOverlap,
}

/// Memory effect metadata shared by all descriptors.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct IntrinsicEffects {
    pub memory: MemoryEffect,
    pub volatile: VolatilePolicy,
}

impl IntrinsicEffects {
    pub const PURE: Self = Self {
        memory: MemoryEffect::None,
        volatile: VolatilePolicy::NotApplicable,
    };

    pub const READ_MEMORY: Self = Self {
        memory: MemoryEffect::Read,
        volatile: VolatilePolicy::NonVolatileOnly,
    };

    pub const WRITE_MEMORY: Self = Self {
        memory: MemoryEffect::Write,
        volatile: VolatilePolicy::NonVolatileOnly,
    };

    pub const READ_WRITE_MEMORY: Self = Self {
        memory: MemoryEffect::ReadWrite,
        volatile: VolatilePolicy::NonVolatileOnly,
    };

    pub const SCALAR_BYTE_ACCESS: Self = Self {
        memory: MemoryEffect::Read,
        volatile: VolatilePolicy::PreservesAccess,
    };

    pub const SCALAR_BYTE_WRITE: Self = Self {
        memory: MemoryEffect::Write,
        volatile: VolatilePolicy::PreservesAccess,
    };
}

/// Constant arguments required by a descriptor.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ConstantRequirement {
    /// The argument is a bit index and must be in the value's width.
    BitIndex { argument: usize },
    /// The two arguments form an inclusive-exclusive bit range.
    BitRange {
        offset_argument: usize,
        width_argument: usize,
    },
}

/// A target-independent description of one intrinsic spelling and operation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct IntrinsicDescriptor {
    /// The spelling used in HIR/TBIR and lowering tables.
    pub canonical_name: &'static str,
    /// Accepted source spellings besides `canonical_name`.
    pub aliases: &'static [&'static str],
    pub operation: IntrinsicOperation,
    pub argument_count: usize,
    pub result_count: ResultCount,
    pub effects: IntrinsicEffects,
    pub overlap: OverlapRule,
    pub constant_requirements: &'static [ConstantRequirement],
}

impl IntrinsicDescriptor {
    pub fn accepts(&self, name: &str) -> bool {
        self.canonical_name == name || self.aliases.iter().any(|alias| *alias == name)
    }
}

/// A resolved argument supplied to the catalog.
///
/// `constant` is deliberately separate from the AST. The compiler can fill it
/// from a literal, a resolved constant, or a constant expression without this
/// module knowing how those values are represented. `None` means that the
/// argument is not known at compile time.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct IntrinsicArgument {
    pub ty: Type,
    pub constant: Option<i64>,
}

impl IntrinsicArgument {
    pub fn new(ty: Type) -> Self {
        Self { ty, constant: None }
    }

    pub fn with_constant(ty: Type, constant: i64) -> Self {
        Self {
            ty,
            constant: Some(constant),
        }
    }
}

/// The result of resolving an intrinsic call.
///
/// `result_types` has zero, one, or two entries. It is not a tuple type and is
/// safe to pass through compiler stages while multi-return syntax is evolving.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct IntrinsicResolution {
    pub descriptor: &'static IntrinsicDescriptor,
    pub argument_types: Vec<Type>,
    pub result_types: Vec<Type>,
}

impl IntrinsicResolution {
    pub fn canonical_name(&self) -> &'static str {
        self.descriptor.canonical_name
    }

    pub fn result_count(&self) -> ResultCount {
        ResultCount::from_len(self.result_types.len())
    }
}

/// The integer properties needed by the catalog.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct IntegerInfo {
    pub bits: u16,
    pub signed: bool,
}

/// Errors produced while looking up or validating an intrinsic.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum IntrinsicError {
    UnknownIntrinsic {
        name: String,
    },
    WrongArgumentCount {
        canonical_name: &'static str,
        expected: usize,
        actual: usize,
    },
    ArgumentType {
        canonical_name: &'static str,
        argument: usize,
        expected: &'static str,
        actual: Type,
    },
    MismatchedArguments {
        canonical_name: &'static str,
        first_argument: usize,
        second_argument: usize,
        first_type: Type,
        second_type: Type,
        expected: &'static str,
    },
    ConstantRequired {
        canonical_name: &'static str,
        argument: usize,
        requirement: &'static str,
    },
    ConstantOutOfRange {
        canonical_name: &'static str,
        argument: usize,
        value: i64,
        requirement: &'static str,
    },
    UnsupportedOperandWidths {
        canonical_name: &'static str,
        expected: &'static str,
        first_type: Type,
        second_type: Type,
    },
}

impl fmt::Display for IntrinsicError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnknownIntrinsic { name } => write!(f, "unknown intrinsic `{name}`"),
            Self::WrongArgumentCount {
                canonical_name,
                expected,
                actual,
            } => write!(
                f,
                "intrinsic `{canonical_name}` expects {expected} arguments, got {actual}"
            ),
            Self::ArgumentType {
                canonical_name,
                argument,
                expected,
                actual,
            } => write!(
                f,
                "intrinsic `{canonical_name}` argument {} must be {expected}, got {actual:?}",
                argument + 1
            ),
            Self::MismatchedArguments {
                canonical_name,
                first_argument,
                second_argument,
                first_type,
                second_type,
                expected,
            } => write!(
                f,
                "intrinsic `{canonical_name}` arguments {} and {} must {expected}; got {first_type:?} and {second_type:?}",
                first_argument + 1,
                second_argument + 1
            ),
            Self::ConstantRequired {
                canonical_name,
                argument,
                requirement,
            } => write!(
                f,
                "intrinsic `{canonical_name}` argument {} must be a compile-time {requirement}",
                argument + 1
            ),
            Self::ConstantOutOfRange {
                canonical_name,
                argument,
                value,
                requirement,
            } => write!(
                f,
                "intrinsic `{canonical_name}` argument {} has value {value}; {requirement}",
                argument + 1
            ),
            Self::UnsupportedOperandWidths {
                canonical_name,
                expected,
                first_type,
                second_type,
            } => write!(
                f,
                "intrinsic `{canonical_name}` requires {expected}; got {first_type:?} and {second_type:?}"
            ),
        }
    }
}

#[cfg(feature = "std")]
impl std::error::Error for IntrinsicError {}

const NO_CONSTANTS: &[ConstantRequirement] = &[];
const BIT_INDEX: &[ConstantRequirement] = &[ConstantRequirement::BitIndex { argument: 1 }];
const EXTRACT_RANGE: &[ConstantRequirement] = &[ConstantRequirement::BitRange {
    offset_argument: 1,
    width_argument: 2,
}];
const INSERT_RANGE: &[ConstantRequirement] = &[ConstantRequirement::BitRange {
    offset_argument: 2,
    width_argument: 3,
}];

const fn descriptor(
    canonical_name: &'static str,
    aliases: &'static [&'static str],
    operation: IntrinsicOperation,
    argument_count: usize,
    result_count: ResultCount,
    effects: IntrinsicEffects,
    overlap: OverlapRule,
    constant_requirements: &'static [ConstantRequirement],
) -> IntrinsicDescriptor {
    IntrinsicDescriptor {
        canonical_name,
        aliases,
        operation,
        argument_count,
        result_count,
        effects,
        overlap,
        constant_requirements,
    }
}

/// All canonical intrinsic descriptors.
pub static INTRINSIC_DESCRIPTORS: &[IntrinsicDescriptor] = &[
    descriptor(
        "ezra.bits.rotate_left",
        &["bits.rotate_left"],
        IntrinsicOperation::Bits(BitsIntrinsic::RotateLeft),
        2,
        ResultCount::One,
        IntrinsicEffects::PURE,
        OverlapRule::NotApplicable,
        NO_CONSTANTS,
    ),
    descriptor(
        "ezra.bits.rotate_right",
        &["bits.rotate_right"],
        IntrinsicOperation::Bits(BitsIntrinsic::RotateRight),
        2,
        ResultCount::One,
        IntrinsicEffects::PURE,
        OverlapRule::NotApplicable,
        NO_CONSTANTS,
    ),
    descriptor(
        "ezra.bits.test",
        &["bits.test"],
        IntrinsicOperation::Bits(BitsIntrinsic::Test),
        2,
        ResultCount::One,
        IntrinsicEffects::PURE,
        OverlapRule::NotApplicable,
        BIT_INDEX,
    ),
    descriptor(
        "ezra.bits.set",
        &["bits.set"],
        IntrinsicOperation::Bits(BitsIntrinsic::Set),
        2,
        ResultCount::One,
        IntrinsicEffects::PURE,
        OverlapRule::NotApplicable,
        BIT_INDEX,
    ),
    descriptor(
        "ezra.bits.clear",
        &["bits.clear"],
        IntrinsicOperation::Bits(BitsIntrinsic::Clear),
        2,
        ResultCount::One,
        IntrinsicEffects::PURE,
        OverlapRule::NotApplicable,
        BIT_INDEX,
    ),
    descriptor(
        "ezra.bits.toggle",
        &["bits.toggle"],
        IntrinsicOperation::Bits(BitsIntrinsic::Toggle),
        2,
        ResultCount::One,
        IntrinsicEffects::PURE,
        OverlapRule::NotApplicable,
        BIT_INDEX,
    ),
    descriptor(
        "ezra.bits.extract",
        &["bits.extract"],
        IntrinsicOperation::Bits(BitsIntrinsic::Extract),
        3,
        ResultCount::One,
        IntrinsicEffects::PURE,
        OverlapRule::NotApplicable,
        EXTRACT_RANGE,
    ),
    descriptor(
        "ezra.bits.insert",
        &["bits.insert"],
        IntrinsicOperation::Bits(BitsIntrinsic::Insert),
        4,
        ResultCount::One,
        IntrinsicEffects::PURE,
        OverlapRule::NotApplicable,
        INSERT_RANGE,
    ),
    descriptor(
        "ezra.bits.byte_swap",
        &["bits.byte_swap"],
        IntrinsicOperation::Bits(BitsIntrinsic::ByteSwap),
        1,
        ResultCount::One,
        IntrinsicEffects::PURE,
        OverlapRule::NotApplicable,
        NO_CONSTANTS,
    ),
    descriptor(
        "ezra.bits.reverse",
        &["bits.reverse"],
        IntrinsicOperation::Bits(BitsIntrinsic::Reverse),
        1,
        ResultCount::One,
        IntrinsicEffects::PURE,
        OverlapRule::NotApplicable,
        NO_CONSTANTS,
    ),
    descriptor(
        "ezra.bits.count_ones",
        &["bits.count_ones"],
        IntrinsicOperation::Bits(BitsIntrinsic::CountOnes),
        1,
        ResultCount::One,
        IntrinsicEffects::PURE,
        OverlapRule::NotApplicable,
        NO_CONSTANTS,
    ),
    descriptor(
        "ezra.bits.leading_zeros",
        &["bits.leading_zeros"],
        IntrinsicOperation::Bits(BitsIntrinsic::LeadingZeros),
        1,
        ResultCount::One,
        IntrinsicEffects::PURE,
        OverlapRule::NotApplicable,
        NO_CONSTANTS,
    ),
    descriptor(
        "ezra.bits.trailing_zeros",
        &["bits.trailing_zeros"],
        IntrinsicOperation::Bits(BitsIntrinsic::TrailingZeros),
        1,
        ResultCount::One,
        IntrinsicEffects::PURE,
        OverlapRule::NotApplicable,
        NO_CONSTANTS,
    ),
    descriptor(
        "ezra.int.widening_mul",
        &["int.widening_mul"],
        IntrinsicOperation::Int(IntIntrinsic::WideningMul),
        2,
        ResultCount::One,
        IntrinsicEffects::PURE,
        OverlapRule::NotApplicable,
        NO_CONSTANTS,
    ),
    descriptor(
        "ezra.int.mul_high",
        &["int.mul_high"],
        IntrinsicOperation::Int(IntIntrinsic::MulHigh),
        2,
        ResultCount::One,
        IntrinsicEffects::PURE,
        OverlapRule::NotApplicable,
        NO_CONSTANTS,
    ),
    descriptor(
        "ezra.int.saturating_add",
        &["int.saturating_add"],
        IntrinsicOperation::Int(IntIntrinsic::SaturatingAdd),
        2,
        ResultCount::One,
        IntrinsicEffects::PURE,
        OverlapRule::NotApplicable,
        NO_CONSTANTS,
    ),
    descriptor(
        "ezra.int.saturating_sub",
        &["int.saturating_sub"],
        IntrinsicOperation::Int(IntIntrinsic::SaturatingSub),
        2,
        ResultCount::One,
        IntrinsicEffects::PURE,
        OverlapRule::NotApplicable,
        NO_CONSTANTS,
    ),
    descriptor(
        "ezra.int.divmod",
        &["int.divmod"],
        IntrinsicOperation::Int(IntIntrinsic::Divmod),
        2,
        ResultCount::Two,
        IntrinsicEffects::PURE,
        OverlapRule::NotApplicable,
        NO_CONSTANTS,
    ),
    descriptor(
        "ezra.int.add_carry",
        &["int.add_carry"],
        IntrinsicOperation::Int(IntIntrinsic::AddCarry),
        3,
        ResultCount::Two,
        IntrinsicEffects::PURE,
        OverlapRule::NotApplicable,
        NO_CONSTANTS,
    ),
    descriptor(
        "ezra.int.sub_borrow",
        &["int.sub_borrow"],
        IntrinsicOperation::Int(IntIntrinsic::SubBorrow),
        3,
        ResultCount::Two,
        IntrinsicEffects::PURE,
        OverlapRule::NotApplicable,
        NO_CONSTANTS,
    ),
    descriptor(
        "ezra.int.full_mul",
        &["int.full_mul"],
        IntrinsicOperation::Int(IntIntrinsic::FullMul),
        2,
        ResultCount::Two,
        IntrinsicEffects::PURE,
        OverlapRule::NotApplicable,
        NO_CONSTANTS,
    ),
    descriptor(
        "ezra.mem.copy_nonoverlapping",
        &["mem.copy_nonoverlapping", "ezra.mem.memcpy", "mem.memcpy"],
        IntrinsicOperation::Mem(MemIntrinsic::CopyNonoverlapping),
        3,
        ResultCount::Zero,
        IntrinsicEffects::READ_WRITE_MEMORY,
        OverlapRule::MustNotOverlap,
        NO_CONSTANTS,
    ),
    descriptor(
        "ezra.mem.move",
        &["mem.move"],
        IntrinsicOperation::Mem(MemIntrinsic::Move),
        3,
        ResultCount::Zero,
        IntrinsicEffects::READ_WRITE_MEMORY,
        OverlapRule::MayOverlap,
        NO_CONSTANTS,
    ),
    descriptor(
        "ezra.mem.fill",
        &["mem.fill", "ezra.mem.memset", "mem.memset"],
        IntrinsicOperation::Mem(MemIntrinsic::Fill),
        3,
        ResultCount::Zero,
        IntrinsicEffects::WRITE_MEMORY,
        OverlapRule::NotApplicable,
        NO_CONSTANTS,
    ),
    descriptor(
        "ezra.mem.find_byte",
        &["mem.find_byte"],
        IntrinsicOperation::Mem(MemIntrinsic::FindByte),
        3,
        ResultCount::Two,
        IntrinsicEffects::READ_MEMORY,
        OverlapRule::NotApplicable,
        NO_CONSTANTS,
    ),
    descriptor(
        "ezra.mem.compare",
        &["mem.compare"],
        IntrinsicOperation::Mem(MemIntrinsic::Compare),
        3,
        ResultCount::One,
        IntrinsicEffects::READ_MEMORY,
        OverlapRule::NotApplicable,
        NO_CONSTANTS,
    ),
    descriptor(
        "ezra.mem.load_le16",
        &["mem.load_le16"],
        IntrinsicOperation::Mem(MemIntrinsic::LoadLe16),
        1,
        ResultCount::One,
        IntrinsicEffects::READ_MEMORY,
        OverlapRule::NotApplicable,
        NO_CONSTANTS,
    ),
    descriptor(
        "ezra.mem.load_le24",
        &["mem.load_le24"],
        IntrinsicOperation::Mem(MemIntrinsic::LoadLe24),
        1,
        ResultCount::One,
        IntrinsicEffects::READ_MEMORY,
        OverlapRule::NotApplicable,
        NO_CONSTANTS,
    ),
    descriptor(
        "ezra.mem.load_be16",
        &["mem.load_be16"],
        IntrinsicOperation::Mem(MemIntrinsic::LoadBe16),
        1,
        ResultCount::One,
        IntrinsicEffects::READ_MEMORY,
        OverlapRule::NotApplicable,
        NO_CONSTANTS,
    ),
    descriptor(
        "ezra.mem.load_be24",
        &["mem.load_be24"],
        IntrinsicOperation::Mem(MemIntrinsic::LoadBe24),
        1,
        ResultCount::One,
        IntrinsicEffects::READ_MEMORY,
        OverlapRule::NotApplicable,
        NO_CONSTANTS,
    ),
    descriptor(
        "ezra.mem.store_le16",
        &["mem.store_le16"],
        IntrinsicOperation::Mem(MemIntrinsic::StoreLe16),
        2,
        ResultCount::Zero,
        IntrinsicEffects::WRITE_MEMORY,
        OverlapRule::NotApplicable,
        NO_CONSTANTS,
    ),
    descriptor(
        "ezra.mem.store_le24",
        &["mem.store_le24"],
        IntrinsicOperation::Mem(MemIntrinsic::StoreLe24),
        2,
        ResultCount::Zero,
        IntrinsicEffects::WRITE_MEMORY,
        OverlapRule::NotApplicable,
        NO_CONSTANTS,
    ),
    descriptor(
        "ezra.mem.store_be16",
        &["mem.store_be16"],
        IntrinsicOperation::Mem(MemIntrinsic::StoreBe16),
        2,
        ResultCount::Zero,
        IntrinsicEffects::WRITE_MEMORY,
        OverlapRule::NotApplicable,
        NO_CONSTANTS,
    ),
    descriptor(
        "ezra.mem.store_be24",
        &["mem.store_be24"],
        IntrinsicOperation::Mem(MemIntrinsic::StoreBe24),
        2,
        ResultCount::Zero,
        IntrinsicEffects::WRITE_MEMORY,
        OverlapRule::NotApplicable,
        NO_CONSTANTS,
    ),
    descriptor(
        "ezra.mem.peek8",
        &["mem.peek8"],
        IntrinsicOperation::Mem(MemIntrinsic::Peek8),
        1,
        ResultCount::One,
        IntrinsicEffects::SCALAR_BYTE_ACCESS,
        OverlapRule::NotApplicable,
        NO_CONSTANTS,
    ),
    descriptor(
        "ezra.mem.poke8",
        &["mem.poke8"],
        IntrinsicOperation::Mem(MemIntrinsic::Poke8),
        2,
        ResultCount::Zero,
        IntrinsicEffects::SCALAR_BYTE_WRITE,
        OverlapRule::NotApplicable,
        NO_CONSTANTS,
    ),
];

/// The target-independent intrinsic catalog.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct IntrinsicCatalog;

impl IntrinsicCatalog {
    pub const fn new() -> Self {
        Self
    }

    pub fn descriptors(&self) -> &'static [IntrinsicDescriptor] {
        INTRINSIC_DESCRIPTORS
    }

    pub fn lookup(&self, name: &str) -> Option<&'static IntrinsicDescriptor> {
        self.descriptors()
            .iter()
            .find(|descriptor| descriptor.accepts(name))
    }

    pub fn canonical_name(&self, name: &str) -> Option<&'static str> {
        self.lookup(name)
            .map(|descriptor| descriptor.canonical_name)
    }

    /// Validate a call whose arguments include any known constant values.
    pub fn validate(
        &self,
        name: &str,
        arguments: &[IntrinsicArgument],
    ) -> Result<IntrinsicResolution, IntrinsicError> {
        self.resolve_with_mode(name, arguments, true)
    }

    /// Resolve a call and enforce constant-only bit indexes and ranges.
    pub fn resolve(
        &self,
        name: &str,
        arguments: &[IntrinsicArgument],
    ) -> Result<IntrinsicResolution, IntrinsicError> {
        self.resolve_with_mode(name, arguments, true)
    }

    fn resolve_with_mode(
        &self,
        name: &str,
        arguments: &[IntrinsicArgument],
        enforce_constants: bool,
    ) -> Result<IntrinsicResolution, IntrinsicError> {
        let descriptor = self
            .lookup(name)
            .ok_or_else(|| IntrinsicError::UnknownIntrinsic {
                name: name.to_owned(),
            })?;
        if arguments.len() != descriptor.argument_count {
            return Err(IntrinsicError::WrongArgumentCount {
                canonical_name: descriptor.canonical_name,
                expected: descriptor.argument_count,
                actual: arguments.len(),
            });
        }

        let result_types = validate_operation(descriptor, arguments, enforce_constants)?;
        debug_assert_eq!(result_types.len(), descriptor.result_count.as_usize());
        Ok(IntrinsicResolution {
            descriptor,
            argument_types: arguments
                .iter()
                .map(|argument| argument.ty.clone())
                .collect(),
            result_types,
        })
    }

    /// Validate only from already resolved types. Constant index/range checks
    /// are deferred because this form has no constant information.
    pub fn validate_types(
        &self,
        name: &str,
        argument_types: &[Type],
    ) -> Result<IntrinsicResolution, IntrinsicError> {
        let arguments = argument_types
            .iter()
            .cloned()
            .map(IntrinsicArgument::new)
            .collect::<Vec<_>>();
        self.resolve_with_mode(name, &arguments, false)
    }

    /// Validate resolved types and explicitly supplied constant values.
    pub fn validate_types_with_constants(
        &self,
        name: &str,
        argument_types: &[Type],
        constants: &[Option<i64>],
    ) -> Result<IntrinsicResolution, IntrinsicError> {
        if argument_types.len() != constants.len() {
            return Err(IntrinsicError::WrongArgumentCount {
                canonical_name: self
                    .lookup(name)
                    .map(|descriptor| descriptor.canonical_name)
                    .unwrap_or("<unknown>"),
                expected: argument_types.len(),
                actual: constants.len(),
            });
        }
        let arguments = argument_types
            .iter()
            .cloned()
            .zip(constants.iter().copied())
            .map(|(ty, constant)| IntrinsicArgument { ty, constant })
            .collect::<Vec<_>>();
        self.validate(name, &arguments)
    }

    /// Infer zero, one, or two result types without requiring AST result
    /// fields or an aggregate type.
    pub fn infer_result_types(
        &self,
        name: &str,
        argument_types: &[Type],
    ) -> Result<Vec<Type>, IntrinsicError> {
        Ok(self.validate_types(name, argument_types)?.result_types)
    }
}

/// The shared catalog instance.
pub const CATALOG: IntrinsicCatalog = IntrinsicCatalog;

pub fn lookup(name: &str) -> Option<&'static IntrinsicDescriptor> {
    CATALOG.lookup(name)
}

pub fn canonical_name(name: &str) -> Option<&'static str> {
    CATALOG.canonical_name(name)
}

pub fn validate_intrinsic(
    name: &str,
    arguments: &[IntrinsicArgument],
) -> Result<IntrinsicResolution, IntrinsicError> {
    CATALOG.validate(name, arguments)
}

pub fn infer_intrinsic_results(
    name: &str,
    argument_types: &[Type],
) -> Result<Vec<Type>, IntrinsicError> {
    CATALOG.infer_result_types(name, argument_types)
}

/// Return the width and signedness of a primitive integer type.
pub fn integer_info(ty: &Type) -> Option<IntegerInfo> {
    let Type::Named(name) = ty else {
        return None;
    };
    let (signed, bits) = match name.as_str() {
        "u8" => (false, 8),
        "i8" => (true, 8),
        "u16" => (false, 16),
        "i16" => (true, 16),
        "u20" => (false, 20),
        "i20" => (true, 20),
        "u24" => (false, 24),
        "i24" => (true, 24),
        "u32" => (false, 32),
        "i32" => (true, 32),
        _ => return None,
    };
    Some(IntegerInfo { bits, signed })
}

pub fn is_primitive_integer(ty: &Type) -> bool {
    integer_info(ty).is_some()
}

pub fn is_unsigned_integer(ty: &Type) -> bool {
    integer_info(ty).is_some_and(|info| !info.signed)
}

/// Whether a type is allowed as one of the language's scalar multi-results.
pub fn is_primitive_scalar(ty: &Type) -> bool {
    matches!(ty, Type::Named(name) if name == "bool" || name == "ptr")
        || integer_info(ty).is_some()
        || matches!(ty, Type::Ptr(_))
}

fn validate_operation(
    descriptor: &IntrinsicDescriptor,
    args: &[IntrinsicArgument],
    enforce_constants: bool,
) -> Result<Vec<Type>, IntrinsicError> {
    match descriptor.operation {
        IntrinsicOperation::Bits(operation) => {
            validate_bits(descriptor, operation, args, enforce_constants)
        }
        IntrinsicOperation::Int(operation) => validate_int(descriptor, operation, args),
        IntrinsicOperation::Mem(operation) => validate_mem(descriptor, operation, args),
    }
}

fn validate_bits(
    descriptor: &IntrinsicDescriptor,
    operation: BitsIntrinsic,
    args: &[IntrinsicArgument],
    enforce_constants: bool,
) -> Result<Vec<Type>, IntrinsicError> {
    match operation {
        BitsIntrinsic::RotateLeft | BitsIntrinsic::RotateRight => {
            let value = require_unsigned_bit_type(descriptor, args, 0)?;
            require_unsigned_integer(descriptor, args, 1, "an unsigned integer")?;
            Ok(vec![value])
        }
        BitsIntrinsic::Test | BitsIntrinsic::Set | BitsIntrinsic::Clear | BitsIntrinsic::Toggle => {
            let value = require_unsigned_bit_type(descriptor, args, 0)?;
            require_unsigned_integer(descriptor, args, 1, "an unsigned integer")?;
            let info = integer_info(&value).expect("unsigned bit type is an integer");
            require_bit_index(descriptor, args, 1, info.bits, enforce_constants)?;
            if matches!(operation, BitsIntrinsic::Test) {
                Ok(vec![named("bool")])
            } else {
                Ok(vec![value])
            }
        }
        BitsIntrinsic::Extract => {
            let value = require_unsigned_bit_type(descriptor, args, 0)?;
            require_unsigned_integer(descriptor, args, 1, "an unsigned integer")?;
            require_unsigned_integer(descriptor, args, 2, "an unsigned integer")?;
            let info = integer_info(&value).expect("unsigned bit type is an integer");
            require_bit_range(descriptor, args, 1, 2, info.bits, enforce_constants)?;
            Ok(vec![value])
        }
        BitsIntrinsic::Insert => {
            let base = require_unsigned_bit_type(descriptor, args, 0)?;
            let value = require_unsigned_bit_type(descriptor, args, 1)?;
            if base != value {
                return Err(IntrinsicError::MismatchedArguments {
                    canonical_name: descriptor.canonical_name,
                    first_argument: 0,
                    second_argument: 1,
                    first_type: base,
                    second_type: value,
                    expected: "have the same unsigned bit width",
                });
            }
            require_unsigned_integer(descriptor, args, 2, "an unsigned integer")?;
            require_unsigned_integer(descriptor, args, 3, "an unsigned integer")?;
            let info = integer_info(&base).expect("unsigned bit type is an integer");
            require_bit_range(descriptor, args, 2, 3, info.bits, enforce_constants)?;
            Ok(vec![base])
        }
        BitsIntrinsic::ByteSwap => {
            let value = require_unsigned_bit_type(descriptor, args, 0)?;
            if !matches!(integer_info(&value).map(|info| info.bits), Some(16 | 24)) {
                return Err(argument_type(descriptor, 0, "u16 or u24", &args[0].ty));
            }
            Ok(vec![value])
        }
        BitsIntrinsic::Reverse => {
            let value = require_unsigned_bit_type(descriptor, args, 0)?;
            Ok(vec![value])
        }
        BitsIntrinsic::CountOnes | BitsIntrinsic::LeadingZeros | BitsIntrinsic::TrailingZeros => {
            require_unsigned_bit_type(descriptor, args, 0)?;
            Ok(vec![named("u8")])
        }
    }
}

fn validate_int(
    descriptor: &IntrinsicDescriptor,
    operation: IntIntrinsic,
    args: &[IntrinsicArgument],
) -> Result<Vec<Type>, IntrinsicError> {
    match operation {
        IntIntrinsic::WideningMul => {
            let first = require_integer(descriptor, args, 0, "an integer")?;
            let second = require_integer(descriptor, args, 1, "an integer")?;
            if first.signed != second.signed {
                return Err(IntrinsicError::UnsupportedOperandWidths {
                    canonical_name: descriptor.canonical_name,
                    expected: "integers with matching signedness whose product fits u32/i32",
                    first_type: args[0].ty.clone(),
                    second_type: args[1].ty.clone(),
                });
            }
            let product_bits = first.bits.saturating_add(second.bits);
            let Some(result) = integer_type(product_bits, first.signed) else {
                return Err(IntrinsicError::UnsupportedOperandWidths {
                    canonical_name: descriptor.canonical_name,
                    expected: "a product width of 16, 24, or 32 bits",
                    first_type: args[0].ty.clone(),
                    second_type: args[1].ty.clone(),
                });
            };
            Ok(vec![result])
        }
        IntIntrinsic::MulHigh => {
            let first = require_integer(descriptor, args, 0, "an integer")?;
            let second = require_integer(descriptor, args, 1, "an integer")?;
            require_same_type(descriptor, args, 0, 1, "have the same exact integer type")?;
            if first.signed != second.signed {
                unreachable!("same exact type implies matching signedness");
            }
            Ok(vec![args[0].ty.clone()])
        }
        IntIntrinsic::SaturatingAdd | IntIntrinsic::SaturatingSub => {
            require_integer(descriptor, args, 0, "an integer")?;
            require_integer(descriptor, args, 1, "an integer")?;
            require_same_type(descriptor, args, 0, 1, "have the same exact integer type")?;
            Ok(vec![args[0].ty.clone()])
        }
        IntIntrinsic::Divmod => {
            require_integer(descriptor, args, 0, "an integer")?;
            require_integer(descriptor, args, 1, "an integer")?;
            require_same_type(descriptor, args, 0, 1, "have the same exact integer type")?;
            Ok(vec![args[0].ty.clone(), args[0].ty.clone()])
        }
        IntIntrinsic::AddCarry | IntIntrinsic::SubBorrow => {
            require_integer(descriptor, args, 0, "an integer")?;
            require_integer(descriptor, args, 1, "an integer")?;
            require_same_type(descriptor, args, 0, 1, "have the same exact integer type")?;
            require_type(descriptor, args, 2, "bool", &named("bool"))?;
            Ok(vec![args[0].ty.clone(), named("bool")])
        }
        IntIntrinsic::FullMul => {
            require_integer(descriptor, args, 0, "an integer")?;
            require_integer(descriptor, args, 1, "an integer")?;
            require_same_type(descriptor, args, 0, 1, "have the same exact integer type")?;
            Ok(vec![args[0].ty.clone(), args[0].ty.clone()])
        }
    }
}

fn validate_mem(
    descriptor: &IntrinsicDescriptor,
    operation: MemIntrinsic,
    args: &[IntrinsicArgument],
) -> Result<Vec<Type>, IntrinsicError> {
    let ptr = ptr_u8();
    match operation {
        MemIntrinsic::CopyNonoverlapping | MemIntrinsic::Move => {
            require_type(descriptor, args, 0, "ptr<u8>", &ptr)?;
            require_type(descriptor, args, 1, "ptr<u8>", &ptr)?;
            require_unsigned_integer(descriptor, args, 2, "an unsigned integer length")?;
            Ok(Vec::new())
        }
        MemIntrinsic::Fill => {
            require_type(descriptor, args, 0, "ptr<u8>", &ptr)?;
            require_type(descriptor, args, 1, "u8", &named("u8"))?;
            require_unsigned_integer(descriptor, args, 2, "an unsigned integer length")?;
            Ok(Vec::new())
        }
        MemIntrinsic::FindByte => {
            require_type(descriptor, args, 0, "ptr<u8>", &ptr)?;
            require_unsigned_integer(descriptor, args, 1, "an unsigned integer length")?;
            require_type(descriptor, args, 2, "u8", &named("u8"))?;
            Ok(vec![ptr, named("bool")])
        }
        MemIntrinsic::Compare => {
            require_type(descriptor, args, 0, "ptr<u8>", &ptr)?;
            require_type(descriptor, args, 1, "ptr<u8>", &ptr)?;
            require_unsigned_integer(descriptor, args, 2, "an unsigned integer length")?;
            Ok(vec![named("i8")])
        }
        MemIntrinsic::LoadLe16 | MemIntrinsic::LoadBe16 => {
            require_type(descriptor, args, 0, "ptr<u8>", &ptr)?;
            Ok(vec![named("u16")])
        }
        MemIntrinsic::LoadLe24 | MemIntrinsic::LoadBe24 => {
            require_type(descriptor, args, 0, "ptr<u8>", &ptr)?;
            Ok(vec![named("u24")])
        }
        MemIntrinsic::StoreLe16 | MemIntrinsic::StoreBe16 => {
            require_type(descriptor, args, 0, "ptr<u8>", &ptr)?;
            require_type(descriptor, args, 1, "u16", &named("u16"))?;
            Ok(Vec::new())
        }
        MemIntrinsic::StoreLe24 | MemIntrinsic::StoreBe24 => {
            require_type(descriptor, args, 0, "ptr<u8>", &ptr)?;
            require_type(descriptor, args, 1, "u24", &named("u24"))?;
            Ok(Vec::new())
        }
        MemIntrinsic::Peek8 => {
            require_type(descriptor, args, 0, "ptr<u8>", &ptr)?;
            Ok(vec![named("u8")])
        }
        MemIntrinsic::Poke8 => {
            require_type(descriptor, args, 0, "ptr<u8>", &ptr)?;
            require_type(descriptor, args, 1, "u8", &named("u8"))?;
            Ok(Vec::new())
        }
    }
}

fn require_integer(
    descriptor: &IntrinsicDescriptor,
    args: &[IntrinsicArgument],
    index: usize,
    expected: &'static str,
) -> Result<IntegerInfo, IntrinsicError> {
    integer_info(&args[index].ty)
        .ok_or_else(|| argument_type(descriptor, index, expected, &args[index].ty))
}

fn require_unsigned_integer(
    descriptor: &IntrinsicDescriptor,
    args: &[IntrinsicArgument],
    index: usize,
    expected: &'static str,
) -> Result<IntegerInfo, IntrinsicError> {
    let info = require_integer(descriptor, args, index, expected)?;
    if info.signed {
        return Err(argument_type(descriptor, index, expected, &args[index].ty));
    }
    Ok(info)
}

fn require_unsigned_bit_type(
    descriptor: &IntrinsicDescriptor,
    args: &[IntrinsicArgument],
    index: usize,
) -> Result<Type, IntrinsicError> {
    let Some(info) = integer_info(&args[index].ty) else {
        return Err(argument_type(
            descriptor,
            index,
            "u8, u16, or u24",
            &args[index].ty,
        ));
    };
    if info.signed || !matches!(info.bits, 8 | 16 | 24) {
        return Err(argument_type(
            descriptor,
            index,
            "u8, u16, or u24",
            &args[index].ty,
        ));
    }
    Ok(args[index].ty.clone())
}

fn require_type(
    descriptor: &IntrinsicDescriptor,
    args: &[IntrinsicArgument],
    index: usize,
    expected_name: &'static str,
    expected: &Type,
) -> Result<(), IntrinsicError> {
    if args[index].ty == *expected {
        Ok(())
    } else {
        Err(argument_type(
            descriptor,
            index,
            expected_name,
            &args[index].ty,
        ))
    }
}

fn require_same_type(
    descriptor: &IntrinsicDescriptor,
    args: &[IntrinsicArgument],
    first: usize,
    second: usize,
    expected: &'static str,
) -> Result<(), IntrinsicError> {
    if args[first].ty == args[second].ty {
        Ok(())
    } else {
        Err(IntrinsicError::MismatchedArguments {
            canonical_name: descriptor.canonical_name,
            first_argument: first,
            second_argument: second,
            first_type: args[first].ty.clone(),
            second_type: args[second].ty.clone(),
            expected,
        })
    }
}

fn argument_type(
    descriptor: &IntrinsicDescriptor,
    argument: usize,
    expected: &'static str,
    actual: &Type,
) -> IntrinsicError {
    IntrinsicError::ArgumentType {
        canonical_name: descriptor.canonical_name,
        argument,
        expected,
        actual: actual.clone(),
    }
}

fn require_bit_index(
    descriptor: &IntrinsicDescriptor,
    args: &[IntrinsicArgument],
    argument: usize,
    bits: u16,
    enforce_constants: bool,
) -> Result<(), IntrinsicError> {
    if !enforce_constants {
        return Ok(());
    }
    let Some(value) = args[argument].constant else {
        return Err(IntrinsicError::ConstantRequired {
            canonical_name: descriptor.canonical_name,
            argument,
            requirement: "constant bit index",
        });
    };
    if value < 0 || value >= i64::from(bits) {
        return Err(IntrinsicError::ConstantOutOfRange {
            canonical_name: descriptor.canonical_name,
            argument,
            value,
            requirement: "the bit index must be within the input width",
        });
    }
    Ok(())
}

fn require_bit_range(
    descriptor: &IntrinsicDescriptor,
    args: &[IntrinsicArgument],
    offset_argument: usize,
    width_argument: usize,
    bits: u16,
    enforce_constants: bool,
) -> Result<(), IntrinsicError> {
    if !enforce_constants {
        return Ok(());
    }
    let offset =
        args[offset_argument]
            .constant
            .ok_or_else(|| IntrinsicError::ConstantRequired {
                canonical_name: descriptor.canonical_name,
                argument: offset_argument,
                requirement: "constant bit-range offset",
            })?;
    let width = args[width_argument]
        .constant
        .ok_or_else(|| IntrinsicError::ConstantRequired {
            canonical_name: descriptor.canonical_name,
            argument: width_argument,
            requirement: "constant bit-range width",
        })?;
    let in_range = offset >= 0
        && width > 0
        && offset
            .checked_add(width)
            .is_some_and(|end| end <= i64::from(bits));
    if !in_range {
        return Err(IntrinsicError::ConstantOutOfRange {
            canonical_name: descriptor.canonical_name,
            argument: offset_argument,
            value: offset,
            requirement: "the offset and positive width must describe a range inside the input width",
        });
    }
    Ok(())
}

fn integer_type(bits: u16, signed: bool) -> Option<Type> {
    let name = match (bits, signed) {
        (8, false) => "u8",
        (8, true) => "i8",
        (16, false) => "u16",
        (16, true) => "i16",
        (20, false) => "u20",
        (20, true) => "i20",
        (24, false) => "u24",
        (24, true) => "i24",
        (32, false) => "u32",
        (32, true) => "i32",
        _ => return None,
    };
    Some(named(name))
}

fn named(name: &str) -> Type {
    Type::Named(name.to_owned())
}

fn ptr_u8() -> Type {
    Type::Ptr(alloc::boxed::Box::new(named("u8")))
}

#[cfg(test)]
mod tests;
