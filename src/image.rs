//! Shared indexed-image encoders for the native pixel layouts used by EZRA targets.

use alloc::{format, string::String, vec, vec::Vec};
use core::fmt;

/// The role an image has on its target.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ImageKind {
    /// An image made from fixed-size hardware or character tiles.
    Tiles,
    /// An image used as a hardware or software sprite.
    Sprite,
    /// A linear bitmap or framebuffer image.
    Bitmap,
}

impl ImageKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Tiles => "tiles",
            Self::Sprite => "sprite",
            Self::Bitmap => "bitmap",
        }
    }
}

/// Native byte layouts supported by the shared image encoder.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NativeImageFormat {
    /// Game Boy 2bpp tiles: low and high planes interleaved for each row.
    GameBoy2Bpp,
    /// NES 2bpp tiles: the complete low plane followed by the high plane.
    Nes2Bpp,
    /// Sega SMS/Game Gear 4bpp tiles: four plane bytes interleaved for each row.
    Sms4Bpp,
    /// 1bpp 8x8 tiles with one row byte per row.
    OneBppTiles,
    /// A 24x21 Commodore 64 hires sprite, including its 64th padding byte.
    C64HiresSprite,
    /// A 1bpp image stored in vertical 8-pixel pages.
    Arduboy1Bpp,
    /// A 1bpp bitmap stored as horizontal row-major bytes for TI Z80 targets.
    TiZ80OneBpp,
    /// TI-84 Plus CE chunky RGB565 pixels, little endian.
    Ti84PlusCeRgb565,
    /// Agon chunky RGBA8888 pixels.
    AgonRgba8888,
}

impl NativeImageFormat {
    /// The number of indexed colors accepted by the format.
    pub const fn max_palette_entries(self) -> usize {
        match self {
            Self::GameBoy2Bpp | Self::Nes2Bpp => 4,
            Self::Sms4Bpp => 16,
            Self::OneBppTiles | Self::C64HiresSprite | Self::Arduboy1Bpp | Self::TiZ80OneBpp => 2,
            Self::Ti84PlusCeRgb565 | Self::AgonRgba8888 => 256,
        }
    }

    pub const fn bits_per_pixel(self) -> u8 {
        match self {
            Self::GameBoy2Bpp | Self::Nes2Bpp => 2,
            Self::Sms4Bpp => 4,
            Self::OneBppTiles | Self::C64HiresSprite | Self::Arduboy1Bpp | Self::TiZ80OneBpp => 1,
            Self::Ti84PlusCeRgb565 => 16,
            Self::AgonRgba8888 => 32,
        }
    }

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::GameBoy2Bpp => "Game Boy 2bpp",
            Self::Nes2Bpp => "NES 2bpp",
            Self::Sms4Bpp => "SMS/Game Gear 4bpp",
            Self::OneBppTiles => "1bpp 8x8 tiles",
            Self::C64HiresSprite => "C64 hires sprite",
            Self::Arduboy1Bpp => "Arduboy 1bpp",
            Self::TiZ80OneBpp => "TI Z80 1bpp",
            Self::Ti84PlusCeRgb565 => "TI-84 Plus CE RGB565",
            Self::AgonRgba8888 => "Agon RGBA8888",
        }
    }

    /// Select this module's native format for a target triple and image role.
    pub fn for_target(target: &str, kind: ImageKind) -> Result<Self, ImageError> {
        format_for_target(target, kind)
    }
}

/// Borrowed indexed image input.
///
/// `indices` is row-major. Each value indexes one entry in `palette`, whose
/// entries are RGBA bytes in red, green, blue, alpha order.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct IndexedImage<'a> {
    pub width: usize,
    pub height: usize,
    pub indices: &'a [u8],
    pub palette: &'a [[u8; 4]],
}

impl<'a> IndexedImage<'a> {
    pub const fn new(
        width: usize,
        height: usize,
        indices: &'a [u8],
        palette: &'a [[u8; 4]],
    ) -> Self {
        Self {
            width,
            height,
            indices,
            palette,
        }
    }

    pub fn to_native_bytes(&self, format: NativeImageFormat) -> Result<Vec<u8>, ImageError> {
        encode_image(self, format)
    }
}

/// Short alias for callers that do not need to distinguish an indexed image
/// from other image representations.
pub type Image<'a> = IndexedImage<'a>;

/// An owned indexed image decoded from PNG bytes.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DecodedIndexedImage {
    pub width: usize,
    pub height: usize,
    pub indices: Vec<u8>,
    pub palette: Vec<[u8; 4]>,
}

impl DecodedIndexedImage {
    pub fn as_indexed(&self) -> IndexedImage<'_> {
        IndexedImage::new(self.width, self.height, &self.indices, &self.palette)
    }

    pub fn to_native_bytes(&self, format: NativeImageFormat) -> Result<Vec<u8>, ImageError> {
        encode_image(&self.as_indexed(), format)
    }
}

