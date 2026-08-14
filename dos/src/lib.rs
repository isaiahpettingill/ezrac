#![no_std]
#![feature(alloc_error_handler)]

extern crate alloc;

mod dos;
#[cfg(feature = "ezas")]
mod ezas;
#[cfg(feature = "ezcg")]
mod ezcg;
#[cfg(feature = "ezfe")]
mod ezfe;
#[cfg(feature = "ezopt")]
mod ezopt;
mod heap;

use alloc::string::String;
use core::{alloc::Layout, fmt::Write, panic::PanicInfo};
use dos::Writer;

#[cfg(not(any(
    feature = "ezfe",
    feature = "ezopt",
    feature = "ezcg",
    feature = "ezas"
)))]
compile_error!("enable one DOS stage feature");
#[cfg(any(
    all(feature = "ezfe", feature = "ezopt"),
    all(feature = "ezfe", feature = "ezcg"),
    all(feature = "ezfe", feature = "ezas"),
    all(feature = "ezopt", feature = "ezcg"),
    all(feature = "ezopt", feature = "ezas"),
    all(feature = "ezcg", feature = "ezas"),
))]
compile_error!("enable only one DOS stage feature");

#[global_allocator]
static ALLOCATOR: heap::DpmiAllocator = heap::DpmiAllocator::new();

#[unsafe(no_mangle)]
pub extern "C" fn rust_start() -> ! {
    let result = dos::command_line()
        .map_err(String::from)
        .and_then(|args| run_stage(&args));
    match result {
        Ok(()) => dos::exit(0),
        Err(message) => {
            let _ = writeln!(Writer::stderr(), "error: {message}\r");
            dos::exit(1)
        }
    }
}

fn run_stage(args: &[String]) -> Result<(), String> {
    #[cfg(feature = "ezfe")]
    return ezfe::run(args);
    #[cfg(feature = "ezopt")]
    return ezopt::run(args);
    #[cfg(feature = "ezcg")]
    return ezcg::run(args);
    #[cfg(feature = "ezas")]
    return ezas::run(args);
    #[allow(unreachable_code)]
    Err("no DOS compiler stage selected".into())
}

#[panic_handler]
fn panic(_info: &PanicInfo<'_>) -> ! {
    let _ = writeln!(Writer::stderr(), "fatal compiler error\r");
    dos::exit(2)
}

#[alloc_error_handler]
fn allocation_error(_layout: Layout) -> ! {
    let _ = writeln!(Writer::stderr(), "fatal compiler error: out of memory\r");
    dos::exit(2)
}
