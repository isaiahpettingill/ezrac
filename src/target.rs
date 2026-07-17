use alloc::{borrow::ToOwned, format, string::String, vec::Vec};

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
    M6800,
    I8080,
    I8085,
    I8086,
    Lr35902,
    Avr,
    Mos6502,
    Cmos65C02,
    Wdc65C816,
    Ricoh2A03,
    Tms9900,
    Dcpu,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AssemblerCpu {
    I8080,
    I8085,
    I8086,
    Z80,
    Z80N,
    Z180,
    Ez80,
    Lr35902,
    Avr,
    M6800,
    M68k,
    Mos6502,
    Cmos65C02,
    Wdc65C816,
    Ricoh2A03,
    Tms9900,
    Dcpu,
}

impl AssemblerCpu {
    pub fn parse(value: &str) -> Result<Self, String> {
        let cpu = match value {
            "i8080" | "8080" => Self::I8080,
            "i8085" | "8085" => Self::I8085,
            "i8086" | "8086" => Self::I8086,
            "z80" => Self::Z80,
            "z80n" => Self::Z80N,
            "z180" => Self::Z180,
            "ez80" => Self::Ez80,
            "lr35902" | "gameboy" | "gb" => Self::Lr35902,
            "avr" | "atmega32u4" => Self::Avr,
            "m6800" | "6800" => Self::M6800,
            "m68k" | "68000" | "m68000" => Self::M68k,
            "6502" | "mos6502" | "m6502" => Self::Mos6502,
            "65c02" | "cmos65c02" => Self::Cmos65C02,
            "65c816" | "wdc65c816" | "65816" => Self::Wdc65C816,
            "2a03" | "ricoh2a03" | "nes" => Self::Ricoh2A03,
            "tms9900" | "9900" => Self::Tms9900,
            "dcpu" | "dcpu16" | "dcpu-16" => Self::Dcpu,
            _ => {
                return Err(format!(
                    "unsupported assembler CPU `{value}`; expected i8080, i8085, i8086, z80, z80n, z180, ez80, lr35902, 6502, 65c02, 65c816, 2a03, tms9900, dcpu, m6800, m68k, or avr"
                ));
            }
        };
        if cpu.is_enabled() {
            Ok(cpu)
        } else {
            Err(format!(
                "assembler CPU `{}` requires the `{}` Cargo feature",
                cpu.as_str(),
                cpu.feature_name()
            ))
        }
    }

    pub const fn is_enabled(self) -> bool {
        match self {
            Self::I8080 | Self::I8085 => cfg!(feature = "intel"),
            Self::I8086 => cfg!(feature = "i8086"),
            Self::Z80 | Self::Z80N | Self::Z180 | Self::Ez80 => cfg!(feature = "z80"),
            Self::Lr35902 => cfg!(feature = "lr35902"),
            Self::Avr => cfg!(feature = "avr"),
            Self::M6800 => cfg!(feature = "m6800"),
            Self::M68k => cfg!(feature = "m68k"),
            Self::Mos6502 | Self::Cmos65C02 | Self::Wdc65C816 | Self::Ricoh2A03 => {
                cfg!(feature = "mos6502")
            }
            Self::Tms9900 => cfg!(feature = "tms9900"),
            Self::Dcpu => cfg!(feature = "dcpu"),
        }
    }