/// Errors returned by PNG decoding, target selection, and image encoding.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ImageError {
    InvalidPng(String),
    UnsupportedTarget {
        target: String,
    },
    UnsupportedKind {
        target: String,
        kind: ImageKind,
    },
    InvalidDimensions {
        format: NativeImageFormat,
        width: usize,
        height: usize,
        reason: &'static str,
    },
    DimensionOverflow {
        width: usize,
        height: usize,
    },
    PixelCountMismatch {
        expected: usize,
        actual: usize,
    },
    EmptyPalette,
    ColorIndexTooLarge {
        format: NativeImageFormat,
        position: usize,
        index: u8,
        max: usize,
    },
    PaletteIndexOutOfRange {
        position: usize,
        index: u8,
        palette_len: usize,
    },
}

impl fmt::Display for ImageError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidPng(message) => write!(formatter, "invalid indexed PNG: {message}"),
            Self::UnsupportedTarget { target } => {
                write!(formatter, "unsupported image target `{target}`")
            }
            Self::UnsupportedKind { target, kind } => write!(
                formatter,
                "image kind `{}` is unsupported for target `{target}`",
                kind.as_str()
            ),
            Self::InvalidDimensions {
                format,
                width,
                height,
                reason,
            } => write!(
                formatter,
                "invalid {} dimensions {width}x{height}: {reason}",
                format.as_str()
            ),
            Self::DimensionOverflow { width, height } => {
                write!(formatter, "image dimensions {width}x{height} overflow")
            }
            Self::PixelCountMismatch { expected, actual } => write!(
                formatter,
                "pixel count mismatch: expected {expected}, got {actual}"
            ),
            Self::EmptyPalette => formatter.write_str("palette must not be empty"),
            Self::ColorIndexTooLarge {
                format,
                position,
                index,
                max,
            } => write!(
                formatter,
                "{} accepts color indices 0..{}, got {index} at pixel {position}",
                format.as_str(),
                max - 1
            ),
            Self::PaletteIndexOutOfRange {
                position,
                index,
                palette_len,
            } => write!(
                formatter,
                "palette index {index} at pixel {position} is out of range for {palette_len} entries"
            ),
        }
    }
}

#[cfg(feature = "std")]
impl std::error::Error for ImageError {}

