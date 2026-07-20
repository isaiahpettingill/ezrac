//! Filesystem-free executable packaging for library consumers.

use alloc::{
    borrow::ToOwned,
    format,
    string::{String, ToString},
    vec,
    vec::Vec,
};
use core::fmt;

use crate::target::{Address24, OutputFormat};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PackageError {
    pub message: String,
}

impl PackageError {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl fmt::Display for PackageError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

#[cfg(feature = "std")]
impl std::error::Error for PackageError {}

/// Parameters shared by target executable packagers.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PackageRequest {
    pub target: String,
    pub output_format: OutputFormat,
    pub load_addr: u32,
    pub entry_addr: u32,
    pub executable_name: Option<String>,
}

impl PackageRequest {
    pub fn new(
        target: impl Into<String>,
        output_format: OutputFormat,
        load_addr: u32,
        entry_addr: u32,
    ) -> Self {
        Self {
            target: target.into(),
            output_format,
            load_addr,
            entry_addr,
            executable_name: None,
        }
    }
}

/// Resolved input for target packagers that require metadata or auxiliary payloads.
///
/// The package layer never reads host files. Callers must resolve project metadata,
/// executable names, and bank payloads before creating this context.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct PackageContext {
    /// Whether the input begins at the program entry point or is a sparse image
    /// beginning at the configured load address.
    pub image_kind: PackageImageKind,
    /// The resolved executable name, without an extension.
    pub executable_name: Option<String>,
    pub arduboy: Option<ArduboyPackageOptions>,
    pub ti8xp: Option<Ti8xpPackageOptions>,
    pub zx_spectrum: Option<ZxSpectrumPackageOptions>,
    pub game_boy: Option<GameBoyPackageOptions>,
}

/// Describes the address represented by the first byte provided to a packager.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum PackageImageKind {
    /// Code starts at the executable entry address.
    #[default]
    EntryCode,
    /// A linked image starts at the layout load address and can contain space
    /// before the entry point.
    LoadImage,
}

impl PackageContext {
    pub const fn new() -> Self {
        Self {
            image_kind: PackageImageKind::EntryCode,
            executable_name: None,
            arduboy: None,
            ti8xp: None,
            zx_spectrum: None,
            game_boy: None,
        }
    }
}

/// Metadata written to an Arduboy package's `info.json` entry.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ArduboyPackageOptions {
    pub title: String,
    pub author: String,
    pub version: String,
    pub description: Option<String>,
    pub date: Option<String>,
    pub genre: Option<String>,
    pub source_url: Option<String>,
}

/// Optional TI-8xp variable-name override.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Ti8xpPackageOptions {
    pub variable_name: Option<String>,
}

/// A resolved ZX Spectrum 128K RAM-page payload.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ZxSpectrumBankPayload {
    pub page: u8,
    pub name: Option<String>,
    pub bytes: Vec<u8>,
}

/// Resolved options for ZX Spectrum TAP packaging.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ZxSpectrumPackageOptions {
    pub banks: Vec<ZxSpectrumBankPayload>,
}

/// Game Boy cartridge mapper selection.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum GameBoyMapper {
    #[default]
    RomOnly,
    Mbc1,
    Mbc5,
}

/// A resolved payload for an explicitly selected switchable Game Boy ROM bank.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GameBoyBankPayload {
    pub bank: usize,
    pub bytes: Vec<u8>,
}

/// Resolved Game Boy cartridge options.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct GameBoyPackageOptions {
    pub mapper: GameBoyMapper,
    pub rom_banks: Option<u16>,
    pub ram_banks: u8,
    pub battery: bool,
    pub rumble: bool,
    /// Payloads corresponding to configured bank files, assigned from bank 2.
    pub bank_payloads: Vec<Vec<u8>>,
    /// Payloads emitted by explicit source banking, keyed by their selected bank.
    pub generated_bank_payloads: Vec<GameBoyBankPayload>,
    /// Whether fixed code is constrained to the first 16 KiB ROM bank.
    pub explicit_banking: bool,
}

/// Package assembled code without reading or writing host files.
///
/// This compatibility wrapper supplies an empty [`PackageContext`]. Use
/// [`package_executable_with_context`] for formats that need resolved metadata,
/// names, or auxiliary payloads.
pub fn package_executable(request: &PackageRequest, code: &[u8]) -> Result<Vec<u8>, PackageError> {
    package_executable_with_context(request, &PackageContext::new(), code)
}

/// Package assembled code using caller-resolved metadata and auxiliary payloads.
pub fn package_executable_with_context(
    request: &PackageRequest,
    context: &PackageContext,
    code: &[u8],
) -> Result<Vec<u8>, PackageError> {
    if request.target.starts_with("agonlight-mos-ez80") {
        return match context.image_kind {
            PackageImageKind::EntryCode => agon_mos_bytes(request.entry_addr, code),
            PackageImageKind::LoadImage => agon_mos_load_image_bytes(request, code),
        };
    }
    match request.output_format {
        OutputFormat::RawBin | OutputFormat::CpmCom | OutputFormat::Ez180nGaem => Ok(code.to_vec()),
        OutputFormat::IntelHex | OutputFormat::ArduinoHex => {
            Ok(intel_hex_bytes(request.load_addr, code))
        }
        OutputFormat::Arduboy => arduboy_package_bytes(request, context, code),
        OutputFormat::Ti8xp => ti8xp_bytes(request, context, code),
        OutputFormat::ZxSpectrumTap => zx_spectrum_tap_bytes(request, context, code),
        OutputFormat::GameBoyGb => game_boy_rom_bytes(request, context, code),
        OutputFormat::Commodore64Prg => commodore64_prg_bytes(request, code),
        OutputFormat::Commodore64Crt => commodore64_crt_bytes(request, code),
        OutputFormat::Ti8ek | OutputFormat::Ti8xk => Err(PackageError::new(format!(
            "TI flash application output `.{}` is not implemented; use `.8xp` protected-program output",
            request.output_format.extension()
        ))),
    }
}

