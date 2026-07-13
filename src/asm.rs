pub mod avr;
pub mod chip8;
pub mod ez80;
pub mod lr35902;
pub mod m6800;
#[cfg(feature = "m68k")]
pub mod m68k;
#[cfg(feature = "m68k")]
pub mod m68k_emitter;
pub mod mos6502;
pub mod mos6502_emitter;

pub use ez80::{
    AssemblyOptions, CheckedEz80Program, emit_ez80_assembly, emit_ez80_assembly_from_checked,
    emit_ez80_assembly_with_debug_comments, emit_ez80_assembly_with_options,
};
pub use lr35902::emit_lr35902_assembly_with_options;
#[cfg(feature = "m68k")]
pub use m68k_emitter::emit_m68k_assembly_with_options;
pub use mos6502_emitter::emit_mos6502_assembly_with_options;