/// Decode a non-interlaced indexed-color PNG from memory.
///
/// This function uses only `alloc` and works in both std and no-std builds.
/// Palette indices are preserved without color conversion or remapping.
pub fn decode_indexed_png(bytes: &[u8]) -> Result<DecodedIndexedImage, ImageError> {
    const SIGNATURE: &[u8; 8] = b"\x89PNG\r\n\x1a\n";
    if bytes.get(..8) != Some(SIGNATURE.as_slice()) {
        return invalid_png("missing PNG signature");
    }

    let mut offset = 8usize;
    let mut dimensions = None;
    let mut bit_depth = 0u8;
    let mut palette_bytes = Vec::new();
    let mut transparency = Vec::new();
    let mut compressed = Vec::new();
    let mut saw_end = false;

    while offset < bytes.len() {
        let header_end = offset
            .checked_add(8)
            .ok_or_else(|| ImageError::InvalidPng(String::from("chunk offset overflow")))?;
        let header = bytes
            .get(offset..header_end)
            .ok_or_else(|| ImageError::InvalidPng(String::from("truncated chunk header")))?;
        let length = usize::try_from(u32::from_be_bytes([
            header[0], header[1], header[2], header[3],
        ]))
        .map_err(|_| ImageError::InvalidPng(String::from("chunk is too large")))?;
        let data_start = header_end;
        let data_end = data_start
            .checked_add(length)
            .ok_or_else(|| ImageError::InvalidPng(String::from("chunk length overflow")))?;
        let chunk_end = data_end
            .checked_add(4)
            .ok_or_else(|| ImageError::InvalidPng(String::from("chunk length overflow")))?;
        let data = bytes
            .get(data_start..data_end)
            .ok_or_else(|| ImageError::InvalidPng(String::from("truncated chunk data")))?;
        if chunk_end > bytes.len() {
            return invalid_png("truncated chunk checksum");
        }
        let kind = &header[4..8];
        match kind {
            b"IHDR" => {
                if dimensions.is_some() || data.len() != 13 {
                    return invalid_png("invalid IHDR chunk");
                }
                let width =
                    usize::try_from(u32::from_be_bytes([data[0], data[1], data[2], data[3]]))
                        .map_err(|_| ImageError::InvalidPng(String::from("width is too large")))?;
                let height =
                    usize::try_from(u32::from_be_bytes([data[4], data[5], data[6], data[7]]))
                        .map_err(|_| ImageError::InvalidPng(String::from("height is too large")))?;
                if width == 0 || height == 0 {
                    return invalid_png("width and height must be greater than zero");
                }
                bit_depth = data[8];
                if !matches!(bit_depth, 1 | 2 | 4 | 8) {
                    return Err(ImageError::InvalidPng(format!(
                        "unsupported indexed bit depth {bit_depth}"
                    )));
                }
                if data[9] != 3 {
                    return Err(ImageError::InvalidPng(format!(
                        "expected color type 3, got {}",
                        data[9]
                    )));
                }
                if data[10] != 0 || data[11] != 0 {
                    return invalid_png("unsupported compression or filter method");
                }
                if data[12] != 0 {
                    return invalid_png("interlaced PNGs are not supported");
                }
                dimensions = Some((width, height));
            }
            b"PLTE" => palette_bytes.extend_from_slice(data),
            b"tRNS" => transparency.extend_from_slice(data),
            b"IDAT" => compressed.extend_from_slice(data),
            b"IEND" => {
                saw_end = true;
                break;
            }
            _ => {}
        }
        offset = chunk_end;
    }

    let (width, height) =
        dimensions.ok_or_else(|| ImageError::InvalidPng(String::from("missing IHDR chunk")))?;
    if !saw_end {
        return invalid_png("missing IEND chunk");
    }
    if palette_bytes.is_empty() || palette_bytes.len() % 3 != 0 {
        return invalid_png("missing or invalid PLTE chunk");
    }
    let palette_len = palette_bytes.len() / 3;
    let max_palette_len = 1usize << bit_depth;
    if palette_len > max_palette_len || palette_len > 256 {
        return invalid_png("palette has more entries than the indexed bit depth allows");
    }
    if transparency.len() > palette_len {
        return invalid_png("transparency table is longer than the palette");
    }
    if compressed.is_empty() {
        return invalid_png("missing IDAT data");
    }

    let row_bits = width
        .checked_mul(usize::from(bit_depth))
        .ok_or_else(|| ImageError::InvalidPng(String::from("row size overflow")))?;
    let row_bytes = row_bits
        .checked_add(7)
        .ok_or_else(|| ImageError::InvalidPng(String::from("row size overflow")))?
        / 8;
    let filtered_row_bytes = row_bytes
        .checked_add(1)
        .ok_or_else(|| ImageError::InvalidPng(String::from("row size overflow")))?;
    let filtered_len = filtered_row_bytes
        .checked_mul(height)
        .ok_or_else(|| ImageError::InvalidPng(String::from("image size overflow")))?;
    let filtered =
        miniz_oxide::inflate::decompress_to_vec_zlib_with_limit(&compressed, filtered_len)
            .map_err(|error| ImageError::InvalidPng(format!("zlib decode failed: {error}")))?;
    if filtered.len() != filtered_len {
        return Err(ImageError::InvalidPng(format!(
            "decoded byte count mismatch: expected {filtered_len}, got {}",
            filtered.len()
        )));
    }

    let packed = unfilter_png_rows(&filtered, row_bytes, height)?;
    let pixel_count = width
        .checked_mul(height)
        .ok_or_else(|| ImageError::InvalidPng(String::from("image size overflow")))?;
    let mut indices = Vec::with_capacity(pixel_count);
    if bit_depth == 8 {
        indices.extend_from_slice(&packed);
    } else {
        let bits = usize::from(bit_depth);
        let mask = (1u8 << bit_depth) - 1;
        for row in packed.chunks_exact(row_bytes) {
            for x in 0..width {
                let bit_offset = x * bits;
                let shift = 8 - bits - (bit_offset % 8);
                indices.push((row[bit_offset / 8] >> shift) & mask);
            }
        }
    }

    let palette = palette_bytes
        .chunks_exact(3)
        .enumerate()
        .map(|(index, rgb)| {
            [
                rgb[0],
                rgb[1],
                rgb[2],
                transparency.get(index).copied().unwrap_or(255),
            ]
        })
        .collect();
    Ok(DecodedIndexedImage {
        width,
        height,
        indices,
        palette,
    })
}

/// Decode indexed PNG bytes and encode them for a target in one step.
pub fn indexed_png_to_native_bytes(
    png: &[u8],
    target: &str,
    kind: ImageKind,
) -> Result<Vec<u8>, ImageError> {
    let image = decode_indexed_png(png)?;
    encode_for_target(&image.as_indexed(), target, kind)
}

fn invalid_png<T>(message: &str) -> Result<T, ImageError> {
    Err(ImageError::InvalidPng(String::from(message)))
}

fn unfilter_png_rows(
    filtered: &[u8],
    row_bytes: usize,
    height: usize,
) -> Result<Vec<u8>, ImageError> {
    let mut output = vec![0; row_bytes * height];
    for row_index in 0..height {
        let input_start = row_index * (row_bytes + 1);
        let filter = filtered[input_start];
        let input = &filtered[input_start + 1..input_start + 1 + row_bytes];
        let output_start = row_index * row_bytes;
        for column in 0..row_bytes {
            let left = if column == 0 {
                0
            } else {
                output[output_start + column - 1]
            };
            let above = if row_index == 0 {
                0
            } else {
                output[output_start - row_bytes + column]
            };
            let upper_left = if row_index == 0 || column == 0 {
                0
            } else {
                output[output_start - row_bytes + column - 1]
            };
            let predictor = match filter {
                0 => 0,
                1 => left,
                2 => above,
                3 => ((u16::from(left) + u16::from(above)) / 2) as u8,
                4 => paeth_predictor(left, above, upper_left),
                other => {
                    return Err(ImageError::InvalidPng(format!(
                        "unsupported row filter {other}"
                    )));
                }
            };
            output[output_start + column] = input[column].wrapping_add(predictor);
        }
    }
    Ok(output)
}

