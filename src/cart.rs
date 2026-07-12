use std::{
    collections::{HashMap, HashSet},
    fs,
    path::{Path, PathBuf},
};

use crate::{
    ast::{
        AccessPath, AccessSegment, BinaryOp, Declaration, EmbedSource, Expr, Program, Stmt,
        StructDecl, Type, UnaryOp,
    },
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
const SECTION_CUSTOM_BASE: u8 = 0x10;

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
    validate_header_section_fit(layout)?;
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
    let symbol_table = serialize_symbol_table(symbols)?;
    let assets = collect_assets(program)?;
    let asset_table_len = asset_table_len(&assets)?;
    let loaded_text_len = loaded_text_len(
        code.len(),
        layout_table.len(),
        symbol_table.len(),
        asset_table_len,
    )?;
    validate_text_section_fit(layout, loaded_text_len)?;
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
    let section_usage = section_usage_without_assets(program)?;
    let section_starts = layout_section_starts(layout, &section_usage, &assets)?;
    let mut section_cursors = Vec::<(String, u32)>::new();

    for asset in assets {
        let name_offset = u16::try_from(names.len())
            .map_err(|_| Diagnostic::new("asset name table exceeds current 16-bit offset limit"))?;
        names.extend_from_slice(asset.name.as_bytes());
        names.push(0);

        let (section, section_align) = layout_section_placement(layout, &asset.section)?;
        let section_start = section_start(&section_starts, &asset.section)?;
        let cursor = section_cursor(&mut section_cursors, &asset.section, section_start);
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
    validate_header_section_fit(layout)?;
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
    let symbol_table = serialize_symbol_table(symbols)?;
    let assets = collect_assets(program)?;
    let asset_table_len = asset_table_len(&assets)?;
    let code_len_usize = usize::try_from(code_len)
        .map_err(|_| Diagnostic::new("program code exceeds addressable image size"))?;
    let loaded_text_len = loaded_text_len(
        code_len_usize,
        layout_table.len(),
        symbol_table.len(),
        asset_table_len,
    )?;
    validate_text_section_fit(layout, loaded_text_len)?;
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

    let section_usage = section_usage_without_assets(program)?;
    let section_starts = layout_section_starts(layout, &section_usage, &assets)?;
    append_layout_section_map_entries(
        &mut entries,
        layout,
        &section_usage,
        &assets,
        &section_starts,
    )?;

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
        let section_start = section_start(&section_starts, &asset.section)?;
        let cursor = section_cursor(&mut section_cursors, &asset.section, section_start);
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

fn append_layout_section_map_entries(
    entries: &mut Vec<CartridgeMapEntry>,
    layout: &Layout,
    section_usage: &HashMap<String, u32>,
    assets: &[AssetEntry],
    section_starts: &HashMap<String, u32>,
) -> Result<(), Diagnostic> {
    for section in &layout.sections {
        if matches!(section.name.as_str(), ".header" | ".text") {
            continue;
        }
        let start = section_start(section_starts, &section.name)?;
        let asset_size = asset_section_usage_at(layout, assets, &section.name, start)?;
        let size = asset_size
            .checked_add(section_usage.get(&section.name).copied().unwrap_or(0))
            .ok_or_else(|| {
                Diagnostic::new(format!(
                    "section `{}` exceeds 24-bit address space",
                    section.name
                ))
            })?;
        entries.push(map_entry(&section.name, start, size)?);
    }
    Ok(())
}

pub fn layout_section_bases(
    program: &Program,
    layout: &Layout,
) -> Result<Vec<(String, Address24)>, Diagnostic> {
    let assets = collect_assets(program)?;
    let section_usage = section_usage_without_assets(program)?;
    let starts = layout_section_starts(layout, &section_usage, &assets)?;
    layout
        .sections
        .iter()
        .filter_map(|section| {
            starts
                .get(&section.name)
                .copied()
                .map(|start| (section.name.clone(), start))
        })
        .map(|(name, start)| {
            Ok((
                name,
                Address24::try_from(start).map_err(|error| Diagnostic::new(error.to_string()))?,
            ))
        })
        .collect()
}

fn section_usage_without_assets(program: &Program) -> Result<HashMap<String, u32>, Diagnostic> {
    let mut usage = section_usage_from_globals(program)?;
    add_section_usage(
        &mut usage,
        ".rodata",
        section_usage_from_string_literals(program)?,
    )?;
    Ok(usage)
}

fn layout_section_starts(
    layout: &Layout,
    section_usage: &HashMap<String, u32>,
    assets: &[AssetEntry],
) -> Result<HashMap<String, u32>, Diagnostic> {
    let mut region_cursors = Vec::<(String, u32)>::new();
    let mut starts = HashMap::new();
    for section in &layout.sections {
        if matches!(section.name.as_str(), ".header" | ".text") {
            continue;
        }
        let (region, section_align) = layout_section_placement(layout, &section.name)?;
        let cursor = section_cursor(&mut region_cursors, &region.name, region.start.get());
        *cursor = align_addr(*cursor, section_align)?;
        let start = *cursor;
        starts.insert(section.name.clone(), start);
        let asset_size = asset_section_usage_at(layout, assets, &section.name, start)?;
        let size = asset_size
            .checked_add(section_usage.get(&section.name).copied().unwrap_or(0))
            .ok_or_else(|| {
                Diagnostic::new(format!(
                    "section `{}` exceeds 24-bit address space",
                    section.name
                ))
            })?;
        validate_section_range(region, &section.name, start, size)?;
        *cursor = cursor.checked_add(size).ok_or_else(|| {
            Diagnostic::new(format!(
                "section `{}` exceeds 24-bit address space",
                section.name
            ))
        })?;
    }
    Ok(starts)
}

fn asset_section_usage_at(
    layout: &Layout,
    assets: &[AssetEntry],
    section_name: &str,
    section_start: u32,
) -> Result<u32, Diagnostic> {
    let (section, section_align) = layout_section_placement(layout, section_name)?;
    let mut cursor = section_start;
    for asset in assets.iter().filter(|asset| asset.section == section_name) {
        cursor = align_addr(cursor, asset.align.max(section_align))?;
        let asset_addr =
            Address24::try_from(cursor).map_err(|error| Diagnostic::new(error.to_string()))?;
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
        cursor = cursor
            .checked_add(asset_len)
            .ok_or_else(|| Diagnostic::new("asset payload exceeds 24-bit address space"))?;
    }
    cursor
        .checked_sub(section_start)
        .ok_or_else(|| Diagnostic::new("asset payload starts before section"))
}

fn validate_section_range(
    region: &Region,
    section_name: &str,
    start: u32,
    size: u32,
) -> Result<(), Diagnostic> {
    if size == 0 {
        return Ok(());
    }
    let start_addr =
        Address24::try_from(start).map_err(|error| Diagnostic::new(error.to_string()))?;
    let end = start.checked_add(size - 1).ok_or_else(|| {
        Diagnostic::new(format!(
            "section `{section_name}` exceeds 24-bit address space"
        ))
    })?;
    let end_addr = Address24::try_from(end).map_err(|error| Diagnostic::new(error.to_string()))?;
    if !region.contains_range(start_addr, end_addr) {
        return Err(Diagnostic::new(format!(
            "section `{section_name}` does not fit in region `{}`",
            region.name
        )));
    }
    Ok(())
}

fn section_start(starts: &HashMap<String, u32>, section: &str) -> Result<u32, Diagnostic> {
    starts
        .get(section)
        .copied()
        .ok_or_else(|| Diagnostic::new(format!("layout has no section `{section}`")))
}

fn section_usage_from_globals(program: &Program) -> Result<HashMap<String, u32>, Diagnostic> {
    let aliases = collect_embed_aliases(program);
    let constants = collect_embed_constants(program)?;
    let structs = collect_structs(program);
    let mut usage = HashMap::<String, u32>::new();

    for declaration in &program.declarations {
        let Declaration::Global(decl) = declaration else {
            continue;
        };
        if module_alias_original_name(&decl.name).is_some() {
            continue;
        }
        let size = cart_type_size(&decl.ty, &aliases, &structs, &constants)?;
        let section = if global_initializer_is_zero(&decl.value, &constants, &aliases) {
            ".bss"
        } else {
            ".data"
        };
        add_section_usage(&mut usage, section, size)?;
    }

    Ok(usage)
}

fn add_section_usage(
    usage: &mut HashMap<String, u32>,
    section: &str,
    size: u32,
) -> Result<(), Diagnostic> {
    let total = usage
        .get(section)
        .copied()
        .unwrap_or(0)
        .checked_add(size)
        .ok_or_else(|| {
            Diagnostic::new(format!("section `{section}` exceeds 24-bit address space"))
        })?;
    usage.insert(section.to_owned(), total);
    Ok(())
}

fn section_usage_from_string_literals(program: &Program) -> Result<u32, Diagnostic> {
    let mut literals = HashSet::<String>::new();
    for declaration in &program.declarations {
        collect_declaration_string_literals(declaration, &mut literals);
    }

    literals.into_iter().try_fold(0u32, |size, value| {
        let len = value
            .len()
            .checked_add(1)
            .ok_or_else(|| Diagnostic::new("string literal is too large"))?;
        let len = u32::try_from(len).map_err(|_| Diagnostic::new("string literal is too large"))?;
        size.checked_add(len)
            .ok_or_else(|| Diagnostic::new("section `.rodata` exceeds 24-bit address space"))
    })
}

fn collect_declaration_string_literals(declaration: &Declaration, literals: &mut HashSet<String>) {
    match declaration {
        Declaration::Cfg { declaration, .. } => {
            collect_declaration_string_literals(declaration, literals)
        }
        Declaration::Const(decl) => collect_expr_string_literals(&decl.value, literals),
        Declaration::Port(decl) => collect_expr_string_literals(&decl.value, literals),
        Declaration::Mmio(decl) => collect_expr_string_literals(&decl.value, literals),
        Declaration::Global(decl) => collect_expr_string_literals(&decl.value, literals),
        Declaration::Embed(_) => {}
        Declaration::Function(function) => collect_stmt_string_literals(&function.body, literals),
        Declaration::Import(_)
        | Declaration::Alias(_)
        | Declaration::Struct(_)
        | Declaration::ExternAsmFunction(_) => {}
    }
}

fn collect_stmt_string_literals(stmts: &[Stmt], literals: &mut HashSet<String>) {
    for stmt in stmts {
        match stmt {
            Stmt::Let { value, .. } | Stmt::Out { value, .. } | Stmt::Expr(value) => {
                collect_expr_string_literals(value, literals)
            }
            Stmt::Assign { target, value, .. } => {
                collect_place_string_literals(target, literals);
                collect_expr_string_literals(value, literals);
            }
            Stmt::If {
                condition,
                then_body,
                else_body,
            } => {
                collect_expr_string_literals(condition, literals);
                collect_stmt_string_literals(then_body, literals);
                collect_stmt_string_literals(else_body, literals);
            }
            Stmt::While { condition, body } => {
                collect_expr_string_literals(condition, literals);
                collect_stmt_string_literals(body, literals);
            }
            Stmt::Loop { body } => collect_stmt_string_literals(body, literals),
            Stmt::Return(Some(value)) => collect_expr_string_literals(value, literals),
            Stmt::Return(None) | Stmt::Break | Stmt::Continue | Stmt::Asm { .. } => {}
        }
    }
}

fn collect_place_string_literals(place: &crate::ast::Place, literals: &mut HashSet<String>) {
    match place {
        crate::ast::Place::Index { index, .. } | crate::ast::Place::Deref(index) => {
            collect_expr_string_literals(index, literals)
        }
        crate::ast::Place::Access(path) => collect_access_path_string_literals(path, literals),
        crate::ast::Place::Ident(_) | crate::ast::Place::Field { .. } => {}
    }
}

fn collect_expr_string_literals(expr: &Expr, literals: &mut HashSet<String>) {
    match expr {
        Expr::String(value) => {
            literals.insert(value.clone());
        }
        Expr::Array(values) => {
            for value in values {
                collect_expr_string_literals(value, literals);
            }
        }
        Expr::Index { index, .. } | Expr::AddressOfIndex { index, .. } => {
            collect_expr_string_literals(index, literals)
        }
        Expr::Access(path) | Expr::AddressOfAccess(path) => {
            collect_access_path_string_literals(path, literals)
        }
        Expr::StructInit { fields, .. } => {
            for (_, value) in fields {
                collect_expr_string_literals(value, literals);
            }
        }
        Expr::Deref(value) | Expr::Unary { expr: value, .. } | Expr::Cast { expr: value, .. } => {
            collect_expr_string_literals(value, literals)
        }
        Expr::Call { args, .. } => {
            for arg in args {
                collect_expr_string_literals(arg, literals);
            }
        }
        Expr::Binary { left, right, .. } => {
            collect_expr_string_literals(left, literals);
            collect_expr_string_literals(right, literals);
        }
        Expr::Int(_)
        | Expr::TypedInt(_, _)
        | Expr::Bool(_)
        | Expr::Char(_)
        | Expr::Ident(_)
        | Expr::In(_)
        | Expr::Field { .. }
        | Expr::AddressOfField { .. }
        | Expr::AddressOf(_) => {}
    }
}

fn collect_access_path_string_literals(path: &AccessPath, literals: &mut HashSet<String>) {
    for segment in &path.segments {
        if let AccessSegment::Index(index) = segment {
            collect_expr_string_literals(index, literals);
        }
    }
}

fn collect_structs(program: &Program) -> HashMap<String, &StructDecl> {
    let mut structs = HashMap::new();
    for declaration in &program.declarations {
        if let Declaration::Struct(decl) = declaration {
            structs.insert(decl.name.clone(), decl);
        }
    }
    structs
}

fn cart_type_size(
    ty: &Type,
    aliases: &HashMap<String, Type>,
    structs: &HashMap<String, &StructDecl>,
    constants: &HashMap<String, i64>,
) -> Result<u32, Diagnostic> {
    match resolve_embed_const_type(ty, aliases)? {
        Type::Array { element, len } => {
            let element_size = cart_type_size(&element, aliases, structs, constants)?;
            let len = eval_embed_expr_with_aliases(&len, constants, aliases)?;
            if !(0..=0xFF_FFFF).contains(&len) {
                return Err(Diagnostic::new(format!(
                    "array length {len} is outside u24 range"
                )));
            }
            let size = element_size
                .checked_mul(len as u32)
                .ok_or_else(|| Diagnostic::new("array size exceeds 24-bit address space"))?;
            if size > 0xFF_FFFF {
                return Err(Diagnostic::new(format!(
                    "array size {size} exceeds 24-bit address space"
                )));
            }
            Ok(size)
        }
        Type::Named(name) if structs.contains_key(&name) => {
            let decl = structs[&name];
            let mut size = 0u32;
            for field in &decl.fields {
                size = size
                    .checked_add(cart_type_size(&field.ty, aliases, structs, constants)?)
                    .ok_or_else(|| {
                        Diagnostic::new(format!(
                            "struct `{name}` size exceeds 24-bit address space"
                        ))
                    })?;
            }
            if size > 0xFF_FFFF {
                return Err(Diagnostic::new(format!(
                    "struct `{name}` size {size} exceeds 24-bit address space"
                )));
            }
            Ok(size)
        }
        Type::Named(name) => match name.as_str() {
            "bool" | "u8" | "i8" => Ok(1),
            "u16" | "i16" => Ok(2),
            "u24" | "i24" | "ptr24" => Ok(3),
            _ => Err(Diagnostic::new(format!("unknown storage type `{name}`"))),
        },
        Type::Ptr(_) => Ok(3),
    }
}

fn global_initializer_is_zero(
    expr: &Expr,
    constants: &HashMap<String, i64>,
    aliases: &HashMap<String, Type>,
) -> bool {
    match expr {
        Expr::Array(values) => values
            .iter()
            .all(|value| global_initializer_is_zero(value, constants, aliases)),
        Expr::StructInit { fields, .. } => fields
            .iter()
            .all(|(_, value)| global_initializer_is_zero(value, constants, aliases)),
        _ => eval_embed_expr_with_aliases(expr, constants, aliases).is_ok_and(|value| value == 0),
    }
}

fn collect_assets(program: &Program) -> Result<Vec<AssetEntry>, Diagnostic> {
    let mut assets = Vec::new();
    let mut custom_section_ids = HashMap::<String, u8>::new();
    let mut next_custom_section_id = SECTION_CUSTOM_BASE;
    let aliases = collect_embed_aliases(program);
    let constants = collect_embed_constants(program)?;
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
            .map(|expr| {
                validate_embed_alignment_expr(&decl.name, expr, program, &aliases)?;
                eval_embed_expr_with_aliases(expr, &constants, &aliases)
            })
            .transpose()?
            .unwrap_or(1);
        if align <= 0 || (align & (align - 1)) != 0 {
            return Err(Diagnostic::new(format!(
                "embed `{}` alignment {align} is not a positive power of two",
                decl.name
            )));
        }
        let align = u32::try_from(align).map_err(|_| {
            Diagnostic::new(format!(
                "embed `{}` alignment {align} exceeds 24-bit address space",
                decl.name
            ))
        })?;
        let section = decl.section.clone().unwrap_or_else(|| ".assets".to_owned());
        let section_id = section_id(
            &section,
            &mut custom_section_ids,
            &mut next_custom_section_id,
        )?;
        let bytes = embed_bytes(&decl.source, &program.source_path, &constants, &aliases)?;
        assets.push(AssetEntry {
            name: decl.name.clone(),
            bytes,
            align,
            section,
            section_id,
            flags: 0,
        });
    }
    Ok(assets)
}

fn validate_embed_alignment_expr(
    name: &str,
    expr: &Expr,
    program: &Program,
    aliases: &HashMap<String, Type>,
) -> Result<(), Diagnostic> {
    match expr {
        Expr::Bool(_) | Expr::String(_) => err_embed_alignment_not_integer(name),
        Expr::Unary {
            op: UnaryOp::Not, ..
        } => err_embed_alignment_not_integer(name),
        Expr::Unary { expr, .. } => validate_embed_alignment_expr(name, expr, program, aliases),
        Expr::Binary { op, .. } if is_bool_result_op(*op) => err_embed_alignment_not_integer(name),
        Expr::Binary { left, right, .. } => {
            validate_embed_alignment_expr(name, left, program, aliases)?;
            validate_embed_alignment_expr(name, right, program, aliases)
        }
        Expr::Cast { ty, .. } => {
            let ty = resolve_embed_const_type(ty, aliases)?;
            if type_is_non_integer_alignment(&ty) {
                err_embed_alignment_not_integer(name)
            } else {
                Ok(())
            }
        }
        Expr::Ident(const_name) => {
            if let Some((_, Some(ty))) = find_embed_constant_declaration(program, const_name) {
                let ty = resolve_embed_const_type(ty, aliases)?;
                if type_is_non_integer_alignment(&ty) {
                    return err_embed_alignment_not_integer(name);
                }
            }
            Ok(())
        }
        _ => Ok(()),
    }
}

fn err_embed_alignment_not_integer(name: &str) -> Result<(), Diagnostic> {
    Err(Diagnostic::new(format!(
        "embed `{name}` alignment must be an integer constant"
    )))
}

fn type_is_non_integer_alignment(ty: &Type) -> bool {
    matches!(ty, Type::Ptr(_) | Type::Array { .. })
        || matches!(ty, Type::Named(name) if name == "bool")
}

fn is_bool_result_op(op: BinaryOp) -> bool {
    matches!(
        op,
        BinaryOp::Lt
            | BinaryOp::Le
            | BinaryOp::Gt
            | BinaryOp::Ge
            | BinaryOp::Eq
            | BinaryOp::Ne
            | BinaryOp::And
            | BinaryOp::Or
    )
}

fn collect_embed_aliases(program: &Program) -> HashMap<String, Type> {
    let mut aliases = HashMap::new();
    for declaration in &program.declarations {
        if let Declaration::Alias(decl) = declaration {
            aliases.insert(decl.name.clone(), decl.ty.clone());
        }
    }
    aliases
}

fn section_id(
    section: &str,
    custom_section_ids: &mut HashMap<String, u8>,
    next_custom_section_id: &mut u8,
) -> Result<u8, Diagnostic> {
    match section {
        ".assets" => Ok(SECTION_ASSETS),
        ".rodata" => Ok(SECTION_RODATA),
        section => {
            if let Some(id) = custom_section_ids.get(section) {
                return Ok(*id);
            }
            let id = *next_custom_section_id;
            *next_custom_section_id = next_custom_section_id.checked_add(1).ok_or_else(|| {
                Diagnostic::new("too many custom embed sections for u8 section ids")
            })?;
            custom_section_ids.insert(section.to_owned(), id);
            Ok(id)
        }
    }
}

fn read_embed_file(path: &str, source_path: &Path) -> Result<Vec<u8>, Diagnostic> {
    let path = Path::new(path);
    if path.is_absolute() {
        return read_embed_file_candidate(path);
    }

    let candidates = embed_file_candidates(path, source_path);
    let missing_path = candidates
        .first()
        .cloned()
        .unwrap_or_else(|| path.to_path_buf());
    for candidate in candidates {
        match fs::read(&candidate) {
            Ok(bytes) => return Ok(bytes),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(Diagnostic::new(format!(
                    "failed to read embedded file `{}`: {error}",
                    candidate.display()
                )));
            }
        }
    }
    Err(Diagnostic::new(format!(
        "embedded file `{}` not found",
        missing_path.display()
    )))
}

