pub mod ez80;
pub mod lr35902;

pub use ez80::{
    AssemblyOptions, CheckedEz80Program, emit_ez80_assembly, emit_ez80_assembly_from_checked,
    emit_ez80_assembly_with_debug_comments, emit_ez80_assembly_with_options,
};
pub use lr35902::emit_lr35902_assembly_with_options;

pub mod m68k;
