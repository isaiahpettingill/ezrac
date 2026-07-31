//! Target-independent byte liveness and byte-aligned helpers for TBIR.
//!
//! This module contains facts and pure helpers only. It does not rewrite expressions or memory
//! accesses. In particular, callers must keep volatile accesses and their required access widths
//! intact when using these facts.

/// The largest value represented by this module is a 24-bit value.
pub const MAX_BYTES: u8 = 3;

const MAX_BYTE_MASK: u8 = (1u8 << MAX_BYTES) - 1;

/// A supported TBIR scalar width whose storage is one to three bytes.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct ByteWidth(u8);

impl ByteWidth {
    pub const W8: Self = Self(8);
    pub const W16: Self = Self(16);
    pub const W24: Self = Self(24);

    /// Creates a width for an 8-, 16-, or 24-bit value.
    pub const fn new(bits: u8) -> Option<Self> {
        match bits {
            8 | 16 | 24 => Some(Self(bits)),
            _ => None,
        }
    }

    /// Creates a width from a one-, two-, or three-byte storage size.
    pub const fn from_bytes(bytes: u8) -> Option<Self> {
        match bytes {
            1 => Some(Self::W8),
            2 => Some(Self::W16),
            3 => Some(Self::W24),
            _ => None,
        }
    }

    pub const fn bits(self) -> u8 {
        self.0
    }

    pub const fn bytes(self) -> u8 {
        self.0 / 8
    }

    pub const fn byte_mask(self) -> ByteMask {
        ByteMask((1u8 << self.bytes()) - 1)
    }

    pub const fn value_mask(self) -> u32 {
        (1u32 << self.bits()) - 1
    }
}

/// A mask with one bit per byte, starting at the least-significant byte.
///
/// Bit zero names byte zero. Only the three byte positions supported by [`ByteWidth`] can be set.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct ByteMask(u8);

impl ByteMask {
    pub const EMPTY: Self = Self(0);
    pub const BYTE0: Self = Self(0b001);
    pub const BYTE1: Self = Self(0b010);
    pub const BYTE2: Self = Self(0b100);
    pub const ALL: Self = Self(MAX_BYTE_MASK);

    /// Creates a mask, rejecting bits for byte positions outside 8/16/24-bit values.
    pub const fn from_bits(bits: u8) -> Option<Self> {
        if bits & !MAX_BYTE_MASK == 0 {
            Some(Self(bits))
        } else {
            None
        }
    }

    pub const fn bits(self) -> u8 {
        self.0
    }

    pub const fn for_byte(byte: u8) -> Option<Self> {
        if byte < MAX_BYTES {
            Some(Self(1u8 << byte))
        } else {
            None
        }
    }

    pub const fn for_range(range: ByteRange) -> Self {
        Self(range.mask())
    }

    pub const fn is_empty(self) -> bool {
        self.0 == 0
    }

    pub const fn contains(self, byte: u8) -> bool {
        byte < MAX_BYTES && self.0 & (1u8 << byte) != 0
    }

    pub const fn is_subset_of(self, other: Self) -> bool {
        self.0 & !other.0 == 0
    }

    pub const fn intersects(self, other: Self) -> bool {
        self.0 & other.0 != 0
    }

    pub const fn union(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }

    pub const fn intersection(self, other: Self) -> Self {
        Self(self.0 & other.0)
    }

    pub const fn difference(self, other: Self) -> Self {
        Self(self.0 & !other.0)
    }

    pub const fn is_valid_for(self, width: ByteWidth) -> bool {
        self.is_subset_of(width.byte_mask())
    }

    /// Shifts byte positions and clips positions that leave `width`.
    ///
    /// A positive offset moves bytes toward more-significant byte positions. A negative offset
    /// moves them toward less-significant positions. The operation is mask propagation, so bytes
    /// shifted outside the value are dropped rather than causing an invalid integer shift.
    pub const fn shifted(self, offset: i8, width: ByteWidth) -> Option<Self> {
        if !self.is_valid_for(width) {
            return None;
        }

        let shifted = if offset >= 0 {
            let amount = offset as u8;
            if amount >= MAX_BYTES {
                0
            } else {
                self.0 << amount
            }
        } else {
            let amount = (-(offset as i16)) as u8;
            if amount >= MAX_BYTES {
                0
            } else {
                self.0 >> amount
            }
        };
        Some(Self(shifted & width.byte_mask().bits()))
    }
}

