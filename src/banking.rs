//! Deterministic, target-neutral planning for fixed-size asset banks.
//!
//! A [`BankGeometry`] describes the visible bank window and available bank IDs;
//! it has no dependency on a compiler or target configuration.  Manual images
//! provide an already-used prefix for a bank, while sealed images are retained
//! verbatim and never receive automatically placed assets.

use alloc::{format, string::String, vec, vec::Vec};
use core::fmt;

/// An asset to place in a bank, in caller-provided order.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AssetCandidate {
    pub name: String,
    pub bytes: Vec<u8>,
    /// Required alignment of the asset's linked window address. Must be non-zero.
    pub align: usize,
}

impl AssetCandidate {
    pub fn new(name: impl Into<String>, bytes: Vec<u8>, align: usize) -> Self {
        Self {
            name: name.into(),
            bytes,
            align,
        }
    }
}

/// The target-independent geometry shared by all banks in a plan.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BankGeometry {
    /// A descriptive label used in diagnostics and output metadata.
    pub target_label: String,
    /// Address at which every selected bank is visible to the linker.
    pub window_address: u64,
    /// Number of bytes in each bank.
    pub capacity: usize,
    /// Bank IDs in deterministic allocation order.
    pub bank_ids: Vec<u32>,
    /// Byte used for unoccupied space and alignment padding.
    pub fill_byte: u8,
}

impl BankGeometry {
    pub fn new(
        target_label: impl Into<String>,
        window_address: u64,
        capacity: usize,
        bank_ids: Vec<u32>,
        fill_byte: u8,
    ) -> Self {
        Self {
            target_label: target_label.into(),
            window_address,
            capacity,
            bank_ids,
            fill_byte,
        }
    }
}

/// Existing bytes at the start of an unsealed bank.
///
/// The bytes are retained and treated as already allocated; candidates may be
/// appended after them if sufficient space remains.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ManualBankImage {
    pub bank_id: u32,
    pub bytes: Vec<u8>,
}

impl ManualBankImage {
    pub fn new(bank_id: u32, bytes: Vec<u8>) -> Self {
        Self { bank_id, bytes }
    }
}

/// Existing bytes at the start of a bank that the planner must not modify.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SealedBankImage {
    pub bank_id: u32,
    pub bytes: Vec<u8>,
}

impl SealedBankImage {
    pub fn new(bank_id: u32, bytes: Vec<u8>) -> Self {
        Self { bank_id, bytes }
    }
}

/// A caller-supplied bank image.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum BankImage {
    Manual(ManualBankImage),
    Sealed(SealedBankImage),
}

impl BankImage {
    pub fn bank_id(&self) -> u32 {
        match self {
            Self::Manual(image) => image.bank_id,
            Self::Sealed(image) => image.bank_id,
        }
    }

    pub fn bytes(&self) -> &[u8] {
        match self {
            Self::Manual(image) => &image.bytes,
            Self::Sealed(image) => &image.bytes,
        }
    }

    pub fn is_sealed(&self) -> bool {
        matches!(self, Self::Sealed(_))
    }
}

/// A linkable location, either in ordinary resident memory or through a bank window.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LinkedLocation {
    Resident {
        address: u64,
    },
    Banked {
        bank_id: u32,
        /// Address in the bank window at which the asset is visible.
        address: u64,
        /// Byte offset from [`BankGeometry::window_address`].
        offset: usize,
    },
}

/// The location assigned to an asset candidate.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AssetPlacement {
    pub name: String,
    pub size: usize,
    pub location: LinkedLocation,
}

/// A fully materialized bank in the output plan.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PlannedBank {
    pub bank_id: u32,
    /// Always exactly [`BankGeometry::capacity`] bytes long.
    pub bytes: Vec<u8>,
    /// The first free byte after manual data, padding, and planned assets.
    pub used: usize,
    pub sealed: bool,
}

/// The deterministic output of [`plan_asset_banks`].
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BankPlan {
    pub geometry: BankGeometry,
    pub banks: Vec<PlannedBank>,
    pub placements: Vec<AssetPlacement>,
}

/// A diagnostic produced while validating or packing a bank plan.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum BankPlanError {
    InvalidGeometry {
        message: String,
    },
    InvalidAlignment {
        asset: String,
        align: usize,
    },
    UnknownBankImage {
        bank_id: u32,
    },
    DuplicateBankImage {
        bank_id: u32,
    },
    BankImageTooLarge {
        bank_id: u32,
        size: usize,
        capacity: usize,
    },
    AssetTooLarge {
        asset: String,
        size: usize,
        capacity: usize,
    },
    CapacityExhausted {
        asset: String,
        size: usize,
    },
}

impl fmt::Display for BankPlanError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidGeometry { message } => {
                write!(formatter, "invalid bank geometry: {message}")
            }
            Self::InvalidAlignment { asset, align } => write!(
                formatter,
                "asset `{asset}` has invalid alignment {align}; alignment must be non-zero"
            ),
            Self::UnknownBankImage { bank_id } => {
                write!(
                    formatter,
                    "bank image references unavailable bank ID {bank_id}"
                )
            }
            Self::DuplicateBankImage { bank_id } => {
                write!(
                    formatter,
                    "more than one bank image was supplied for bank ID {bank_id}"
                )
            }
            Self::BankImageTooLarge {
                bank_id,
                size,
                capacity,
            } => write!(
                formatter,
                "bank image for bank ID {bank_id} is {size} bytes, exceeding its {capacity}-byte capacity"
            ),
            Self::AssetTooLarge {
                asset,
                size,
                capacity,
            } => write!(
                formatter,
                "asset `{asset}` is {size} bytes and cannot fit in an empty {capacity}-byte bank with its alignment"
            ),
            Self::CapacityExhausted { asset, size } => write!(
                formatter,
                "insufficient unsealed bank capacity for asset `{asset}` ({size} bytes)"
            ),
        }
    }
}

