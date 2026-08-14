use alloc::{string::String, vec, vec::Vec};
use core::{arch::asm, fmt, str};

const STDOUT: u16 = 1;
const STDERR: u16 = 2;
const READ_ONLY: u16 = 0;
const CREATE_ATTRIBUTES: u16 = 0;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Error(pub u16);

pub struct Writer {
    handle: u16,
}

impl Writer {
    pub const fn stdout() -> Self {
        Self { handle: STDOUT }
    }

    pub const fn stderr() -> Self {
        Self { handle: STDERR }
    }
}

impl fmt::Write for Writer {
    fn write_str(&mut self, text: &str) -> fmt::Result {
        write_all(self.handle, text.as_bytes()).map_err(|_| fmt::Error)
    }
}

pub fn command_line() -> Result<Vec<String>, &'static str> {
    let psp = psp_selector();
    let length = read_psp_byte(psp, 0x80) as usize;
    let mut raw = vec![0u8; length];
    for (index, byte) in raw.iter_mut().enumerate() {
        *byte = read_psp_byte(psp, 0x81 + index as u32);
    }
    let text = str::from_utf8(&raw).map_err(|_| "DOS command line is not valid UTF-8")?;
    split_command_line(text)
}

fn split_command_line(text: &str) -> Result<Vec<String>, &'static str> {
    let mut args = Vec::new();
    let mut current = String::new();
    let mut quoted = false;
    for character in text.trim().chars() {
        match character {
            '"' => quoted = !quoted,
            ' ' | '\t' if !quoted => {
                if !current.is_empty() {
                    args.push(core::mem::take(&mut current));
                }
            }
            _ => current.push(character),
        }
    }
    if quoted {
        return Err("unterminated quote in DOS command line");
    }
    if !current.is_empty() {
        args.push(current);
    }
    Ok(args)
}

pub fn read_file(path: &str) -> Result<Vec<u8>, Error> {
    let path = c_string(path)?;
    let handle = open(&path, READ_ONLY)?;
    let result = (|| {
        let size = file_size(handle)?;
        let mut bytes = vec![0u8; size];
        let mut offset = 0;
        while offset < bytes.len() {
            let count = read(handle, &mut bytes[offset..])?;
            if count == 0 {
                bytes.truncate(offset);
                break;
            }
            offset += count;
        }
        Ok(bytes)
    })();
    let close_result = close(handle);
    match (result, close_result) {
        (Ok(bytes), Ok(())) => Ok(bytes),
        (Err(error), _) | (Ok(_), Err(error)) => Err(error),
    }
}

pub fn write_file(path: &str, bytes: &[u8]) -> Result<(), Error> {
    let path = c_string(path)?;
    let handle = create(&path, CREATE_ATTRIBUTES)?;
    let result = write_all(handle, bytes);
    let close_result = close(handle);
    result.and(close_result)
}

pub fn exit(code: u8) -> ! {
    unsafe {
        asm!("int 0x21", in("ax") 0x4c00u16 | code as u16, options(noreturn));
    }
}

fn c_string(text: &str) -> Result<Vec<u8>, Error> {
    if text.as_bytes().contains(&0) {
        return Err(Error(3));
    }
    let mut bytes = Vec::with_capacity(text.len() + 1);
    bytes.extend_from_slice(text.as_bytes());
    bytes.push(0);
    Ok(bytes)
}

fn psp_selector() -> u16 {
    let selector: u16;
    unsafe {
        asm!("int 0x21", in("ax") 0x6200u16, lateout("bx") selector, options(nostack));
    }
    selector
}

fn read_psp_byte(selector: u16, offset: u32) -> u8 {
    let byte: u8;
    unsafe {
        asm!(
            "push es",
            "mov es, {selector:x}",
            "mov {byte}, byte ptr es:[{offset:e}]",
            "pop es",
            selector = in(reg) selector,
            offset = in(reg) offset,
            byte = lateout(reg_byte) byte,
        );
    }
    byte
}

