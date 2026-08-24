//! Shared SDK module registry and provider boundary.
//!
//! Import resolution uses one normalized module name (`foo.bar`), one source
//! path (`foo/bar.ezra`), and one provider interface for filesystem, virtual,
//! and embedded SDK sources.

use crate::compat::prelude::*;
#[cfg(feature = "embedded-sdk")]
use crate::target::SNES_5A22_TARGET;
use crate::target::is_msdos_i8086_target;

/// Controls whether the compiler may fall back to SDK bytes built into the
/// library.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum SdkLookupMode {
    /// Search caller sources first, then embedded SDK sources.
    #[default]
    ExternalThenEmbedded,
    /// Search caller sources only. This is useful when SDK files are installed
    /// beside a constrained compiler binary.
    ExternalOnly,
}

impl SdkLookupMode {
    pub const fn allows_embedded(self) -> bool {
        matches!(self, Self::ExternalThenEmbedded)
    }
}

/// A normalized SDK module returned by a provider.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SdkModuleSource {
    /// Normalized module path used for diagnostics and relative embeds.
    pub path: String,
    /// UTF-8 EZRA source.
    pub source: String,
}

/// Source provider used after project/source-relative imports have been tried.
///
/// Providers own the source returned from `module_source`, so the same
/// boundary works for host files, caller-owned virtual workspaces, and static
/// embedded bytes.
pub trait SdkProvider {
    fn module_source(
        &self,
        target: Option<&str>,
        module: &str,
        mode: SdkLookupMode,
    ) -> Result<Option<SdkModuleSource>, String>;

    fn module_names(&self, target: Option<&str>, mode: SdkLookupMode) -> Vec<String>;
}

#[cfg_attr(not(feature = "embedded-sdk"), allow(dead_code))]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum EmbeddedTarget {
    Contains(&'static str),
    Prefix(&'static str),
    Prefixes(&'static [&'static str]),
    Exact(&'static str),
    ExactAny(&'static [&'static str]),
    Msdos,
    Cpm,
}

impl EmbeddedTarget {
    fn matches(self, target: Option<&str>) -> bool {
        let Some(target) = target else {
            return false;
        };
        match self {
            Self::Contains(value) => target.contains(value),
            Self::Prefix(value) => target.starts_with(value),
            Self::Prefixes(values) => values.iter().any(|value| target.starts_with(value)),
            Self::Exact(value) => target == value,
            Self::ExactAny(values) => values.iter().any(|value| target == *value),
            Self::Msdos => is_msdos_i8086_target(target),
            Self::Cpm => target.split('-').any(|part| part == "cpm"),
        }
    }
}

#[derive(Clone, Copy)]
struct EmbeddedSdkModule {
    name: &'static str,
    target: EmbeddedTarget,
    source_path: &'static str,
    source: &'static [u8],
}

#[cfg(feature = "embedded-sdk")]
macro_rules! embedded_module {
    ($name:literal, $target:expr, $path:literal) => {
        EmbeddedSdkModule {
            name: $name,
            target: $target,
            source_path: $path,
            source: include_bytes!(concat!("../", $path)),
        }
    };
}