fn commodore64_prg_bytes(request: &PackageRequest, code: &[u8]) -> Result<Vec<u8>, PackageError> {
    if !request.target.starts_with("commodore64-6502") {
        return Err(PackageError::new(format!(
            "target `{}` does not support Commodore 64 .prg output",
            request.target
        )));
    }
    if request.load_addr != 0x080D || request.entry_addr != 0x080D {
        return Err(PackageError::new(
            "Commodore 64 PRG layouts must load and enter at 0x080D",
        ));
    }
    const BASIC_AUTOSTART: [u8; 12] = [
        0x0B, 0x08, 0x0A, 0x00, 0x9E, b'2', b'0', b'6', b'1', 0x00, 0x00, 0x00,
    ];
    let mut output = Vec::with_capacity(code.len() + 14);
    output.extend_from_slice(&0x0801u16.to_le_bytes());
    output.extend_from_slice(&BASIC_AUTOSTART);
    output.extend_from_slice(code);
    Ok(output)
}

fn commodore64_crt_bytes(request: &PackageRequest, code: &[u8]) -> Result<Vec<u8>, PackageError> {
    if !request.target.starts_with("commodore64-6502") {
        return Err(PackageError::new(format!(
            "target `{}` does not support Commodore 64 .crt output",
            request.target
        )));
    }
    if request.load_addr != 0x8009 || request.entry_addr != 0x8009 {
        return Err(PackageError::new(
            "standard Commodore 64 CRT layouts must load and enter at 0x8009",
        ));
    }
    const ROM_SIZE: usize = 0x2000;
    const HEADER_SIZE: usize = 0x40;
    const CHIP_HEADER_SIZE: usize = 0x10;
    if code.len() > ROM_SIZE - 9 {
        return Err(PackageError::new(
            "program code exceeds the standard 8 KiB Commodore 64 CRT capacity",
        ));
    }
    let mut output = Vec::with_capacity(HEADER_SIZE + CHIP_HEADER_SIZE + ROM_SIZE);
    output.extend_from_slice(b"C64 CARTRIDGE   ");
    output.extend_from_slice(&0x40u32.to_be_bytes());
    output.extend_from_slice(&0x0100u16.to_be_bytes());
    output.extend_from_slice(&0u16.to_be_bytes());
    output.push(0);
    output.push(1);
    output.extend_from_slice(&[0; 6]);
    let mut name = [0u8; 32];
    name[..10].copy_from_slice(b"EZRA C64  ");
    output.extend_from_slice(&name);
    output.extend_from_slice(b"CHIP");
    output.extend_from_slice(&((CHIP_HEADER_SIZE + ROM_SIZE) as u32).to_be_bytes());
    output.extend_from_slice(&0u16.to_be_bytes());
    output.extend_from_slice(&0u16.to_be_bytes());
    output.extend_from_slice(&0x8000u16.to_be_bytes());
    output.extend_from_slice(&(ROM_SIZE as u16).to_be_bytes());
    output.extend_from_slice(&0x8009u16.to_le_bytes());
    output.extend_from_slice(&0x8009u16.to_le_bytes());
    output.extend_from_slice(b"CBM80");
    output.extend_from_slice(code);
    output.resize(HEADER_SIZE + CHIP_HEADER_SIZE + ROM_SIZE, 0xFF);
    Ok(output)
}

fn agon_mos_bytes(entry: u32, code: &[u8]) -> Result<Vec<u8>, PackageError> {
    if entry > Address24::MAX {
        return Err(PackageError::new(format!(
            "Agon MOS entry address 0x{entry:X} is outside the 24-bit address space"
        )));
    }
    let mut out = Vec::with_capacity(69 + code.len());
    out.extend_from_slice(&[0xC3, entry as u8, (entry >> 8) as u8, (entry >> 16) as u8]);
    out.resize(64, 0);
    out.extend_from_slice(b"MOS\0\x01");
    out.extend_from_slice(code);
    Ok(out)
}

fn agon_mos_load_image_bytes(
    request: &PackageRequest,
    code: &[u8],
) -> Result<Vec<u8>, PackageError> {
    if request.entry_addr > Address24::MAX {
        return Err(PackageError::new(format!(
            "Agon MOS entry address 0x{:X} is outside the 24-bit address space",
            request.entry_addr
        )));
    }
    let mut image = code.to_vec();
    image.resize(image.len().max(69), 0);
    image[0..4].copy_from_slice(&[
        0xC3,
        request.entry_addr as u8,
        (request.entry_addr >> 8) as u8,
        (request.entry_addr >> 16) as u8,
    ]);
    image[64..69].copy_from_slice(b"MOS\0\x01");
    Ok(image)
}

fn intel_hex_bytes(base_addr: u32, code: &[u8]) -> Vec<u8> {
    let mut out = String::new();
    let mut current_upper = None;
    for (offset, chunk) in code.chunks(16).enumerate() {
        let addr = base_addr + (offset * 16) as u32;
        let upper = (addr >> 16) as u16;
        if current_upper != Some(upper) {
            current_upper = Some(upper);
            push_ihex_record(&mut out, 0, 4, &upper.to_be_bytes());
        }
        push_ihex_record(&mut out, addr as u16, 0, chunk);
    }
    push_ihex_record(&mut out, 0, 1, &[]);
    out.into_bytes()
}
fn push_ihex_record(out: &mut String, address: u16, kind: u8, data: &[u8]) {
    let mut sum = (data.len() as u8)
        .wrapping_add((address >> 8) as u8)
        .wrapping_add(address as u8)
        .wrapping_add(kind);
    out.push_str(&format!(":{:02X}{address:04X}{kind:02X}", data.len()));
    for byte in data {
        sum = sum.wrapping_add(*byte);
        out.push_str(&format!("{byte:02X}"));
    }
    out.push_str(&format!("{:02X}\n", (!sum).wrapping_add(1)));
}

fn executable_name<'a>(request: &'a PackageRequest, context: &'a PackageContext) -> &'a str {
    context
        .executable_name
        .as_deref()
        .or(request.executable_name.as_deref())
        .unwrap_or("EZRA")
}