unsafe extern "C" {
    fn dos_open(path: *const u8, mode: u32) -> u32;
}

fn open(path: &[u8], mode: u16) -> Result<u16, Error> {
    let result = unsafe { dos_open(path.as_ptr(), mode as u32) };
    if result & 0x8000_0000 == 0 {
        Ok(result as u16)
    } else {
        Err(Error(result as u16))
    }
}

fn create(path: &[u8], attributes: u16) -> Result<u16, Error> {
    let mut value: u16;
    let mut failed: u8;
    unsafe {
        asm!(
            "int 0x21",
            "setc {failed}",
            inlateout("ax") 0x3c00u16 => value,
            in("cx") attributes,
            in("edx") path.as_ptr(),
            failed = lateout(reg_byte) failed,
            options(nostack),
        );
    }
    status(value, failed)
}

fn close(handle: u16) -> Result<(), Error> {
    let mut value: u16;
    let mut failed: u8;
    unsafe {
        asm!(
            "int 0x21",
            "setc {failed}",
            inlateout("ax") 0x3e00u16 => value,
            in("bx") handle,
            failed = lateout(reg_byte) failed,
            options(nostack),
        );
    }
    status(value, failed).map(|_| ())
}

fn read(handle: u16, bytes: &mut [u8]) -> Result<usize, Error> {
    let count = bytes.len().min(u16::MAX as usize) as u16;
    let mut value: u16;
    let mut failed: u8;
    unsafe {
        asm!(
            "int 0x21",
            "setc {failed}",
            inlateout("ax") 0x3f00u16 => value,
            in("bx") handle,
            in("cx") count,
            in("edx") bytes.as_mut_ptr(),
            failed = lateout(reg_byte) failed,
            options(nostack),
        );
    }
    status(value, failed).map(usize::from)
}

fn write_all(handle: u16, mut bytes: &[u8]) -> Result<(), Error> {
    while !bytes.is_empty() {
        let count = bytes.len().min(u16::MAX as usize) as u16;
        let mut value: u16;
        let mut failed: u8;
        unsafe {
            asm!(
                "int 0x21",
                "setc {failed}",
                inlateout("ax") 0x4000u16 => value,
                in("bx") handle,
                in("cx") count,
                in("edx") bytes.as_ptr(),
                failed = lateout(reg_byte) failed,
                options(nostack),
            );
        }
        let written = status(value, failed).map(usize::from)?;
        if written == 0 {
            return Err(Error(5));
        }
        bytes = &bytes[written..];
    }
    Ok(())
}

fn file_size(handle: u16) -> Result<usize, Error> {
    let mut low: u16;
    let mut high: u16;
    let mut failed: u8;
    unsafe {
        asm!(
            "int 0x21",
            "setc {failed}",
            inlateout("ax") 0x4202u16 => low,
            in("bx") handle,
            in("cx") 0u16,
            inlateout("dx") 0u16 => high,
            failed = lateout(reg_byte) failed,
            options(nostack),
        );
    }
    if failed != 0 {
        return Err(Error(low));
    }
    let size = ((high as usize) << 16) | low as usize;
    seek_start(handle)?;
    Ok(size)
}

fn seek_start(handle: u16) -> Result<(), Error> {
    let mut value: u16;
    let mut failed: u8;
    unsafe {
        asm!(
            "int 0x21",
            "setc {failed}",
            inlateout("ax") 0x4200u16 => value,
            in("bx") handle,
            in("cx") 0u16,
            in("dx") 0u16,
            failed = lateout(reg_byte) failed,
            options(nostack),
        );
    }
    status(value, failed).map(|_| ())
}

fn status(value: u16, failed: u8) -> Result<u16, Error> {
    if failed == 0 {
        Ok(value)
    } else {
        Err(Error(value))
    }
}