fn paeth_predictor(left: u8, above: u8, upper_left: u8) -> u8 {
    let left = i32::from(left);
    let above = i32::from(above);
    let upper_left = i32::from(upper_left);
    let estimate = left + above - upper_left;
    let left_distance = (estimate - left).abs();
    let above_distance = (estimate - above).abs();
    let upper_left_distance = (estimate - upper_left).abs();
    if left_distance <= above_distance && left_distance <= upper_left_distance {
        left as u8
    } else if above_distance <= upper_left_distance {
        above as u8
    } else {
        upper_left as u8
    }
}

/// Select the native image format for a repository target triple and image role.
pub fn format_for_target(target: &str, kind: ImageKind) -> Result<NativeImageFormat, ImageError> {
    let format = if matches_any_target(target, &["gameboy-dmg-lr35902", "gameboy-color-lr35902"]) {
        match kind {
            ImageKind::Tiles | ImageKind::Sprite => NativeImageFormat::GameBoy2Bpp,
            ImageKind::Bitmap => return unsupported_kind(target, kind),
        }
    } else if matches_target(target, "nes-2a03") {
        match kind {
            ImageKind::Tiles | ImageKind::Sprite => NativeImageFormat::Nes2Bpp,
            ImageKind::Bitmap => return unsupported_kind(target, kind),
        }
    } else if target == "sega-master-system-z80" || target == "sega-game-gear-z80" {
        match kind {
            ImageKind::Tiles | ImageKind::Sprite => NativeImageFormat::Sms4Bpp,
            ImageKind::Bitmap => return unsupported_kind(target, kind),
        }
    } else if matches_target(target, "ti99-4a-tms9900") || matches_target(target, "zxspectrum-z80")
    {
        match kind {
            ImageKind::Tiles => NativeImageFormat::OneBppTiles,
            ImageKind::Sprite | ImageKind::Bitmap => return unsupported_kind(target, kind),
        }
    } else if matches_target(target, "commodore64-6502") {
        match kind {
            ImageKind::Tiles => NativeImageFormat::OneBppTiles,
            ImageKind::Sprite => NativeImageFormat::C64HiresSprite,
            ImageKind::Bitmap => return unsupported_kind(target, kind),
        }
    } else if matches_target(target, "arduboy-avr") {
        match kind {
            ImageKind::Bitmap | ImageKind::Sprite => NativeImageFormat::Arduboy1Bpp,
            ImageKind::Tiles => return unsupported_kind(target, kind),
        }
    } else if matches_any_target(
        target,
        &["ti83-z80", "ti83plus-z80", "ti84-z80", "ti84plus-z80"],
    ) {
        match kind {
            ImageKind::Bitmap => NativeImageFormat::TiZ80OneBpp,
            ImageKind::Tiles | ImageKind::Sprite => return unsupported_kind(target, kind),
        }
    } else if matches_any_target(target, &["ti84plusce-ez80", "ti83premiumce-ez80"]) {
        match kind {
            ImageKind::Bitmap => NativeImageFormat::Ti84PlusCeRgb565,
            ImageKind::Tiles | ImageKind::Sprite => return unsupported_kind(target, kind),
        }
    } else if matches_any_target(
        target,
        &[
            "agonlight-mos-ez80",
            "agonlight-vdp-ez80",
            "agonlight-console8-ez80",
        ],
    ) {
        match kind {
            ImageKind::Bitmap | ImageKind::Sprite => NativeImageFormat::AgonRgba8888,
            ImageKind::Tiles => return unsupported_kind(target, kind),
        }
    } else {
        return Err(ImageError::UnsupportedTarget {
            target: String::from(target),
        });
    };

    Ok(format)
}

/// Alias for [`format_for_target`].
pub fn native_format_for_target(
    target: &str,
    kind: ImageKind,
) -> Result<NativeImageFormat, ImageError> {
    format_for_target(target, kind)
}

/// Alias for [`format_for_target`].
pub fn select_native_format(
    target: &str,
    kind: ImageKind,
) -> Result<NativeImageFormat, ImageError> {
    format_for_target(target, kind)
}

/// Encode an indexed image using an explicit native format.
pub fn encode_image(
    image: &IndexedImage<'_>,
    format: NativeImageFormat,
) -> Result<Vec<u8>, ImageError> {
    validate_image(image, format)?;

    match format {
        NativeImageFormat::GameBoy2Bpp => encode_planar_tiles(image, 2, true),
        NativeImageFormat::Nes2Bpp => encode_planar_tiles(image, 2, false),
        NativeImageFormat::Sms4Bpp => encode_planar_tiles(image, 4, true),
        NativeImageFormat::OneBppTiles => encode_one_bpp_tiles(image),
        NativeImageFormat::C64HiresSprite => encode_c64_hires_sprite(image),
        NativeImageFormat::Arduboy1Bpp => encode_arduboy(image),
        NativeImageFormat::TiZ80OneBpp => encode_ti_z80(image),
        NativeImageFormat::Ti84PlusCeRgb565 => encode_rgb565(image),
        NativeImageFormat::AgonRgba8888 => encode_rgba8888(image),
    }
}