/// A half-open range of byte positions: `[start, end)`.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct ByteRange {
    start: u8,
    end: u8,
}

impl ByteRange {
    /// Creates a range with `start <= end`, where both positions are at most `MAX_BYTES`.
    pub const fn new(start: u8, end: u8) -> Option<Self> {
        if start <= end && end <= MAX_BYTES {
            Some(Self { start, end })
        } else {
            None
        }
    }

    pub const fn from_start_len(start: u8, len: u8) -> Option<Self> {
        if start <= MAX_BYTES && len <= MAX_BYTES - start {
            Some(Self {
                start,
                end: start + len,
            })
        } else {
            None
        }
    }

    pub const fn start(self) -> u8 {
        self.start
    }

    pub const fn end(self) -> u8 {
        self.end
    }

    pub const fn len(self) -> u8 {
        self.end - self.start
    }

    pub const fn is_empty(self) -> bool {
        self.start == self.end
    }

    pub const fn contains(self, byte: u8) -> bool {
        self.start <= byte && byte < self.end
    }

    pub const fn within(self, width: ByteWidth) -> bool {
        self.end <= width.bytes()
    }

    pub const fn mask(self) -> u8 {
        if self.is_empty() {
            0
        } else {
            ((1u8 << self.len()) - 1) << self.start
        }
    }

    pub const fn value_mask(self) -> u32 {
        if self.is_empty() {
            0
        } else {
            (1u32 << (self.len() as u32 * 8)) - 1
        }
    }
}

/// Returns the mask for one byte position.
pub const fn byte_mask(byte: u8) -> Option<ByteMask> {
    ByteMask::for_byte(byte)
}

/// Returns the mask for the half-open byte range `[start, start + len)`.
pub const fn byte_range_mask(start: u8, len: u8) -> Option<ByteMask> {
    match ByteRange::from_start_len(start, len) {
        Some(range) => Some(ByteMask::for_range(range)),
        None => None,
    }
}

/// Returns a validated half-open byte range.
pub const fn byte_range(start: u8, end: u8) -> Option<ByteRange> {
    ByteRange::new(start, end)
}

/// Shifts a byte mask by a signed byte offset and clips it to `width`.
pub const fn shift_mask(mask: ByteMask, offset: i8, width: ByteWidth) -> Option<ByteMask> {
    mask.shifted(offset, width)
}

/// Shifts a value left by a whole number of bytes, masking it to `width`.
pub const fn shift_left_bytes(value: u32, bytes: u8, width: ByteWidth) -> Option<u32> {
    if bytes > width.bytes() {
        return None;
    }
    let shift = bytes as u32 * 8;
    let shifted = match value.checked_shl(shift) {
        Some(value) => value,
        None => 0,
    };
    Some(shifted & width.value_mask())
}

/// Shifts a value right by a whole number of bytes, masking it to `width`.
pub const fn shift_right_bytes(value: u32, bytes: u8, width: ByteWidth) -> Option<u32> {
    if bytes > width.bytes() {
        return None;
    }
    Some((value & width.value_mask()) >> (bytes as u32 * 8))
}

/// Extracts bytes in `range` and packs them into the low bytes of the result.
pub const fn extract_bytes(value: u32, range: ByteRange, width: ByteWidth) -> Option<u32> {
    if !range.within(width) {
        return None;
    }
    Some((value >> (range.start() as u32 * 8)) & range.value_mask())
}

/// Inserts the low bytes of `value` into `range`, clearing the rest of that range first.
pub const fn insert_bytes(
    base: u32,
    value: u32,
    range: ByteRange,
    width: ByteWidth,
) -> Option<u32> {
    if !range.within(width) {
        return None;
    }
    let shift = range.start() as u32 * 8;
    let range_mask = range.value_mask() << shift;
    Some(((base & !range_mask) | ((value & range.value_mask()) << shift)) & width.value_mask())
}

/// Extracts a byte mask into the low byte positions of the result.
pub const fn extract_mask(mask: ByteMask, range: ByteRange) -> Option<ByteMask> {
    Some(ByteMask((mask.bits() & range.mask()) >> range.start()))
}

