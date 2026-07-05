use crate::diagnostic::Diagnostic;
use crate::target::{
    Address24, EZRA_ASSET_BASE, EZRA_AUDIO_BASE, EZRA_ENTRY_ADDR, EZRA_LOAD_ADDR, EZRA_RAM_BASE,
    EZRA_RODATA_BASE, EZRA_STACK_TOP, EZRA_VRAM_BASE,
};
use pest::{Parser, iterators::Pair};
use pest_derive::Parser;

#[derive(Parser)]
#[grammar = "ezra.pest"]
struct LayoutParser;

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

pub fn parse_layout(source: &str) -> Result<Layout, Diagnostic> {
    let mut pairs = LayoutParser::parse(Rule::layout_file, source)
        .map_err(|error| Diagnostic::new(error.to_string()))?;
    let file = pairs
        .next()
        .ok_or_else(|| Diagnostic::new("parser produced no layout"))?;
    let declaration = file
        .into_inner()
        .find(|pair| pair.as_rule() == Rule::layout_decl)
        .ok_or_else(|| Diagnostic::new("parser produced no layout declaration"))?;
    build_layout(declaration)
}

fn build_layout(pair: Pair<'_, Rule>) -> Result<Layout, Diagnostic> {
    let mut inner = pair.into_inner();
    let name = inner
        .next()
        .ok_or_else(|| Diagnostic::new("layout is missing a name"))?
        .as_str()
        .to_owned();
    let mut load = None;
    let mut entry = None;
    let mut stack = None;
    let mut regions = Vec::new();
    let mut sections = Vec::new();
    let mut symbols = Vec::new();

    for item in inner {
        match item.as_rule() {
            Rule::layout_load => load = Some(parse_single_address(item, "load")?),
            Rule::layout_entry => entry = Some(parse_single_address(item, "entry")?),
            Rule::layout_stack => stack = Some(parse_single_address(item, "stack")?),
            Rule::layout_region => regions.push(parse_region(item)?),
            Rule::layout_section => sections.push(parse_section(item)?),
            Rule::layout_symbol => symbols.push(parse_symbol(item)?),
            _ => unreachable!("unexpected layout item {:?}", item.as_rule()),
        }
    }

    Ok(Layout {
        name,
        load: load.ok_or_else(|| Diagnostic::new("layout is missing `load`"))?,
        entry: entry.ok_or_else(|| Diagnostic::new("layout is missing `entry`"))?,
        stack: stack.ok_or_else(|| Diagnostic::new("layout is missing `stack`"))?,
        regions,
        sections,
        symbols,
    })
}

fn parse_single_address(pair: Pair<'_, Rule>, field: &str) -> Result<Address24, Diagnostic> {
    let value = pair
        .into_inner()
        .next()
        .ok_or_else(|| Diagnostic::new(format!("layout `{field}` is missing an address")))?;
    parse_address(value)
}

fn parse_region(pair: Pair<'_, Rule>) -> Result<Region, Diagnostic> {
    let mut inner = pair.into_inner();
    let name = inner
        .next()
        .ok_or_else(|| Diagnostic::new("region is missing a name"))?
        .as_str()
        .to_owned();
    let start = parse_address(
        inner
            .next()
            .ok_or_else(|| Diagnostic::new(format!("region `{name}` is missing a start")))?,
    )?;
    let end = parse_address(
        inner
            .next()
            .ok_or_else(|| Diagnostic::new(format!("region `{name}` is missing an end")))?,
    )?;
    let mut flags = RegionFlags::empty();
    for flag in inner {
        let flag = match flag.as_str() {
            "read" => RegionFlags::READ,
            "write" => RegionFlags::WRITE,
            "execute" => RegionFlags::EXECUTE,
            "volatile" => RegionFlags::VOLATILE,
            "reserved" => RegionFlags::RESERVED,
            other => {
                return Err(Diagnostic::new(format!(
                    "unknown region flag `{other}` on region `{name}`"
                )));
            }
        };
        flags = flags.union(flag);
    }
    Ok(Region {
        name,
        start,
        end,
        flags,
    })
}

fn parse_section(pair: Pair<'_, Rule>) -> Result<Section, Diagnostic> {
    let mut inner = pair.into_inner();
    let name = inner
        .next()
        .ok_or_else(|| Diagnostic::new("section is missing a name"))?
        .as_str()
        .to_owned();
    let region = inner
        .next()
        .ok_or_else(|| Diagnostic::new(format!("section `{name}` is missing a region")))?
        .as_str()
        .to_owned();
    let align = parse_u32(
        inner
            .next()
            .ok_or_else(|| Diagnostic::new(format!("section `{name}` is missing alignment")))?,
    )?;
    Ok(Section {
        name,
        region,
        align,
    })
}

