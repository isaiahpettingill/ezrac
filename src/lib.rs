#![cfg_attr(feature = "no-std", no_std)]

#[cfg(all(feature = "std", feature = "no-std"))]
compile_error!("features `std` and `no-std` are mutually exclusive");
#[cfg(not(any(feature = "std", feature = "no-std")))]
compile_error!("enable either the `std` or `no-std` feature");

extern crate alloc;

#[cfg(feature = "std")]
pub mod api;
#[cfg(feature = "std")]
pub mod asm;
#[cfg(feature = "std")]
pub mod ast;
#[cfg(feature = "std")]
pub mod cart;
#[cfg(feature = "std")]
pub mod compile;
#[cfg(feature = "std")]
pub mod diagnostic;
#[cfg(feature = "std")]
pub mod hir;
#[cfg(feature = "std")]
pub mod layout;
pub mod package;
#[cfg(feature = "std")]
pub mod parser;
#[cfg(feature = "std")]
pub mod project;
pub mod target;
#[cfg(feature = "std")]
pub mod tbir;
#[cfg(feature = "std")]
pub mod vm;
pub mod workspace;
