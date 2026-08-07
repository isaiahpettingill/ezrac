use super::*;

fn geometry() -> BankGeometry {
    BankGeometry::new("test", 0x8001, 16, vec![2, 5], 0xFF)
}

#[test]
fn aligns_assets_against_the_linked_window_address() {
    let plan = plan_asset_banks(
        geometry(),
        &[],
        &[AssetCandidate::new("sprite", vec![1, 2], 4)],
    )
    .unwrap();

    assert_eq!(
        plan.placements[0].location,
        LinkedLocation::Banked {
            bank_id: 2,
            address: 0x8004,
            offset: 3,
        }
    );
    assert_eq!(&plan.banks[0].bytes[..5], &[0xFF, 0xFF, 0xFF, 1, 2]);
}

#[test]
fn placements_are_deterministic_and_follow_input_order() {
    let plan = plan_asset_banks(
        geometry(),
        &[],
        &[
            AssetCandidate::new("first", vec![1; 8], 1),
            AssetCandidate::new("second", vec![2; 8], 1),
            AssetCandidate::new("third", vec![3], 1),
        ],
    )
    .unwrap();

    assert_eq!(
        plan.placements
            .iter()
            .map(|placement| (&placement.name, placement.location))
            .collect::<Vec<_>>(),
        vec![
            (
                &"first".to_owned(),
                LinkedLocation::Banked {
                    bank_id: 2,
                    address: 0x8001,
                    offset: 0,
                },
            ),
            (
                &"second".to_owned(),
                LinkedLocation::Banked {
                    bank_id: 2,
                    address: 0x8009,
                    offset: 8,
                },
            ),
            (
                &"third".to_owned(),
                LinkedLocation::Banked {
                    bank_id: 5,
                    address: 0x8001,
                    offset: 0,
                },
            ),
        ]
    );
}

#[test]
fn skips_sealed_banks_and_preserves_their_contents() {
    let plan = plan_asset_banks(
        geometry(),
        &[BankImage::Sealed(SealedBankImage::new(2, vec![0xAA, 0xBB]))],
        &[AssetCandidate::new("asset", vec![1, 2], 1)],
    )
    .unwrap();

    assert_eq!(plan.banks[0].bytes[..2], [0xAA, 0xBB]);
    assert!(plan.banks[0].sealed);
    assert_eq!(
        plan.placements[0].location,
        LinkedLocation::Banked {
            bank_id: 5,
            address: 0x8001,
            offset: 0,
        }
    );
}

#[test]
fn reports_overflow_after_available_capacity_is_exhausted() {
    let error = plan_asset_banks(
        BankGeometry::new("test", 0, 4, vec![1], 0),
        &[],
        &[
            AssetCandidate::new("fits", vec![1, 2, 3, 4], 1),
            AssetCandidate::new("overflow", vec![5], 1),
        ],
    )
    .unwrap_err();

    assert_eq!(
        error,
        BankPlanError::CapacityExhausted {
            asset: "overflow".into(),
            size: 1,
        }
    );
    assert_eq!(
        error.to_string(),
        "insufficient unsealed bank capacity for asset `overflow` (1 bytes)"
    );
}