fn arduboy_package_bytes(
    request: &PackageRequest,
    context: &PackageContext,
    code: &[u8],
) -> Result<Vec<u8>, PackageError> {
    if !request.target.starts_with("arduboy-") {
        return Err(PackageError::new(format!(
            "target `{}` does not support Arduboy .arduboy output",
            request.target
        )));
    }
    let options = context.arduboy.as_ref().ok_or_else(|| {
        PackageError::new("Arduboy metadata is required for `.arduboy` packaging")
    })?;
    let executable = context
        .executable_name
        .as_deref()
        .or(request.executable_name.as_deref())
        .filter(|name| !name.is_empty())
        .ok_or_else(|| {
            PackageError::new("Arduboy packaging requires a resolved executable name")
        })?;
    let hex_filename = format!("{executable}.hex");
    let hex = intel_hex_bytes(request.load_addr, code);
    let info = arduboy_info_json(options, &hex_filename);
    stored_zip_bytes(&[("info.json", info.as_bytes()), (&hex_filename, &hex)])
}

fn arduboy_info_json(options: &ArduboyPackageOptions, hex_filename: &str) -> String {
    let mut fields = vec![
        ("schemaVersion", "2".to_owned()),
        ("title", json_string(&options.title)),
        ("author", json_string(&options.author)),
        ("version", json_string(&options.version)),
    ];
    for (name, value) in [
        ("description", options.description.as_deref()),
        ("date", options.date.as_deref()),
        ("genre", options.genre.as_deref()),
        ("sourceUrl", options.source_url.as_deref()),
    ] {
        if let Some(value) = value {
            fields.push((name, json_string(value)));
        }
    }
    fields.push((
        "binaries",
        format!(
            "[{{\"filename\":{},\"device\":\"Arduboy\"}}]",
            json_string(hex_filename)
        ),
    ));
    let mut info = String::from("{");
    for (index, (name, value)) in fields.into_iter().enumerate() {
        if index != 0 {
            info.push(',');
        }
        info.push_str(&json_string(name));
        info.push(':');
        info.push_str(&value);
    }
    info.push('}');
    info
}

fn json_string(value: &str) -> String {
    let mut json = String::with_capacity(value.len() + 2);
    json.push('"');
    for character in value.chars() {
        match character {
            '"' => json.push_str("\\\""),
            '\\' => json.push_str("\\\\"),
            '\n' => json.push_str("\\n"),
            '\r' => json.push_str("\\r"),
            '\t' => json.push_str("\\t"),
            character if character <= '\u{1F}' => {
                json.push_str(&format!("\\u{:04X}", character as u32));
            }
            character => json.push(character),
        }
    }
    json.push('"');
    json
}

fn stored_zip_bytes(entries: &[(&str, &[u8])]) -> Result<Vec<u8>, PackageError> {
    const LOCAL_FILE_HEADER: u32 = 0x0403_4B50;
    const CENTRAL_DIRECTORY_HEADER: u32 = 0x0201_4B50;
    const END_OF_CENTRAL_DIRECTORY: u32 = 0x0605_4B50;
    const VERSION_NEEDED: u16 = 20;
    const VERSION_MADE_BY: u16 = 20;
    const UTF8_FLAG: u16 = 1 << 11;
    const STORED: u16 = 0;
    const DOS_DATE_1980_01_01: u16 = 0x0021;

    let entry_count = u16::try_from(entries.len())
        .map_err(|_| PackageError::new("Arduboy ZIP contains too many entries"))?;
    let mut zip = Vec::new();
    let mut central_directory = Vec::new();
    for (name, data) in entries {
        let name = name.as_bytes();
        let name_len = u16::try_from(name.len())
            .map_err(|_| PackageError::new("Arduboy ZIP entry name is too long"))?;
        let data_len = u32::try_from(data.len())
            .map_err(|_| PackageError::new("Arduboy ZIP entry exceeds 4 GiB"))?;
        let offset =
            u32::try_from(zip.len()).map_err(|_| PackageError::new("Arduboy ZIP exceeds 4 GiB"))?;
        let crc = zip_crc32(data);

        push_u32_le(&mut zip, LOCAL_FILE_HEADER);
        push_u16_le(&mut zip, VERSION_NEEDED);
        push_u16_le(&mut zip, UTF8_FLAG);
        push_u16_le(&mut zip, STORED);
        push_u16_le(&mut zip, 0);
        push_u16_le(&mut zip, DOS_DATE_1980_01_01);
        push_u32_le(&mut zip, crc);
        push_u32_le(&mut zip, data_len);
        push_u32_le(&mut zip, data_len);
        push_u16_le(&mut zip, name_len);
        push_u16_le(&mut zip, 0);
        zip.extend_from_slice(name);
        zip.extend_from_slice(data);

        push_u32_le(&mut central_directory, CENTRAL_DIRECTORY_HEADER);
        push_u16_le(&mut central_directory, VERSION_MADE_BY);
        push_u16_le(&mut central_directory, VERSION_NEEDED);
        push_u16_le(&mut central_directory, UTF8_FLAG);
        push_u16_le(&mut central_directory, STORED);
        push_u16_le(&mut central_directory, 0);
        push_u16_le(&mut central_directory, DOS_DATE_1980_01_01);
        push_u32_le(&mut central_directory, crc);
        push_u32_le(&mut central_directory, data_len);
        push_u32_le(&mut central_directory, data_len);
        push_u16_le(&mut central_directory, name_len);
        push_u16_le(&mut central_directory, 0);
        push_u16_le(&mut central_directory, 0);
        push_u16_le(&mut central_directory, 0);
        push_u16_le(&mut central_directory, 0);
        push_u32_le(&mut central_directory, 0);
        push_u32_le(&mut central_directory, offset);
        central_directory.extend_from_slice(name);
    }

    let central_offset =
        u32::try_from(zip.len()).map_err(|_| PackageError::new("Arduboy ZIP exceeds 4 GiB"))?;
    let central_len = u32::try_from(central_directory.len())
        .map_err(|_| PackageError::new("Arduboy ZIP central directory exceeds 4 GiB"))?;
    zip.extend_from_slice(&central_directory);
    push_u32_le(&mut zip, END_OF_CENTRAL_DIRECTORY);
    push_u16_le(&mut zip, 0);
    push_u16_le(&mut zip, 0);
    push_u16_le(&mut zip, entry_count);
    push_u16_le(&mut zip, entry_count);
    push_u32_le(&mut zip, central_len);
    push_u32_le(&mut zip, central_offset);
    push_u16_le(&mut zip, 0);
    Ok(zip)
}

