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

    #[cfg(all(feature = "no-std", not(feature = "std")))]
    pub use hashbrown::{HashMap, HashSet};
    #[cfg(feature = "std")]
    pub use std::collections::{HashMap, HashSet};
}

#[cfg(all(feature = "no-std", not(feature = "std")))]
pub(crate) type SourcePath = str;
#[cfg(feature = "std")]
pub(crate) type SourcePath = std::path::Path;

#[cfg(all(feature = "no-std", not(feature = "std")))]
pub(crate) type SourcePathBuf = alloc::string::String;
#[cfg(feature = "std")]
pub(crate) type SourcePathBuf = std::path::PathBuf;

#[cfg(all(feature = "no-std", not(feature = "std")))]
pub(crate) fn source_path_owned(path: &SourcePath) -> SourcePathBuf {
    alloc::borrow::ToOwned::to_owned(path)
}

#[cfg(feature = "std")]
pub(crate) fn source_path_owned(path: &SourcePath) -> SourcePathBuf {
    path.to_path_buf()
}

#[cfg(all(feature = "no-std", not(feature = "std")))]
pub(crate) fn source_path_text(path: &SourcePath) -> alloc::string::String {
    alloc::borrow::ToOwned::to_owned(path)
}

#[cfg(feature = "std")]
pub(crate) fn source_path_text(path: &SourcePath) -> alloc::string::String {
    alloc::string::ToString::to_string(&path.display())
}
