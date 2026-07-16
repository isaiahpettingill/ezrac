//! Filesystem-free executable packaging for library consumers.

use alloc::{format, string::String, vec::Vec};

use crate::{
    diagnostic::Diagnostic,
    target::{Address24, OutputFormat},
};

/// Parameters shared by target executable packagers.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PackageRequest {
    pub target: String,
    pub output_format: OutputFormat,
    pub load_addr: u32,
    pub entry_addr: u32,
    pub executable_name: Option<String>,
}

impl PackageRequest {
    pub fn new(
        target: impl Into<String>,
        output_format: OutputFormat,
        load_addr: u32,
        entry_addr: u32,
    ) -> Self {
        Self {
            target: target.into(),
            output_format,
            load_addr,
            entry_addr,
            executable_name: None,
        }
    }
}

/// Package assembled code without reading or writing host files.
pub fn package_executable(request: &PackageRequest, code: &[u8]) -> Result<Vec<u8>, Diagnostic> {
    if request.target.starts_with("agonlight-mos-ez80") {
        return agon_mos_bytes(request.entry_addr, code);
    }
    match request.output_format {
        OutputFormat::RawBin | OutputFormat::CpmCom | OutputFormat::Ez180nGaem => Ok(code.to_vec()),
        OutputFormat::IntelHex | OutputFormat::ArduinoHex => {
            Ok(intel_hex_bytes(request.load_addr, code))
        }
        OutputFormat::Commodore64Prg => commodore64_prg_bytes(request, code),
        OutputFormat::Commodore64Crt => commodore64_crt_bytes(request, code),
        _ => Err(Diagnostic::new(format!(
            "in-memory packaging for `{}` output is not implemented yet",
            request.output_format.extension()
        ))),
    }
}

fn commodore64_prg_bytes(request: &PackageRequest, code: &[u8]) -> Result<Vec<u8>, Diagnostic> {
    if !request.target.starts_with("commodore64-6502") {
        return Err(Diagnostic::new(format!(
            "target `{}` does not support Commodore 64 .prg output",
            request.target
        )));
    }
    if request.load_addr != 0x080D || request.entry_addr != 0x080D {
        return Err(Diagnostic::new(
            "Commodore 64 PRG layouts must load and enter at 0x080D",
        ));
    }
    const BASIC_AUTOSTART: [u8; 12] = [
        0x0B, 0x08, 0x0A, 0x00, 0x9E, b'2', b'0', b'6', b'1', 0x00, 0x00, 0x00,
    ];
    let mut output = Vec::with_capacity(code.len() + 14);
    output.extend_from_slice(&0x0801u16.to_le_bytes());
    output.extend_from_slice(&BASIC_AUTOSTART);
    output.extend_from_slice(code);
    Ok(output)
}

fn commodore64_crt_bytes(request: &PackageRequest, code: &[u8]) -> Result<Vec<u8>, Diagnostic> {
    if !request.target.starts_with("commodore64-6502") {
        return Err(Diagnostic::new(format!(
            "target `{}` does not support Commodore 64 .crt output",
            request.target
        )));
    }
    if request.load_addr != 0x8009 || request.entry_addr != 0x8009 {
        return Err(Diagnostic::new(
            "standard Commodore 64 CRT layouts must load and enter at 0x8009",
        ));
    }
    const ROM_SIZE: usize = 0x2000;
    const HEADER_SIZE: usize = 0x40;
    const CHIP_HEADER_SIZE: usize = 0x10;
    if code.len() > ROM_SIZE - 9 {
        return Err(Diagnostic::new(
            "program code exceeds the standard 8 KiB Commodore 64 CRT capacity",
        ));
    }
    let mut output = Vec::with_capacity(HEADER_SIZE + CHIP_HEADER_SIZE + ROM_SIZE);
    output.extend_from_slice(b"C64 CARTRIDGE   ");
    output.extend_from_slice(&0x40u32.to_be_bytes());
    output.extend_from_slice(&0x0100u16.to_be_bytes());
    output.extend_from_slice(&0u16.to_be_bytes());
    output.push(0);
    output.push(1);
    output.extend_from_slice(&[0; 6]);
    let mut name = [0u8; 32];
    name[..10].copy_from_slice(b"EZRA C64  ");
    output.extend_from_slice(&name);
    output.extend_from_slice(b"CHIP");
    output.extend_from_slice(&((CHIP_HEADER_SIZE + ROM_SIZE) as u32).to_be_bytes());
    output.extend_from_slice(&0u16.to_be_bytes());
    output.extend_from_slice(&0u16.to_be_bytes());
    output.extend_from_slice(&0x8000u16.to_be_bytes());
    output.extend_from_slice(&(ROM_SIZE as u16).to_be_bytes());
    output.extend_from_slice(&0x8009u16.to_le_bytes());
    output.extend_from_slice(&0x8009u16.to_le_bytes());
    output.extend_from_slice(b"CBM80");
    output.extend_from_slice(code);
    output.resize(HEADER_SIZE + CHIP_HEADER_SIZE + ROM_SIZE, 0xFF);
    Ok(output)
}

fn agon_mos_bytes(entry: u32, code: &[u8]) -> Result<Vec<u8>, Diagnostic> {
    if entry > Address24::MAX {
        return Err(Diagnostic::new(format!(
            "Agon MOS entry address 0x{entry:X} is outside the 24-bit address space"
        )));
    }
    let mut out = Vec::with_capacity(69 + code.len());
    out.extend_from_slice(&[0xC3, entry as u8, (entry >> 8) as u8, (entry >> 16) as u8]);
    out.resize(64, 0);
    out.extend_from_slice(b"MOS\0\x01");
    out.extend_from_slice(code);
    Ok(out)
}

fn intel_hex_bytes(base_addr: u32, code: &[u8]) -> Vec<u8> {
    let mut out = String::new();
    let mut current_upper = None;
    for (offset, chunk) in code.chunks(16).enumerate() {
        let addr = base_addr + (offset * 16) as u32;
        let upper = (addr >> 16) as u16;
        if current_upper != Some(upper) {
            current_upper = Some(upper);
            push_ihex_record(&mut out, 0, 4, &upper.to_be_bytes());
        }
        push_ihex_record(&mut out, addr as u16, 0, chunk);
    }
    push_ihex_record(&mut out, 0, 1, &[]);
    out.into_bytes()
}
fn push_ihex_record(out: &mut String, address: u16, kind: u8, data: &[u8]) {
    let mut sum = (data.len() as u8)
        .wrapping_add((address >> 8) as u8)
        .wrapping_add(address as u8)
        .wrapping_add(kind);
    out.push_str(&format!(":{:02X}{address:04X}{kind:02X}", data.len()));
    for byte in data {
        sum = sum.wrapping_add(*byte);
        out.push_str(&format!("{byte:02X}"));
    }
    out.push_str(&format!("{:02X}\n", (!sum).wrapping_add(1)));
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn packages_agon_mos_in_memory() {
        let image = package_executable(
            &PackageRequest::new("agonlight-mos-ez80", OutputFormat::RawBin, 0x40000, 0x40045),
            &[0],
        )
        .unwrap();
        assert_eq!(&image[64..69], b"MOS\0\x01");
    }
    #[test]
    fn packages_c64_prg_in_memory() {
        let image = package_executable(
            &PackageRequest::new(
                "commodore64-6502",
                OutputFormat::Commodore64Prg,
                0x80d,
                0x80d,
            ),
            &[0xea],
        )
        .unwrap();
        assert_eq!(&image[..2], &[1, 8]);
    }
}
