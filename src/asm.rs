#[cfg(any(feature = "std", feature = "avr"))]
pub mod avr;
#[cfg(feature = "avr")]
pub mod avr_emitter;
mod comments;

#[cfg(feature = "dcpu")]
pub mod dcpu;
#[cfg(feature = "dcpu")]
pub mod dcpu_emitter;
pub mod ez80;
#[cfg(any(feature = "std", feature = "lr35902"))]
pub mod lr35902;
#[cfg(any(feature = "std", feature = "lr35902"))]
pub mod lr35902_emitter;
#[cfg(any(feature = "std", feature = "m6800"))]
pub mod m6800;
#[cfg(feature = "m6800")]
pub mod m6800_emitter;
#[cfg(feature = "m68k")]
pub mod m68k;
#[cfg(feature = "m68k")]
pub mod m68k_emitter;
#[cfg(any(feature = "std", feature = "mos6502"))]
pub mod mos6502;
#[cfg(any(feature = "std", feature = "mos6502"))]
pub mod mos6502_emitter;
#[cfg(feature = "tms9900")]
pub mod tms9900;
#[cfg(feature = "tms9900")]
pub mod tms9900_emitter;

#[cfg(feature = "avr")]
pub use avr_emitter::emit_avr_assembly_with_options;
#[cfg(feature = "dcpu")]
pub use dcpu_emitter::emit_dcpu_assembly_with_options;
pub use ez80::{
    AssemblyOptions, CheckedEz80Program, emit_ez80_assembly, emit_ez80_assembly_from_checked,
    emit_ez80_assembly_with_debug_comments, emit_ez80_assembly_with_options,
};
#[cfg(any(feature = "std", feature = "lr35902"))]
pub use lr35902_emitter::emit_lr35902_assembly_with_options;
#[cfg(feature = "m68k")]
pub use m68k_emitter::emit_m68k_assembly_with_options;
#[cfg(feature = "m6800")]
pub use m6800_emitter::emit_m6800_assembly_with_options;
#[cfg(any(feature = "std", feature = "mos6502"))]
pub use mos6502_emitter::emit_mos6502_assembly_with_options;
#[cfg(feature = "tms9900")]
pub use tms9900_emitter::emit_tms9900_assembly_with_options;
