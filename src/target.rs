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
    Z80N,
    Z180,
    M68k,
    I8080,
    I8085,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AssemblerCpu {
    I8080,
    I8085,
    Z80,
    Z80N,
    Z180,
    Ez80,
}

impl AssemblerCpu {
    pub fn parse(value: &str) -> Result<Self, String> {
        match value {
            "i8080" | "8080" => Ok(Self::I8080),
            "i8085" | "8085" => Ok(Self::I8085),
            "z80" => Ok(Self::Z80),
            "z80n" => Ok(Self::Z80N),
            "z180" => Ok(Self::Z180),
            "ez80" => Ok(Self::Ez80),
            _ => Err(format!(
                "unsupported assembler CPU `{value}`; expected i8080, i8085, z80, z80n, z180, or ez80"
            )),
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::I8080 => "i8080",
            Self::I8085 => "i8085",
            Self::Z80 => "z80",
            Self::Z80N => "z80n",
            Self::Z180 => "z180",
            Self::Ez80 => "ez80",
        }
    }

    pub fn encoding_family(self) -> Option<CpuFamily> {
        match self {
            Self::Z80 => Some(CpuFamily::Z80),
            Self::Z80N => Some(CpuFamily::Z80),
            Self::Z180 => Some(CpuFamily::Z80),
            Self::Ez80 => Some(CpuFamily::Ez80),
            Self::I8080 | Self::I8085 => None,
        }
    }

    pub fn supports_z80_syntax(self) -> bool {
        matches!(self, Self::Z80 | Self::Z80N | Self::Z180 | Self::Ez80)
    }

    pub fn supports_ez80_syntax(self) -> bool {
        self == Self::Ez80
    }
}

impl From<CpuFamily> for AssemblerCpu {
    fn from(cpu: CpuFamily) -> Self {
        match cpu {
            CpuFamily::Ez80 => Self::Ez80,
            CpuFamily::Z80 => Self::Z80,
            CpuFamily::Z80N => Self::Z80N,
            CpuFamily::Z180 => Self::Z180,
            CpuFamily::I8080 => Self::I8080,
            CpuFamily::I8085 => Self::I8085,
            CpuFamily::M68k => Self::Ez80,
        }
    }
}