fn zip_crc32(data: &[u8]) -> u32 {
    let mut crc = !0u32;
    for byte in data {
        crc ^= u32::from(*byte);
        for _ in 0..8 {
            crc = (crc >> 1) ^ (0xEDB8_8320 & (0u32.wrapping_sub(crc & 1)));
        }
    }
    !crc
}

fn ti8xp_bytes(
    request: &PackageRequest,
    context: &PackageContext,
    code: &[u8],
) -> Result<Vec<u8>, PackageError> {
    let raw_name = context
        .ti8xp
        .as_ref()
        .and_then(|options| options.variable_name.as_deref())
        .unwrap_or_else(|| executable_name(request, context));
    let name = ti8xp_variable_name(raw_name)?;
    let mut program = ti8xp_payload_prefix(&request.target)?.to_vec();
    program.extend_from_slice(code);
    let program_len = u16::try_from(program.len())
        .map_err(|_| PackageError::new("TI .8xp program exceeds 65535 bytes"))?;
    let payload_len = program_len
        .checked_add(2)
        .ok_or_else(|| PackageError::new("TI .8xp payload exceeds 65535 bytes"))?;

    let mut data = Vec::new();
    push_u16_le(&mut data, 13);
    push_u16_le(&mut data, payload_len);
    data.push(0x06);
    data.extend_from_slice(&name);
    data.push(0x00);
    data.push(0x00);
    push_u16_le(&mut data, payload_len);
    push_u16_le(&mut data, program_len);
    data.extend_from_slice(&program);
    let data_len = u16::try_from(data.len())
        .map_err(|_| PackageError::new("TI .8xp data section exceeds 65535 bytes"))?;
    let checksum = data
        .iter()
        .fold(0u16, |sum, byte| sum.wrapping_add(u16::from(*byte)));

    let mut out = Vec::with_capacity(11 + 42 + 2 + data.len() + 2);
    out.extend_from_slice(b"**TI83F*\x1A\x0A\x00");
    let mut comment = [0u8; 42];
    let text = b"Generated by ezrac";
    comment[..text.len()].copy_from_slice(text);
    out.extend_from_slice(&comment);
    push_u16_le(&mut out, data_len);
    out.extend_from_slice(&data);
    push_u16_le(&mut out, checksum);
    Ok(out)
}

fn ti8xp_payload_prefix(target: &str) -> Result<&'static [u8], PackageError> {
    if target.starts_with("ti84plusce-ez80") || target.starts_with("ti83premiumce-ez80") {
        Ok(&[0xEF, 0x7B])
    } else if target.starts_with("ti83-z80")
        || target.starts_with("ti83plus-z80")
        || target.starts_with("ti84-z80")
        || target.starts_with("ti84plus-z80")
    {
        Ok(&[0xBB, 0x6D])
    } else {
        Err(PackageError::new(format!(
            "target `{target}` does not support TI .8xp output"
        )))
    }
}

fn ti8xp_variable_name(raw: &str) -> Result<[u8; 8], PackageError> {
    let mut out = [0u8; 8];
    let mut len = 0;
    for ch in raw.chars() {
        if len == out.len() {
            break;
        }
        if ch.is_ascii_alphanumeric() || ch == '_' {
            out[len] = ch.to_ascii_uppercase() as u8;
            len += 1;
        }
    }
    if len == 0 {
        return Err(PackageError::new(format!(
            "TI .8xp variable name `{raw}` does not contain any ASCII letters, digits, or underscores"
        )));
    }
    Ok(out)
}

fn push_u16_le(out: &mut Vec<u8>, value: u16) {
    out.extend_from_slice(&value.to_le_bytes());
}

fn push_u32_le(out: &mut Vec<u8>, value: u32) {
    out.extend_from_slice(&value.to_le_bytes());
}

fn zx_spectrum_tap_bytes(
    request: &PackageRequest,
    context: &PackageContext,
    code: &[u8],
) -> Result<Vec<u8>, PackageError> {
    if !request.target.starts_with("zxspectrum-z80") {
        return Err(PackageError::new(format!(
            "target `{}` does not support ZX Spectrum .tap output",
            request.target
        )));
    }
    if request.target == "zxspectrum-z80-128k" {
        return zx_spectrum_128k_tap_bytes(request, context, code);
    }
    let load = u16::try_from(request.load_addr)
        .map_err(|_| PackageError::new("ZX Spectrum load address exceeds 16-bit address space"))?;
    let entry = u16::try_from(request.entry_addr)
        .map_err(|_| PackageError::new("ZX Spectrum entry address exceeds 16-bit address space"))?;
    let ram_top = load
        .checked_sub(1)
        .ok_or_else(|| PackageError::new("ZX Spectrum CODE load address must be above zero"))?;
    let length = u16::try_from(code.len())
        .map_err(|_| PackageError::new("ZX Spectrum CODE block exceeds 65535 bytes"))?;
    let name = zx_tap_name(executable_name(request, context));

    let mut loader = Vec::new();
    let mut clear = vec![0xfd, b' '];
    push_zx_basic_integer(&mut clear, ram_top);
    push_zx_basic_line(&mut loader, 10, &clear)?;
    push_zx_basic_line(&mut loader, 20, &[0xef, b' ', b'"', b'"', b' ', 0xaf])?;
    let mut run = vec![0xf9, b' ', 0xc0, b' '];
    push_zx_basic_integer(&mut run, entry);
    push_zx_basic_line(&mut loader, 30, &run)?;
    let loader_length = u16::try_from(loader.len())
        .map_err(|_| PackageError::new("ZX Spectrum BASIC loader exceeds 65535 bytes"))?;

    let mut loader_header = Vec::with_capacity(17);
    loader_header.push(0);
    loader_header.extend_from_slice(&name);
    loader_header.extend_from_slice(&loader_length.to_le_bytes());
    loader_header.extend_from_slice(&10u16.to_le_bytes());
    loader_header.extend_from_slice(&loader_length.to_le_bytes());
    let mut out = Vec::new();
    push_zx_tap_block(&mut out, 0x00, &loader_header)?;
    push_zx_tap_block(&mut out, 0xff, &loader)?;
    push_zx_code_block(&mut out, name, load, length, code)?;
    Ok(out)
}