    pub const fn feature_name(self) -> &'static str {
        match self {
            Self::I8080 | Self::I8085 => "intel",
            Self::I8086 => "i8086",
            Self::Z80 | Self::Z80N | Self::Z180 | Self::Ez80 => "z80",
            Self::Lr35902 => "lr35902",
            Self::Avr => "avr",
            Self::M6800 => "m6800",
            Self::M68k => "m68k",
            Self::Mos6502 | Self::Cmos65C02 | Self::Wdc65C816 | Self::Ricoh2A03 => "mos6502",
            Self::Tms9900 => "tms9900",
            Self::Dcpu => "dcpu",
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::I8080 => "i8080",
            Self::I8085 => "i8085",
            Self::I8086 => "i8086",
            Self::Z80 => "z80",
            Self::Z80N => "z80n",
            Self::Z180 => "z180",
            Self::Ez80 => "ez80",
            Self::Lr35902 => "lr35902",
            Self::Avr => "avr",
            Self::M6800 => "m6800",
            Self::M68k => "m68k",
            Self::Mos6502 => "6502",
            Self::Cmos65C02 => "65c02",
            Self::Wdc65C816 => "65c816",
            Self::Ricoh2A03 => "2a03",
            Self::Tms9900 => "tms9900",
            Self::Dcpu => "dcpu",
        }
    }

    pub fn encoding_family(self) -> Option<CpuFamily> {
        match self {
            Self::Z80 => Some(CpuFamily::Z80),
            Self::Z80N => Some(CpuFamily::Z80),
            Self::Z180 => Some(CpuFamily::Z80),
            Self::Ez80 => Some(CpuFamily::Ez80),
            Self::I8080 | Self::I8085 | Self::I8086 => None,
            Self::Lr35902
            | Self::M6800
            | Self::M68k
            | Self::Mos6502
            | Self::Cmos65C02
            | Self::Wdc65C816
            | Self::Ricoh2A03
            | Self::Tms9900
            | Self::Dcpu => None,
            Self::Avr => None,
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
            CpuFamily::I8086 => Self::I8086,
            CpuFamily::M68k => Self::M68k,
            CpuFamily::Lr35902 => Self::Lr35902,
            CpuFamily::Avr => Self::Avr,
            CpuFamily::M6800 => Self::M6800,
            CpuFamily::Mos6502 => Self::Mos6502,
            CpuFamily::Cmos65C02 => Self::Cmos65C02,
            CpuFamily::Wdc65C816 => Self::Wdc65C816,
            CpuFamily::Ricoh2A03 => Self::Ricoh2A03,
            CpuFamily::Tms9900 => Self::Tms9900,
            CpuFamily::Dcpu => Self::Dcpu,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TargetCapabilities {
    pub name: &'static str,
    pub memory: TargetMemoryModel,
    pub native_int_widths: &'static [u8],
    pub supports_port_io: bool,
    pub prefer_code_size: bool,
    pub has_cache: bool,
}

impl CpuFamily {
    pub const fn capabilities(self) -> TargetCapabilities {
        let memory16 = TargetMemoryModel {
            pointer_width_bits: 16,
            address_width_bits: 16,
        };

        let memory24 = TargetMemoryModel {
            pointer_width_bits: 24,
            address_width_bits: 24,
        };
        match self {
            Self::Ez80 => TargetCapabilities {
                name: "ez80-adl",
                memory: memory24,
                native_int_widths: &[8, 16, 24],
                supports_port_io: true,
                prefer_code_size: true,
                has_cache: false,
            },
            Self::Z80 | Self::Z80N | Self::Z180 | Self::I8080 | Self::I8085 => TargetCapabilities {
                name: self.as_str(),
                memory: memory16,
                native_int_widths: &[8, 16],
                supports_port_io: true,
                prefer_code_size: true,
                has_cache: false,
            },
            Self::I8086 => TargetCapabilities {
                name: self.as_str(),
                memory: memory16,
                native_int_widths: &[8, 16],
                supports_port_io: true,
                prefer_code_size: true,
                has_cache: false,
            },
            Self::M68k | Self::Wdc65C816 => TargetCapabilities {
                name: self.as_str(),
                memory: memory24,
                native_int_widths: &[8, 16, 24],
                supports_port_io: false,
                prefer_code_size: true,
                has_cache: false,
            },
            Self::Lr35902
            | Self::M6800
            | Self::Avr
            | Self::Mos6502
            | Self::Cmos65C02
            | Self::Ricoh2A03
            | Self::Tms9900
            | Self::Dcpu => TargetCapabilities {
                name: self.as_str(),
                memory: memory16,
                native_int_widths: &[8, 16],
                supports_port_io: false,
                prefer_code_size: true,
                has_cache: false,
            },
        }
    }

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Ez80 => "ez80",
            Self::Z80 => "z80",
            Self::Z80N => "z80n",
            Self::Z180 => "z180",
            Self::M68k => "m68k",
            Self::I8080 => "i8080",
            Self::I8085 => "i8085",
            Self::I8086 => "i8086",
            Self::Lr35902 => "lr35902",
            Self::Avr => "avr",
            Self::M6800 => "m6800",
            Self::Mos6502 => "6502",
            Self::Cmos65C02 => "65c02",
            Self::Wdc65C816 => "65c816",
            Self::Ricoh2A03 => "2a03",
            Self::Tms9900 => "tms9900",
            Self::Dcpu => "dcpu",
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

impl TargetProfile {
    pub const fn supports_port_io(&self) -> bool {
        self.triple.cpu.capabilities().supports_port_io
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TargetMemoryModel {
    pub pointer_width_bits: u16,
    pub address_width_bits: u16,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OutputFormat {
    CpmCom,
    Ez180nGaem,
    IntelHex,
    RawBin,
    Ti8ek,
    Ti8xp,
    Ti8xk,
    ZxSpectrumTap,
    GameBoyGb,
    ArduinoHex,
    Commodore64Prg,
    Commodore64Crt,
}

impl OutputFormat {
    pub const fn extension(self) -> &'static str {
        match self {
            Self::CpmCom => "com",
            Self::Ez180nGaem => "gaem",
            Self::IntelHex => "hex",
            Self::RawBin => "bin",
            Self::Ti8ek => "8ek",
            Self::Ti8xp => "8xp",
            Self::Ti8xk => "8xk",
            Self::ZxSpectrumTap => "tap",
            Self::GameBoyGb => "gb",
            Self::ArduinoHex => "hex",
            Self::Commodore64Prg => "prg",
            Self::Commodore64Crt => "crt",
        }
    }
}

pub const DEFAULT_TARGET_TRIPLE: &str = "custom-unknown-ez80";

pub fn resolve_target_profile(target: Option<&str>) -> Result<TargetProfile, String> {
    let triple = parse_target_triple(target.unwrap_or(DEFAULT_TARGET_TRIPLE))?;
    validate_target_cpu_combination(&triple)?;
    let memory =
        memory_model_for_cpu(triple.cpu).expect("all parsed CPU families have a memory model");
    Ok(TargetProfile {
        output_format: output_format_for_target(&triple),
        memory,
        default_sdk_symbols: !is_bare_target(&triple),
        triple,
    })
}

fn validate_target_cpu_combination(triple: &TargetTriple) -> Result<(), String> {
    let target = triple.value.as_str();
    let expected = if target.split('-').any(|part| part == "cpm") {
        Some(&[CpuFamily::Z80, CpuFamily::I8080, CpuFamily::I8085][..])
    } else if target.starts_with("zxspectrum-") {
        Some(&[CpuFamily::Z80][..])
    } else if target.starts_with("ti84plusce-") || target.starts_with("ti83premiumce-") {
        Some(&[CpuFamily::Ez80][..])
    } else if target.starts_with("ti83-")
        || target.starts_with("ti83plus-")
        || target.starts_with("ti84-")
        || target.starts_with("ti84plus-")
    {
        Some(&[CpuFamily::Z80][..])
    } else if target.starts_with("gameboy-") {
        Some(&[CpuFamily::Lr35902][..])
    } else if target.starts_with("agonlight-")
        || target.starts_with("ez180n-")
        || target.starts_with("ezra-test-")
    {
        Some(&[CpuFamily::Ez80][..])
    } else if target.starts_with("commodore64-") {
        Some(&[CpuFamily::Mos6502][..])
    } else if target.starts_with("arduboy-") {
        Some(&[CpuFamily::Avr][..])
    } else if target.starts_with("ti99-4a-") {
        Some(&[CpuFamily::Tms9900][..])
    } else {
        None
    };

    if expected.is_some_and(|cpus| !cpus.contains(&triple.cpu)) {
        let expected = expected
            .expect("checked above")
            .iter()
            .map(|cpu| cpu.as_str())
            .collect::<Vec<_>>()
            .join(" or ");
        return Err(format!(
            "target `{target}` requires CPU `{expected}`, not `{}`",
            triple.cpu.as_str()
        ));
    }
    Ok(())
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
    } else if triple.value.starts_with("ez180n-ez80") {
        OutputFormat::Ez180nGaem
    } else if is_ti_calculator_target(triple) {
        OutputFormat::Ti8xp
    } else if triple.value.starts_with("zxspectrum-z80") {
        OutputFormat::ZxSpectrumTap
    } else if triple.value.starts_with("gameboy-") {
        OutputFormat::GameBoyGb
    } else if triple.value.starts_with("arduboy-") {
        OutputFormat::ArduinoHex
    } else if triple.value.starts_with("commodore64-6502") {
        OutputFormat::Commodore64Prg
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
    Some(cpu.capabilities().memory)
}

pub fn parse_output_format(value: &str) -> Result<OutputFormat, String> {
    match value {
        "bin" => Ok(OutputFormat::RawBin),
        "com" => Ok(OutputFormat::CpmCom),
        "gaem" => Ok(OutputFormat::Ez180nGaem),
        "hex" | "ihex" | "intel-hex" => Ok(OutputFormat::IntelHex),
        "8ek" | "ti8ek" => Ok(OutputFormat::Ti8ek),
        "8xp" | "ti8xp" => Ok(OutputFormat::Ti8xp),
        "8xk" | "ti8xk" => Ok(OutputFormat::Ti8xk),
        "tap" | "zxtap" | "spectrum-tap" => Ok(OutputFormat::ZxSpectrumTap),
        "gb" | "gameboy" | "gameboy-gb" => Ok(OutputFormat::GameBoyGb),
        "prg" | "c64" | "commodore64-prg" => Ok(OutputFormat::Commodore64Prg),
        "crt" | "commodore64-crt" => Ok(OutputFormat::Commodore64Crt),
        _ => Err(format!(
            "unsupported output format `{value}`; expected `bin`, `com`, `gaem`, `hex`, `tap`, `gb`, `prg`, `crt`, `8xp`, `8ek`, or `8xk`"
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
            "i8086" | "8086" => Some(CpuFamily::I8086),
            "lr35902" => Some(CpuFamily::Lr35902),
            "avr" | "atmega32u4" => Some(CpuFamily::Avr),
            "m6800" | "6800" => Some(CpuFamily::M6800),
            "6502" | "mos6502" | "m6502" => Some(CpuFamily::Mos6502),
            "65c02" | "cmos65c02" => Some(CpuFamily::Cmos65C02),
            "65c816" | "wdc65c816" | "65816" => Some(CpuFamily::Wdc65C816),
            "2a03" | "ricoh2a03" | "nes" => Some(CpuFamily::Ricoh2A03),
            "tms9900" | "9900" => Some(CpuFamily::Tms9900),
            "dcpu" | "dcpu16" => Some(CpuFamily::Dcpu),
            _ => None,
        })
        .ok_or_else(|| format!("target triple `{value}` is missing a supported CPU family"))?;
    let assembler_cpu = AssemblerCpu::from(cpu);
    if !assembler_cpu.is_enabled() {
        return Err(format!(
            "target triple `{value}` requires the `{}` Cargo feature",
            assembler_cpu.feature_name()
        ));
    }
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

impl core::fmt::Display for Address24 {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "0x{:06X}", self.0)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AddressOutOfRange(pub u32);

impl core::fmt::Display for AddressOutOfRange {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(
            f,
            "address 0x{:X} is outside the 24-bit address space",
            self.0
        )
    }
}

#[cfg(feature = "std")]
impl std::error::Error for AddressOutOfRange {}

#[cfg(test)]
mod tests;
