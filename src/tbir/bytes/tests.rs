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
