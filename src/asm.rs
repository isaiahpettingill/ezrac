pub mod avr;
#[cfg(feature = "avr")]
pub mod avr_emitter;

#[cfg(feature = "dcpu")]
pub mod dcpu;
pub mod ez80;
pub mod lr35902;
pub mod lr35902_emitter;
pub mod m6800;
#[cfg(feature = "m68k")]
pub mod m68k;
#[cfg(feature = "m68k")]
pub mod m68k_emitter;
pub mod mos6502;
pub mod mos6502_emitter;
#[cfg(feature = "tms9900")]
pub mod tms9900;

#[cfg(feature = "avr")]
pub use avr_emitter::emit_avr_assembly_with_options;
pub use ez80::{
    AssemblyOptions, CheckedEz80Program, emit_ez80_assembly, emit_ez80_assembly_from_checked,
    emit_ez80_assembly_with_debug_comments, emit_ez80_assembly_with_options,
};
pub use lr35902_emitter::emit_lr35902_assembly_with_options;
#[cfg(feature = "m68k")]
pub use m68k_emitter::emit_m68k_assembly_with_options;
pub use mos6502_emitter::emit_mos6502_assembly_with_options;
