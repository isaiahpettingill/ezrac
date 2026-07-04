use crate::target::{
    Address24, CART_MAGIC, CPU_MODE_EZ80_ADL, EZRA_ASSET_BASE, EZRA_AUDIO_BASE, EZRA_ENTRY_ADDR,
    EZRA_RAM_BASE, EZRA_STACK_TOP, EZRA_VRAM_BASE, FORMAT_VERSION, HEADER_SIZE,
};

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

fn write_addr24(bytes: &mut [u8], offset: usize, addr: Address24) {
    bytes[offset..offset + 3].copy_from_slice(&addr.to_le_bytes3());
}

fn write_optional_addr24(bytes: &mut [u8], offset: usize, addr: Option<Address24>) {
    write_addr24(bytes, offset, addr.unwrap_or(Address24::new(0)));
}

#[cfg(test)]
mod tests {
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
}