fn parse_symbol(pair: Pair<'_, Rule>) -> Result<Symbol, Diagnostic> {
    let mut inner = pair.into_inner();
    let name = inner
        .next()
        .ok_or_else(|| Diagnostic::new("symbol is missing a name"))?
        .as_str()
        .to_owned();
    let value = parse_address(
        inner
            .next()
            .ok_or_else(|| Diagnostic::new(format!("symbol `{name}` is missing a value")))?,
    )?;
    Ok(Symbol { name, value })
}

fn parse_address(pair: Pair<'_, Rule>) -> Result<Address24, Diagnostic> {
    Address24::try_from(parse_u32(pair)?).map_err(|error| Diagnostic::new(error.to_string()))
}

fn parse_u32(pair: Pair<'_, Rule>) -> Result<u32, Diagnostic> {
    let text = pair.as_str();
    let value = text
        .trim_end_matches("u8")
        .trim_end_matches("i8")
        .trim_end_matches("u16")
        .trim_end_matches("i16")
        .trim_end_matches("u24")
        .trim_end_matches("i24");
    let parsed = if let Some(hex) = value.strip_prefix("0x") {
        u32::from_str_radix(hex, 16)
    } else if let Some(bin) = value.strip_prefix("0b") {
        u32::from_str_radix(bin, 2)
    } else {
        value.parse::<u32>()
    };
    parsed.map_err(|_| Diagnostic::new(format!("invalid integer literal `{text}`")))
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

    #[test]
    fn parses_default_layout_file_shape() {
        let source = r#"
            layout ezra_default {
                load  0x010000;
                entry 0x010040;
                stack 0xF00000;

                region low       0x000000..0x00FFFF reserved;
                region code      0x010000..0x01FFFF read execute;
                region rodata    0x020000..0x03FFFF read;
                region ram       0x040000..0x07FFFF read write;
                region vram      0x080000..0x0BFFFF read write volatile;
                region audio     0x0C0000..0x0FFFFF read write volatile;
                region assets    0x100000..0xDFFFFF read;
                region scratch   0xE00000..0xEFFFFF read write;
                region stack     0xF00000..0xFFFFFF read write reserved;

                section .header  -> code   align 64;
                section .text    -> code   align 16;
                section .rodata  -> rodata align 16;
                section .data    -> ram    align 16;
                section .bss     -> ram    align 16;
                section .assets  -> assets align 256;
                section .scratch -> scratch align 16;

                symbol EZRA_LOAD_ADDR   = 0x010000;
                symbol EZRA_ENTRY_ADDR  = 0x010040;
                symbol EZRA_STACK_TOP   = 0xF00000;
                symbol EZRA_RAM_BASE    = 0x040000;
                symbol EZRA_VRAM_BASE   = 0x080000;
                symbol EZRA_AUDIO_BASE  = 0x0C0000;
                symbol EZRA_ASSET_BASE  = 0x100000;
                symbol EZRA_RODATA_BASE = 0x020000;
            }
        "#;

        let layout = parse_layout(source).unwrap();

        assert_eq!(layout, Layout::ezra_default());
        assert_eq!(layout.validate(), Ok(()));
    }

    #[test]
    fn parsed_layout_uses_existing_validator() {
        let source = r#"
            layout bad {
                load 0x010000;
                entry 0x010040;
                stack 0xF00000;

                region code 0x010000..0x01FFFF read execute;
                region also_code 0x018000..0x02FFFF read;
                section .text -> missing align 24;
            }
        "#;

        let layout = parse_layout(source).unwrap();
        let errors = layout.validate().unwrap_err();

        assert!(
            errors.iter().any(|error| error.message.contains("overlap")),
            "{errors:?}"
        );
        assert!(
            errors
                .iter()
                .any(|error| error.message.contains("alignment")),
            "{errors:?}"
        );
        assert!(
            errors
                .iter()
                .any(|error| error.message.contains("unknown region")),
            "{errors:?}"
        );
    }

    #[test]
    fn rejects_layout_address_outside_24_bit_space() {
        let error = parse_layout(
            r#"
                layout too_wide {
                    load 0x1000000;
                    entry 0x010040;
                    stack 0xF00000;
                }
            "#,
        )
        .unwrap_err();

        assert!(error.message.contains("outside the 24-bit address space"));
    }
}