#[cfg(feature = "embedded-sdk")]
static EMBEDDED_SDK_MODULES: &[EmbeddedSdkModule] = &[
    embedded_module!(
        "dcpu.hardware",
        EmbeddedTarget::Contains("dcpu"),
        "toolchains/generic-dcpu-bare/sdk/dcpu/hardware.ezra"
    ),
    embedded_module!(
        "dcpu.lem1802",
        EmbeddedTarget::Contains("dcpu"),
        "toolchains/generic-dcpu-bare/sdk/dcpu/lem1802.ezra"
    ),
    embedded_module!(
        "dcpu.keyboard",
        EmbeddedTarget::Contains("dcpu"),
        "toolchains/generic-dcpu-bare/sdk/dcpu/keyboard.ezra"
    ),
    embedded_module!(
        "dcpu.clock",
        EmbeddedTarget::Contains("dcpu"),
        "toolchains/generic-dcpu-bare/sdk/dcpu/clock.ezra"
    ),
    embedded_module!(
        "dcpu.speaker",
        EmbeddedTarget::Contains("dcpu"),
        "toolchains/generic-dcpu-bare/sdk/dcpu/speaker.ezra"
    ),
    embedded_module!(
        "dos.constants",
        EmbeddedTarget::Msdos,
        "toolchains/msdos-i8086/sdk/dos/constants.ezra"
    ),
    embedded_module!(
        "dos.raw",
        EmbeddedTarget::Msdos,
        "toolchains/msdos-i8086/sdk/dos/raw.ezra"
    ),
    embedded_module!(
        "dos.console",
        EmbeddedTarget::Msdos,
        "toolchains/msdos-i8086/sdk/dos/console.ezra"
    ),
    embedded_module!(
        "dos.file",
        EmbeddedTarget::Msdos,
        "toolchains/msdos-i8086/sdk/dos/file.ezra"
    ),
    embedded_module!(
        "dos.directory",
        EmbeddedTarget::Msdos,
        "toolchains/msdos-i8086/sdk/dos/directory.ezra"
    ),
    embedded_module!(
        "dos.memory",
        EmbeddedTarget::Msdos,
        "toolchains/msdos-i8086/sdk/dos/memory.ezra"
    ),
    embedded_module!(
        "dos.datetime",
        EmbeddedTarget::Msdos,
        "toolchains/msdos-i8086/sdk/dos/datetime.ezra"
    ),
    embedded_module!(
        "dos.process",
        EmbeddedTarget::Msdos,
        "toolchains/msdos-i8086/sdk/dos/process.ezra"
    ),
    embedded_module!(
        "dos.psp",
        EmbeddedTarget::Msdos,
        "toolchains/msdos-i8086/sdk/dos/psp.ezra"
    ),
    embedded_module!(
        "arduboy.core",
        EmbeddedTarget::Prefix("arduboy-"),
        "toolchains/arduboy-avr/sdk/arduboy/core.ezra"
    ),
    embedded_module!(
        "arduboy.input",
        EmbeddedTarget::Prefix("arduboy-"),
        "toolchains/arduboy-avr/sdk/arduboy/input.ezra"
    ),
    embedded_module!(
        "arduboy.oled",
        EmbeddedTarget::Prefix("arduboy-"),
        "toolchains/arduboy-avr/sdk/arduboy/oled.ezra"
    ),
    embedded_module!(
        "arduboy.eeprom",
        EmbeddedTarget::Prefix("arduboy-"),
        "toolchains/arduboy-avr/sdk/arduboy/eeprom.ezra"
    ),
    embedded_module!(
        "arduboy.timing",
        EmbeddedTarget::Prefix("arduboy-"),
        "toolchains/arduboy-avr/sdk/arduboy/timing.ezra"
    ),
    embedded_module!(
        "arduboy.audio",
        EmbeddedTarget::Prefix("arduboy-"),
        "toolchains/arduboy-avr/sdk/arduboy/audio.ezra"
    ),
    embedded_module!(
        "arduboy.graphics",
        EmbeddedTarget::Prefix("arduboy-"),
        "toolchains/arduboy-avr/sdk/arduboy/graphics.ezra"
    ),
    embedded_module!(
        "gb.video",
        EmbeddedTarget::Prefix("gameboy-"),
        "toolchains/gameboy-lr35902/sdk/gb/video.ezra"
    ),
    embedded_module!(
        "gb.sprites",
        EmbeddedTarget::Prefix("gameboy-"),
        "toolchains/gameboy-lr35902/sdk/gb/sprites.ezra"
    ),
    embedded_module!(
        "gb.serial",
        EmbeddedTarget::Prefix("gameboy-"),
        "toolchains/gameboy-lr35902/sdk/gb/serial.ezra"
    ),
    embedded_module!(
        "gb.input",
        EmbeddedTarget::Prefix("gameboy-"),
        "toolchains/gameboy-lr35902/sdk/gb/input.ezra"
    ),
    embedded_module!(
        "gb.audio",
        EmbeddedTarget::Prefix("gameboy-"),
        "toolchains/gameboy-lr35902/sdk/gb/audio.ezra"
    ),
    embedded_module!(
        "gb.color",
        EmbeddedTarget::Prefix("gameboy-"),
        "toolchains/gameboy-lr35902/sdk/gb/color.ezra"
    ),
    embedded_module!(
        "gb.text",
        EmbeddedTarget::Prefix("gameboy-"),
        "toolchains/gameboy-lr35902/sdk/gb/text.ezra"
    ),
    embedded_module!(
        "agon.buffers",
        EmbeddedTarget::Prefix("agonlight-mos-ez80"),
        "toolchains/agonlight-mos-ez80/sdk/agon/buffers.ezra"
    ),
    embedded_module!(
        "agon.console",
        EmbeddedTarget::Prefix("agonlight-mos-ez80"),
        "toolchains/agonlight-mos-ez80/sdk/agon/console.ezra"
    ),
    embedded_module!(
        "agon.mos",
        EmbeddedTarget::Prefix("agonlight-mos-ez80"),
        "toolchains/agonlight-mos-ez80/sdk/agon/mos.ezra"
    ),
    embedded_module!(
        "agon.gpio",
        EmbeddedTarget::Prefix("agonlight-mos-ez80"),
        "toolchains/agonlight-mos-ez80/sdk/agon/gpio.ezra"
    ),
    embedded_module!(
        "agon.keyboard",
        EmbeddedTarget::Prefix("agonlight-mos-ez80"),
        "toolchains/agonlight-mos-ez80/sdk/agon/keyboard.ezra"
    ),
    embedded_module!(
        "agon.mouse",
        EmbeddedTarget::Prefix("agonlight-mos-ez80"),
        "toolchains/agonlight-mos-ez80/sdk/agon/mouse.ezra"
    ),
    embedded_module!(
        "agon.sprites",
        EmbeddedTarget::Prefix("agonlight-mos-ez80"),
        "toolchains/agonlight-mos-ez80/sdk/agon/sprites.ezra"
    ),
    embedded_module!(
        "agon.vdp",
        EmbeddedTarget::Prefix("agonlight-mos-ez80"),
        "toolchains/agonlight-mos-ez80/sdk/agon/vdp.ezra"
    ),
    embedded_module!(
        "agon.text",
        EmbeddedTarget::Prefix("agonlight-mos-ez80"),
        "toolchains/agonlight-mos-ez80/sdk/agon/text.ezra"
    ),
    embedded_module!(
        "ez180n.console",
        EmbeddedTarget::Prefix("ez180n-"),
        "toolchains/ez180n-ez80/sdk/ez180n/console.ezra"
    ),
    embedded_module!(
        "tice.os",
        EmbeddedTarget::Prefixes(&["ti84plusce-ez80", "ti83premiumce-ez80"]),
        "toolchains/tice-ez80/sdk/tice/os.ezra"
    ),
    embedded_module!(
        "tice.lcd",
        EmbeddedTarget::Prefixes(&["ti84plusce-ez80", "ti83premiumce-ez80"]),
        "toolchains/tice-ez80/sdk/tice/lcd.ezra"
    ),
    embedded_module!(
        "tice.input",
        EmbeddedTarget::Prefixes(&["ti84plusce-ez80", "ti83premiumce-ez80"]),
        "toolchains/tice-ez80/sdk/tice/input.ezra"
    ),
    embedded_module!(
        "tice.vars",
        EmbeddedTarget::Prefixes(&["ti84plusce-ez80", "ti83premiumce-ez80"]),
        "toolchains/tice-ez80/sdk/tice/vars.ezra"
    ),
    embedded_module!(
        "ti99.console",
        EmbeddedTarget::Prefix("ti99-4a-tms9900"),
        "toolchains/ti99-4a-tms9900/sdk/ti99/console.ezra"
    ),
    embedded_module!(
        "ti99.graphics",
        EmbeddedTarget::Prefix("ti99-4a-tms9900"),
        "toolchains/ti99-4a-tms9900/sdk/ti99/graphics.ezra"
    ),
    embedded_module!(
        "ti99.input",
        EmbeddedTarget::Prefix("ti99-4a-tms9900"),
        "toolchains/ti99-4a-tms9900/sdk/ti99/input.ezra"
    ),
    embedded_module!(
        "ti99.sprites",
        EmbeddedTarget::Prefix("ti99-4a-tms9900"),
        "toolchains/ti99-4a-tms9900/sdk/ti99/sprites.ezra"
    ),
    embedded_module!(
        "ti99.memory",
        EmbeddedTarget::Prefix("ti99-4a-tms9900"),
        "toolchains/ti99-4a-tms9900/sdk/ti99/memory.ezra"
    ),
    embedded_module!(
        "ti99.sound",
        EmbeddedTarget::Prefix("ti99-4a-tms9900"),
        "toolchains/ti99-4a-tms9900/sdk/ti99/sound.ezra"
    ),
    embedded_module!(
        "ti99.vdp",
        EmbeddedTarget::Prefix("ti99-4a-tms9900"),
        "toolchains/ti99-4a-tms9900/sdk/ti99/vdp.ezra"
    ),
    embedded_module!(
        "ti.os",
        EmbeddedTarget::Prefixes(&["ti83-z80", "ti83plus-z80", "ti84-z80", "ti84plus-z80"]),
        "toolchains/ti-z80/sdk/ti/os.ezra"
    ),
    embedded_module!(
        "ti.lcd",
        EmbeddedTarget::Prefixes(&["ti83-z80", "ti83plus-z80", "ti84-z80", "ti84plus-z80"]),
        "toolchains/ti-z80/sdk/ti/lcd.ezra"
    ),
    embedded_module!(
        "ti.input",
        EmbeddedTarget::Prefixes(&["ti83-z80", "ti83plus-z80", "ti84-z80", "ti84plus-z80"]),
        "toolchains/ti-z80/sdk/ti/input.ezra"
    ),
    embedded_module!(
        "ti.vars",
        EmbeddedTarget::Prefixes(&["ti83-z80", "ti83plus-z80", "ti84-z80", "ti84plus-z80"]),
        "toolchains/ti-z80/sdk/ti/vars.ezra"
    ),
    embedded_module!(
        "sms.system",
        EmbeddedTarget::ExactAny(&["sega-master-system-z80", "sega-game-gear-z80"]),
        "toolchains/sega-master-system-z80/sdk/sms/system.ezra"
    ),
    embedded_module!(
        "sms.vdp",
        EmbeddedTarget::ExactAny(&["sega-master-system-z80", "sega-game-gear-z80"]),
        "toolchains/sega-master-system-z80/sdk/sms/vdp.ezra"
    ),
    embedded_module!(
        "sms.video",
        EmbeddedTarget::ExactAny(&["sega-master-system-z80", "sega-game-gear-z80"]),
        "toolchains/sega-master-system-z80/sdk/sms/video.ezra"
    ),
    embedded_module!(
        "sms.palette",
        EmbeddedTarget::ExactAny(&["sega-master-system-z80", "sega-game-gear-z80"]),
        "toolchains/sega-master-system-z80/sdk/sms/palette.ezra"
    ),
    embedded_module!(
        "sms.memory",
        EmbeddedTarget::ExactAny(&["sega-master-system-z80", "sega-game-gear-z80"]),
        "toolchains/sega-master-system-z80/sdk/sms/memory.ezra"
    ),
    embedded_module!(
        "sms.input",
        EmbeddedTarget::ExactAny(&["sega-master-system-z80", "sega-game-gear-z80"]),
        "toolchains/sega-master-system-z80/sdk/sms/input.ezra"
    ),
    embedded_module!(
        "sms.bank",
        EmbeddedTarget::ExactAny(&["sega-master-system-z80", "sega-game-gear-z80"]),
        "toolchains/sega-master-system-z80/sdk/sms/bank.ezra"
    ),
    embedded_module!(
        "gg.palette",
        EmbeddedTarget::Exact("sega-game-gear-z80"),
        "toolchains/sega-game-gear-z80/sdk/gg/palette.ezra"
    ),
    embedded_module!(
        "gg.input",
        EmbeddedTarget::Exact("sega-game-gear-z80"),
        "toolchains/sega-game-gear-z80/sdk/gg/input.ezra"
    ),
    embedded_module!(
        "gg.viewport",
        EmbeddedTarget::Exact("sega-game-gear-z80"),
        "toolchains/sega-game-gear-z80/sdk/gg/viewport.ezra"
    ),
    embedded_module!(
        "gg.audio",
        EmbeddedTarget::Exact("sega-game-gear-z80"),
        "toolchains/sega-game-gear-z80/sdk/gg/audio.ezra"
    ),
    embedded_module!(
        "snes.system",
        EmbeddedTarget::Exact(SNES_5A22_TARGET),
        "toolchains/snes-5a22/sdk/snes/system.ezra"
    ),
    embedded_module!(
        "snes.memory",
        EmbeddedTarget::Exact(SNES_5A22_TARGET),
        "toolchains/snes-5a22/sdk/snes/memory.ezra"
    ),
    embedded_module!(
        "snes.ppu",
        EmbeddedTarget::Exact(SNES_5A22_TARGET),
        "toolchains/snes-5a22/sdk/snes/ppu.ezra"
    ),
    embedded_module!(
        "snes.dma",
        EmbeddedTarget::Exact(SNES_5A22_TARGET),
        "toolchains/snes-5a22/sdk/snes/dma.ezra"
    ),
    embedded_module!(
        "snes.input",
        EmbeddedTarget::Exact(SNES_5A22_TARGET),
        "toolchains/snes-5a22/sdk/snes/input.ezra"
    ),
    embedded_module!(
        "snes.audio",
        EmbeddedTarget::Exact(SNES_5A22_TARGET),
        "toolchains/snes-5a22/sdk/snes/audio.ezra"
    ),
    embedded_module!(
        "snes.timing",
        EmbeddedTarget::Exact(SNES_5A22_TARGET),
        "toolchains/snes-5a22/sdk/snes/timing.ezra"
    ),
    embedded_module!(
        "nes.ppu",
        EmbeddedTarget::Prefix("nes-"),
        "toolchains/nes-2a03/sdk/nes/ppu.ezra"
    ),
    embedded_module!(
        "nes.palette",
        EmbeddedTarget::Prefix("nes-"),
        "toolchains/nes-2a03/sdk/nes/palette.ezra"
    ),
    embedded_module!(
        "nes.sprites",
        EmbeddedTarget::Prefix("nes-"),
        "toolchains/nes-2a03/sdk/nes/sprites.ezra"
    ),
    embedded_module!(
        "nes.input",
        EmbeddedTarget::Prefix("nes-"),
        "toolchains/nes-2a03/sdk/nes/input.ezra"
    ),
    embedded_module!(
        "nes.audio",
        EmbeddedTarget::Prefix("nes-"),
        "toolchains/nes-2a03/sdk/nes/audio.ezra"
    ),
    embedded_module!(
        "nes.timing",
        EmbeddedTarget::Prefix("nes-"),
        "toolchains/nes-2a03/sdk/nes/timing.ezra"
    ),
    embedded_module!(
        "nes.memory",
        EmbeddedTarget::Prefix("nes-"),
        "toolchains/nes-2a03/sdk/nes/memory.ezra"
    ),
    embedded_module!(
        "c64.vic",
        EmbeddedTarget::Prefix("commodore64-6502"),
        "toolchains/commodore64-6502/sdk/c64/vic.ezra"
    ),
    embedded_module!(
        "c64.sid",
        EmbeddedTarget::Prefix("commodore64-6502"),
        "toolchains/commodore64-6502/sdk/c64/sid.ezra"
    ),
    embedded_module!(
        "c64.cia",
        EmbeddedTarget::Prefix("commodore64-6502"),
        "toolchains/commodore64-6502/sdk/c64/cia.ezra"
    ),
    embedded_module!(
        "c64.memory",
        EmbeddedTarget::Prefix("commodore64-6502"),
        "toolchains/commodore64-6502/sdk/c64/memory.ezra"
    ),
    embedded_module!(
        "c64.kernal",
        EmbeddedTarget::Prefix("commodore64-6502"),
        "toolchains/commodore64-6502/sdk/c64/kernal.ezra"
    ),
    embedded_module!(
        "c64.text",
        EmbeddedTarget::Prefix("commodore64-6502"),
        "toolchains/commodore64-6502/sdk/c64/text.ezra"
    ),
    embedded_module!(
        "zx.rom",
        EmbeddedTarget::Prefix("zxspectrum-z80"),
        "toolchains/zxspectrum-z80/sdk/zx/rom.ezra"
    ),
    embedded_module!(
        "zx.screen",
        EmbeddedTarget::Prefix("zxspectrum-z80"),
        "toolchains/zxspectrum-z80/sdk/zx/screen.ezra"
    ),
    embedded_module!(
        "zx.io",
        EmbeddedTarget::Prefix("zxspectrum-z80"),
        "toolchains/zxspectrum-z80/sdk/zx/io.ezra"
    ),
    embedded_module!(
        "zx.keyboard",
        EmbeddedTarget::Prefix("zxspectrum-z80"),
        "toolchains/zxspectrum-z80/sdk/zx/keyboard.ezra"
    ),
    embedded_module!(
        "zx.sound",
        EmbeddedTarget::Prefix("zxspectrum-z80"),
        "toolchains/zxspectrum-z80/sdk/zx/sound.ezra"
    ),
    embedded_module!(
        "zx.memory",
        EmbeddedTarget::Prefix("zxspectrum-z80"),
        "toolchains/zxspectrum-z80/sdk/zx/memory.ezra"
    ),
    embedded_module!(
        "zx.interrupt",
        EmbeddedTarget::Prefix("zxspectrum-z80"),
        "toolchains/zxspectrum-z80/sdk/zx/interrupt.ezra"
    ),
    embedded_module!(
        "zx.text",
        EmbeddedTarget::Prefix("zxspectrum-z80"),
        "toolchains/zxspectrum-z80/sdk/zx/text.ezra"
    ),
    embedded_module!(
        "harness.io",
        EmbeddedTarget::Prefix("ezra-test-"),
        "toolchains/ezra-test-ez80/sdk/harness/io.ezra"
    ),
    embedded_module!(
        "harness.layout",
        EmbeddedTarget::Prefix("ezra-test-"),
        "toolchains/ezra-test-ez80/sdk/harness/layout.ezra"
    ),
    embedded_module!(
        "harness.memory",
        EmbeddedTarget::Prefix("ezra-test-"),
        "toolchains/ezra-test-ez80/sdk/harness/memory.ezra"
    ),
    embedded_module!(
        "cpm.bdos",
        EmbeddedTarget::Cpm,
        "toolchains/cpm-2.2-z80/sdk/cpm/bdos.ezra"
    ),
    embedded_module!(
        "cpm.console",
        EmbeddedTarget::Cpm,
        "toolchains/cpm-2.2-z80/sdk/cpm/console.ezra"
    ),
    embedded_module!(
        "cpm.text",
        EmbeddedTarget::Cpm,
        "toolchains/cpm-2.2-z80/sdk/cpm/text.ezra"
    ),
    embedded_module!(
        "cpm.dma",
        EmbeddedTarget::Cpm,
        "toolchains/cpm-2.2-z80/sdk/cpm/dma.ezra"
    ),
    embedded_module!(
        "cpm.fcb",
        EmbeddedTarget::Cpm,
        "toolchains/cpm-2.2-z80/sdk/cpm/fcb.ezra"
    ),
];

