//! Internal compatibility helpers shared by std and alloc-only compiler modules.

pub(crate) mod prelude {
    pub use alloc::{
        borrow::ToOwned,
        boxed::Box,
        collections::BTreeMap,
        format,
        string::{String, ToString},
        vec,
        vec::Vec,
    };

    #[cfg(feature = "no-std")]
    pub use hashbrown::{HashMap, HashSet};
    #[cfg(feature = "std")]
    pub use std::collections::{HashMap, HashSet};
}

#[cfg(feature = "no-std")]
pub(crate) type SourcePath = str;
#[cfg(feature = "std")]
pub(crate) type SourcePath = std::path::Path;

#[cfg(feature = "no-std")]
pub(crate) type SourcePathBuf = alloc::string::String;
#[cfg(feature = "std")]
pub(crate) type SourcePathBuf = std::path::PathBuf;

#[cfg(feature = "no-std")]
pub(crate) fn source_path_owned(path: &SourcePath) -> SourcePathBuf {
    alloc::borrow::ToOwned::to_owned(path)
}

#[cfg(feature = "std")]
pub(crate) fn source_path_owned(path: &SourcePath) -> SourcePathBuf {
    path.to_path_buf()
}

#[cfg(feature = "no-std")]
pub(crate) fn source_path_text(path: &SourcePath) -> alloc::string::String {
    alloc::borrow::ToOwned::to_owned(path)
}

#[cfg(feature = "std")]
pub(crate) fn source_path_text(path: &SourcePath) -> alloc::string::String {
    alloc::string::ToString::to_string(&path.display())
}