const ZX_128K_BANK_WINDOW: u16 = 0xC000;
const ZX_128K_BANK_SIZE: usize = 0x4000;
const ZX_128K_PAGE_PORT: u16 = 0x7FFD;

fn zx_spectrum_128k_tap_bytes(
    request: &PackageRequest,
    context: &PackageContext,
    code: &[u8],
) -> Result<Vec<u8>, PackageError> {
    let load = u16::try_from(request.load_addr)
        .map_err(|_| PackageError::new("ZX Spectrum load address exceeds 16-bit address space"))?;
    let entry = u16::try_from(request.entry_addr)
        .map_err(|_| PackageError::new("ZX Spectrum entry address exceeds 16-bit address space"))?;
    if load != 0x8000 || !(load..=0xBFFF).contains(&entry) {
        return Err(PackageError::new(
            "the `zxspectrum-z80-128k` target requires resident code and its entry point in 0x8000..0xBFFF",
        ));
    }
    let code_length = u16::try_from(code.len())
        .map_err(|_| PackageError::new("ZX Spectrum resident CODE block exceeds 65535 bytes"))?;
    let mut banks = context
        .zx_spectrum
        .as_ref()
        .map(|options| options.banks.clone())
        .unwrap_or_default();
    banks.sort_by_key(|bank| bank.page);
    for bank in &banks {
        if bank.bytes.len() > ZX_128K_BANK_SIZE {
            return Err(PackageError::new(format!(
                "ZX Spectrum RAM page {} payload is {} bytes, but a pageable RAM bank holds at most {} bytes",
                bank.page,
                bank.bytes.len(),
                ZX_128K_BANK_SIZE
            )));
        }
    }

    let mut loader = Vec::new();
    let mut clear = vec![0xfd, b' '];
    push_zx_basic_integer(&mut clear, 0x5FFF);
    push_zx_basic_line(&mut loader, 10, &clear)?;
    push_zx_basic_line(&mut loader, 20, &[0xef, b' ', b'"', b'"', b' ', 0xaf])?;
    let mut line = 30u16;
    for bank in &banks {
        let mut page = vec![0xdf, b' '];
        push_zx_basic_integer(&mut page, ZX_128K_PAGE_PORT);
        page.extend_from_slice(b", ");
        push_zx_basic_integer(&mut page, u16::from(bank.page));
        push_zx_basic_line(&mut loader, line, &page)?;
        line = line
            .checked_add(10)
            .ok_or_else(|| PackageError::new("ZX Spectrum BASIC loader line number overflow"))?;
        push_zx_basic_line(&mut loader, line, &[0xef, b' ', b'"', b'"', b' ', 0xaf])?;
        line = line
            .checked_add(10)
            .ok_or_else(|| PackageError::new("ZX Spectrum BASIC loader line number overflow"))?;
    }
    let mut restore_page_zero = vec![0xdf, b' '];
    push_zx_basic_integer(&mut restore_page_zero, ZX_128K_PAGE_PORT);
    restore_page_zero.extend_from_slice(b", ");
    push_zx_basic_integer(&mut restore_page_zero, 0);
    push_zx_basic_line(&mut loader, line, &restore_page_zero)?;
    line = line
        .checked_add(10)
        .ok_or_else(|| PackageError::new("ZX Spectrum BASIC loader line number overflow"))?;
    let mut run = vec![0xf9, b' ', 0xc0, b' '];
    push_zx_basic_integer(&mut run, entry);
    push_zx_basic_line(&mut loader, line, &run)?;

    let loader_length = u16::try_from(loader.len())
        .map_err(|_| PackageError::new("ZX Spectrum BASIC loader exceeds 65535 bytes"))?;
    let name = zx_tap_name(executable_name(request, context));
    let mut loader_header = Vec::with_capacity(17);
    loader_header.push(0);
    loader_header.extend_from_slice(&name);
    loader_header.extend_from_slice(&loader_length.to_le_bytes());
    loader_header.extend_from_slice(&10u16.to_le_bytes());
    loader_header.extend_from_slice(&loader_length.to_le_bytes());
    let mut out = Vec::new();
    push_zx_tap_block(&mut out, 0x00, &loader_header)?;
    push_zx_tap_block(&mut out, 0xff, &loader)?;
    push_zx_code_block(&mut out, name, load, code_length, code)?;
    for bank in &banks {
        let length = u16::try_from(bank.bytes.len())
            .map_err(|_| PackageError::new("ZX Spectrum RAM bank payload exceeds 65535 bytes"))?;
        let default_name = format!("BANK{}", bank.page);
        let name = zx_tap_name(bank.name.as_deref().unwrap_or(&default_name));
        push_zx_code_block(&mut out, name, ZX_128K_BANK_WINDOW, length, &bank.bytes)?;
    }
    Ok(out)
}

fn push_zx_code_block(
    out: &mut Vec<u8>,
    name: [u8; 10],
    load: u16,
    length: u16,
    payload: &[u8],
) -> Result<(), PackageError> {
    let mut header = Vec::with_capacity(17);
    header.push(3);
    header.extend_from_slice(&name);
    header.extend_from_slice(&length.to_le_bytes());
    header.extend_from_slice(&load.to_le_bytes());
    header.extend_from_slice(&0u16.to_le_bytes());
    push_zx_tap_block(out, 0x00, &header)?;
    push_zx_tap_block(out, 0xff, payload)
}