#[cfg(feature = "std")]
impl std::error::Error for BankPlanError {}

/// Pack assets into unsealed banks in bank-ID order, preserving caller order.
///
/// Manual images are immutable prefixes but remain eligible for appending.
/// Sealed images are immutable and skipped entirely. Alignment applies to the
/// linked window address, so a non-aligned window base is handled correctly.
pub fn plan_asset_banks(
    geometry: BankGeometry,
    supplied_images: &[BankImage],
    candidates: &[AssetCandidate],
) -> Result<BankPlan, BankPlanError> {
    validate_geometry(&geometry)?;

    let mut banks = geometry
        .bank_ids
        .iter()
        .copied()
        .map(|bank_id| PlannedBank {
            bank_id,
            bytes: vec![geometry.fill_byte; geometry.capacity],
            used: 0,
            sealed: false,
        })
        .collect::<Vec<_>>();

    for (image_index, image) in supplied_images.iter().enumerate() {
        let bank_id = image.bank_id();
        if supplied_images[..image_index]
            .iter()
            .any(|earlier| earlier.bank_id() == bank_id)
        {
            return Err(BankPlanError::DuplicateBankImage { bank_id });
        }
        let Some(index) = banks.iter().position(|bank| bank.bank_id == bank_id) else {
            return Err(BankPlanError::UnknownBankImage { bank_id });
        };
        let bank = &mut banks[index];
        if image.bytes().len() > geometry.capacity {
            return Err(BankPlanError::BankImageTooLarge {
                bank_id,
                size: image.bytes().len(),
                capacity: geometry.capacity,
            });
        }

        bank.bytes[..image.bytes().len()].copy_from_slice(image.bytes());
        bank.used = image.bytes().len();
        bank.sealed = image.is_sealed();
    }

    let mut placements = Vec::with_capacity(candidates.len());
    for candidate in candidates {
        if candidate.align == 0 {
            return Err(BankPlanError::InvalidAlignment {
                asset: candidate.name.clone(),
                align: candidate.align,
            });
        }

        let first_offset = aligned_offset(geometry.window_address, 0, candidate.align)?;
        if first_offset
            .checked_add(candidate.bytes.len())
            .is_none_or(|end| end > geometry.capacity)
        {
            return Err(BankPlanError::AssetTooLarge {
                asset: candidate.name.clone(),
                size: candidate.bytes.len(),
                capacity: geometry.capacity,
            });
        }

        let mut placed = None;
        for bank in &mut banks {
            if bank.sealed {
                continue;
            }
            let offset = aligned_offset(geometry.window_address, bank.used, candidate.align)?;
            let Some(end) = offset.checked_add(candidate.bytes.len()) else {
                continue;
            };
            if end > geometry.capacity {
                continue;
            }

            bank.bytes[offset..end].copy_from_slice(&candidate.bytes);
            bank.used = end;
            placed = Some(LinkedLocation::Banked {
                bank_id: bank.bank_id,
                address: geometry
                    .window_address
                    .checked_add(offset as u64)
                    .ok_or_else(|| BankPlanError::InvalidGeometry {
                        message: format!("window address overflows when offset by {offset} bytes"),
                    })?,
                offset,
            });
            break;
        }

        let Some(location) = placed else {
            return Err(BankPlanError::CapacityExhausted {
                asset: candidate.name.clone(),
                size: candidate.bytes.len(),
            });
        };
        placements.push(AssetPlacement {
            name: candidate.name.clone(),
            size: candidate.bytes.len(),
            location,
        });
    }

    Ok(BankPlan {
        geometry,
        banks,
        placements,
    })
}

fn validate_geometry(geometry: &BankGeometry) -> Result<(), BankPlanError> {
    if geometry.capacity == 0 {
        return Err(BankPlanError::InvalidGeometry {
            message: "bank capacity must be greater than zero".into(),
        });
    }
    if geometry.bank_ids.is_empty() {
        return Err(BankPlanError::InvalidGeometry {
            message: "at least one bank ID is required".into(),
        });
    }
    for (index, bank_id) in geometry.bank_ids.iter().enumerate() {
        if geometry.bank_ids[..index].contains(bank_id) {
            return Err(BankPlanError::InvalidGeometry {
                message: format!("bank ID {bank_id} appears more than once"),
            });
        }
    }
    let Some(last_offset) = geometry.capacity.checked_sub(1) else {
        unreachable!("zero capacity is rejected above");
    };
    geometry
        .window_address
        .checked_add(last_offset as u64)
        .ok_or_else(|| BankPlanError::InvalidGeometry {
            message: "bank window exceeds the supported address range".into(),
        })?;
    Ok(())
}

fn aligned_offset(window_address: u64, used: usize, align: usize) -> Result<usize, BankPlanError> {
    let address =
        window_address
            .checked_add(used as u64)
            .ok_or_else(|| BankPlanError::InvalidGeometry {
                message: format!("window address overflows when offset by {used} bytes"),
            })?;
    let remainder = address % align as u64;
    let padding = if remainder == 0 {
        0
    } else {
        align as u64 - remainder
    };
    used.checked_add(padding as usize)
        .ok_or_else(|| BankPlanError::InvalidGeometry {
            message: "aligned bank offset exceeds the supported size range".into(),
        })
}

#[cfg(test)]
mod tests {
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
}