/// Inserts the low byte positions of `value` into a byte mask range.
pub const fn insert_mask(base: ByteMask, value: ByteMask, range: ByteRange) -> Option<ByteMask> {
    let range_mask = range.mask();
    let value_mask = if range.is_empty() {
        0
    } else {
        (1u8 << range.len()) - 1
    };
    Some(ByteMask(
        (base.bits() & !range_mask) | ((value.bits() & value_mask) << range.start()),
    ))
}

/// Per-byte liveness and known-byte facts for an 8-, 16-, or 24-bit value.
///
/// The masks are independent. A byte can be live while its value is known, and known-zero and
/// known-sign-extension facts may overlap when the sign byte is known to be zero.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ByteLiveness {
    width: ByteWidth,
    live: ByteMask,
    known_zero: ByteMask,
    known_sign_extension: ByteMask,
}

impl ByteLiveness {
    /// Creates an empty fact set for `width`.
    pub const fn new(width: ByteWidth) -> Self {
        Self {
            width,
            live: ByteMask::EMPTY,
            known_zero: ByteMask::EMPTY,
            known_sign_extension: ByteMask::EMPTY,
        }
    }

    /// Creates a fact set after checking that every mask fits the value width.
    pub const fn from_masks(
        width: ByteWidth,
        live: ByteMask,
        known_zero: ByteMask,
        known_sign_extension: ByteMask,
    ) -> Option<Self> {
        if live.is_valid_for(width)
            && known_zero.is_valid_for(width)
            && known_sign_extension.is_valid_for(width)
        {
            Some(Self {
                width,
                live,
                known_zero,
                known_sign_extension,
            })
        } else {
            None
        }
    }

    pub const fn all_live(width: ByteWidth) -> Self {
        Self {
            width,
            live: width.byte_mask(),
            known_zero: ByteMask::EMPTY,
            known_sign_extension: ByteMask::EMPTY,
        }
    }

    pub const fn width(self) -> ByteWidth {
        self.width
    }

    pub const fn live_bytes(self) -> ByteMask {
        self.live
    }

    pub const fn known_zero_bytes(self) -> ByteMask {
        self.known_zero
    }

    pub const fn known_sign_extension_bytes(self) -> ByteMask {
        self.known_sign_extension
    }

    pub const fn is_live(self, byte: u8) -> bool {
        self.live.contains(byte)
    }

    pub const fn is_known_zero(self, byte: u8) -> bool {
        self.known_zero.contains(byte)
    }

    pub const fn is_known_sign_extension(self, byte: u8) -> bool {
        self.known_sign_extension.contains(byte)
    }

    pub const fn with_live_bytes(self, live: ByteMask) -> Option<Self> {
        Self::from_masks(self.width, live, self.known_zero, self.known_sign_extension)
    }

    pub const fn with_known_zero_bytes(self, known_zero: ByteMask) -> Option<Self> {
        Self::from_masks(self.width, self.live, known_zero, self.known_sign_extension)
    }

    pub const fn with_known_sign_extension_bytes(
        self,
        known_sign_extension: ByteMask,
    ) -> Option<Self> {
        Self::from_masks(self.width, self.live, self.known_zero, known_sign_extension)
    }

    pub const fn add_live(self, bytes: ByteMask) -> Option<Self> {
        self.with_live_bytes(self.live.union(bytes))
    }

    pub const fn add_known_zero(self, bytes: ByteMask) -> Option<Self> {
        self.with_known_zero_bytes(self.known_zero.union(bytes))
    }

