use std::path::Path;

use crate::{
    asm::{AssemblyOptions, emit_ez80_assembly, emit_ez80_assembly_with_options},
    compile::load_program,
    parser::parse_program,
    target::{Address24, EZRA_RAM_BASE},
};

use super::*;

fn compile_and_run_source(source: &str, instruction_budget: u64) -> (String, TestRun) {
    let program = parse_program(Path::new("game.ezra"), source).unwrap();
    let asm = emit_ez80_assembly(&program).unwrap();
    let run = run_assembly_test(&asm, instruction_budget).unwrap();
    (asm, run)
}

fn temp_root(name: &str) -> std::path::PathBuf {
    std::env::temp_dir().join(format!(
        "ezra_vm_{name}_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ))
}

mod assembler_parsing_encoding;
mod cpu_behavior;
#[cfg(feature = "dcpu")]
mod dcpu_backend;
mod execution_control_flow;
mod fixtures;
mod gameboy_examples;
mod lr35902_backend;
#[cfg(feature = "m6800")]
mod m6800_backend;
#[cfg(feature = "m68k")]
mod m68k_backend;
#[cfg(feature = "mos6502")]
mod mos6502_backend;