fn push_zx_basic_integer(out: &mut Vec<u8>, value: u16) {
    out.extend_from_slice(value.to_string().as_bytes());
    out.push(0x0e);
    out.extend_from_slice(&[0x00, 0x00, value as u8, (value >> 8) as u8, 0x00]);
}

fn push_zx_basic_line(out: &mut Vec<u8>, number: u16, body: &[u8]) -> Result<(), PackageError> {
    let length = body
        .len()
        .checked_add(1)
        .and_then(|length| u16::try_from(length).ok())
        .ok_or_else(|| PackageError::new("ZX Spectrum BASIC line exceeds 65535 bytes"))?;
    out.extend_from_slice(&number.to_be_bytes());
    out.extend_from_slice(&length.to_le_bytes());
    out.extend_from_slice(body);
    out.push(0x0d);
    Ok(())
}

fn zx_tap_name(raw: &str) -> [u8; 10] {
    let mut name = [b' '; 10];
    for (slot, ch) in name.iter_mut().zip(raw.chars()) {
        *slot = if ch.is_ascii_alphanumeric() || ch == '_' {
            ch.to_ascii_uppercase() as u8
        } else {
            b'_'
        };
    }
    name
}

fn push_zx_tap_block(out: &mut Vec<u8>, flag: u8, data: &[u8]) -> Result<(), PackageError> {
    let block_len = data
        .len()
        .checked_add(2)
        .ok_or_else(|| PackageError::new("ZX Spectrum TAP block is too large"))?;
    let block_len = u16::try_from(block_len)
        .map_err(|_| PackageError::new("ZX Spectrum TAP block exceeds 65535 bytes"))?;
    out.extend_from_slice(&block_len.to_le_bytes());
    out.push(flag);
    out.extend_from_slice(data);
    out.push(data.iter().fold(flag, |checksum, byte| checksum ^ byte));
    Ok(())
}

fn game_boy_rom_bytes(
    request: &PackageRequest,
    context: &PackageContext,
    code: &[u8],
) -> Result<Vec<u8>, PackageError> {
    if !request.target.starts_with("gameboy-") {
        return Err(PackageError::new(format!(
            "target `{}` does not support Game Boy .gb output",
            request.target
        )));
    }
    if request.load_addr != 0x0150 || request.entry_addr != 0x0150 {
        return Err(PackageError::new(
            "Game Boy ROM layouts must load and enter at 0x0150",
        ));
    }
    const BANK_SIZE: usize = 0x4000;
    const INITIAL_ROM_SIZE: usize = 0x8000;
    const CODE_OFFSET: usize = 0x0150;
    let options = context.game_boy.clone().unwrap_or_default();
    let payload_banks = game_boy_payload_banks(options.mapper, options.bank_payloads.len())?;
    for payload in &options.bank_payloads {
        if payload.len() > BANK_SIZE {
            return Err(PackageError::new(format!(
                "Game Boy switchable ROM bank payload is {} bytes, but a bank holds at most {} bytes",
                payload.len(),
                BANK_SIZE
            )));
        }
    }
    for payload in &options.generated_bank_payloads {
        validate_game_boy_generated_bank(options.mapper, payload.bank)?;
        if payload.bytes.len() > BANK_SIZE {
            return Err(PackageError::new(format!(
                "Game Boy generated ROM bank {} is {} bytes, but a bank holds at most {} bytes",
                payload.bank,
                payload.bytes.len(),
                BANK_SIZE
            )));
        }
        if payload_banks.contains(&payload.bank) {
            return Err(PackageError::new(format!(
                "Game Boy ROM bank {} is used by both generated and configured payloads",
                payload.bank
            )));
        }
        if options
            .generated_bank_payloads
            .iter()
            .filter(|other| other.bank == payload.bank)
            .count()
            != 1
        {
            return Err(PackageError::new(format!(
                "Game Boy generated ROM bank {} has multiple payloads",
                payload.bank
            )));
        }
    }
    let required_banks = payload_banks
        .iter()
        .copied()
        .chain(
            options
                .generated_bank_payloads
                .iter()
                .map(|payload| payload.bank),
        )
        .max()
        .map(|bank| bank + 1)
        .unwrap_or(2);
    let rom_banks = game_boy_rom_banks(&options, required_banks)?;
    let rom_size = rom_banks
        .checked_mul(BANK_SIZE)
        .ok_or_else(|| PackageError::new("Game Boy ROM size overflow"))?;
    let fixed_code_capacity = if options.explicit_banking {
        BANK_SIZE - CODE_OFFSET
    } else {
        INITIAL_ROM_SIZE - CODE_OFFSET
    };
    if code.len() > fixed_code_capacity {
        return Err(PackageError::new(format!(
            "Game Boy fixed-bank code is {} bytes, but bank 0 supports at most {} bytes from 0x0150 when explicit banking is enabled",
            code.len(),
            fixed_code_capacity
        )));
    }

    let (cartridge_type, ram_size_code) = game_boy_cartridge_header(&options)?;
    let mut rom = vec![0xFF; rom_size];
    rom[0x0100..0x0104].copy_from_slice(&[0xC3, 0x50, 0x01, 0x00]);
    rom[0x0104..0x0134].copy_from_slice(&[
        0xCE, 0xED, 0x66, 0x66, 0xCC, 0x0D, 0x00, 0x0B, 0x03, 0x73, 0x00, 0x83, 0x00, 0x0C, 0x00,
        0x0D, 0x00, 0x08, 0x11, 0x1F, 0x88, 0x89, 0x00, 0x0E, 0xDC, 0xCC, 0x6E, 0xE6, 0xDD, 0xDD,
        0xD9, 0x99, 0xBB, 0xBB, 0x67, 0x63, 0x6E, 0x0E, 0xEC, 0xCC, 0xDD, 0xDC, 0x99, 0x9F, 0xBB,
        0xB9, 0x33, 0x3E,
    ]);
    rom[0x0134..0x0144].fill(0);
    for (slot, ch) in rom[0x0134..0x0143]
        .iter_mut()
        .zip(executable_name(request, context).bytes())
    {
        *slot = if ch.is_ascii_alphanumeric() || ch == b' ' {
            ch
        } else {
            b'_'
        };
    }
    rom[0x0143] = if request.target.starts_with("gameboy-color-") {
        0xC0
    } else {
        0x00
    };
    rom[0x0144..0x0146].copy_from_slice(b"00");
    rom[0x0146] = 0x00;
    rom[0x0147] = cartridge_type;
    rom[0x0148] = game_boy_rom_size_code(rom_banks)?;
    rom[0x0149] = ram_size_code;
    rom[0x014A] = 0x01;
    rom[0x014B] = 0x33;
    rom[0x014C] = 0x00;
    rom[CODE_OFFSET..CODE_OFFSET + code.len()].copy_from_slice(code);
    for (index, payload) in options.bank_payloads.iter().enumerate() {
        let offset = payload_banks[index] * BANK_SIZE;
        rom[offset..offset + payload.len()].copy_from_slice(payload);
    }
    for payload in &options.generated_bank_payloads {
        let offset = payload.bank * BANK_SIZE;
        rom[offset..offset + payload.bytes.len()].copy_from_slice(&payload.bytes);
    }
    rom[0x014D] = rom[0x0134..=0x014C].iter().fold(0u8, |checksum, byte| {
        checksum.wrapping_sub(*byte).wrapping_sub(1)
    });
    let checksum = rom
        .iter()
        .enumerate()
        .filter(|(index, _)| !matches!(*index, 0x014E | 0x014F))
        .fold(0u16, |sum, (_, byte)| sum.wrapping_add(u16::from(*byte)));
    rom[0x014E..0x0150].copy_from_slice(&checksum.to_be_bytes());
    Ok(rom)
}