/// Encode an image after selecting its format from a target triple and kind.
pub fn encode_for_target(
    image: &IndexedImage<'_>,
    target: &str,
    kind: ImageKind,
) -> Result<Vec<u8>, ImageError> {
    let format = format_for_target(target, kind)?;
    encode_image(image, format)
}

/// Alias for [`encode_image`].
pub fn to_native_bytes(
    image: &IndexedImage<'_>,
    format: NativeImageFormat,
) -> Result<Vec<u8>, ImageError> {
    encode_image(image, format)
}

fn validate_image(image: &IndexedImage<'_>, format: NativeImageFormat) -> Result<(), ImageError> {
    if image.width == 0 || image.height == 0 {
        return Err(ImageError::InvalidDimensions {
            format,
            width: image.width,
            height: image.height,
            reason: "width and height must be greater than zero",
        });
    }

    let expected = image
        .width
        .checked_mul(image.height)
        .ok_or(ImageError::DimensionOverflow {
            width: image.width,
            height: image.height,
        })?;
    if image.indices.len() != expected {
        return Err(ImageError::PixelCountMismatch {
            expected,
            actual: image.indices.len(),
        });
    }

    if image.palette.is_empty() {
        return Err(ImageError::EmptyPalette);
    }
    let max_palette_entries = format.max_palette_entries();
    for (position, &index) in image.indices.iter().enumerate() {
        if usize::from(index) >= max_palette_entries {
            return Err(ImageError::ColorIndexTooLarge {
                format,
                position,
                index,
                max: max_palette_entries,
            });
        }
        if usize::from(index) >= image.palette.len() {
            return Err(ImageError::PaletteIndexOutOfRange {
                position,
                index,
                palette_len: image.palette.len(),
            });
        }
    }

    match format {
        NativeImageFormat::GameBoy2Bpp
        | NativeImageFormat::Nes2Bpp
        | NativeImageFormat::Sms4Bpp
        | NativeImageFormat::OneBppTiles => require_tile_dimensions(image, format),
        NativeImageFormat::C64HiresSprite => {
            if image.width != 24 || image.height != 21 {
                Err(ImageError::InvalidDimensions {
                    format,
                    width: image.width,
                    height: image.height,
                    reason: "expected exactly 24x21",
                })
            } else {
                Ok(())
            }
        }
        NativeImageFormat::Arduboy1Bpp => {
            if image.height % 8 != 0 {
                Err(ImageError::InvalidDimensions {
                    format,
                    width: image.width,
                    height: image.height,
                    reason: "height must be a multiple of 8",
                })
            } else {
                Ok(())
            }
        }
        NativeImageFormat::TiZ80OneBpp => {
            if image.width % 8 != 0 {
                Err(ImageError::InvalidDimensions {
                    format,
                    width: image.width,
                    height: image.height,
                    reason: "width must be a multiple of 8",
                })
            } else {
                Ok(())
            }
        }
        NativeImageFormat::Ti84PlusCeRgb565 | NativeImageFormat::AgonRgba8888 => Ok(()),
    }
}

fn require_tile_dimensions(
    image: &IndexedImage<'_>,
    format: NativeImageFormat,
) -> Result<(), ImageError> {
    if image.width % 8 != 0 || image.height % 8 != 0 {
        return Err(ImageError::InvalidDimensions {
            format,
            width: image.width,
            height: image.height,
            reason: "width and height must be multiples of 8",
        });
    }
    Ok(())
}

fn encode_planar_tiles(
    image: &IndexedImage<'_>,
    bits_per_pixel: u8,
    interleave_planes: bool,
) -> Result<Vec<u8>, ImageError> {
    let tile_count =
        (image.width / 8)
            .checked_mul(image.height / 8)
            .ok_or(ImageError::DimensionOverflow {
                width: image.width,
                height: image.height,
            })?;
    let bytes_per_tile =
        usize::from(bits_per_pixel)
            .checked_mul(8)
            .ok_or(ImageError::DimensionOverflow {
                width: image.width,
                height: image.height,
            })?;
    let output_len =
        tile_count
            .checked_mul(bytes_per_tile)
            .ok_or(ImageError::DimensionOverflow {
                width: image.width,
                height: image.height,
            })?;
    let mut output = Vec::with_capacity(output_len);

    for tile_y in (0..image.height).step_by(8) {
        for tile_x in (0..image.width).step_by(8) {
            if interleave_planes {
                for row in 0..8 {
                    for plane in 0..bits_per_pixel {
                        output.push(plane_byte(image, tile_x, tile_y, row, plane));
                    }
                }
            } else {
                for plane in 0..bits_per_pixel {
                    for row in 0..8 {
                        output.push(plane_byte(image, tile_x, tile_y, row, plane));
                    }
                }
            }
        }
    }

    Ok(output)
}

