use alloc::{
    format,
    string::{String, ToString},
};
use core::str;
use ezra::api::build_generated_assembly;

use crate::dos;

pub fn run(args: &[String]) -> Result<(), String> {
    if args.len() != 2 {
        return Err("usage: EZAS input.asm output.com".into());
    }
    let input = &args[0];
    let output = &args[1];
    let bytes = dos::read_file(input)
        .map_err(|error| format!("cannot read `{input}` (DOS error {})", error.0))?;
    let assembly = str::from_utf8(&bytes).map_err(|_| format!("`{input}` is not valid UTF-8"))?;
    let linked = build_generated_assembly(input, assembly, "msdos-com-i8086")
        .map_err(|diagnostic| diagnostic.to_string())?;
    dos::write_file(output, &linked.executable)
        .map_err(|error| format!("cannot write `{output}` (DOS error {})", error.0))
}