#[cfg(not(feature = "embedded-sdk"))]
static EMBEDDED_SDK_MODULES: &[EmbeddedSdkModule] = &[];

const COMPILER_INTRINSIC_MODULES: &[&str] = &["ezra.bits", "ezra.int", "ezra.mem"];

/// Return the compiler-provided intrinsic module names. These catalogs remain
/// available without the `embedded-sdk` feature.
pub const fn compiler_intrinsic_modules() -> &'static [&'static str] {
    COMPILER_INTRINSIC_MODULES
}

/// Return the compiler-provided intrinsic module source.
pub fn compiler_intrinsic_source(module: &str) -> Option<&'static str> {
    COMPILER_INTRINSIC_MODULES
        .contains(&module)
        .then_some("// Compiler intrinsic module. Calls are resolved by ezrac.\n")
}

/// Return embedded source text and its manifest source path.
pub fn embedded_sdk_source(
    target: Option<&str>,
    module: &str,
) -> Option<(&'static str, &'static str)> {
    EMBEDDED_SDK_MODULES
        .iter()
        .find(|entry| entry.name == module && entry.target.matches(target))
        .map(|entry| {
            let source =
                core::str::from_utf8(entry.source).expect("embedded SDK source must be UTF-8");
            (source, entry.source_path)
        })
}

