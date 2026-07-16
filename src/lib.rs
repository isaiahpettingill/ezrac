#![cfg_attr(all(feature = "no-std", not(feature = "std")), no_std)]

// `std` intentionally takes precedence when both features are selected so
// conventional `--all-features` checks continue to exercise every backend.
#[cfg(not(any(feature = "std", feature = "no-std")))]
compile_error!("enable either the `std` or `no-std` feature");

extern crate alloc;

mod compat;

#[cfg(feature = "std")]
pub mod api;
#[cfg(all(feature = "no-std", not(feature = "std")))]
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