    pub const fn add_known_sign_extension(self, bytes: ByteMask) -> Option<Self> {
        self.with_known_sign_extension_bytes(self.known_sign_extension.union(bytes))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_widths_and_masks() {
        assert_eq!(ByteWidth::new(8), Some(ByteWidth::W8));
        assert_eq!(ByteWidth::new(16), Some(ByteWidth::W16));
        assert_eq!(ByteWidth::new(24), Some(ByteWidth::W24));
        assert_eq!(ByteWidth::new(32), None);
        assert_eq!(ByteWidth::W8.byte_mask().bits(), 0b001);
        assert_eq!(ByteWidth::W16.byte_mask().bits(), 0b011);
        assert_eq!(ByteWidth::W24.byte_mask().bits(), 0b111);
        assert_eq!(ByteWidth::W24.value_mask(), 0x00FF_FFFF);
        assert_eq!(ByteMask::from_bits(0b1000), None);
        assert_eq!(byte_mask(2).unwrap(), ByteMask::BYTE2);
        assert_eq!(byte_mask(3), None);
    }

    #[test]
    fn creates_safe_half_open_ranges_and_masks() {
        let middle = byte_range(1, 3).unwrap();
        assert_eq!(middle.start(), 1);
        assert_eq!(middle.end(), 3);
        assert_eq!(middle.len(), 2);
        assert_eq!(middle.mask(), 0b110);
        assert_eq!(byte_range_mask(1, 2).unwrap().bits(), 0b110);
        assert_eq!(byte_range_mask(3, 0).unwrap(), ByteMask::EMPTY);
        assert!(byte_range(3, 2).is_none());
        assert!(byte_range_mask(2, 2).is_none());
        assert!(ByteRange::from_start_len(4, 0).is_none());
    }

    #[test]
    fn keeps_live_zero_and_sign_extension_facts_independent() {
        let facts = ByteLiveness::from_masks(
            ByteWidth::W24,
            ByteMask::for_range(byte_range(0, 1).unwrap()),
            ByteMask::for_range(byte_range(1, 2).unwrap()),
            ByteMask::for_range(byte_range(2, 3).unwrap()),
        )
        .unwrap();

        assert_eq!(facts.live_bytes().bits(), 0b001);
        assert_eq!(facts.known_zero_bytes().bits(), 0b010);
        assert_eq!(facts.known_sign_extension_bytes().bits(), 0b100);
        assert!(facts.is_live(0));
        assert!(!facts.is_live(1));
        assert!(facts.is_known_zero(1));
        assert!(facts.is_known_sign_extension(2));
        assert!(
            ByteLiveness::from_masks(
                ByteWidth::W8,
                ByteMask::BYTE0,
                ByteMask::BYTE1,
                ByteMask::EMPTY,
            )
            .is_none()
        );
    }

    #[test]
    fn shifts_byte_masks_without_invalid_shift_counts() {
        let mask = ByteMask::BYTE0.union(ByteMask::BYTE1);
        assert_eq!(shift_mask(mask, 1, ByteWidth::W24).unwrap().bits(), 0b110);
        assert_eq!(shift_mask(mask, -1, ByteWidth::W24).unwrap().bits(), 0b001);
        assert_eq!(
            shift_mask(ByteMask::BYTE2, 1, ByteWidth::W24).unwrap(),
            ByteMask::EMPTY
        );
        assert_eq!(
            shift_mask(ByteMask::BYTE0, i8::MIN, ByteWidth::W24).unwrap(),
            ByteMask::EMPTY
        );
        assert_eq!(shift_mask(ByteMask::BYTE1, 0, ByteWidth::W8), None);

        assert_eq!(
            shift_left_bytes(0x12_3456, 1, ByteWidth::W24),
            Some(0x34_5600)
        );
        assert_eq!(
            shift_right_bytes(0x12_3456, 1, ByteWidth::W24),
            Some(0x0012_34)
        );
        assert_eq!(shift_left_bytes(0x12, 2, ByteWidth::W8), None);
    }

    #[test]
    fn extracts_and_inserts_byte_aligned_values_and_masks() {
        let middle = byte_range(1, 2).unwrap();
        assert_eq!(extract_bytes(0x12_3456, middle, ByteWidth::W24), Some(0x34));
        assert_eq!(
            insert_bytes(0x12_0056, 0xAB, middle, ByteWidth::W24),
            Some(0x12_AB_56)
        );
        assert_eq!(
            insert_bytes(0xFF_0000, 0x1234, middle, ByteWidth::W24),
            Some(0xFF_3400)
        );
        assert_eq!(
            extract_bytes(0x12_3456, byte_range(2, 3).unwrap(), ByteWidth::W16),
            None
        );

        let source = ByteMask::BYTE0.union(ByteMask::BYTE2);
        assert_eq!(extract_mask(source, middle).unwrap(), ByteMask::EMPTY);
        assert_eq!(
            insert_mask(ByteMask::BYTE2, ByteMask::BYTE0, middle)
                .unwrap()
                .bits(),
            0b110
        );
    }
}