/// Return the logical source path used for an embedded module. DOS keeps its
/// historical toolchain path; other embedded modules use the stable logical
/// path that the LSP materializes into its cache.
pub fn embedded_sdk_module_path(target: Option<&str>, module: &str) -> Option<String> {
    let (_, manifest_path) = embedded_sdk_source(target, module)?;
    if target.is_some_and(is_msdos_i8086_target) && module.starts_with("dos.") {
        Some(manifest_path.to_owned())
    } else {
        Some(format!("builtin-sdk/{}", module_file_name(module)))
    }
}

/// Return the target-filtered embedded module names from the same manifest as
/// source lookup.
pub fn embedded_sdk_modules(target: Option<&str>) -> Vec<&'static str> {
    EMBEDDED_SDK_MODULES
        .iter()
        .filter(|entry| entry.target.matches(target))
        .map(|entry| entry.name)
        .collect()
}

/// Build the shared diagnostic used when no provider can resolve an import.
pub fn missing_module_message(import: &str, importer: &str) -> String {
    format!(
        "failed to resolve import `{import}` from `{importer}`: no source-relative, caller SDK, or embedded SDK module was found"
    )
}

/// Convert a normalized module name to its relative `.ezra` path.
pub fn module_file_name(module: &str) -> String {
    format!("{}.ezra", module.replace('.', "/"))
}

/// Convert an SDK source path to its normalized module name when it is an EZRA
/// source file below `root`.
pub fn module_name_from_relative_path(path: &str) -> Option<String> {
    let path = path.replace('\\', "/");
    let path = path.strip_suffix(".ezra")?;
    if path.is_empty() || path.ends_with('/') {
        return None;
    }
    Some(path.replace('/', "."))
}