impl CpuFamily {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Ez80 => "ez80",
            Self::Z80 => "z80",
            Self::Z80N => "z80n",
            Self::Z180 => "z180",
            Self::M68k => "m68k",
            Self::I8080 => "i8080",
            Self::I8085 => "i8085",
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
    pub memory: TargetMemoryModel,
    pub default_sdk_symbols: bool,
    pub output_format: OutputFormat,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TargetMemoryModel {
    pub pointer_width_bits: u16,
    pub address_width_bits: u16,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OutputFormat {
    CpmCom,
    IntelHex,
    RawBin,
    Ti8ek,
    Ti8xp,
    Ti8xk,
}

impl OutputFormat {
    pub const fn extension(self) -> &'static str {
        match self {
            Self::CpmCom => "com",
            Self::IntelHex => "hex",
            Self::RawBin => "bin",
            Self::Ti8ek => "8ek",
            Self::Ti8xp => "8xp",
            Self::Ti8xk => "8xk",
        }
    }
}

pub const DEFAULT_TARGET_TRIPLE: &str = "custom-unknown-ez80";

pub fn resolve_target_profile(target: Option<&str>) -> Result<TargetProfile, String> {
    let triple = parse_target_triple(target.unwrap_or(DEFAULT_TARGET_TRIPLE))?;
    let Some(memory) = memory_model_for_cpu(triple.cpu) else {
        return Err(format!(
            "target `{}` uses CPU `{}`, but no target profile is implemented",
            triple.value,
            triple.cpu.as_str()
        ));
    };
    Ok(TargetProfile {
        output_format: output_format_for_target(&triple),
        memory,
        default_sdk_symbols: !is_bare_target(&triple),
        triple,
    })
}

fn is_bare_target(triple: &TargetTriple) -> bool {
    triple.value.split('-').any(|part| part == "bare")
}

fn output_format_for_target(triple: &TargetTriple) -> OutputFormat {
    if matches!(
        triple.cpu,
        CpuFamily::Z80 | CpuFamily::Z80N | CpuFamily::Z180 | CpuFamily::I8080 | CpuFamily::I8085
    ) && triple.value.split('-').any(|part| part == "cpm")
    {
        OutputFormat::CpmCom
    } else if is_ti_calculator_target(triple) {
        OutputFormat::Ti8xp
    } else {
        OutputFormat::RawBin
    }
}

fn is_ti_calculator_target(triple: &TargetTriple) -> bool {
    triple.value.starts_with("ti83-z80")
        || triple.value.starts_with("ti83plus-z80")
        || triple.value.starts_with("ti84-z80")
        || triple.value.starts_with("ti84plus-z80")
        || triple.value.starts_with("ti84plusce-ez80")
        || triple.value.starts_with("ti83premiumce-ez80")
}

pub fn memory_model_for_cpu(cpu: CpuFamily) -> Option<TargetMemoryModel> {
    match cpu {
        CpuFamily::Ez80 => Some(TargetMemoryModel {
            pointer_width_bits: 24,
            address_width_bits: 24,
        }),
        CpuFamily::Z80 | CpuFamily::Z80N | CpuFamily::Z180 => Some(TargetMemoryModel {
            pointer_width_bits: 16,
            address_width_bits: 16,
        }),
        CpuFamily::I8080 | CpuFamily::I8085 => Some(TargetMemoryModel {
            pointer_width_bits: 16,
            address_width_bits: 16,
        }),
        CpuFamily::M68k => None,
    }
}

pub fn parse_output_format(value: &str) -> Result<OutputFormat, String> {
    match value {
        "bin" => Ok(OutputFormat::RawBin),
        "com" => Ok(OutputFormat::CpmCom),
        "hex" | "ihex" | "intel-hex" => Ok(OutputFormat::IntelHex),
        "8ek" | "ti8ek" => Ok(OutputFormat::Ti8ek),
        "8xp" | "ti8xp" => Ok(OutputFormat::Ti8xp),
        "8xk" | "ti8xk" => Ok(OutputFormat::Ti8xk),
        _ => Err(format!(
            "unsupported output format `{value}`; expected `bin`, `com`, `hex`, `8xp`, `8ek`, or `8xk`"
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
            "z180" => Some(CpuFamily::Z180),
            "z80n" => Some(CpuFamily::Z80N),
            "z80" => Some(CpuFamily::Z80),
            "m68k" => Some(CpuFamily::M68k),
            "i8080" | "8080" => Some(CpuFamily::I8080),
            "i8085" | "8085" => Some(CpuFamily::I8085),
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
            parse_target_triple("bare-z80n").unwrap().cpu,
            CpuFamily::Z80N
        );
        assert_eq!(
            parse_target_triple("bare-z180").unwrap().cpu,
            CpuFamily::Z180
        );
        assert_eq!(
            parse_target_triple("bare-i8085").unwrap().cpu,
            CpuFamily::I8085
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
    fn resolves_z80_and_ez80_target_profiles() {
        assert!(resolve_target_profile(Some("ti84plusce-ez80")).is_ok());
        let z80 = resolve_target_profile(Some("zxspectrum-z80")).unwrap();

        assert_eq!(z80.triple.cpu, CpuFamily::Z80);
        assert_eq!(z80.memory.pointer_width_bits, 16);
        assert_eq!(z80.memory.address_width_bits, 16);
    }

    #[test]
    fn cpm_z80_targets_default_to_com_output() {
        let cpm = resolve_target_profile(Some("cpm-2.2-z80")).unwrap();

        assert_eq!(cpm.output_format, OutputFormat::CpmCom);
        assert_eq!(cpm.output_format.extension(), "com");
    }

    #[test]
    fn cpm_8080_targets_default_to_com_output() {
        let cpm = resolve_target_profile(Some("cpm-2.2-i8080")).unwrap();

        assert_eq!(cpm.triple.cpu, CpuFamily::I8080);
        assert_eq!(cpm.output_format, OutputFormat::CpmCom);
        assert_eq!(cpm.output_format.extension(), "com");
        assert_eq!(cpm.memory.address_width_bits, 16);
    }

    #[test]
    fn cpm_8085_targets_default_to_com_output() {
        let cpm = resolve_target_profile(Some("cpm-2.2-i8085")).unwrap();

        assert_eq!(cpm.triple.cpu, CpuFamily::I8085);
        assert_eq!(cpm.output_format, OutputFormat::CpmCom);
        assert_eq!(cpm.output_format.extension(), "com");
        assert_eq!(cpm.memory.address_width_bits, 16);
    }

    #[test]
    fn resolves_bare_targets_without_default_sdk_symbols() {
        let target = resolve_target_profile(Some("bare-z180")).unwrap();

        assert_eq!(target.triple.cpu, CpuFamily::Z180);
        assert_eq!(target.output_format, OutputFormat::RawBin);
        assert_eq!(target.memory.address_width_bits, 16);
        assert!(!target.default_sdk_symbols);
    }

    #[test]
    fn ti_calculator_targets_default_to_8xp_output() {
        for target in [
            "ti83-z80",
            "ti84plus-z80",
            "ti84plusce-ez80",
            "ti83premiumce-ez80",
        ] {
            let target = resolve_target_profile(Some(target)).unwrap();
            assert_eq!(target.output_format, OutputFormat::Ti8xp);
            assert_eq!(target.output_format.extension(), "8xp");
        }
    }

    #[test]
    fn rejects_cpus_without_target_profiles_for_now() {
        let error = resolve_target_profile(Some("sega-genesis-m68k")).unwrap_err();
        assert!(
            error.contains("no target profile is implemented"),
            "{error}"
        );
    }

    #[test]
    fn parses_output_formats() {
        assert_eq!(parse_output_format("bin"), Ok(OutputFormat::RawBin));
        assert_eq!(parse_output_format("com"), Ok(OutputFormat::CpmCom));
        assert_eq!(parse_output_format("hex"), Ok(OutputFormat::IntelHex));
        assert_eq!(parse_output_format("8xp"), Ok(OutputFormat::Ti8xp));
        assert_eq!(parse_output_format("8ek"), Ok(OutputFormat::Ti8ek));
        assert_eq!(parse_output_format("8xk"), Ok(OutputFormat::Ti8xk));
        let error = parse_output_format("bad").unwrap_err();
        assert!(
            error.contains("expected `bin`, `com`, `hex`, `8xp`, `8ek`, or `8xk`"),
            "{error}"
        );
    }
}
