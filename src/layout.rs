use std::collections::{HashMap, HashSet};

use crate::diagnostic::Diagnostic;
use crate::target::{
    Address24, EZRA_ASSET_BASE, EZRA_AUDIO_BASE, EZRA_CODE_BASE, EZRA_ENTRY_ADDR, EZRA_LOAD_ADDR,
    EZRA_RAM_BASE, EZRA_RODATA_BASE, EZRA_STACK_TOP, EZRA_VRAM_BASE,
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
                symbol("EZRA_CODE_BASE", EZRA_CODE_BASE),
                symbol("EZRA_STACK_TOP", EZRA_STACK_TOP),
                symbol("EZRA_RAM_BASE", EZRA_RAM_BASE),
                symbol("EZRA_VRAM_BASE", EZRA_VRAM_BASE),
                symbol("EZRA_AUDIO_BASE", EZRA_AUDIO_BASE),
                symbol("EZRA_ASSET_BASE", EZRA_ASSET_BASE),
                symbol("EZRA_RODATA_BASE", EZRA_RODATA_BASE),
            ],
        }
    }

    pub fn agon_light_mos() -> Self {
        Self {
            name: "agon_light_mos".to_owned(),
            load: Address24::new(0x04_0000),
            entry: Address24::new(0x04_0045),
            stack: Address24::new(0x0B_FF00),
            regions: vec![
                region("low", 0x00_0000, 0x00_FFFF, &[RegionFlags::RESERVED]),
                region(
                    "mos",
                    0x01_0000,
                    0x03_FFBF,
                    &[
                        RegionFlags::READ,
                        RegionFlags::EXECUTE,
                        RegionFlags::RESERVED,
                    ],
                ),
                region("header", 0x04_0000, 0x04_003F, &[RegionFlags::READ]),
                region(
                    "code",
                    0x04_0045,
                    0x05_FFFF,
                    &[RegionFlags::READ, RegionFlags::EXECUTE],
                ),
                region("rodata", 0x06_0000, 0x06_FFFF, &[RegionFlags::READ]),
                region(
                    "ram",
                    0x07_0000,
                    0x0B_FFFF,
                    &[RegionFlags::READ, RegionFlags::WRITE],
                ),
                region("assets", 0x0C_0000, 0x0D_FFFF, &[RegionFlags::READ]),
                region(
                    "vdp",
                    0x0E_0000,
                    0x0E_FFFF,
                    &[RegionFlags::READ, RegionFlags::WRITE, RegionFlags::VOLATILE],
                ),
                region(
                    "scratch",
                    0x0F_0000,
                    0x0F_7FFF,
                    &[RegionFlags::READ, RegionFlags::WRITE],
                ),
                region(
                    "stack",
                    0x0F_8000,
                    0x0F_FFFF,
                    &[RegionFlags::READ, RegionFlags::WRITE, RegionFlags::RESERVED],
                ),
            ],
            sections: vec![
                section(".header", "header", 1),
                section(".text", "code", 16),
                section(".rodata", "rodata", 16),
                section(".data", "ram", 16),
                section(".bss", "ram", 16),
                section(".assets", "assets", 256),
                section(".scratch", "scratch", 16),
            ],
            symbols: vec![
                symbol("EZRA_LOAD_ADDR", Address24::new(0x04_0000)),
                symbol("EZRA_ENTRY_ADDR", Address24::new(0x04_0045)),
                symbol("EZRA_CODE_BASE", Address24::new(0x04_0045)),
                symbol("EZRA_STACK_TOP", Address24::new(0x0B_FF00)),
                symbol("EZRA_RAM_BASE", Address24::new(0x07_0000)),
                symbol("EZRA_RODATA_BASE", Address24::new(0x06_0000)),
                symbol("EZRA_ASSET_BASE", Address24::new(0x0C_0000)),
                symbol("AGON_VDP_PORT", Address24::new(0x0000_B0)),
                symbol("AGON_EMULATOR_EXIT_PORT", Address24::new(0x0000_00)),
            ],
        }
    }

    pub fn z80_default() -> Self {
        Self {
            name: "z80_default".to_owned(),
            load: Address24::new(0x0000),
            entry: Address24::new(0x0040),
            stack: Address24::new(0xFF00),
            regions: vec![
                region("header", 0x0000, 0x003F, &[RegionFlags::READ]),
                region(
                    "code",
                    0x0040,
                    0x7FFF,
                    &[RegionFlags::READ, RegionFlags::EXECUTE],
                ),
                region("rodata", 0x8000, 0x9FFF, &[RegionFlags::READ]),
                region(
                    "ram",
                    0xA000,
                    0xBFFF,
                    &[RegionFlags::READ, RegionFlags::WRITE],
                ),
                region("assets", 0xC000, 0xDFFF, &[RegionFlags::READ]),
                region(
                    "scratch",
                    0xE000,
                    0xEFFF,
                    &[RegionFlags::READ, RegionFlags::WRITE],
                ),
                region(
                    "stack",
                    0xF000,
                    0xFFFF,
                    &[RegionFlags::READ, RegionFlags::WRITE, RegionFlags::RESERVED],
                ),
            ],
            sections: vec![
                section(".header", "header", 1),
                section(".text", "code", 16),
                section(".rodata", "rodata", 16),
                section(".data", "ram", 16),
                section(".bss", "ram", 16),
                section(".assets", "assets", 256),
                section(".scratch", "scratch", 16),
            ],
            symbols: vec![
                symbol("EZRA_LOAD_ADDR", Address24::new(0x0000)),
                symbol("EZRA_ENTRY_ADDR", Address24::new(0x0040)),
                symbol("EZRA_CODE_BASE", Address24::new(0x0040)),
                symbol("EZRA_STACK_TOP", Address24::new(0xFF00)),
                symbol("EZRA_RAM_BASE", Address24::new(0xA000)),
                symbol("EZRA_RODATA_BASE", Address24::new(0x8000)),
                symbol("EZRA_ASSET_BASE", Address24::new(0xC000)),
            ],
        }
    }

    pub fn validate(&self) -> Result<(), Vec<Diagnostic>> {
        let mut diagnostics = Vec::new();
        let mut region_names = HashSet::new();

        for (index, region) in self.regions.iter().enumerate() {
            if !region_names.insert(region.name.clone()) {
                diagnostics.push(Diagnostic::new(format!(
                    "duplicate region `{}`",
                    region.name
                )));
            }

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

        let mut section_names = HashSet::new();
        for section in &self.sections {
            if !section_names.insert(section.name.clone()) {
                diagnostics.push(Diagnostic::new(format!(
                    "duplicate section `{}`",
                    section.name
                )));
            }

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
        for required in [
            ".header", ".text", ".rodata", ".data", ".bss", ".assets", ".scratch",
        ] {
            if !section_names.contains(required) {
                diagnostics.push(Diagnostic::new(format!(
                    "layout is missing required section `{required}`"
                )));
            }
        }

        let mut symbol_names = HashSet::new();
        for symbol in &self.symbols {
            if !symbol_names.insert(symbol.name.clone()) {
                diagnostics.push(Diagnostic::new(format!(
                    "duplicate symbol `{}`",
                    symbol.name
                )));
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
    let mut symbol_values = HashMap::new();

    for item in inner {
        match item.as_rule() {
            Rule::layout_load => load = Some(parse_single_address(item, "load")?),
            Rule::layout_entry => entry = Some(parse_single_address(item, "entry")?),
            Rule::layout_stack => stack = Some(parse_single_address(item, "stack")?),
            Rule::layout_region => regions.push(parse_region(item)?),
            Rule::layout_section => sections.push(parse_section(item)?),
            Rule::layout_symbol => {
                let symbol = parse_symbol(item, &symbol_values)?;
                symbol_values.insert(symbol.name.clone(), i128::from(symbol.value.get()));
                symbols.push(symbol);
            }
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

fn parse_symbol(
    pair: Pair<'_, Rule>,
    symbols: &HashMap<String, i128>,
) -> Result<Symbol, Diagnostic> {
    let mut inner = pair.into_inner();
    let name = inner
        .next()
        .ok_or_else(|| Diagnostic::new("symbol is missing a name"))?
        .as_str()
        .to_owned();
    let value = parse_symbol_address(
        inner
            .next()
            .ok_or_else(|| Diagnostic::new(format!("symbol `{name}` is missing a value")))?,
        symbols,
    )?;
    Ok(Symbol { name, value })
}

fn parse_symbol_address(
    pair: Pair<'_, Rule>,
    symbols: &HashMap<String, i128>,
) -> Result<Address24, Diagnostic> {
    let value = eval_layout_expr(pair, symbols)?;
    let value = u32::try_from(value).map_err(|_| {
        Diagnostic::new(format!(
            "address 0x{value:X} is outside the 24-bit address space"
        ))
    })?;
    Address24::try_from(value).map_err(|error| Diagnostic::new(error.to_string()))
}

fn parse_address(pair: Pair<'_, Rule>) -> Result<Address24, Diagnostic> {
    Address24::try_from(parse_u32(pair)?).map_err(|error| Diagnostic::new(error.to_string()))
}

fn eval_layout_expr(
    pair: Pair<'_, Rule>,
    symbols: &HashMap<String, i128>,
) -> Result<i128, Diagnostic> {
    match pair.as_rule() {
        Rule::expr
        | Rule::logical_or
        | Rule::logical_and
        | Rule::bit_or
        | Rule::bit_xor
        | Rule::bit_and
        | Rule::equality
        | Rule::comparison
        | Rule::shift
        | Rule::additive
        | Rule::multiplicative => eval_layout_binary_expr(pair, symbols),
        Rule::unary => eval_layout_unary_expr(pair, symbols),
        Rule::primary | Rule::literal => {
            let inner = pair
                .into_inner()
                .next()
                .ok_or_else(|| Diagnostic::new("layout expression is empty"))?;
            eval_layout_expr(inner, symbols)
        }
        Rule::int_lit => parse_i128(pair),
        Rule::bool_lit => Ok(if pair.as_str() == "true" { 1 } else { 0 }),
        Rule::path_expr => symbols
            .get(pair.as_str())
            .copied()
            .ok_or_else(|| Diagnostic::new(format!("unknown layout symbol `{}`", pair.as_str()))),
        Rule::cast_expr => {
            let mut inner = pair.into_inner();
            let ty = inner
                .next()
                .ok_or_else(|| Diagnostic::new("layout cast is missing a type"))?;
            let expr = inner
                .next()
                .ok_or_else(|| Diagnostic::new("layout cast is missing an expression"))?;
            let value = eval_layout_expr(expr, symbols)?;
            eval_layout_cast(value, ty)
        }
        other => Err(Diagnostic::new(format!(
            "unsupported layout expression `{}` ({other:?})",
            pair.as_str()
        ))),
    }
}

fn eval_layout_binary_expr(
    pair: Pair<'_, Rule>,
    symbols: &HashMap<String, i128>,
) -> Result<i128, Diagnostic> {
    let mut inner = pair.into_inner();
    let first = inner
        .next()
        .ok_or_else(|| Diagnostic::new("layout expression is empty"))?;
    let mut value = eval_layout_expr(first, symbols)?;
    while let Some(op) = inner.next() {
        let right = inner.next().ok_or_else(|| {
            Diagnostic::new("layout binary expression is missing a right operand")
        })?;
        let right = eval_layout_expr(right, symbols)?;
        value = eval_layout_binary_op(value, op.as_str().trim(), right)?;
    }
    Ok(value)
}

fn eval_layout_binary_op(left: i128, op: &str, right: i128) -> Result<i128, Diagnostic> {
    match op {
        "||" => Ok(if left != 0 || right != 0 { 1 } else { 0 }),
        "&&" => Ok(if left != 0 && right != 0 { 1 } else { 0 }),
        "|" => Ok(left | right),
        "^" => Ok(left ^ right),
        "&" => Ok(left & right),
        "==" => Ok(if left == right { 1 } else { 0 }),
        "!=" => Ok(if left != right { 1 } else { 0 }),
        "<" => Ok(if left < right { 1 } else { 0 }),
        "<=" => Ok(if left <= right { 1 } else { 0 }),
        ">" => Ok(if left > right { 1 } else { 0 }),
        ">=" => Ok(if left >= right { 1 } else { 0 }),
        "<<" => Ok(checked_layout_shift(left, right, true)),
        ">>" => Ok(checked_layout_shift(left, right, false)),
        "+" => left
            .checked_add(right)
            .ok_or_else(|| Diagnostic::new("layout expression addition overflowed")),
        "-" => left
            .checked_sub(right)
            .ok_or_else(|| Diagnostic::new("layout expression subtraction overflowed")),
        "*" => left
            .checked_mul(right)
            .ok_or_else(|| Diagnostic::new("layout expression multiplication overflowed")),
        "/" => {
            if right == 0 {
                Ok(0)
            } else if left == i128::MIN && right == -1 {
                Ok(i128::MIN)
            } else {
                Ok(left / right)
            }
        }
        "%" => {
            if right == 0 {
                Ok(0)
            } else if left == i128::MIN && right == -1 {
                Ok(0)
            } else {
                Ok(left % right)
            }
        }
        other => Err(Diagnostic::new(format!(
            "unsupported layout binary operator `{other}`"
        ))),
    }
}

fn eval_layout_cast(value: i128, ty: Pair<'_, Rule>) -> Result<i128, Diagnostic> {
    let inner = ty
        .into_inner()
        .next()
        .ok_or_else(|| Diagnostic::new("layout cast is missing a type"))?;
    match inner.as_rule() {
        Rule::named_ty => cast_layout_named_type(value, inner.as_str()),
        Rule::ptr_ty => Ok(wrap_layout_unsigned(value, 24)),
        Rule::array_ty => Err(Diagnostic::new("layout casts cannot target array types")),
        other => Err(Diagnostic::new(format!(
            "unsupported layout cast type `{}` ({other:?})",
            inner.as_str()
        ))),
    }
}

fn cast_layout_named_type(value: i128, name: &str) -> Result<i128, Diagnostic> {
    match name {
        "bool" => Ok(i128::from(value != 0)),
        "u8" => Ok(wrap_layout_unsigned(value, 8)),
        "u16" => Ok(wrap_layout_unsigned(value, 16)),
        "u24" | "ptr24" => Ok(wrap_layout_unsigned(value, 24)),
        "i8" => Ok(wrap_layout_signed(value, 8)),
        "i16" => Ok(wrap_layout_signed(value, 16)),
        "i24" => Ok(wrap_layout_signed(value, 24)),
        other => Err(Diagnostic::new(format!(
            "unknown layout cast type `{other}`"
        ))),
    }
}

fn wrap_layout_unsigned(value: i128, bits: u32) -> i128 {
    value & ((1_i128 << bits) - 1)
}

fn wrap_layout_signed(value: i128, bits: u32) -> i128 {
    let unsigned = wrap_layout_unsigned(value, bits);
    let sign_bit = 1_i128 << (bits - 1);
    if unsigned & sign_bit != 0 {
        unsigned - (1_i128 << bits)
    } else {
        unsigned
    }
}

fn checked_layout_shift(left: i128, right: i128, shift_left: bool) -> i128 {
    let Ok(shift) = u32::try_from(right) else {
        return 0;
    };
    if shift >= i128::BITS {
        return 0;
    }
    if shift_left {
        left.checked_shl(shift).unwrap_or(0)
    } else {
        left.checked_shr(shift).unwrap_or(0)
    }
}

fn eval_layout_unary_expr(
    pair: Pair<'_, Rule>,
    symbols: &HashMap<String, i128>,
) -> Result<i128, Diagnostic> {
    let mut ops = Vec::new();
    let mut value = None;
    for item in pair.into_inner() {
        match item.as_rule() {
            Rule::unary_op => ops.push(item.as_str().to_owned()),
            _ => value = Some(eval_layout_expr(item, symbols)?),
        }
    }
    let mut value = value.ok_or_else(|| Diagnostic::new("layout unary expression is empty"))?;
    for op in ops.iter().rev() {
        value = eval_layout_unary_op(op, value)?;
    }
    Ok(value)
}

fn eval_layout_unary_op(op: &str, value: i128) -> Result<i128, Diagnostic> {
    match op {
        "-" => Ok(value.wrapping_neg()),
        "~" => Ok(!value),
        "!" => Ok(if value == 0 { 1 } else { 0 }),
        other => Err(Diagnostic::new(format!(
            "unsupported layout unary operator `{other}`"
        ))),
    }
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

fn parse_i128(pair: Pair<'_, Rule>) -> Result<i128, Diagnostic> {
    let text = pair.as_str();
    let value = text
        .trim_end_matches("u8")
        .trim_end_matches("i8")
        .trim_end_matches("u16")
        .trim_end_matches("i16")
        .trim_end_matches("u24")
        .trim_end_matches("i24");
    let parsed = if let Some(hex) = value.strip_prefix("0x") {
        i128::from_str_radix(hex, 16)
    } else if let Some(bin) = value.strip_prefix("0b") {
        i128::from_str_radix(bin, 2)
    } else {
        value.parse::<i128>()
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
    fn z80_default_layout_validates_and_stays_in_16_bit_address_space() {
        let layout = Layout::z80_default();

        assert_eq!(layout.validate(), Ok(()));
        assert!(layout.load.get() <= 0xFFFF);
        assert!(layout.entry.get() <= 0xFFFF);
        assert!(layout.stack.get() <= 0xFFFF);
        assert!(
            layout
                .regions
                .iter()
                .all(|region| region.end.get() <= 0xFFFF)
        );
        assert!(
            layout
                .symbols
                .iter()
                .all(|symbol| symbol.value.get() <= 0xFFFF)
        );
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
                symbol EZRA_CODE_BASE   = 0x010040;
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
    fn parses_layout_symbol_expressions() {
        let source = r#"
            layout exprs {
                load 0x010000;
                entry 0x010040;
                stack 0xF00000;

                symbol TEXT_END = 0x010040 + 0x20 * 3;
                symbol MIRROR = TEXT_END + (0b1000 | 0b0011);
                symbol DIV_ZERO = 0x123456 / 0;
                symbol MOD_ZERO = 0x123456 % 0;
                symbol CAST_BYTE = cast<u8>(0x1234);
                symbol CAST_BOOL = cast<bool>(0x20);
                symbol CAST_PTR = cast<ptr<u8>>(0x1020003);
                symbol NEG_WRAP = cast<u8>(-3);
                symbol NOT_ZERO = !0;
                symbol BIT_NOT = ~0x00FF & 0xFFFF;
                symbol SHIFT_LEFT = 3 << 4;
                symbol SHIFT_RIGHT = 0x8000 >> 8;
                symbol LOGIC = (1 < 2) && (3 >= 3) || false;
                symbol COMPARE = 0x20 != 0x10;
            }
        "#;

        let layout = parse_layout(source).unwrap();

        assert_eq!(layout.symbols[0].value.get(), 0x010040 + 0x20 * 3);
        assert_eq!(
            layout.symbols[1].value.get(),
            0x010040 + 0x20 * 3 + (0b1000 | 0b0011)
        );
        assert_eq!(layout.symbols[2].value.get(), 0);
        assert_eq!(layout.symbols[3].value.get(), 0);
        assert_eq!(layout.symbols[4].value.get(), 0x34);
        assert_eq!(layout.symbols[5].value.get(), 1);
        assert_eq!(layout.symbols[6].value.get(), 0x020003);
        assert_eq!(layout.symbols[7].value.get(), 253);
        assert_eq!(layout.symbols[8].value.get(), 1);
        assert_eq!(layout.symbols[9].value.get(), (!0x00FF_u32) & 0xFFFF);
        assert_eq!(layout.symbols[10].value.get(), 3 << 4);
        assert_eq!(layout.symbols[11].value.get(), 0x8000 >> 8);
        assert_eq!(layout.symbols[12].value.get(), 1);
        assert_eq!(layout.symbols[13].value.get(), 1);
    }

    #[test]
    fn layout_arithmetic_overflow_is_defined() {
        assert_eq!(
            eval_layout_binary_op(i128::MIN, "/", -1).unwrap(),
            i128::MIN
        );
        assert_eq!(eval_layout_binary_op(i128::MIN, "%", -1).unwrap(), 0);
        assert_eq!(eval_layout_unary_op("-", i128::MIN).unwrap(), i128::MIN);
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
    fn rejects_duplicate_layout_names() {
        let source = r#"
            layout duplicate_names {
                load 0x010000;
                entry 0x010040;
                stack 0xF00000;

                region code 0x010000..0x01FFFF read execute;
                region code 0x020000..0x02FFFF read;
                section .text -> code align 16;
                section .text -> code align 32;
                symbol BASE = 0x010000;
                symbol BASE = 0x020000;
            }
        "#;

        let layout = parse_layout(source).unwrap();
        let errors = layout.validate().unwrap_err();

        assert!(
            errors
                .iter()
                .any(|error| error.message == "duplicate region `code`"),
            "{errors:?}"
        );
        assert!(
            errors
                .iter()
                .any(|error| error.message == "duplicate section `.text`"),
            "{errors:?}"
        );
        assert!(
            errors
                .iter()
                .any(|error| error.message == "duplicate symbol `BASE`"),
            "{errors:?}"
        );
    }

    #[test]
    fn rejects_layouts_missing_required_sections() {
        let source = r#"
            layout missing_sections {
                load 0x010000;
                entry 0x010040;
                stack 0xF00000;

                region code 0x010000..0x01FFFF read execute;
                region ram 0x040000..0x07FFFF read write;
                section .header -> code align 64;
                section .text -> code align 16;
            }
        "#;

        let layout = parse_layout(source).unwrap();
        let errors = layout.validate().unwrap_err();

        for section in [".rodata", ".data", ".bss", ".assets", ".scratch"] {
            assert!(
                errors.iter().any(|error| {
                    error.message == format!("layout is missing required section `{section}`")
                }),
                "{errors:?}"
            );
        }
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
