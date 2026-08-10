use alloc::{borrow::ToOwned, format, string::String, vec::Vec};

pub const CART_MAGIC: &[u8; 4] = b"EZRA";
pub const FORMAT_VERSION: u8 = 1;
pub const CPU_MODE_I8080: u8 = 0;
pub const CPU_MODE_I8085: u8 = 1;
pub const CPU_MODE_Z80: u8 = 2;
pub const CPU_MODE_Z80N: u8 = 3;
pub const CPU_MODE_Z180: u8 = 4;
pub const CPU_MODE_EZ80_ADL: u8 = 5;

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
    R800,
    Z80N,
    Z180,
    M68k,
    M6800,
    M6809,
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
    Msp430,
    Msp430X,
    Msp430X2,
    Dcpu,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AssemblerCpu {
    I8080,
    I8085,
    I8086,
    Z80,
    R800,
    Z80N,
    Z180,
    Ez80,
    Lr35902,
    Avr,
    M6800,
    M6809,
    M68k,
    Mos6502,
    Cmos65C02,
    Wdc65C816,
    Ricoh2A03,
    Tms9900,
    Msp430,
    Msp430X,
    Msp430X2,
    Dcpu,
}

impl AssemblerCpu {
    pub fn parse(value: &str) -> Result<Self, String> {
        let cpu = match value {
            "i8080" | "8080" => Self::I8080,
            "i8085" | "8085" => Self::I8085,
            "i8086" | "8086" => Self::I8086,
            "z80" => Self::Z80,
            "r800" => Self::R800,
            "z80n" => Self::Z80N,
            "z180" => Self::Z180,
            "ez80" => Self::Ez80,
            "lr35902" | "gameboy" | "gb" => Self::Lr35902,
            "avr" | "atmega32u4" => Self::Avr,
            "m6800" | "6800" => Self::M6800,
            "m6809" | "6809" | "6809e" => Self::M6809,
            "m68k" | "68000" | "m68000" => Self::M68k,
            "6502" | "mos6502" | "m6502" => Self::Mos6502,
            "65c02" | "cmos65c02" => Self::Cmos65C02,
            "65c816" | "wdc65c816" | "65816" | "5a22" => Self::Wdc65C816,
            "2a03" | "ricoh2a03" | "nes" => Self::Ricoh2A03,
            "tms9900" | "9900" => Self::Tms9900,
            "msp430" => Self::Msp430,
            "msp430x" => Self::Msp430X,
            "msp430x2" | "msp430xv2" => Self::Msp430X2,
            "dcpu" | "dcpu16" | "dcpu-16" => Self::Dcpu,
            _ => {
                return Err(format!(
                    "unsupported assembler CPU `{value}`; expected i8080, i8085, i8086, z80, r800, z80n, z180, ez80, lr35902, 6502, 65c02, 65c816, 2a03, tms9900, msp430, msp430x, msp430x2, dcpu, m6800, m6809, m68k, or avr"
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
            Self::Z80 | Self::R800 | Self::Z80N | Self::Z180 | Self::Ez80 => {
                cfg!(feature = "z80")
            }
            Self::Lr35902 => cfg!(feature = "lr35902"),
            Self::Avr => cfg!(feature = "avr"),
            Self::M6800 => cfg!(feature = "m6800"),
            Self::M6809 => cfg!(feature = "m6809"),
            Self::M68k => cfg!(feature = "m68k"),
            Self::Mos6502 | Self::Cmos65C02 | Self::Wdc65C816 | Self::Ricoh2A03 => {
                cfg!(feature = "mos6502")
            }
            Self::Tms9900 => cfg!(feature = "tms9900"),
            Self::Msp430 | Self::Msp430X | Self::Msp430X2 => cfg!(feature = "msp430"),
            Self::Dcpu => cfg!(feature = "dcpu"),
        }
    }

    pub const fn feature_name(self) -> &'static str {
        match self {
            Self::I8080 | Self::I8085 => "intel",
            Self::I8086 => "i8086",
            Self::Z80 | Self::R800 | Self::Z80N | Self::Z180 | Self::Ez80 => "z80",
            Self::Lr35902 => "lr35902",
            Self::Avr => "avr",
            Self::M6800 => "m6800",
            Self::M6809 => "m6809",
            Self::M68k => "m68k",
            Self::Mos6502 | Self::Cmos65C02 | Self::Wdc65C816 | Self::Ricoh2A03 => "mos6502",
            Self::Tms9900 => "tms9900",
            Self::Msp430 | Self::Msp430X | Self::Msp430X2 => "msp430",
            Self::Dcpu => "dcpu",
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::I8080 => "i8080",
            Self::I8085 => "i8085",
            Self::I8086 => "i8086",
            Self::Z80 => "z80",
            Self::R800 => "r800",
            Self::Z80N => "z80n",
            Self::Z180 => "z180",
            Self::Ez80 => "ez80",
            Self::Lr35902 => "lr35902",
            Self::Avr => "avr",
            Self::M6800 => "m6800",
            Self::M6809 => "m6809",
            Self::M68k => "m68k",
            Self::Mos6502 => "6502",
            Self::Cmos65C02 => "65c02",
            Self::Wdc65C816 => "65c816",
            Self::Ricoh2A03 => "2a03",
            Self::Tms9900 => "tms9900",
            Self::Msp430 => "msp430",
            Self::Msp430X => "msp430x",
            Self::Msp430X2 => "msp430x2",
            Self::Dcpu => "dcpu",
        }
    }

    pub fn encoding_family(self) -> Option<CpuFamily> {
        match self {
            Self::Z80 | Self::R800 | Self::Z80N => Some(CpuFamily::Z80),
            Self::Z180 => Some(CpuFamily::Z80),
            Self::Ez80 => Some(CpuFamily::Ez80),
            Self::I8080 | Self::I8085 | Self::I8086 => None,
            Self::Lr35902
            | Self::M6800
            | Self::M6809
            | Self::M68k
            | Self::Mos6502
            | Self::Cmos65C02
            | Self::Wdc65C816
            | Self::Ricoh2A03
            | Self::Tms9900
            | Self::Msp430
            | Self::Msp430X
            | Self::Msp430X2
            | Self::Dcpu => None,
            Self::Avr => None,
        }
    }

    pub fn supports_z80_syntax(self) -> bool {
        matches!(
            self,
            Self::Z80 | Self::R800 | Self::Z80N | Self::Z180 | Self::Ez80
        )
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
            CpuFamily::R800 => Self::R800,
            CpuFamily::Z80N => Self::Z80N,
            CpuFamily::Z180 => Self::Z180,
            CpuFamily::I8080 => Self::I8080,
            CpuFamily::I8085 => Self::I8085,
            CpuFamily::I8086 => Self::I8086,
            CpuFamily::M68k => Self::M68k,
            CpuFamily::Lr35902 => Self::Lr35902,
            CpuFamily::Avr => Self::Avr,
            CpuFamily::M6800 => Self::M6800,
            CpuFamily::M6809 => Self::M6809,
            CpuFamily::Mos6502 => Self::Mos6502,
            CpuFamily::Cmos65C02 => Self::Cmos65C02,
            CpuFamily::Wdc65C816 => Self::Wdc65C816,
            CpuFamily::Ricoh2A03 => Self::Ricoh2A03,
            CpuFamily::Tms9900 => Self::Tms9900,
            CpuFamily::Msp430 => Self::Msp430,
            CpuFamily::Msp430X => Self::Msp430X,
            CpuFamily::Msp430X2 => Self::Msp430X2,
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
            Self::Z80 | Self::R800 | Self::Z80N | Self::Z180 | Self::I8080 | Self::I8085 => {
                TargetCapabilities {
                    name: self.as_str(),
                    memory: memory16,
                    native_int_widths: &[8, 16],
                    supports_port_io: true,
                    prefer_code_size: true,
                    has_cache: false,
                }
            }
            Self::Msp430X | Self::Msp430X2 => TargetCapabilities {
                name: self.as_str(),
                memory: TargetMemoryModel {
                    pointer_width_bits: 20,
                    address_width_bits: 20,
                },
                native_int_widths: &[8, 16, 20],
                supports_port_io: false,
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
            Self::M68k => TargetCapabilities {
                name: self.as_str(),
                memory: memory24,
                native_int_widths: &[8, 16, 24, 32],
                supports_port_io: false,
                prefer_code_size: true,
                has_cache: false,
            },
            Self::Wdc65C816 => TargetCapabilities {
                name: self.as_str(),
                memory: memory24,
                native_int_widths: &[8, 16, 24],
                supports_port_io: false,
                prefer_code_size: true,
                has_cache: false,
            },
            Self::Lr35902
            | Self::M6800
            | Self::M6809
            | Self::Avr
            | Self::Mos6502
            | Self::Cmos65C02
            | Self::Ricoh2A03
            | Self::Tms9900
            | Self::Msp430
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
            Self::R800 => "r800",
            Self::Z80N => "z80n",
            Self::Z180 => "z180",
            Self::M68k => "m68k",
            Self::I8080 => "i8080",
            Self::I8085 => "i8085",
            Self::I8086 => "i8086",
            Self::Lr35902 => "lr35902",
            Self::Avr => "avr",
            Self::M6800 => "m6800",
            Self::M6809 => "m6809",
            Self::Mos6502 => "6502",
            Self::Cmos65C02 => "65c02",
            Self::Wdc65C816 => "65c816",
            Self::Ricoh2A03 => "2a03",
            Self::Tms9900 => "tms9900",
            Self::Msp430 => "msp430",
            Self::Msp430X => "msp430x",
            Self::Msp430X2 => "msp430x2",
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
    Elf32,
    RawBin,
    Ti8ek,
    Ti8xp,
    Ti8xk,
    ZxSpectrumTap,
    GameBoyGb,
    ArduinoHex,
    Arduboy,
    Commodore64Prg,
    Commodore64Crt,
    NesRom,
    SnesRom,
    SmsRom,
    GameGearRom,
}

impl OutputFormat {
    pub const fn extension(self) -> &'static str {
        match self {
            Self::CpmCom => "com",
            Self::Ez180nGaem => "gaem",
            Self::IntelHex => "hex",
            Self::Elf32 => "elf",
            Self::RawBin => "bin",
            Self::Ti8ek => "8ek",
            Self::Ti8xp => "8xp",
            Self::Ti8xk => "8xk",
            Self::ZxSpectrumTap => "tap",
            Self::GameBoyGb => "gb",
            Self::ArduinoHex => "hex",
            Self::Arduboy => "arduboy",
            Self::Commodore64Prg => "prg",
            Self::Commodore64Crt => "crt",
            Self::NesRom => "nes",
            Self::SnesRom => "sfc",
            Self::SmsRom => "sms",
            Self::GameGearRom => "gg",
        }
    }
}

pub const DEFAULT_TARGET_TRIPLE: &str = "custom-unknown-ez80";
pub const MSP430_ELF_TARGET: &str = "msp430-none-elf";
pub const MSDOS_COM_I8086_TARGET: &str = "msdos-com-i8086";
pub const NES_2A03_TARGET: &str = "nes-2a03";
pub const SNES_5A22_TARGET: &str = "snes-5a22";
pub const SEGA_MASTER_SYSTEM_Z80_TARGET: &str = "sega-master-system-z80";
pub const SEGA_GAME_GEAR_Z80_TARGET: &str = "sega-game-gear-z80";

pub fn is_msdos_i8086_target(target: &str) -> bool {
    target == MSDOS_COM_I8086_TARGET
        || target
            .strip_prefix(MSDOS_COM_I8086_TARGET)
            .is_some_and(|suffix| suffix.len() > 1 && suffix.starts_with('-'))
}

/// Return the ez180N cartridge CPU ID for a target triple.
///
/// The ID is stored at byte 4 of a GAEM file, immediately after EZRA.
pub fn ez180n_cpu_id(target: &str) -> Option<u8> {
    Some(match target {
        "ez180n-i8080" => CPU_MODE_I8080,
        "ez180n-i8085" => CPU_MODE_I8085,
        "ez180n-z80" => CPU_MODE_Z80,
        "ez180n-z80n" => CPU_MODE_Z80N,
        "ez180n-z180" => CPU_MODE_Z180,
        "ez180n-ez80" => CPU_MODE_EZ80_ADL,
        _ => return None,
    })
}

pub fn is_ez180n_target(target: &str) -> bool {
    ez180n_cpu_id(target).is_some()
}

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
    let expected = if target.starts_with("msdos-") {
        Some(&[CpuFamily::I8086][..])
    } else if target.split('-').any(|part| part == "cpm") {
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
    } else if target.starts_with("agonlight-") || target.starts_with("ezra-test-") {
        Some(&[CpuFamily::Ez80][..])
    } else if is_ez180n_target(target) {
        Some(
            &[
                CpuFamily::I8080,
                CpuFamily::I8085,
                CpuFamily::Z80,
                CpuFamily::Z80N,
                CpuFamily::Z180,
                CpuFamily::Ez80,
            ][..],
        )
    } else if target.starts_with("commodore64-") {
        Some(&[CpuFamily::Mos6502][..])
    } else if target.starts_with("nes-") {
        Some(&[CpuFamily::Ricoh2A03][..])
    } else if target.starts_with("snes-") {
        Some(&[CpuFamily::Wdc65C816][..])
    } else if target.starts_with("sega-master-system-") || target.starts_with("sega-game-gear-") {
        Some(&[CpuFamily::Z80][..])
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
    if target.starts_with("msdos-") && !is_msdos_i8086_target(target) {
        return Err(format!(
            "unsupported MS-DOS target `{target}`; expected `{MSDOS_COM_I8086_TARGET}`"
        ));
    }
    Ok(())
}

fn is_bare_target(triple: &TargetTriple) -> bool {
    triple.value.split('-').any(|part| part == "bare") || triple.value.starts_with("nes-")
}

fn output_format_for_target(triple: &TargetTriple) -> OutputFormat {
    if is_msdos_i8086_target(&triple.value)
        || (matches!(
            triple.cpu,
            CpuFamily::Z80
                | CpuFamily::Z80N
                | CpuFamily::Z180
                | CpuFamily::I8080
                | CpuFamily::I8085
        ) && triple.value.split('-').any(|part| part == "cpm"))
    {
        OutputFormat::CpmCom
    } else if is_ez180n_target(&triple.value) {
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
    } else if triple.value.starts_with("nes-2a03") {
        OutputFormat::NesRom
    } else if triple.value == SNES_5A22_TARGET {
        OutputFormat::SnesRom
    } else if triple.value == SEGA_MASTER_SYSTEM_Z80_TARGET {
        OutputFormat::SmsRom
    } else if triple.value == SEGA_GAME_GEAR_Z80_TARGET {
        OutputFormat::GameGearRom
    } else if triple.value.starts_with("msp430") {
        OutputFormat::Elf32
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
        "elf" | "elf32" => Ok(OutputFormat::Elf32),
        "8ek" | "ti8ek" => Ok(OutputFormat::Ti8ek),
        "8xp" | "ti8xp" => Ok(OutputFormat::Ti8xp),
        "8xk" | "ti8xk" => Ok(OutputFormat::Ti8xk),
        "tap" | "zxtap" | "spectrum-tap" => Ok(OutputFormat::ZxSpectrumTap),
        "gb" | "gameboy" | "gameboy-gb" => Ok(OutputFormat::GameBoyGb),
        "arduboy" => Ok(OutputFormat::Arduboy),
        "prg" | "c64" | "commodore64-prg" => Ok(OutputFormat::Commodore64Prg),
        "crt" | "commodore64-crt" => Ok(OutputFormat::Commodore64Crt),
        "nes" | "nes-rom" => Ok(OutputFormat::NesRom),
        "sfc" | "smc" | "snes" | "snes-rom" => Ok(OutputFormat::SnesRom),
        "sms" | "sega-master-system" => Ok(OutputFormat::SmsRom),
        "gg" | "game-gear" | "sega-game-gear" => Ok(OutputFormat::GameGearRom),
        _ => Err(format!(
            "unsupported output format `{value}`; expected `bin`, `com`, `gaem`, `hex`, `elf`, `arduboy`, `tap`, `gb`, `prg`, `crt`, `nes`, `sms`, `gg`, `8xp`, `8ek`, or `8xk`"
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
            "r800" => Some(CpuFamily::R800),
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
            "m6809" | "6809" | "6809e" => Some(CpuFamily::M6809),
            "6502" | "mos6502" | "m6502" => Some(CpuFamily::Mos6502),
            "65c02" | "cmos65c02" => Some(CpuFamily::Cmos65C02),
            "65c816" | "wdc65c816" | "65816" | "5a22" => Some(CpuFamily::Wdc65C816),
            "2a03" | "ricoh2a03" | "nes" => Some(CpuFamily::Ricoh2A03),
            "tms9900" | "9900" => Some(CpuFamily::Tms9900),
            "msp430" => Some(CpuFamily::Msp430),
            "msp430x" => Some(CpuFamily::Msp430X),
            "msp430x2" | "msp430xv2" => Some(CpuFamily::Msp430X2),
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
mod arduboy_tests {
    use super::*;

    #[test]
    fn parses_arduboy_container_output() {
        assert_eq!(parse_output_format("arduboy"), Ok(OutputFormat::Arduboy));
        assert_eq!(OutputFormat::Arduboy.extension(), "arduboy");
    }
}

#[cfg(test)]
mod tests;