fn plane_byte(image: &IndexedImage<'_>, tile_x: usize, tile_y: usize, row: usize, plane: u8) -> u8 {
    let mut byte = 0;
    for x in 0..8 {
        let index = image.indices[(tile_y + row) * image.width + tile_x + x];
        byte |= ((index >> plane) & 1) << (7 - x);
    }
    byte
}

fn encode_one_bpp_tiles(image: &IndexedImage<'_>) -> Result<Vec<u8>, ImageError> {
    let tile_count =
        (image.width / 8)
            .checked_mul(image.height / 8)
            .ok_or(ImageError::DimensionOverflow {
                width: image.width,
                height: image.height,
            })?;
    let output_len = tile_count
        .checked_mul(8)
        .ok_or(ImageError::DimensionOverflow {
            width: image.width,
            height: image.height,
        })?;
    let mut output = Vec::with_capacity(output_len);

    for tile_y in (0..image.height).step_by(8) {
        for tile_x in (0..image.width).step_by(8) {
            for row in 0..8 {
                output.push(plane_byte(image, tile_x, tile_y, row, 0));
            }
        }
    }

    Ok(output)
}

fn encode_c64_hires_sprite(image: &IndexedImage<'_>) -> Result<Vec<u8>, ImageError> {
    let mut output = Vec::with_capacity(64);
    for row in 0..21 {
        for byte_x in 0..3 {
            let mut byte = 0;
            for bit in 0..8 {
                let index = image.indices[row * image.width + byte_x * 8 + bit];
                byte |= (index & 1) << (7 - bit);
            }
            output.push(byte);
        }
    }
    output.push(0);
    Ok(output)
}

fn encode_arduboy(image: &IndexedImage<'_>) -> Result<Vec<u8>, ImageError> {
    let page_count = image.height / 8;
    let output_len = page_count
        .checked_mul(image.width)
        .ok_or(ImageError::DimensionOverflow {
            width: image.width,
            height: image.height,
        })?;
    let mut output = Vec::with_capacity(output_len);

    for page in 0..page_count {
        for x in 0..image.width {
            let mut byte = 0;
            for bit in 0..8 {
                let index = image.indices[(page * 8 + bit) * image.width + x];
                byte |= (index & 1) << bit;
            }
            output.push(byte);
        }
    }

    Ok(output)
}

fn encode_ti_z80(image: &IndexedImage<'_>) -> Result<Vec<u8>, ImageError> {
    let bytes_per_row = image.width / 8;
    let output_len =
        bytes_per_row
            .checked_mul(image.height)
            .ok_or(ImageError::DimensionOverflow {
                width: image.width,
                height: image.height,
            })?;
    let mut output = Vec::with_capacity(output_len);

    for row in 0..image.height {
        for byte_x in 0..bytes_per_row {
            let mut byte = 0;
            for bit in 0..8 {
                let index = image.indices[row * image.width + byte_x * 8 + bit];
                byte |= (index & 1) << (7 - bit);
            }
            output.push(byte);
        }
    }

    Ok(output)
}

fn encode_rgb565(image: &IndexedImage<'_>) -> Result<Vec<u8>, ImageError> {
    let output_len = image
        .width
        .checked_mul(image.height)
        .and_then(|pixels| pixels.checked_mul(2))
        .ok_or(ImageError::DimensionOverflow {
            width: image.width,
            height: image.height,
        })?;
    let mut output = Vec::with_capacity(output_len);

    for &index in image.indices {
        let [red, green, blue, _alpha] = image.palette[usize::from(index)];
        let color =
            (u16::from(red >> 3) << 11) | (u16::from(green >> 2) << 5) | u16::from(blue >> 3);
        output.push(color as u8);
        output.push((color >> 8) as u8);
    }

    Ok(output)
}

fn encode_rgba8888(image: &IndexedImage<'_>) -> Result<Vec<u8>, ImageError> {
    let output_len = image
        .width
        .checked_mul(image.height)
        .and_then(|pixels| pixels.checked_mul(4))
        .ok_or(ImageError::DimensionOverflow {
            width: image.width,
            height: image.height,
        })?;
    let mut output = Vec::with_capacity(output_len);

    for &index in image.indices {
        output.extend_from_slice(&image.palette[usize::from(index)]);
    }

    Ok(output)
}

fn matches_target(target: &str, base: &str) -> bool {
    target == base
        || target
            .strip_prefix(base)
            .is_some_and(|suffix| suffix.starts_with('-'))
}

fn matches_any_target(target: &str, bases: &[&str]) -> bool {
    bases
        .iter()
        .copied()
        .any(|base| matches_target(target, base))
}