fn validate_game_boy_generated_bank(
    mapper: GameBoyMapper,
    bank: usize,
) -> Result<(), PackageError> {
    let maximum = match mapper {
        GameBoyMapper::RomOnly => 0,
        GameBoyMapper::Mbc1 => 127,
        GameBoyMapper::Mbc5 => 511,
    };
    if bank == 0 || bank > maximum || (mapper == GameBoyMapper::Mbc1 && bank & 0x1F == 0) {
        return Err(PackageError::new(format!(
            "Game Boy mapper `{}` cannot select explicit ROM bank {bank}",
            game_boy_mapper_name(mapper)
        )));
    }
    Ok(())
}

fn game_boy_payload_banks(
    mapper: GameBoyMapper,
    payload_count: usize,
) -> Result<Vec<usize>, PackageError> {
    if payload_count == 0 {
        return Ok(Vec::new());
    }
    let maximum = match mapper {
        GameBoyMapper::RomOnly => 1,
        GameBoyMapper::Mbc1 => 127,
        GameBoyMapper::Mbc5 => 511,
    };
    let mut banks = Vec::with_capacity(payload_count);
    for bank in 2..=maximum {
        if mapper == GameBoyMapper::Mbc1 && bank & 0x1F == 0 {
            continue;
        }
        banks.push(bank);
        if banks.len() == payload_count {
            return Ok(banks);
        }
    }
    Err(PackageError::new(format!(
        "Game Boy mapper `{}` supports at most {} configured switchable bank payload(s)",
        game_boy_mapper_name(mapper),
        banks.len()
    )))
}

fn game_boy_rom_banks(
    options: &GameBoyPackageOptions,
    required_banks: usize,
) -> Result<usize, PackageError> {
    let mapper_max_banks = match options.mapper {
        GameBoyMapper::RomOnly => 2,
        GameBoyMapper::Mbc1 => 128,
        GameBoyMapper::Mbc5 => 512,
    };
    let rom_banks = options
        .rom_banks
        .map(usize::from)
        .unwrap_or_else(|| required_banks.next_power_of_two().max(2));
    if !rom_banks.is_power_of_two() || !(2..=512).contains(&rom_banks) {
        return Err(PackageError::new(
            "Game Boy `rom_banks` must be a power of two from 2 through 512",
        ));
    }
    if rom_banks < required_banks {
        return Err(PackageError::new(format!(
            "Game Boy `rom_banks` is {rom_banks}, but {required_banks} banks are required"
        )));
    }
    if rom_banks > mapper_max_banks {
        return Err(PackageError::new(format!(
            "Game Boy mapper `{}` supports at most {mapper_max_banks} ROM banks, not {rom_banks}",
            game_boy_mapper_name(options.mapper)
        )));
    }
    if options.mapper == GameBoyMapper::RomOnly && (required_banks != 2 || rom_banks != 2) {
        return Err(PackageError::new(
            "Game Boy ROM-only cartridges cannot use bank payloads or more than two ROM banks",
        ));
    }
    Ok(rom_banks)
}

fn game_boy_mapper_name(mapper: GameBoyMapper) -> &'static str {
    match mapper {
        GameBoyMapper::RomOnly => "rom-only",
        GameBoyMapper::Mbc1 => "mbc1",
        GameBoyMapper::Mbc5 => "mbc5",
    }
}

fn game_boy_rom_size_code(rom_banks: usize) -> Result<u8, PackageError> {
    match rom_banks {
        2 | 4 | 8 | 16 | 32 | 64 | 128 | 256 | 512 => Ok(rom_banks.trailing_zeros() as u8 - 1),
        _ => Err(PackageError::new(format!(
            "unsupported Game Boy ROM bank count {rom_banks}"
        ))),
    }
}

