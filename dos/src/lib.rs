#![no_std]
#![feature(alloc_error_handler)]

extern crate alloc;

mod dos;
mod heap;

use alloc::{
    format,
    string::{String, ToString},
};
use core::{alloc::Layout, fmt::Write, panic::PanicInfo, str};
use dos::Writer;
use ezra::api::{CompileRequest, Workspace, WorkspaceFile, build_workspace};

#[global_allocator]
static ALLOCATOR: heap::DpmiAllocator = heap::DpmiAllocator::new();

#[unsafe(no_mangle)]
pub extern "C" fn start() -> ! {
    let code = match run() {
        Ok(()) => 0,
        Err(message) => {
            let _ = writeln!(Writer::stderr(), "error: {message}\r");
            1
        }
    };
    dos::exit(code)
}

fn run() -> Result<(), String> {
    let args = dos::command_line().map_err(String::from)?;
    if args.is_empty() || matches!(args[0].as_str(), "-h" | "--help" | "/?") {
        print_usage();
        return Ok(());
    }
    if args.len() > 2 {
        return Err("expected EZRAC source.ezra [output.com]".into());
    }

    let input = &args[0];
    let output = args
        .get(1)
        .cloned()
        .unwrap_or_else(|| replace_extension(input, "com"));
    let source_bytes = dos::read_file(input)
        .map_err(|error| format!("cannot read `{input}` (DOS error {})", error.0))?;
    let source =
        str::from_utf8(&source_bytes).map_err(|_| format!("`{input}` is not valid UTF-8"))?;
    let files = [WorkspaceFile::text(input, source)];
    let request = CompileRequest::new(input, "msdos-com-i8086");
    let build = build_workspace(&Workspace::new(&files), input, &request)
        .map_err(|diagnostic| diagnostic.to_string())?;
    dos::write_file(&output, &build.executable)
        .map_err(|error| format!("cannot write `{output}` (DOS error {})", error.0))?;
    let _ = writeln!(
        Writer::stdout(),
        "wrote {output} ({} bytes)\r",
        build.executable.len()
    );
    Ok(())
}

fn replace_extension(path: &str, extension: &str) -> String {
    let separator = path.rfind(['/', '\\']);
    let dot = path
        .rfind('.')
        .filter(|dot| separator.is_none_or(|slash| *dot > slash));
    match dot {
        Some(dot) => format!("{}.{}", &path[..dot], extension),
        None => format!("{path}.{extension}"),
    }
}

fn print_usage() {
    let _ = writeln!(Writer::stdout(), "EZRAC for FreeDOS\r");
    let _ = writeln!(Writer::stdout(), "usage: EZRAC source.ezra [output.com]\r");
    let _ = writeln!(
        Writer::stdout(),
        "Compiles one EZRA source file for msdos-com-i8086.\r"
    );
}

#[panic_handler]
fn panic(info: &PanicInfo<'_>) -> ! {
    let _ = writeln!(Writer::stderr(), "fatal compiler error: {info}\r");
    dos::exit(2)
}

#[alloc_error_handler]
fn allocation_error(layout: Layout) -> ! {
    let _ = writeln!(
        Writer::stderr(),
        "fatal compiler error: out of memory allocating {} bytes\r",
        layout.size()
    );
    dos::exit(2)
}