fn read_embed_file_candidate(path: &Path) -> Result<Vec<u8>, Diagnostic> {
    fs::read(path).map_err(|error| {
        if error.kind() == std::io::ErrorKind::NotFound {
            Diagnostic::new(format!("embedded file `{}` not found", path.display()))
        } else {
            Diagnostic::new(format!(
                "failed to read embedded file `{}`: {error}",
                path.display()
            ))
        }
    })
}

fn embed_file_candidates(path: &Path, source_path: &Path) -> Vec<PathBuf> {
    let mut candidates = vec![
        source_path
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .join(path),
    ];
    if let Ok(project_root) = std::env::current_dir() {
        let project_relative = project_root.join(path);
        if !candidates
            .iter()
            .any(|candidate| candidate == &project_relative)
        {
            candidates.push(project_relative);
        }
    }
    candidates
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

fn validate_header_section_fit(layout: &Layout) -> Result<(), Diagnostic> {
    let (region, _) = layout_section_placement(layout, ".header")?;
    let end = layout
        .load
        .get()
        .checked_add(u32::from(HEADER_SIZE) - 1)
        .ok_or_else(|| Diagnostic::new("section `.header` exceeds 24-bit address space"))?;
    let end = Address24::try_from(end).map_err(|error| Diagnostic::new(error.to_string()))?;
    if !region.contains_range(layout.load, end) {
        return Err(Diagnostic::new(format!(
            "section `.header` does not fit in region `{}`",
            region.name
        )));
    }
    Ok(())
}

fn loaded_text_len(
    code_len: usize,
    layout_table_len: usize,
    symbol_table_len: usize,
    asset_table_len: usize,
) -> Result<usize, Diagnostic> {
    code_len
        .checked_add(layout_table_len)
        .and_then(|len| len.checked_add(symbol_table_len))
        .and_then(|len| len.checked_add(asset_table_len))
        .ok_or_else(|| Diagnostic::new("loaded text metadata exceeds addressable image size"))
}

fn asset_table_len(assets: &[AssetEntry]) -> Result<usize, Diagnostic> {
    let entries_len = assets
        .len()
        .checked_mul(ASSET_TABLE_ENTRY_SIZE)
        .ok_or_else(|| Diagnostic::new("asset table exceeds addressable image size"))?;
    let names_len = assets.iter().try_fold(0usize, |len, asset| {
        len.checked_add(asset.name.len())
            .and_then(|len| len.checked_add(1))
            .ok_or_else(|| Diagnostic::new("asset name table exceeds addressable image size"))
    })?;
    entries_len
        .checked_add(names_len)
        .ok_or_else(|| Diagnostic::new("asset table exceeds addressable image size"))
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

fn collect_embed_constants(program: &Program) -> Result<HashMap<String, i64>, Diagnostic> {
    let aliases = collect_embed_aliases(program);

    let mut constants = HashMap::new();
    let mut evaluating = HashSet::new();
    for declaration in &program.declarations {
        match declaration {
            Declaration::Const(decl) => {
                evaluate_embed_constant(
                    &decl.name,
                    &decl.value,
                    Some(&decl.ty),
                    program,
                    &aliases,
                    &mut constants,
                    &mut evaluating,
                )?;
            }
            Declaration::Mmio(decl) => {
                evaluate_embed_constant(
                    &decl.name,
                    &decl.value,
                    None,
                    program,
                    &aliases,
                    &mut constants,
                    &mut evaluating,
                )?;
            }
            _ => {}
        }
    }
    Ok(constants)
}

fn evaluate_embed_constant(
    name: &str,
    value_expr: &Expr,
    ty: Option<&Type>,
    program: &Program,
    aliases: &HashMap<String, Type>,
    constants: &mut HashMap<String, i64>,
    evaluating: &mut HashSet<String>,
) -> Result<(), Diagnostic> {
    if constants.contains_key(name) {
        return Ok(());
    }
    if !evaluating.insert(name.to_owned()) {
        return Err(Diagnostic::new(format!(
            "circular constant reference involving `{name}`"
        )));
    }

    let result = (|| {
        ensure_embed_constant_dependencies_evaluated(
            value_expr, program, aliases, constants, evaluating,
        )?;
        let value = eval_embed_expr_with_aliases(value_expr, constants, aliases)?;
        let value = if let Some(ty) = ty {
            wrap_embed_const_value(value, ty, aliases)?
        } else {
            value
        };
        constants.insert(name.to_owned(), value);
        Ok(())
    })();

    evaluating.remove(name);
    result
}

fn ensure_embed_constant_dependencies_evaluated(
    expr: &Expr,
    program: &Program,
    aliases: &HashMap<String, Type>,
    constants: &mut HashMap<String, i64>,
    evaluating: &mut HashSet<String>,
) -> Result<(), Diagnostic> {
    let mut names = Vec::new();
    collect_embed_constant_dependency_names(expr, &mut names);
    for name in names {
        if constants.contains_key(&name) {
            continue;
        }
        let Some((value_expr, ty)) = find_embed_constant_declaration(program, &name) else {
            continue;
        };
        evaluate_embed_constant(
            &name, value_expr, ty, program, aliases, constants, evaluating,
        )?;
    }
    Ok(())
}

fn find_embed_constant_declaration<'a>(
    program: &'a Program,
    name: &str,
) -> Option<(&'a Expr, Option<&'a Type>)> {
    program
        .declarations
        .iter()
        .find_map(|declaration| match declaration {
            Declaration::Const(decl) if decl.name == name => Some((&decl.value, Some(&decl.ty))),
            Declaration::Mmio(decl) if decl.name == name => Some((&decl.value, None)),
            _ => None,
        })
}

fn wrap_embed_const_value(
    value: i64,
    ty: &Type,
    aliases: &HashMap<String, Type>,
) -> Result<i64, Diagnostic> {
    match resolve_embed_const_type(ty, aliases)? {
        Type::Named(name) if name == "bool" => Ok(i64::from(value != 0)),
        Type::Named(name) => {
            let (bits, signed) = match name.as_str() {
                "u8" => (8, false),
                "i8" => (8, true),
                "u16" => (16, false),
                "i16" => (16, true),
                "u24" | "ptr24" => (24, false),
                "i24" => (24, true),
                _ => return Err(Diagnostic::new(format!("unknown const type `{name}`"))),
            };
            let mask = (1_i128 << bits) - 1;
            let unsigned = (value as i128) & mask;
            if signed {
                let sign_bit = 1_i128 << (bits - 1);
                if unsigned & sign_bit != 0 {
                    Ok((unsigned - (1_i128 << bits)) as i64)
                } else {
                    Ok(unsigned as i64)
                }
            } else {
                Ok(unsigned as i64)
            }
        }
        Type::Ptr(_) => {
            let mask = (1_i128 << 24) - 1;
            Ok(((value as i128) & mask) as i64)
        }
        Type::Array { .. } => Err(Diagnostic::new("array const type is not supported")),
    }
}

fn resolve_embed_const_type(
    ty: &Type,
    aliases: &HashMap<String, Type>,
) -> Result<Type, Diagnostic> {
    match ty {
        Type::Named(name) => aliases
            .get(name)
            .map(|alias| resolve_embed_const_type(alias, aliases))
            .unwrap_or_else(|| Ok(ty.clone())),
        Type::Ptr(inner) => Ok(Type::Ptr(Box::new(resolve_embed_const_type(
            inner, aliases,
        )?))),
        Type::Array { element, len } => Ok(Type::Array {
            element: Box::new(resolve_embed_const_type(element, aliases)?),
            len: len.clone(),
        }),
    }
}

fn embed_bytes(
    source: &EmbedSource,
    source_path: &Path,
    constants: &HashMap<String, i64>,
    aliases: &HashMap<String, Type>,
) -> Result<Vec<u8>, Diagnostic> {
    match source {
        EmbedSource::File(path) => read_embed_file(path, source_path),
        EmbedSource::Bytes(values) => values
            .iter()
            .map(|value| {
                let byte = eval_embed_expr_with_aliases(value, constants, aliases)?;
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
            let byte = eval_embed_expr_with_aliases(value, constants, aliases)?;
            if !(0..=0xFF).contains(&byte) {
                return Err(Diagnostic::new(format!(
                    "embedded repeat byte value {byte} is outside u8 range"
                )));
            }
            let len = eval_embed_expr_with_aliases(len, constants, aliases)?;
            if !(0..=0xFF_FFFF).contains(&len) {
                return Err(Diagnostic::new(format!(
                    "embedded repeat length {len} is outside u24 range"
                )));
            }
            Ok(vec![byte as u8; len as usize])
        }
    }
}

fn eval_embed_expr_with_aliases(
    expr: &Expr,
    constants: &HashMap<String, i64>,
    aliases: &HashMap<String, Type>,
) -> Result<i64, Diagnostic> {
    match expr {
        Expr::Int(value) | Expr::TypedInt(value, _) => Ok(*value),
        Expr::Char(value) => Ok(i64::from(*value)),
        Expr::Bool(value) => Ok(i64::from(*value)),
        Expr::Ident(name) => constants
            .get(name)
            .copied()
            .ok_or_else(|| Diagnostic::new(format!("unknown embed constant `{name}`"))),
        Expr::Field { base, field } => {
            let name = format!("{base}.{field}");
            constants
                .get(&name)
                .copied()
                .ok_or_else(|| Diagnostic::new(format!("unknown embed constant `{name}`")))
        }
        Expr::Access(path) => {
            let name = const_access_name(path)?;
            constants
                .get(&name)
                .copied()
                .ok_or_else(|| Diagnostic::new(format!("unknown embed constant `{name}`")))
        }
        Expr::Unary { op, expr } => {
            let value = eval_embed_expr_with_aliases(expr, constants, aliases)?;
            match op {
                UnaryOp::Neg => Ok(value.wrapping_neg()),
                UnaryOp::BitNot => Ok(!value),
                UnaryOp::Not => Ok(i64::from(value == 0)),
            }
        }
        Expr::Binary { left, op, right } => {
            let left_signed = embed_expr_is_signed(left, aliases);
            let left = eval_embed_expr_with_aliases(left, constants, aliases)?;
            let right = eval_embed_expr_with_aliases(right, constants, aliases)?;
            match op {
                BinaryOp::Add => Ok(left.wrapping_add(right)),
                BinaryOp::Sub => Ok(left.wrapping_sub(right)),
                BinaryOp::Mul => Ok(left.wrapping_mul(right)),
                BinaryOp::Div => Ok(trunc_div_or_zero_i64(left, right)),
                BinaryOp::Mod => Ok(trunc_mod_or_zero_i64(left, right)),
                BinaryOp::BitAnd => Ok(left & right),
                BinaryOp::BitOr => Ok(left | right),
                BinaryOp::BitXor => Ok(left ^ right),
                BinaryOp::Shl => Ok(if (0..64).contains(&right) {
                    left.wrapping_shl(right as u32)
                } else {
                    0
                }),
                BinaryOp::Shr => Ok(const_shr_or_zero_i64(left, right, left_signed)),
                BinaryOp::Lt => Ok(i64::from(left < right)),
                BinaryOp::Le => Ok(i64::from(left <= right)),
                BinaryOp::Gt => Ok(i64::from(left > right)),
                BinaryOp::Ge => Ok(i64::from(left >= right)),
                BinaryOp::Eq => Ok(i64::from(left == right)),
                BinaryOp::Ne => Ok(i64::from(left != right)),
                BinaryOp::And => Ok(i64::from(left != 0 && right != 0)),
                BinaryOp::Or => Ok(i64::from(left != 0 || right != 0)),
            }
        }
        Expr::Cast { expr, ty } => {
            let value = eval_embed_expr_with_aliases(expr, constants, aliases)?;
            wrap_embed_const_value(value, ty, aliases)
        }
        _ => Err(Diagnostic::new(
            "embed expressions must be integer constants",
        )),
    }
}

fn collect_embed_constant_dependency_names(expr: &Expr, names: &mut Vec<String>) {
    match expr {
        Expr::Ident(name) => names.push(name.clone()),
        Expr::Field { base, field } => names.push(format!("{base}.{field}")),
        Expr::Access(path) => {
            if let Ok(name) = const_access_name(path) {
                names.push(name);
            }
            for segment in &path.segments {
                if let AccessSegment::Index(index) = segment {
                    collect_embed_constant_dependency_names(index, names);
                }
            }
        }
        Expr::Cast { expr, .. } | Expr::Unary { expr, .. } | Expr::Deref(expr) => {
            collect_embed_constant_dependency_names(expr, names)
        }
        Expr::Binary { left, right, .. } => {
            collect_embed_constant_dependency_names(left, names);
            collect_embed_constant_dependency_names(right, names);
        }
        Expr::Array(values) => {
            for value in values {
                collect_embed_constant_dependency_names(value, names);
            }
        }
        Expr::Index { index, .. } => collect_embed_constant_dependency_names(index, names),
        Expr::StructInit { fields, .. } => {
            for (_, value) in fields {
                collect_embed_constant_dependency_names(value, names);
            }
        }
        Expr::Call { args, .. } => {
            for arg in args {
                collect_embed_constant_dependency_names(arg, names);
            }
        }
        Expr::AddressOfIndex { index, .. } => collect_embed_constant_dependency_names(index, names),
        Expr::AddressOfAccess(path) => {
            for segment in &path.segments {
                if let AccessSegment::Index(index) = segment {
                    collect_embed_constant_dependency_names(index, names);
                }
            }
        }
        Expr::Int(_)
        | Expr::TypedInt(_, _)
        | Expr::Bool(_)
        | Expr::Char(_)
        | Expr::String(_)
        | Expr::AddressOf(_)
        | Expr::AddressOfField { .. }
        | Expr::In(_) => {}
    }
}

fn const_access_name(path: &AccessPath) -> Result<String, Diagnostic> {
    let mut out = path.root.clone();
    for segment in &path.segments {
        match segment {
            AccessSegment::Field(field) => {
                out.push('.');
                out.push_str(field);
            }
            AccessSegment::Index(_) => {
                return Err(Diagnostic::new(
                    "embed expressions must be integer constants",
                ));
            }
        }
    }
    Ok(out)
}

fn embed_expr_is_signed(expr: &Expr, aliases: &HashMap<String, Type>) -> bool {
    match expr {
        Expr::TypedInt(_, ty) | Expr::Cast { ty, .. } => {
            resolve_embed_const_type(ty, aliases).is_ok_and(|ty| {
                matches!(ty, Type::Named(name) if matches!(name.as_str(), "i8" | "i16" | "i24"))
            })
        }
        Expr::Unary {
            op: UnaryOp::Neg,
            expr,
        } => embed_expr_is_signed(expr, aliases),
        _ => false,
    }
}

fn trunc_div_or_zero_i64(left: i64, right: i64) -> i64 {
    if right == 0 {
        0
    } else {
        ((left as i128) / (right as i128)) as i64
    }
}

fn trunc_mod_or_zero_i64(left: i64, right: i64) -> i64 {
    if right == 0 {
        0
    } else {
        ((left as i128) % (right as i128)) as i64
    }
}

fn const_shr_or_zero_i64(left: i64, right: i64, signed: bool) -> i64 {
    if right < 0 {
        return 0;
    }
    if signed {
        if right >= 64 {
            if left < 0 { -1 } else { 0 }
        } else {
            left >> right as u32
        }
    } else if right >= 64 {
        0
    } else {
        left.wrapping_shr(right as u32)
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

fn serialize_symbol_table(symbols: &[AssemblySymbol]) -> Result<Vec<u8>, Diagnostic> {
    let mut out = Vec::new();
    let mut names = HashSet::new();
    for symbol in symbols {
        if !names.insert(symbol.name.as_str()) {
            return Err(Diagnostic::new(format!(
                "duplicate assembly symbol `{}`",
                symbol.name
            )));
        }
        if symbol.addr > Address24::MAX {
            return Err(Diagnostic::new(format!(
                "assembly symbol `{}` address 0x{:X} is outside the 24-bit address space",
                symbol.name, symbol.addr
            )));
        }
        push_line(
            &mut out,
            format!("symbol {} 0x{:06X}", symbol.name, symbol.addr),
        );
    }
    Ok(out)
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
mod tests;