fn unsupported_kind(target: &str, kind: ImageKind) -> Result<NativeImageFormat, ImageError> {
    Err(ImageError::UnsupportedKind {
        target: String::from(target),
        kind,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn palette(count: usize) -> Vec<[u8; 4]> {
        (0..count)
            .map(|value| [value as u8, value as u8, value as u8, 255])
            .collect()
    }

    fn indexed_png(depth: png::BitDepth, width: u32, height: u32, pixels: &[u8]) -> Vec<u8> {
        let mut bytes = Vec::new();
        {
            let mut encoder = png::Encoder::new(&mut bytes, width, height);
            encoder.set_color(png::ColorType::Indexed);
            encoder.set_depth(depth);
            encoder.set_palette(vec![0, 0, 0, 255, 0, 0, 0, 255, 0, 0, 0, 255]);
            let mut writer = encoder.write_header().unwrap();
            writer.write_image_data(pixels).unwrap();
        }
        bytes
    }

    #[test]
    fn decodes_indexed_png_in_memory_and_encodes_it_for_a_target() {
        let png = indexed_png(png::BitDepth::Two, 8, 8, &[0b0001_1011; 16]);
        let image = decode_indexed_png(&png).unwrap();
        assert_eq!(image.width, 8);
        assert_eq!(image.height, 8);
        assert_eq!(&image.indices[..4], &[0, 1, 2, 3]);
        assert_eq!(image.palette[1], [255, 0, 0, 255]);
        assert_eq!(
            indexed_png_to_native_bytes(&png, "gameboy-dmg-lr35902", ImageKind::Tiles)
                .unwrap()
                .len(),
            16
        );
    }

    #[test]
    fn rejects_nonindexed_png_data() {
        let mut bytes = Vec::new();
        {
            let mut encoder = png::Encoder::new(&mut bytes, 1, 1);
            encoder.set_color(png::ColorType::Rgb);
            encoder.set_depth(png::BitDepth::Eight);
            let mut writer = encoder.write_header().unwrap();
            writer.write_image_data(&[1, 2, 3]).unwrap();
        }
        assert!(matches!(
            decode_indexed_png(&bytes),
            Err(ImageError::InvalidPng(_))
        ));
    }

    #[test]
    fn selects_formats_for_repository_targets() {
        assert_eq!(
            format_for_target("gameboy-dmg-lr35902", ImageKind::Tiles),
            Ok(NativeImageFormat::GameBoy2Bpp)
        );
        assert_eq!(
            format_for_target("nes-2a03", ImageKind::Sprite),
            Ok(NativeImageFormat::Nes2Bpp)
        );
        assert_eq!(
            format_for_target("sega-game-gear-z80", ImageKind::Tiles),
            Ok(NativeImageFormat::Sms4Bpp)
        );
        assert_eq!(
            format_for_target("ti99-4a-tms9900", ImageKind::Tiles),
            Ok(NativeImageFormat::OneBppTiles)
        );
        assert_eq!(
            format_for_target("commodore64-6502", ImageKind::Sprite),
            Ok(NativeImageFormat::C64HiresSprite)
        );
        assert_eq!(
            format_for_target("arduboy-avr", ImageKind::Bitmap),
            Ok(NativeImageFormat::Arduboy1Bpp)
        );
        assert_eq!(
            format_for_target("ti84plus-z80", ImageKind::Bitmap),
            Ok(NativeImageFormat::TiZ80OneBpp)
        );
        assert_eq!(
            format_for_target("ti84plusce-ez80", ImageKind::Bitmap),
            Ok(NativeImageFormat::Ti84PlusCeRgb565)
        );
        assert_eq!(
            format_for_target("agonlight-vdp-ez80-1.0", ImageKind::Bitmap),
            Ok(NativeImageFormat::AgonRgba8888)
        );
        assert_eq!(
            format_for_target("agonlight-mos-ez80", ImageKind::Sprite),
            Ok(NativeImageFormat::AgonRgba8888)
        );
    }

    #[test]
    fn rejects_bare_and_text_only_targets() {
        assert_eq!(
            format_for_target("gameboy", ImageKind::Tiles),
            Err(ImageError::UnsupportedTarget {
                target: String::from("gameboy"),
            })
        );
        assert_eq!(
            format_for_target("nes", ImageKind::Tiles),
            Err(ImageError::UnsupportedTarget {
                target: String::from("nes"),
            })
        );
        assert!(matches!(
            format_for_target("arduboy-avr", ImageKind::Tiles),
            Err(ImageError::UnsupportedKind { .. })
        ));
    }

    #[test]
    fn encodes_game_boy_rows_and_nes_plane_blocks() {
        let indices = [1u8; 64];
        let colors = palette(2);
        let image = IndexedImage::new(8, 8, &indices, &colors);
        assert_eq!(
            encode_image(&image, NativeImageFormat::GameBoy2Bpp).unwrap(),
            vec![
                0xFF, 0x00, 0xFF, 0x00, 0xFF, 0x00, 0xFF, 0x00, 0xFF, 0x00, 0xFF, 0x00, 0xFF, 0x00,
                0xFF, 0x00
            ]
        );

        let indices = [2u8; 64];
        let colors = palette(3);
        let image = IndexedImage::new(8, 8, &indices, &colors);
        let mut expected = vec![0; 16];
        expected[8..].fill(0xFF);
        assert_eq!(
            encode_image(&image, NativeImageFormat::Nes2Bpp).unwrap(),
            expected
        );
    }

    #[test]
    fn traverses_tiles_row_major_and_interleaves_sms_planes() {
        let mut indices = [0u8; 128];
        indices[0] = 1;
        indices[8] = 8;
        let colors = palette(16);
        let image = IndexedImage::new(16, 8, &indices, &colors);
        let bytes = encode_image(&image, NativeImageFormat::Sms4Bpp).unwrap();

        assert_eq!(&bytes[0..4], &[0x80, 0x00, 0x00, 0x00]);
        assert_eq!(&bytes[32..36], &[0x00, 0x00, 0x00, 0x80]);
    }

    #[test]
    fn encodes_one_bpp_tiles_in_row_major_tile_order() {
        let mut indices = [0u8; 128];
        indices[0] = 1;
        indices[8] = 1;
        let colors = palette(2);
        let image = IndexedImage::new(16, 8, &indices, &colors);
        let bytes = encode_image(&image, NativeImageFormat::OneBppTiles).unwrap();

        assert_eq!(&bytes[0..8], &[0x80, 0, 0, 0, 0, 0, 0, 0]);
        assert_eq!(&bytes[8..16], &[0x80, 0, 0, 0, 0, 0, 0, 0]);
    }

    #[test]
    fn encodes_c64_sprite_with_padding() {
        let mut indices = [0u8; 24 * 21];
        indices[0] = 1;
        indices[23] = 1;
        let colors = palette(2);
        let image = IndexedImage::new(24, 21, &indices, &colors);
        let bytes = encode_image(&image, NativeImageFormat::C64HiresSprite).unwrap();

        assert_eq!(bytes.len(), 64);
        assert_eq!(&bytes[0..3], &[0x80, 0x00, 0x01]);
        assert_eq!(bytes[63], 0);
    }

    #[test]
    fn encodes_arduboy_vertical_pages_and_ti_rows() {
        let mut indices = [0u8; 8 * 8];
        indices[0] = 1;
        indices[7 * 8] = 1;
        let colors = palette(2);
        let image = IndexedImage::new(8, 8, &indices, &colors);
        assert_eq!(
            encode_image(&image, NativeImageFormat::Arduboy1Bpp).unwrap(),
            vec![0x81, 0, 0, 0, 0, 0, 0, 0]
        );
        assert_eq!(
            encode_image(&image, NativeImageFormat::TiZ80OneBpp).unwrap(),
            vec![0x80, 0, 0, 0, 0, 0, 0, 0x80]
        );
    }

    #[test]
    fn encodes_chunky_formats_from_rgba_palette_entries() {
        let indices = [0, 1, 2, 3];
        let colors = [
            [255, 0, 0, 255],
            [0, 255, 0, 255],
            [0, 0, 255, 255],
            [1, 2, 3, 4],
        ];
        let image = IndexedImage::new(4, 1, &indices, &colors);
        assert_eq!(
            encode_image(&image, NativeImageFormat::Ti84PlusCeRgb565).unwrap(),
            vec![0x00, 0xF8, 0xE0, 0x07, 0x1F, 0x00, 0x00, 0x00]
        );
        assert_eq!(
            encode_image(&image, NativeImageFormat::AgonRgba8888).unwrap(),
            colors.into_iter().flatten().collect::<Vec<_>>()
        );
    }

    #[test]
    fn reports_pixel_palette_and_dimension_errors() {
        let colors = palette(2);
        let short = IndexedImage::new(8, 8, &[0; 63], &colors);
        assert!(matches!(
            encode_image(&short, NativeImageFormat::OneBppTiles),
            Err(ImageError::PixelCountMismatch {
                expected: 64,
                actual: 63
            })
        ));

        let one_color = palette(1);
        let bad_index = IndexedImage::new(8, 8, &[1; 64], &one_color);
        assert!(matches!(
            encode_image(&bad_index, NativeImageFormat::OneBppTiles),
            Err(ImageError::PaletteIndexOutOfRange {
                position: 0,
                index: 1,
                palette_len: 1
            })
        ));

        let large_palette = palette(16);
        let valid_low_indices = IndexedImage::new(8, 8, &[1; 64], &large_palette);
        encode_image(&valid_low_indices, NativeImageFormat::GameBoy2Bpp).unwrap();
        let high_indices = IndexedImage::new(8, 8, &[4; 64], &large_palette);
        assert!(matches!(
            encode_image(&high_indices, NativeImageFormat::GameBoy2Bpp),
            Err(ImageError::ColorIndexTooLarge {
                index: 4,
                max: 4,
                ..
            })
        ));

        let bad_dimensions = IndexedImage::new(7, 8, &[0; 56], &colors);
        assert!(matches!(
            encode_image(&bad_dimensions, NativeImageFormat::OneBppTiles),
            Err(ImageError::InvalidDimensions { .. })
        ));
    }
}
