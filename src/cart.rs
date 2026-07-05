use std::{fs, path::Path};

use crate::{
    ast::{BinaryOp, Declaration, EmbedSource, Expr, Program, UnaryOp},
    diagnostic::Diagnostic,
    layout::{Layout, Region},
    target::{
        Address24, CART_MAGIC, CPU_MODE_EZ80_ADL, EZRA_ASSET_BASE, EZRA_AUDIO_BASE,
        EZRA_ENTRY_ADDR, EZRA_RAM_BASE, EZRA_STACK_TOP, EZRA_VRAM_BASE, FORMAT_VERSION,
        HEADER_SIZE,
    },
    vm::AssemblySymbol,
};

const ASSET_TABLE_ENTRY_SIZE: usize = 10;
const SECTION_ASSETS: u8 = 1;
const SECTION_RODATA: u8 = 2;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CartridgeHeader {
    pub format_version: u8,
    pub cpu_mode: u8,
    pub flags: u8,
    pub entry_addr: Address24,
    pub stack_top: Address24,
    pub ram_base: Address24,
    pub vram_base: Address24,
    pub audio_base: Address24,
    pub asset_base: Address24,
    pub header_size: u16,
    pub layout_table_addr: Option<Address24>,
    pub asset_table_addr: Option<Address24>,
    pub symbol_table_addr: Option<Address24>,
}

impl CartridgeHeader {
    pub fn serialize(&self) -> [u8; HEADER_SIZE as usize] {
        let mut bytes = [0; HEADER_SIZE as usize];

        bytes[0x00..0x04].copy_from_slice(CART_MAGIC);
        bytes[0x04] = self.format_version;
        bytes[0x05] = self.cpu_mode;
        bytes[0x06] = self.flags;
        write_addr24(&mut bytes, 0x08, self.entry_addr);
        write_addr24(&mut bytes, 0x0B, self.stack_top);
        write_addr24(&mut bytes, 0x0E, self.ram_base);
        write_addr24(&mut bytes, 0x11, self.vram_base);
        write_addr24(&mut bytes, 0x14, self.audio_base);
        write_addr24(&mut bytes, 0x17, self.asset_base);
        bytes[0x1A..0x1C].copy_from_slice(&self.header_size.to_le_bytes());
        write_optional_addr24(&mut bytes, 0x1E, self.layout_table_addr);
        write_optional_addr24(&mut bytes, 0x21, self.asset_table_addr);
        write_optional_addr24(&mut bytes, 0x24, self.symbol_table_addr);

        bytes
    }
}

