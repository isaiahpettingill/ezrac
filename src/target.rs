pub const CART_MAGIC: &[u8; 4] = b"EZRA";
pub const FORMAT_VERSION: u8 = 1;
pub const CPU_MODE_EZ80_ADL: u8 = 1;

pub const ADDRESS_SPACE_SIZE: u32 = 0x0100_0000;
pub const MAX_ADDR: Address24 = Address24::new(0xFF_FFFF);

pub const EZRA_LOAD_ADDR: Address24 = Address24::new(0x01_0000);
pub const EZRA_ENTRY_ADDR: Address24 = Address24::new(0x01_0040);
pub const EZRA_CODE_BASE: Address24 = Address24::new(0x01_0040);
pub const EZRA_RODATA_BASE: Address24 = Address24::new(0x02_0000);
pub const EZRA_RAM_BASE: Address24 = Address24::new(0x04_0000);
pub const EZRA_VRAM_BASE: Address24 = Address24::new(0x08_0000);
pub const EZRA_AUDIO_BASE: Address24 = Address24::new(0x0C_0000);
pub const EZRA_ASSET_BASE: Address24 = Address24::new(0x10_0000);
pub const EZRA_STACK_TOP: Address24 = Address24::new(0xF0_0000);

pub const HEADER_SIZE: u16 = 0x0040;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CpuFamily {
    Ez80,
    Z80,
    M68k,
    I8080,
}

impl CpuFamily {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Ez80 => "ez80",
            Self::Z80 => "z80",
            Self::M68k => "m68k",
            Self::I8080 => "8080",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TargetTriple {
    pub value: String,
    pub cpu: CpuFamily,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TargetProfile {
    pub triple: TargetTriple,
    pub default_sdk_symbols: bool,
    pub output_format: OutputFormat,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OutputFormat {
    RawBin,
}

impl OutputFormat {
    pub const fn extension(self) -> &'static str {
        match self {
            Self::RawBin => "bin",
        }
    }
}

pub const DEFAULT_TARGET_TRIPLE: &str = "custom-unknown-ez80";

pub fn resolve_target_profile(target: Option<&str>) -> Result<TargetProfile, String> {
    let triple = parse_target_triple(target.unwrap_or(DEFAULT_TARGET_TRIPLE))?;
    if triple.cpu != CpuFamily::Ez80 {
        return Err(format!(
            "target `{}` uses CPU `{}`, but only eZ80 codegen is implemented",
            triple.value,
            triple.cpu.as_str()
        ));
    }
    Ok(TargetProfile {
        triple,
        default_sdk_symbols: true,
        output_format: OutputFormat::RawBin,
    })
}

pub fn parse_output_format(value: &str) -> Result<OutputFormat, String> {
    match value {
        "bin" => Ok(OutputFormat::RawBin),
        _ => Err(format!(
            "unsupported output format `{value}`; only `bin` is implemented"
        )),
    }
}

pub fn parse_target_triple(value: &str) -> Result<TargetTriple, String> {
    if value.trim() != value || value.is_empty() {
        return Err(format!("invalid target triple `{value}`"));
    }
    let parts = value.split('-').collect::<Vec<_>>();
    if parts.len() < 2 || parts.iter().any(|part| part.is_empty()) {
        return Err(format!("invalid target triple `{value}`"));
    }
    let cpu = parts
        .iter()
        .rev()
        .find_map(|part| match *part {
            "ez80" => Some(CpuFamily::Ez80),
            "z80" => Some(CpuFamily::Z80),
            "m68k" => Some(CpuFamily::M68k),
            "8080" => Some(CpuFamily::I8080),
            _ => None,
        })
        .ok_or_else(|| format!("target triple `{value}` is missing a supported CPU family"))?;
    Ok(TargetTriple {
        value: value.to_owned(),
        cpu,
    })
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct Address24(u32);

impl Address24 {
    pub const MAX: u32 = 0xFF_FFFF;

    pub const fn new(value: u32) -> Self {
        assert!(value <= Self::MAX);
        Self(value)
    }

    pub const fn get(self) -> u32 {
        self.0
    }

    pub const fn to_le_bytes3(self) -> [u8; 3] {
        [
            (self.0 & 0xFF) as u8,
            ((self.0 >> 8) & 0xFF) as u8,
            ((self.0 >> 16) & 0xFF) as u8,
        ]
    }
}

impl TryFrom<u32> for Address24 {
    type Error = AddressOutOfRange;

    fn try_from(value: u32) -> Result<Self, Self::Error> {
        if value <= Self::MAX {
            Ok(Self(value))
        } else {
            Err(AddressOutOfRange(value))
        }
    }
}

impl std::fmt::Display for Address24 {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "0x{:06X}", self.0)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AddressOutOfRange(pub u32);

impl std::fmt::Display for AddressOutOfRange {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "address 0x{:X} is outside the 24-bit address space",
            self.0
        )
    }
}

impl std::error::Error for AddressOutOfRange {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_target_triples_with_optional_versions() {
        assert_eq!(
            parse_target_triple("agonlight-console8-ez80-1.0")
                .unwrap()
                .cpu,
            CpuFamily::Ez80
        );
        assert_eq!(
            parse_target_triple("cpm-2.2-z80").unwrap().cpu,
            CpuFamily::Z80
        );
        assert_eq!(
            parse_target_triple("sega-genesis-m68k").unwrap().cpu,
            CpuFamily::M68k
        );
    }

    #[test]
    fn rejects_targets_without_known_cpu_family() {
        let error = parse_target_triple("agonlight-console8").unwrap_err();
        assert!(error.contains("missing a supported CPU family"), "{error}");
    }

    #[test]
    fn only_ez80_targets_have_codegen_for_now() {
        assert!(resolve_target_profile(Some("ti84plusce-ez80")).is_ok());
        let error = resolve_target_profile(Some("zxspectrum-z80")).unwrap_err();
        assert!(
            error.contains("only eZ80 codegen is implemented"),
            "{error}"
        );
    }

    #[test]
    fn raw_bin_is_the_only_implemented_output_format_for_now() {
        assert_eq!(parse_output_format("bin"), Ok(OutputFormat::RawBin));
        let error = parse_output_format("hex").unwrap_err();
        assert!(error.contains("only `bin` is implemented"), "{error}");
    }
}
