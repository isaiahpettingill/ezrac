use crate::diagnostic::Diagnostic;
use crate::target::{
    Address24, EZRA_ASSET_BASE, EZRA_AUDIO_BASE, EZRA_ENTRY_ADDR, EZRA_LOAD_ADDR, EZRA_RAM_BASE,
    EZRA_RODATA_BASE, EZRA_STACK_TOP, EZRA_VRAM_BASE,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RegionFlags(u8);

impl RegionFlags {
    pub const READ: Self = Self(1 << 0);
    pub const WRITE: Self = Self(1 << 1);
    pub const EXECUTE: Self = Self(1 << 2);
    pub const VOLATILE: Self = Self(1 << 3);
    pub const RESERVED: Self = Self(1 << 4);

    pub const fn empty() -> Self {
        Self(0)
    }

    pub const fn contains(self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }

    pub const fn bits(self) -> u8 {
        self.0
    }

    pub const fn union(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Region {
    pub name: String,
    pub start: Address24,
    pub end: Address24,
    pub flags: RegionFlags,
}

impl Region {
    pub fn contains_range(&self, start: Address24, end: Address24) -> bool {
        self.start <= start && end <= self.end
    }

    pub fn overlaps(&self, other: &Region) -> bool {
        self.start <= other.end && other.start <= self.end
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Section {
    pub name: String,
    pub region: String,
    pub align: u32,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Symbol {
    pub name: String,
    pub value: Address24,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Layout {
    pub name: String,
    pub load: Address24,
    pub entry: Address24,
    pub stack: Address24,
    pub regions: Vec<Region>,
    pub sections: Vec<Section>,
    pub symbols: Vec<Symbol>,
}

impl Layout {
    pub fn ezra_default() -> Self {
        Self {
            name: "ezra_default".to_owned(),
            load: EZRA_LOAD_ADDR,
            entry: EZRA_ENTRY_ADDR,
            stack: EZRA_STACK_TOP,
            regions: vec![
                region("low", 0x00_0000, 0x00_FFFF, &[RegionFlags::RESERVED]),
                region(
                    "code",
                    0x01_0000,
                    0x01_FFFF,
                    &[RegionFlags::READ, RegionFlags::EXECUTE],
                ),
                region("rodata", 0x02_0000, 0x03_FFFF, &[RegionFlags::READ]),
                region(
                    "ram",
                    0x04_0000,
                    0x07_FFFF,
                    &[RegionFlags::READ, RegionFlags::WRITE],
                ),
                region(
                    "vram",
                    0x08_0000,
                    0x0B_FFFF,
                    &[RegionFlags::READ, RegionFlags::WRITE, RegionFlags::VOLATILE],
                ),
                region(
                    "audio",
                    0x0C_0000,
                    0x0F_FFFF,
                    &[RegionFlags::READ, RegionFlags::WRITE, RegionFlags::VOLATILE],
                ),
                region("assets", 0x10_0000, 0xDF_FFFF, &[RegionFlags::READ]),
                region(
                    "scratch",
                    0xE0_0000,
                    0xEF_FFFF,
                    &[RegionFlags::READ, RegionFlags::WRITE],
                ),
                region(
                    "stack",
                    0xF0_0000,
                    0xFF_FFFF,
                    &[RegionFlags::READ, RegionFlags::WRITE, RegionFlags::RESERVED],
                ),
            ],
            sections: vec![
                section(".header", "code", 64),
                section(".text", "code", 16),
                section(".rodata", "rodata", 16),
                section(".data", "ram", 16),
                section(".bss", "ram", 16),
                section(".assets", "assets", 256),
                section(".scratch", "scratch", 16),
            ],
            symbols: vec![
                symbol("EZRA_LOAD_ADDR", EZRA_LOAD_ADDR),
                symbol("EZRA_ENTRY_ADDR", EZRA_ENTRY_ADDR),
                symbol("EZRA_STACK_TOP", EZRA_STACK_TOP),
                symbol("EZRA_RAM_BASE", EZRA_RAM_BASE),
                symbol("EZRA_VRAM_BASE", EZRA_VRAM_BASE),
                symbol("EZRA_AUDIO_BASE", EZRA_AUDIO_BASE),
                symbol("EZRA_ASSET_BASE", EZRA_ASSET_BASE),
                symbol("EZRA_RODATA_BASE", EZRA_RODATA_BASE),
            ],
        }
    }

    pub fn validate(&self) -> Result<(), Vec<Diagnostic>> {
        let mut diagnostics = Vec::new();

        for (index, region) in self.regions.iter().enumerate() {
            if region.start > region.end {
                diagnostics.push(Diagnostic::new(format!(
                    "region `{}` starts after it ends",
                    region.name
                )));
            }

            for other in self.regions.iter().skip(index + 1) {
                if region.overlaps(other) {
                    diagnostics.push(Diagnostic::new(format!(
                        "regions `{}` and `{}` overlap",
                        region.name, other.name
                    )));
                }
            }
        }

        for section in &self.sections {
            if section.align == 0 || !section.align.is_power_of_two() {
                diagnostics.push(Diagnostic::new(format!(
                    "section `{}` alignment must be a power of two",
                    section.name
                )));
            }

            match self
                .regions
                .iter()
                .find(|region| region.name == section.region)
            {
                Some(region) if region.flags.contains(RegionFlags::RESERVED) => {
                    diagnostics.push(Diagnostic::new(format!(
                        "section `{}` targets reserved region `{}`",
                        section.name, region.name
                    )));
                }
                Some(_) => {}
                None => diagnostics.push(Diagnostic::new(format!(
                    "section `{}` targets unknown region `{}`",
                    section.name, section.region
                ))),
            }
        }

        if diagnostics.is_empty() {
            Ok(())
        } else {
            Err(diagnostics)
        }
    }

    pub fn map_summary(&self) -> String {
        let mut out = String::from("section   region    align\n");
        for section in &self.sections {
            out.push_str(&format!(
                "{:<9} {:<9} {}\n",
                section.name, section.region, section.align
            ));
        }
        out
    }
}

fn region(name: &str, start: u32, end: u32, flags: &[RegionFlags]) -> Region {
    Region {
        name: name.to_owned(),
        start: Address24::new(start),
        end: Address24::new(end),
        flags: flags
            .iter()
            .copied()
            .fold(RegionFlags::empty(), RegionFlags::union),
    }
}

fn section(name: &str, region: &str, align: u32) -> Section {
    Section {
        name: name.to_owned(),
        region: region.to_owned(),
        align,
    }
}

fn symbol(name: &str, value: Address24) -> Symbol {
    Symbol {
        name: name.to_owned(),
        value,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_layout_validates() {
        assert_eq!(Layout::ezra_default().validate(), Ok(()));
    }

    #[test]
    fn overlapping_regions_are_reported() {
        let mut layout = Layout::ezra_default();
        layout
            .regions
            .push(region("bad", 0x01_8000, 0x02_8000, &[RegionFlags::READ]));

        let errors = layout.validate().unwrap_err();

        assert!(errors.iter().any(|error| error.message.contains("overlap")));
    }
}