impl Default for CartridgeHeader {
    fn default() -> Self {
        Self {
            format_version: FORMAT_VERSION,
            cpu_mode: CPU_MODE_EZ80_ADL,
            flags: 0,
            entry_addr: EZRA_ENTRY_ADDR,
            stack_top: EZRA_STACK_TOP,
            ram_base: EZRA_RAM_BASE,
            vram_base: EZRA_VRAM_BASE,
            audio_base: EZRA_AUDIO_BASE,
            asset_base: EZRA_ASSET_BASE,
            header_size: HEADER_SIZE,
            layout_table_addr: None,
            asset_table_addr: None,
            symbol_table_addr: None,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct AssetEntry {
    name: String,
    bytes: Vec<u8>,
    align: u32,
    section: String,
    section_id: u8,
    flags: u8,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct PackedAssetEntry {
    asset_addr: Address24,
    asset_len: u32,
    name_offset: u16,
    bytes: Vec<u8>,
    section_id: u8,
    flags: u8,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CartridgeMapEntry {
    pub name: String,
    pub start: Address24,
    pub end: Address24,
    pub size: u32,
}

pub fn build_cartridge(program: &Program) -> Result<Vec<u8>, Diagnostic> {
    build_cartridge_with_code(program, &[])
}

pub fn build_cartridge_with_code(program: &Program, code: &[u8]) -> Result<Vec<u8>, Diagnostic> {
    build_cartridge_with_code_and_symbols(program, code, &[])
}

pub fn build_cartridge_with_code_and_symbols(
    program: &Program,
    code: &[u8],
    symbols: &[AssemblySymbol],
) -> Result<Vec<u8>, Diagnostic> {
    build_cartridge_with_layout_code_and_symbols(program, &Layout::ezra_default(), code, symbols)
}

pub fn build_cartridge_with_layout_code_and_symbols(
    program: &Program,
    layout: &Layout,
    code: &[u8],
    symbols: &[AssemblySymbol],
) -> Result<Vec<u8>, Diagnostic> {
    validate_text_section_fit(layout, code.len())?;
    let code_offset = layout
        .entry
        .get()
        .checked_sub(layout.load.get())
        .ok_or_else(|| {
            Diagnostic::new(format!(
                "entry {} is below load address {}",
                layout.entry, layout.load
            ))
        })?;
    if code_offset < u32::from(HEADER_SIZE) {
        return Err(Diagnostic::new(format!(
            "entry {} overlaps the cartridge header at {}",
            layout.entry, layout.load
        )));
    }

    let layout_table = serialize_layout_table(layout);
    let symbol_table = serialize_symbol_table(symbols);
    let code_len = u32::try_from(code.len())
        .map_err(|_| Diagnostic::new("program code exceeds 24-bit address space"))?;
    let layout_offset = code_offset
        .checked_add(code_len)
        .ok_or_else(|| Diagnostic::new("program code exceeds 24-bit address space"))?;
    let symbol_offset = layout_offset
        .checked_add(
            u32::try_from(layout_table.len())
                .map_err(|_| Diagnostic::new("layout table exceeds 24-bit address space"))?,
        )
        .ok_or_else(|| Diagnostic::new("layout table exceeds 24-bit address space"))?;
    let asset_table_offset = symbol_offset
        .checked_add(
            u32::try_from(symbol_table.len())
                .map_err(|_| Diagnostic::new("symbol table exceeds 24-bit address space"))?,
        )
        .ok_or_else(|| Diagnostic::new("symbol table exceeds 24-bit address space"))?;

    let layout_table_addr = checked_image_addr(layout.load, layout_offset)?;
    let symbol_table_addr = if symbol_table.is_empty() {
        None
    } else {
        Some(checked_image_addr(layout.load, symbol_offset)?)
    };
    let assets = collect_assets(program)?;
    let asset_table_addr = if assets.is_empty() {
        None
    } else {
        Some(checked_image_addr(layout.load, asset_table_offset)?)
    };
    let mut header = CartridgeHeader {
        layout_table_addr: Some(layout_table_addr),
        asset_table_addr,
        symbol_table_addr,
        ..header_from_layout(layout)
    };
    let mut table = Vec::with_capacity(assets.len() * ASSET_TABLE_ENTRY_SIZE);
    let mut names = Vec::new();
    let mut packed = Vec::new();
    let mut section_cursors = Vec::<(String, u32)>::new();

    for asset in assets {
        let name_offset = u16::try_from(names.len())
            .map_err(|_| Diagnostic::new("asset name table exceeds current 16-bit offset limit"))?;
        names.extend_from_slice(asset.name.as_bytes());
        names.push(0);

        let (section, section_align) = layout_section_placement(layout, &asset.section)?;
        let cursor = section_cursor(&mut section_cursors, &asset.section, section.start.get());
        *cursor = align_addr(*cursor, asset.align.max(section_align))?;
        let asset_addr =
            Address24::try_from(*cursor).map_err(|error| Diagnostic::new(error.to_string()))?;
        let asset_len = u32::try_from(asset.bytes.len())
            .map_err(|_| Diagnostic::new("asset length exceeds 24-bit address space"))?;
        if asset_len > Address24::MAX {
            return Err(Diagnostic::new(format!(
                "asset `{}` length {asset_len} is outside u24 range",
                asset.name
            )));
        }
        let asset_end = asset_addr
            .get()
            .checked_add(asset_len.saturating_sub(1))
            .ok_or_else(|| Diagnostic::new("asset payload exceeds 24-bit address space"))?;
        let asset_end =
            Address24::try_from(asset_end).map_err(|error| Diagnostic::new(error.to_string()))?;
        if asset_len > 0 && !section.contains_range(asset_addr, asset_end) {
            return Err(Diagnostic::new(format!(
                "embed `{}` exceeds section `{}` region `{}`",
                asset.name, asset.section, section.name
            )));
        }
        *cursor = cursor
            .checked_add(asset_len)
            .ok_or_else(|| Diagnostic::new("asset payload exceeds 24-bit address space"))?;
        packed.push(PackedAssetEntry {
            asset_addr,
            asset_len,
            name_offset,
            bytes: asset.bytes,
            section_id: asset.section_id,
            flags: asset.flags,
        });
    }

    for entry in &packed {
        table.extend_from_slice(&entry.asset_addr.to_le_bytes3());
        table.extend_from_slice(&entry.asset_len.to_le_bytes()[..3]);
        table.extend_from_slice(&entry.name_offset.to_le_bytes());
        table.push(entry.section_id);
        table.push(entry.flags);
    }

    let mut image = header.serialize().to_vec();
    let code_start = usize::try_from(code_offset)
        .map_err(|_| Diagnostic::new("code offset exceeds host usize range"))?;
    let code_end = code_start
        .checked_add(code.len())
        .ok_or_else(|| Diagnostic::new("program code exceeds addressable image size"))?;
    if image.len() < code_end {
        image.resize(code_end, 0);
    }
    image[code_start..code_end].copy_from_slice(code);
    image.extend_from_slice(&layout_table);
    image.extend_from_slice(&symbol_table);
    image.append(&mut table);
    image.append(&mut names);
    for entry in &packed {
        let offset = image_offset(layout.load, entry.asset_addr)?;
        let end = offset
            .checked_add(entry.bytes.len())
            .ok_or_else(|| Diagnostic::new("asset payload exceeds addressable image size"))?;
        if image.len() < end {
            image.resize(end, 0);
        }
        image[offset..end].copy_from_slice(&entry.bytes);
    }
    header.header_size = HEADER_SIZE;
    image[..HEADER_SIZE as usize].copy_from_slice(&header.serialize());
    Ok(image)
}

pub fn build_cartridge_map(
    program: &Program,
    layout: &Layout,
    code_len: usize,
    symbols: &[AssemblySymbol],
) -> Result<String, Diagnostic> {
    let entries = cartridge_map_entries(program, layout, code_len, symbols)?;
    let mut out = String::from("section      start      end        size\n");
    for entry in entries {
        out.push_str(&format!(
            "{:<12} {} {} 0x{:06X}\n",
            entry.name, entry.start, entry.end, entry.size
        ));
    }
    Ok(out)
}

pub fn cartridge_map_entries(
    program: &Program,
    layout: &Layout,
    code_len: usize,
    symbols: &[AssemblySymbol],
) -> Result<Vec<CartridgeMapEntry>, Diagnostic> {
    validate_text_section_fit(layout, code_len)?;
    let code_offset = layout
        .entry
        .get()
        .checked_sub(layout.load.get())
        .ok_or_else(|| {
            Diagnostic::new(format!(
                "entry {} is below load address {}",
                layout.entry, layout.load
            ))
        })?;
    if code_offset < u32::from(HEADER_SIZE) {
        return Err(Diagnostic::new(format!(
            "entry {} overlaps the cartridge header at {}",
            layout.entry, layout.load
        )));
    }

    let code_len = u32::try_from(code_len)
        .map_err(|_| Diagnostic::new("program code exceeds 24-bit address space"))?;
    let layout_table = serialize_layout_table(layout);
    let symbol_table = serialize_symbol_table(symbols);
    let layout_len = u32::try_from(layout_table.len())
        .map_err(|_| Diagnostic::new("layout table exceeds 24-bit address space"))?;
    let symbol_len = u32::try_from(symbol_table.len())
        .map_err(|_| Diagnostic::new("symbol table exceeds 24-bit address space"))?;
    let layout_offset = code_offset
        .checked_add(code_len)
        .ok_or_else(|| Diagnostic::new("program code exceeds 24-bit address space"))?;
    let symbol_offset = layout_offset
        .checked_add(layout_len)
        .ok_or_else(|| Diagnostic::new("layout table exceeds 24-bit address space"))?;
    let asset_table_offset = symbol_offset
        .checked_add(symbol_len)
        .ok_or_else(|| Diagnostic::new("symbol table exceeds 24-bit address space"))?;

    let mut entries = Vec::new();
    entries.push(map_entry(
        ".header",
        layout.load.get(),
        u32::from(HEADER_SIZE),
    )?);
    entries.push(map_entry(".text", layout.entry.get(), code_len)?);
    entries.push(map_entry(
        ".layout_table",
        checked_image_addr(layout.load, layout_offset)?.get(),
        layout_len,
    )?);
    if symbol_len > 0 {
        entries.push(map_entry(
            ".symbol_table",
            checked_image_addr(layout.load, symbol_offset)?.get(),
            symbol_len,
        )?);
    }

    let assets = collect_assets(program)?;
    if assets.is_empty() {
        return Ok(entries);
    }

    let mut asset_table_len = u32::try_from(assets.len() * ASSET_TABLE_ENTRY_SIZE)
        .map_err(|_| Diagnostic::new("asset table exceeds 24-bit address space"))?;
    asset_table_len = asset_table_len
        .checked_add(
            assets
                .iter()
                .map(|asset| asset.name.len() + 1)
                .sum::<usize>()
                .try_into()
                .map_err(|_| Diagnostic::new("asset name table exceeds 24-bit address space"))?,
        )
        .ok_or_else(|| Diagnostic::new("asset table exceeds 24-bit address space"))?;
    entries.push(map_entry(
        ".asset_table",
        checked_image_addr(layout.load, asset_table_offset)?.get(),
        asset_table_len,
    )?);

    let mut section_cursors = Vec::<(String, u32)>::new();
    for asset in assets {
        let (section, section_align) = layout_section_placement(layout, &asset.section)?;
        let cursor = section_cursor(&mut section_cursors, &asset.section, section.start.get());
        *cursor = align_addr(*cursor, asset.align.max(section_align))?;
        let asset_addr =
            Address24::try_from(*cursor).map_err(|error| Diagnostic::new(error.to_string()))?;
        let asset_len = u32::try_from(asset.bytes.len())
            .map_err(|_| Diagnostic::new("asset length exceeds 24-bit address space"))?;
        let asset_end = asset_addr
            .get()
            .checked_add(asset_len.saturating_sub(1))
            .ok_or_else(|| Diagnostic::new("asset payload exceeds 24-bit address space"))?;
        let asset_end =
            Address24::try_from(asset_end).map_err(|error| Diagnostic::new(error.to_string()))?;
        if asset_len > 0 && !section.contains_range(asset_addr, asset_end) {
            return Err(Diagnostic::new(format!(
                "embed `{}` exceeds section `{}` region `{}`",
                asset.name, asset.section, section.name
            )));
        }
        entries.push(map_entry(
            &format!("{}:{}", asset.section, asset.name),
            asset_addr.get(),
            asset_len,
        )?);
        *cursor = cursor
            .checked_add(asset_len)
            .ok_or_else(|| Diagnostic::new("asset payload exceeds 24-bit address space"))?;
    }

    Ok(entries)
}

fn collect_assets(program: &Program) -> Result<Vec<AssetEntry>, Diagnostic> {
    let mut assets = Vec::new();
    for declaration in &program.declarations {
        let Declaration::Embed(decl) = declaration else {
            continue;
        };
        if module_alias_original_name(&decl.name).is_some() {
            continue;
        }
        let align = decl
            .align
            .as_ref()
            .map(eval_embed_expr)
            .transpose()?
            .unwrap_or(1);
        if align <= 0 || (align & (align - 1)) != 0 {
            return Err(Diagnostic::new(format!(
                "embed `{}` alignment {align} is not a positive power of two",
                decl.name
            )));
        }
        let section = decl.section.clone().unwrap_or_else(|| ".assets".to_owned());
        let section_id = section_id(&section)?;
        let bytes = embed_bytes(&decl.source, &program.source_path)?;
        assets.push(AssetEntry {
            name: decl.name.clone(),
            bytes,
            align: align as u32,
            section,
            section_id,
            flags: 0,
        });
    }
    Ok(assets)
}

fn section_id(section: &str) -> Result<u8, Diagnostic> {
    match section {
        ".assets" => Ok(SECTION_ASSETS),
        ".rodata" => Ok(SECTION_RODATA),
        section => Err(Diagnostic::new(format!(
            "embed section `{section}` is not supported by the current cartridge packer"
        ))),
    }
}

fn layout_section_placement<'a>(
    layout: &'a Layout,
    section_name: &str,
) -> Result<(&'a Region, u32), Diagnostic> {
    let section = layout
        .sections
        .iter()
        .find(|section| section.name == section_name)
        .ok_or_else(|| Diagnostic::new(format!("layout has no section `{section_name}`")))?;
    let region = layout
        .regions
        .iter()
        .find(|region| region.name == section.region)
        .ok_or_else(|| {
            Diagnostic::new(format!(
                "layout section `{section_name}` targets unknown region `{}`",
                section.region
            ))
        })?;
    Ok((region, section.align))
}

fn validate_text_section_fit(layout: &Layout, code_len: usize) -> Result<(), Diagnostic> {
    let (region, _) = layout_section_placement(layout, ".text")?;
    if code_len == 0 {
        if !region.contains_range(layout.entry, layout.entry) {
            return Err(Diagnostic::new(format!(
                "section `.text` does not fit in region `{}`",
                region.name
            )));
        }
        return Ok(());
    }

    let code_len = u32::try_from(code_len)
        .map_err(|_| Diagnostic::new("program code exceeds 24-bit address space"))?;
    let end = layout
        .entry
        .get()
        .checked_add(code_len - 1)
        .ok_or_else(|| Diagnostic::new("section `.text` exceeds 24-bit address space"))?;
    let end = Address24::try_from(end).map_err(|error| Diagnostic::new(error.to_string()))?;
    if !region.contains_range(layout.entry, end) {
        return Err(Diagnostic::new(format!(
            "section `.text` does not fit in region `{}`",
            region.name
        )));
    }
    Ok(())
}

fn section_cursor<'a>(
    cursors: &'a mut Vec<(String, u32)>,
    section: &str,
    start: u32,
) -> &'a mut u32 {
    if let Some(index) = cursors.iter().position(|(name, _)| name == section) {
        return &mut cursors[index].1;
    }
    cursors.push((section.to_owned(), start));
    &mut cursors.last_mut().expect("cursor was just pushed").1
}

fn embed_bytes(source: &EmbedSource, source_path: &Path) -> Result<Vec<u8>, Diagnostic> {
    match source {
        EmbedSource::File(path) => {
            let path = Path::new(path);
            let resolved = if path.is_absolute() {
                path.to_path_buf()
            } else {
                source_path
                    .parent()
                    .unwrap_or_else(|| Path::new("."))
                    .join(path)
            };
            fs::read(&resolved).map_err(|error| {
                Diagnostic::new(format!(
                    "failed to read embedded file `{}`: {error}",
                    resolved.display()
                ))
            })
        }
        EmbedSource::Bytes(values) => values
            .iter()
            .map(|value| {
                let byte = eval_embed_expr(value)?;
                if !(0..=0xFF).contains(&byte) {
                    return Err(Diagnostic::new(format!(
                        "embedded byte value {byte} is outside u8 range"
                    )));
                }
                Ok(byte as u8)
            })
            .collect(),
        EmbedSource::Text(text) => Ok(text.as_bytes().to_vec()),
        EmbedSource::CStr(text) => {
            let mut bytes = text.as_bytes().to_vec();
            bytes.push(0);
            Ok(bytes)
        }
        EmbedSource::Repeat { value, len } => {
            let byte = eval_embed_expr(value)?;
            if !(0..=0xFF).contains(&byte) {
                return Err(Diagnostic::new(format!(
                    "embedded repeat byte value {byte} is outside u8 range"
                )));
            }
            let len = eval_embed_expr(len)?;
            if !(0..=0xFF_FFFF).contains(&len) {
                return Err(Diagnostic::new(format!(
                    "embedded repeat length {len} is outside u24 range"
                )));
            }
            Ok(vec![byte as u8; len as usize])
        }
    }
}

fn eval_embed_expr(expr: &Expr) -> Result<i64, Diagnostic> {
    match expr {
        Expr::Int(value) => Ok(*value),
        Expr::Char(value) => Ok(i64::from(*value)),
        Expr::Bool(value) => Ok(i64::from(*value)),
        Expr::Unary { op, expr } => {
            let value = eval_embed_expr(expr)?;
            match op {
                UnaryOp::Neg => Ok(value.wrapping_neg()),
                UnaryOp::BitNot => Ok(!value),
                UnaryOp::Not => Ok(i64::from(value == 0)),
            }
        }
        Expr::Binary { left, op, right } => {
            let left = eval_embed_expr(left)?;
            let right = eval_embed_expr(right)?;
            match op {
                BinaryOp::Add => Ok(left.wrapping_add(right)),
                BinaryOp::Sub => Ok(left.wrapping_sub(right)),
                BinaryOp::Mul => Ok(left.wrapping_mul(right)),
                BinaryOp::Div => Ok(if right == 0 { 0 } else { left / right }),
                BinaryOp::Mod => Ok(if right == 0 { 0 } else { left % right }),
                BinaryOp::BitAnd => Ok(left & right),
                BinaryOp::BitOr => Ok(left | right),
                BinaryOp::BitXor => Ok(left ^ right),
                BinaryOp::Shl => Ok(if (0..64).contains(&right) {
                    left.wrapping_shl(right as u32)
                } else {
                    0
                }),
                BinaryOp::Shr => Ok(if (0..64).contains(&right) {
                    left.wrapping_shr(right as u32)
                } else {
                    0
                }),
                _ => Err(Diagnostic::new(
                    "embed expressions must be integer constants",
                )),
            }
        }
        _ => Err(Diagnostic::new(
            "embed expressions must be integer constants",
        )),
    }
}

fn align_addr(addr: u32, align: u32) -> Result<u32, Diagnostic> {
    if align <= 1 {
        return Ok(addr);
    }
    let mask = align - 1;
    addr.checked_add(mask)
        .map(|addr| addr & !mask)
        .ok_or_else(|| Diagnostic::new("asset alignment exceeds 24-bit address space"))
}

fn map_entry(name: &str, start: u32, size: u32) -> Result<CartridgeMapEntry, Diagnostic> {
    let end = if size == 0 {
        start
    } else {
        start
            .checked_add(size - 1)
            .ok_or_else(|| Diagnostic::new("map entry exceeds 24-bit address space"))?
    };
    Ok(CartridgeMapEntry {
        name: name.to_owned(),
        start: Address24::try_from(start).map_err(|error| Diagnostic::new(error.to_string()))?,
        end: Address24::try_from(end).map_err(|error| Diagnostic::new(error.to_string()))?,
        size,
    })
}

fn checked_image_addr(load: Address24, offset: u32) -> Result<Address24, Diagnostic> {
    let addr = load
        .get()
        .checked_add(offset)
        .ok_or_else(|| Diagnostic::new("cartridge image exceeds 24-bit address space"))?;
    Address24::try_from(addr).map_err(|error| Diagnostic::new(error.to_string()))
}

fn image_offset(load: Address24, addr: Address24) -> Result<usize, Diagnostic> {
    let offset = addr.get().checked_sub(load.get()).ok_or_else(|| {
        Diagnostic::new(format!(
            "address {addr} is below cartridge load address {}",
            load
        ))
    })?;
    usize::try_from(offset).map_err(|_| Diagnostic::new("image offset exceeds host usize range"))
}

fn header_from_layout(layout: &Layout) -> CartridgeHeader {
    CartridgeHeader {
        entry_addr: layout.entry,
        stack_top: layout.stack,
        ram_base: layout_symbol(layout, "EZRA_RAM_BASE").unwrap_or(EZRA_RAM_BASE),
        vram_base: layout_symbol(layout, "EZRA_VRAM_BASE").unwrap_or(EZRA_VRAM_BASE),
        audio_base: layout_symbol(layout, "EZRA_AUDIO_BASE").unwrap_or(EZRA_AUDIO_BASE),
        asset_base: layout_symbol(layout, "EZRA_ASSET_BASE").unwrap_or(EZRA_ASSET_BASE),
        ..CartridgeHeader::default()
    }
}

fn layout_symbol(layout: &Layout, name: &str) -> Option<Address24> {
    layout
        .symbols
        .iter()
        .find(|symbol| symbol.name == name)
        .map(|symbol| symbol.value)
}

fn serialize_layout_table(layout: &Layout) -> Vec<u8> {
    let mut out = Vec::new();
    push_line(&mut out, format!("layout {}", layout.name));
    push_line(&mut out, format!("load {}", layout.load));
    push_line(&mut out, format!("entry {}", layout.entry));
    push_line(&mut out, format!("stack {}", layout.stack));
    for region in &layout.regions {
        push_line(
            &mut out,
            format!(
                "region {} {}..{} flags {:02X}",
                region.name,
                region.start,
                region.end,
                region.flags.bits()
            ),
        );
    }
    for section in &layout.sections {
        push_line(
            &mut out,
            format!(
                "section {} {} align {}",
                section.name, section.region, section.align
            ),
        );
    }
    for symbol in &layout.symbols {
        push_line(&mut out, format!("symbol {} {}", symbol.name, symbol.value));
    }
    out
}

fn serialize_symbol_table(symbols: &[AssemblySymbol]) -> Vec<u8> {
    let mut out = Vec::new();
    for symbol in symbols {
        push_line(
            &mut out,
            format!(
                "symbol {} 0x{:06X}",
                symbol.name,
                symbol.addr & Address24::MAX
            ),
        );
    }
    out
}

fn push_line(out: &mut Vec<u8>, line: String) {
    out.extend_from_slice(line.as_bytes());
    out.push(b'\n');
}

fn module_alias_original_name(name: &str) -> Option<&str> {
    name.rsplit_once('.').map(|(_, original)| original)
}

fn write_addr24(bytes: &mut [u8], offset: usize, addr: Address24) {
    bytes[offset..offset + 3].copy_from_slice(&addr.to_le_bytes3());
}

fn write_optional_addr24(bytes: &mut [u8], offset: usize, addr: Option<Address24>) {
    write_addr24(bytes, offset, addr.unwrap_or(Address24::new(0)));
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use crate::layout::parse_layout;
    use crate::parser::parse_program;
    use crate::target::EZRA_LOAD_ADDR;

    use super::*;

    #[test]
    fn default_header_matches_spec_offsets() {
        let bytes = CartridgeHeader::default().serialize();

        assert_eq!(&bytes[0x00..0x04], b"EZRA");
        assert_eq!(bytes[0x04], 1);
        assert_eq!(bytes[0x05], 1);
        assert_eq!(&bytes[0x08..0x0B], &[0x40, 0x00, 0x01]);
        assert_eq!(&bytes[0x0B..0x0E], &[0x00, 0x00, 0xF0]);
        assert_eq!(&bytes[0x1A..0x1C], &[0x40, 0x00]);
        assert_eq!(bytes.len(), 64);
    }

    #[test]
    fn cartridge_without_embeds_writes_layout_table() {
        let program = parse_program(Path::new("game.ezra"), "fn main() { test.pass() }").unwrap();
        let image = build_cartridge(&program).unwrap();

        assert_eq!(&image[0x00..0x04], b"EZRA");
        assert_eq!(
            read_addr24(&image, 0x1E),
            EZRA_LOAD_ADDR.get() + u32::from(HEADER_SIZE)
        );
        assert_eq!(read_addr24(&image, 0x21), 0);
        assert!(image[HEADER_SIZE as usize..].starts_with(b"layout ezra_default\n"));
        let layout_text = std::str::from_utf8(&image[HEADER_SIZE as usize..]).unwrap();
        assert!(layout_text.contains("symbol EZRA_LOAD_ADDR"));
    }

    #[test]
    fn cartridge_with_embeds_writes_asset_table_names_and_payloads() {
        let source = r#"
            embed palette: bytes = bytes [0x11, 0x22, 0x33] section .assets align 1
            embed title: bytes = cstr("OK") section .rodata align 4
            fn main() { test.pass() }
        "#;
        let program = parse_program(Path::new("game.ezra"), source).unwrap();
        let image = build_cartridge(&program).unwrap();

        assert_eq!(&image[0x00..0x04], b"EZRA");
        let layout_table = image_offset(read_addr24(&image, 0x1E));
        let table = image_offset(read_addr24(&image, 0x21));
        assert_eq!(layout_table, HEADER_SIZE as usize);
        assert!(image[layout_table..].starts_with(b"layout ezra_default\n"));
        assert!(table > layout_table);

        let first_addr = read_addr24(&image, table);
        assert_eq!(first_addr, 0x100000);
        assert_eq!(&image[table + 3..table + 6], &[0x03, 0x00, 0x00]);
        assert_eq!(&image[table + 6..table + 8], &[0x00, 0x00]);
        assert_eq!(image[table + 8], SECTION_ASSETS);
        assert_eq!(image[table + 9], 0);

        let second = table + ASSET_TABLE_ENTRY_SIZE;
        let second_addr = read_addr24(&image, second);
        assert_eq!(second_addr, 0x020000);
        assert_eq!(&image[second + 3..second + 6], &[0x03, 0x00, 0x00]);
        assert_eq!(&image[second + 6..second + 8], &[0x08, 0x00]);
        assert_eq!(image[second + 8], SECTION_RODATA);
        assert_eq!(image[second + 9], 0);

        let names = second + ASSET_TABLE_ENTRY_SIZE;
        assert_eq!(&image[names..names + 14], b"palette\0title\0");
        assert_eq!(
            &image[image_offset(first_addr)..image_offset(first_addr) + 3],
            &[0x11, 0x22, 0x33]
        );
        assert_eq!(
            &image[image_offset(second_addr)..image_offset(second_addr) + 3],
            &[b'O', b'K', 0]
        );
    }

    #[test]
    fn cartridge_map_reports_final_placements() {
        let source = r#"
            embed palette: bytes = bytes [0x11, 0x22] section .assets align 1
            embed title: bytes = cstr("OK") section .rodata align 4
            fn main() { test.pass() }
        "#;
        let program = parse_program(Path::new("game.ezra"), source).unwrap();
        let symbols = vec![AssemblySymbol {
            name: "__ezra_start".to_owned(),
            addr: 0x010040,
        }];
        let map = build_cartridge_map(&program, &Layout::ezra_default(), 4, &symbols).unwrap();

        assert!(
            map.starts_with("section      start      end        size\n"),
            "{map}"
        );
        assert!(
            map.contains(".header      0x010000 0x01003F 0x000040"),
            "{map}"
        );
        assert!(
            map.contains(".text        0x010040 0x010043 0x000004"),
            "{map}"
        );
        assert!(
            map.contains(".layout_table") && map.contains(".symbol_table"),
            "{map}"
        );
        assert!(
            map.contains(".assets:palette 0x100000 0x100001 0x000002"),
            "{map}"
        );
        assert!(
            map.contains(".rodata:title 0x020000 0x020002 0x000003"),
            "{map}"
        );
    }

    #[test]
    fn cartridge_with_code_places_text_at_entry_and_metadata_after_it() {
        let program = parse_program(Path::new("game.ezra"), "fn main() { test.pass() }").unwrap();
        let code = [0x31, 0x00, 0x00, 0xF0, 0xCD, 0x55, 0x00, 0x01];
        let image = build_cartridge_with_code(&program, &code).unwrap();

        assert_eq!(read_addr24(&image, 0x08), EZRA_ENTRY_ADDR.get());
        assert_eq!(
            &image[HEADER_SIZE as usize..HEADER_SIZE as usize + code.len()],
            &code
        );

        let layout_table = image_offset(read_addr24(&image, 0x1E));
        assert_eq!(layout_table, HEADER_SIZE as usize + code.len());
        assert!(image[layout_table..].starts_with(b"layout ezra_default\n"));
    }

    #[test]
    fn cartridge_with_code_can_start_after_header_padding() {
        let program = parse_program(Path::new("game.ezra"), "fn main() { test.pass() }").unwrap();
        let code = [0x31, 0x00, 0x00, 0xF0];
        let layout = parse_layout(
            r#"
                layout padded_entry {
                    load 0x020000;
                    entry 0x020080;
                    stack 0xF00000;

                    region code 0x020000..0x02FFFF read execute;
                    section .text -> code align 16;
                }
            "#,
        )
        .unwrap();

        let image =
            build_cartridge_with_layout_code_and_symbols(&program, &layout, &code, &[]).unwrap();

        assert_eq!(read_addr24(&image, 0x08), 0x020080);
        assert!(
            image[HEADER_SIZE as usize..0x80]
                .iter()
                .all(|byte| *byte == 0)
        );
        assert_eq!(&image[0x80..0x80 + code.len()], &code);
        let layout_table = usize::try_from(read_addr24(&image, 0x1E) - layout.load.get()).unwrap();
        assert_eq!(layout_table, 0x80 + code.len());
        assert!(image[layout_table..].starts_with(b"layout padded_entry\n"));
    }

    #[test]
    fn cartridge_rejects_entry_inside_header() {
        let program = parse_program(Path::new("game.ezra"), "fn main() { test.pass() }").unwrap();
        let layout = parse_layout(
            r#"
                layout bad_entry {
                    load 0x020000;
                    entry 0x020020;
                    stack 0xF00000;

                    region code 0x020000..0x02FFFF read execute;
                    section .text -> code align 16;
                }
            "#,
        )
        .unwrap();

        let error = build_cartridge_with_layout_code_and_symbols(&program, &layout, &[0x00], &[])
            .unwrap_err();

        assert_eq!(
            error.message,
            "entry 0x020020 overlaps the cartridge header at 0x020000"
        );
    }

    #[test]
    fn cartridge_rejects_text_section_that_exceeds_region() {
        let program = parse_program(Path::new("game.ezra"), "fn main() { test.pass() }").unwrap();
        let layout = parse_layout(
            r#"
                layout tiny_text {
                    load 0x020000;
                    entry 0x020040;
                    stack 0xF00000;

                    region code 0x020000..0x020043 read execute;
                    section .text -> code align 1;
                }
            "#,
        )
        .unwrap();

        let error = build_cartridge_with_layout_code_and_symbols(&program, &layout, &[0; 5], &[])
            .unwrap_err();

        assert_eq!(
            error.message,
            "section `.text` does not fit in region `code`"
        );
    }

    #[test]
    fn cartridge_rejects_text_entry_outside_region() {
        let program = parse_program(Path::new("game.ezra"), "fn main() { test.pass() }").unwrap();
        let layout = parse_layout(
            r#"
                layout misplaced_text {
                    load 0x020000;
                    entry 0x020080;
                    stack 0xF00000;

                    region code 0x020000..0x02007F read execute;
                    section .text -> code align 1;
                }
            "#,
        )
        .unwrap();

        let error =
            build_cartridge_with_layout_code_and_symbols(&program, &layout, &[], &[]).unwrap_err();

        assert_eq!(
            error.message,
            "section `.text` does not fit in region `code`"
        );
    }

    #[test]
    fn cartridge_with_symbols_writes_symbol_table_after_layout() {
        let program = parse_program(Path::new("game.ezra"), "fn main() { test.pass() }").unwrap();
        let code = [0x31, 0x00, 0x00, 0xF0];
        let symbols = [
            AssemblySymbol {
                name: "__ezra_start".to_owned(),
                addr: EZRA_ENTRY_ADDR.get(),
            },
            AssemblySymbol {
                name: "_main".to_owned(),
                addr: EZRA_ENTRY_ADDR.get() + 0x24,
            },
        ];
        let image = build_cartridge_with_code_and_symbols(&program, &code, &symbols).unwrap();

        let layout_table = image_offset(read_addr24(&image, 0x1E));
        let symbol_table = image_offset(read_addr24(&image, 0x24));
        assert!(symbol_table > layout_table);

        let text = std::str::from_utf8(&image[symbol_table..]).unwrap();
        assert!(text.starts_with("symbol __ezra_start 0x010040\n"), "{text}");
        assert!(text.contains("symbol _main 0x010064\n"), "{text}");
    }

    #[test]
    fn cartridge_rejects_embed_that_exceeds_section_region() {
        let source = r#"
            embed too_big: bytes = repeat(0xAA, 3) section .assets align 1
            fn main() { test.pass() }
        "#;
        let layout = parse_layout(
            r#"
                layout tiny_assets {
                    load 0x010000;
                    entry 0x010040;
                    stack 0xF00000;

                    region code 0x010000..0x01FFFF read execute;
                    region assets 0x100000..0x100001 read;

                    section .text -> code align 16;
                    section .assets -> assets align 1;
                }
            "#,
        )
        .unwrap();
        let program = parse_program(Path::new("game.ezra"), source).unwrap();

        let error =
            build_cartridge_with_layout_code_and_symbols(&program, &layout, &[], &[]).unwrap_err();

        assert_eq!(
            error.message,
            "embed `too_big` exceeds section `.assets` region `assets`"
        );
    }

    fn read_addr24(bytes: &[u8], offset: usize) -> u32 {
        u32::from(bytes[offset])
            | (u32::from(bytes[offset + 1]) << 8)
            | (u32::from(bytes[offset + 2]) << 16)
    }

    fn image_offset(addr: u32) -> usize {
        usize::try_from(addr - EZRA_LOAD_ADDR.get()).unwrap()
    }
}
