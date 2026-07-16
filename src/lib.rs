#![cfg_attr(feature = "no-std", no_std)]

#[cfg(all(feature = "std", feature = "no-std"))]
compile_error!("features `std` and `no-std` are mutually exclusive");
#[cfg(not(any(feature = "std", feature = "no-std")))]
compile_error!("enable either the `std` or `no-std` feature");

extern crate alloc;

mod compat;

#[cfg(feature = "std")]
pub mod api;
#[cfg(feature = "no-std")]
#[path = "api_no_std.rs"]
pub mod api;
pub mod asm;
pub mod ast;
#[cfg(feature = "std")]
pub mod cart;
#[cfg(feature = "std")]
pub mod compile;
pub mod diagnostic;
pub mod hir;
pub mod layout;
pub mod package;
pub mod parser;
#[cfg(feature = "std")]
pub mod project;
pub mod target;
pub mod tbir;
pub mod vm;
pub mod workspace;
