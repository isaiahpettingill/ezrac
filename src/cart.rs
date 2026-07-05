use std::{fs, path::Path};

use crate::{
    ast::{BinaryOp, Declaration, EmbedSource, Expr, Program, UnaryOp},
    diagnostic::Diagnostic,
    layout::Layout,
    target::{
        Address24, CART_MAGIC, CPU_MODE_EZ80_ADL, EZRA_ASSET_BASE, EZRA_AUDIO_BASE,
        EZRA_ENTRY_ADDR, EZRA_RAM_BASE, EZRA_STACK_TOP, EZRA_VRAM_BASE, FORMAT_VERSION,
        HEADER_SIZE,
    },
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
    section_id: u8,
    flags: u8,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct PackedAssetEntry {
    asset_addr: Address24,
    asset_len: u32,
    name_offset: u16,
    section_id: u8,
    flags: u8,
}

pub fn build_cartridge(program: &Program) -> Result<Vec<u8>, Diagnostic> {
    let layout_table = serialize_layout_table(&Layout::ezra_default());
    let layout_table_addr = Address24::new(u32::from(HEADER_SIZE));
    let assets = collect_assets(program)?;
    let asset_table_addr = if assets.is_empty() {
        None
    } else {
        Some(checked_image_addr(
            u32::from(HEADER_SIZE)
                + u32::try_from(layout_table.len())
                    .map_err(|_| Diagnostic::new("layout table exceeds 24-bit address space"))?,
        )?)
    };
    let mut header = CartridgeHeader {
        layout_table_addr: Some(layout_table_addr),
        asset_table_addr,
        ..CartridgeHeader::default()
    };
    let mut table = Vec::with_capacity(assets.len() * ASSET_TABLE_ENTRY_SIZE);
    let mut names = Vec::new();
    let mut payload = Vec::new();
    let mut packed = Vec::new();

    for asset in assets {
        let name_offset = u16::try_from(names.len())
            .map_err(|_| Diagnostic::new("asset name table exceeds current 16-bit offset limit"))?;
        names.extend_from_slice(asset.name.as_bytes());
        names.push(0);

        align_payload(&mut payload, asset.align);
        let payload_offset = u32::try_from(payload.len())
            .map_err(|_| Diagnostic::new("asset payload exceeds 24-bit address space"))?;
        let asset_addr = checked_asset_addr(payload_offset)?;
        let asset_len = u32::try_from(asset.bytes.len())
            .map_err(|_| Diagnostic::new("asset length exceeds 24-bit address space"))?;
        if asset_len > Address24::MAX {
            return Err(Diagnostic::new(format!(
                "asset `{}` length {asset_len} is outside u24 range",
                asset.name
            )));
        }
        payload.extend_from_slice(&asset.bytes);
        packed.push(PackedAssetEntry {
            asset_addr,
            asset_len,
            name_offset,
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
    image.extend_from_slice(&layout_table);
    image.append(&mut table);
    image.append(&mut names);
    image.append(&mut payload);
    header.header_size = HEADER_SIZE;
    image[..HEADER_SIZE as usize].copy_from_slice(&header.serialize());
    Ok(image)
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
        let section_id = section_id(decl.section.as_deref())?;
        let bytes = embed_bytes(&decl.source, &program.source_path)?;
        assets.push(AssetEntry {
            name: decl.name.clone(),
            bytes,
            align: align as u32,
            section_id,
            flags: 0,
        });
    }
    Ok(assets)
}

fn section_id(section: Option<&str>) -> Result<u8, Diagnostic> {
    match section.unwrap_or(".assets") {
        ".assets" => Ok(SECTION_ASSETS),
        ".rodata" => Ok(SECTION_RODATA),
        section => Err(Diagnostic::new(format!(
            "embed section `{section}` is not supported by the current cartridge packer"
        ))),
    }
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

fn align_payload(payload: &mut Vec<u8>, align: u32) {
    if align <= 1 {
        return;
    }
    while payload.len() as u32 % align != 0 {
        payload.push(0);
    }
}

fn checked_asset_addr(offset: u32) -> Result<Address24, Diagnostic> {
    let addr = EZRA_ASSET_BASE
        .get()
        .checked_add(offset)
        .ok_or_else(|| Diagnostic::new("asset payload exceeds 24-bit address space"))?;
    Address24::try_from(addr).map_err(|error| Diagnostic::new(error.to_string()))
}

fn checked_image_addr(offset: u32) -> Result<Address24, Diagnostic> {
    Address24::try_from(offset).map_err(|error| Diagnostic::new(error.to_string()))
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

    use crate::parser::parse_program;

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
        assert_eq!(read_addr24(&image, 0x1E), u32::from(HEADER_SIZE));
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
        let layout_table = read_addr24(&image, 0x1E) as usize;
        let table = read_addr24(&image, 0x21) as usize;
        assert_eq!(layout_table, HEADER_SIZE as usize);
        assert!(image[layout_table..].starts_with(b"layout ezra_default\n"));
        assert!(table > layout_table);

        assert_eq!(&image[table..table + 3], &[0x00, 0x00, 0x10]);
        assert_eq!(&image[table + 3..table + 6], &[0x03, 0x00, 0x00]);
        assert_eq!(&image[table + 6..table + 8], &[0x00, 0x00]);
        assert_eq!(image[table + 8], SECTION_ASSETS);
        assert_eq!(image[table + 9], 0);

        let second = table + ASSET_TABLE_ENTRY_SIZE;
        assert_eq!(&image[second..second + 3], &[0x04, 0x00, 0x10]);
        assert_eq!(&image[second + 3..second + 6], &[0x03, 0x00, 0x00]);
        assert_eq!(&image[second + 6..second + 8], &[0x08, 0x00]);
        assert_eq!(image[second + 8], SECTION_RODATA);
        assert_eq!(image[second + 9], 0);

        let names = second + ASSET_TABLE_ENTRY_SIZE;
        assert_eq!(&image[names..names + 14], b"palette\0title\0");
        assert_eq!(
            &image[names + 14..],
            &[0x11, 0x22, 0x33, 0x00, b'O', b'K', 0]
        );
    }

    fn read_addr24(bytes: &[u8], offset: usize) -> u32 {
        u32::from(bytes[offset])
            | (u32::from(bytes[offset + 1]) << 8)
            | (u32::from(bytes[offset + 2]) << 16)
    }
}