fn game_boy_cartridge_header(options: &GameBoyPackageOptions) -> Result<(u8, u8), PackageError> {
    let ram_size_code = match options.ram_banks {
        0 => 0x00,
        1 => 0x02,
        4 => 0x03,
        8 => 0x05,
        16 => 0x04,
        _ => {
            return Err(PackageError::new(
                "Game Boy RAM bank count must be one of 0, 1, 4, 8, or 16",
            ));
        }
    };
    if options.battery && options.ram_banks == 0 {
        return Err(PackageError::new(
            "Game Boy battery-backed cartridges require at least one external RAM bank",
        ));
    }
    if options.mapper == GameBoyMapper::Mbc1 && options.ram_banks > 4 {
        return Err(PackageError::new(
            "Game Boy MBC1 cartridges support at most four external RAM banks",
        ));
    }
    if options.mapper == GameBoyMapper::Mbc5 && options.rumble && options.ram_banks > 8 {
        return Err(PackageError::new(
            "Game Boy MBC5 rumble cartridges support at most eight external RAM banks",
        ));
    }
    let cartridge_type = match options.mapper {
        GameBoyMapper::RomOnly if options.ram_banks == 0 && !options.battery && !options.rumble => {
            0x00
        }
        GameBoyMapper::RomOnly => {
            return Err(PackageError::new(
                "Game Boy ROM-only cartridges cannot declare RAM, battery, or rumble",
            ));
        }
        GameBoyMapper::Mbc1 if options.rumble => {
            return Err(PackageError::new(
                "Game Boy MBC1 cartridges do not support rumble",
            ));
        }
        GameBoyMapper::Mbc1 if options.ram_banks == 0 => 0x01,
        GameBoyMapper::Mbc1 if options.battery => 0x03,
        GameBoyMapper::Mbc1 => 0x02,
        GameBoyMapper::Mbc5 if options.rumble && options.ram_banks == 0 => 0x1C,
        GameBoyMapper::Mbc5 if options.rumble && options.battery => 0x1E,
        GameBoyMapper::Mbc5 if options.rumble => 0x1D,
        GameBoyMapper::Mbc5 if options.ram_banks == 0 => 0x19,
        GameBoyMapper::Mbc5 if options.battery => 0x1B,
        GameBoyMapper::Mbc5 => 0x1A,
    };
    Ok((cartridge_type, ram_size_code))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn packages_agon_mos_in_memory() {
        let image = package_executable(
            &PackageRequest::new("agonlight-mos-ez80", OutputFormat::RawBin, 0x40000, 0x40045),
            &[0],
        )
        .unwrap();
        assert_eq!(&image[64..69], b"MOS\0\x01");
    }
    #[test]
    fn packages_c64_prg_in_memory() {
        let image = package_executable(
            &PackageRequest::new(
                "commodore64-6502",
                OutputFormat::Commodore64Prg,
                0x80d,
                0x80d,
            ),
            &[0xea],
        )
        .unwrap();
        assert_eq!(&image[..2], &[1, 8]);
    }

    #[test]
    fn packages_arduboy_with_resolved_metadata_and_name() {
        let context = PackageContext {
            executable_name: Some("demo".to_owned()),
            arduboy: Some(ArduboyPackageOptions {
                title: "Demo".to_owned(),
                author: "Ezra".to_owned(),
                version: "1.0".to_owned(),
                description: Some("A test".to_owned()),
                date: None,
                genre: None,
                source_url: None,
            }),
            ..PackageContext::new()
        };
        let image = package_executable_with_context(
            &PackageRequest::new("arduboy-avr", OutputFormat::Arduboy, 0, 0),
            &context,
            &[0],
        )
        .unwrap();
        assert!(
            image
                .windows(b"info.json".len())
                .any(|bytes| bytes == b"info.json")
        );
        assert!(
            image
                .windows(b"demo.hex".len())
                .any(|bytes| bytes == b"demo.hex")
        );
        assert!(
            image
                .windows(b"\"title\":\"Demo\"".len())
                .any(|bytes| bytes == b"\"title\":\"Demo\"")
        );
    }

    #[test]
    fn packages_ti8xp_with_context_variable_name() {
        let context = PackageContext {
            ti8xp: Some(Ti8xpPackageOptions {
                variable_name: Some("demo-1".to_owned()),
            }),
            ..PackageContext::new()
        };
        let image = package_executable_with_context(
            &PackageRequest::new("ti84-z80", OutputFormat::Ti8xp, 0, 0),
            &context,
            &[0xc9],
        )
        .unwrap();
        assert_eq!(&image[..11], b"**TI83F*\x1A\x0A\x00");
        assert!(image.windows(8).any(|bytes| bytes == b"DEMO1\0\0\0"));
        assert!(image.windows(3).any(|bytes| bytes == [0xBB, 0x6D, 0xC9]));
    }

    #[test]
    fn packages_zx_tap_with_resolved_executable_name() {
        let context = PackageContext {
            executable_name: Some("demo-game".to_owned()),
            ..PackageContext::new()
        };
        let image = package_executable_with_context(
            &PackageRequest::new(
                "zxspectrum-z80-48k",
                OutputFormat::ZxSpectrumTap,
                0x8000,
                0x8000,
            ),
            &context,
            &[0xc9],
        )
        .unwrap();
        assert_eq!(&image[4..14], b"DEMO_GAME ");
        assert!(image.windows(1).any(|bytes| bytes == [0xc9]));
    }

    #[test]
    fn packages_game_boy_with_resolved_bank_payloads() {
        let context = PackageContext {
            executable_name: Some("Demo Game".to_owned()),
            game_boy: Some(GameBoyPackageOptions {
                mapper: GameBoyMapper::Mbc1,
                rom_banks: Some(4),
                ram_banks: 0,
                battery: false,
                rumble: false,
                bank_payloads: vec![vec![0x42]],
                generated_bank_payloads: vec![GameBoyBankPayload {
                    bank: 3,
                    bytes: vec![0x24],
                }],
                explicit_banking: true,
            }),
            ..PackageContext::new()
        };
        let image = package_executable_with_context(
            &PackageRequest::new("gameboy-lr35902", OutputFormat::GameBoyGb, 0x0150, 0x0150),
            &context,
            &[0xc9],
        )
        .unwrap();
        assert_eq!(image.len(), 0x10000);
        assert_eq!(&image[0x0134..0x013D], b"Demo Game");
        assert_eq!(image[0x8000], 0x42);
        assert_eq!(image[0xC000], 0x24);
    }
}
