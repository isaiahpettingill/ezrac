use std::{
    fs,
    path::{Component, Path, PathBuf},
};

use ezra::{
    compile::EmbedFileResolver,
    diagnostic::Diagnostic,
    image::{ImageKind, decode_indexed_png, encode_for_target},
    project::{AssetConfig, AssetImageConfig, AssetImageKind},
};

pub struct ConfiguredImageResolver<'a> {
    project_root: &'a Path,
    target: &'a str,
    assets: &'a AssetConfig,
}

impl<'a> ConfiguredImageResolver<'a> {
    pub const fn new(project_root: &'a Path, target: &'a str, assets: &'a AssetConfig) -> Self {
        Self {
            project_root,
            target,
            assets,
        }
    }

    fn configured_image(
        &self,
        source_path: &Path,
        requested_path: &str,
    ) -> Option<&AssetImageConfig> {
        let requested = Path::new(requested_path);
        let module_path = source_path
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .join(requested);
        let root_path = self.project_root.join(requested);
        let module_path = normalize_path(&module_path);
        let root_path = normalize_path(&root_path);

        self.assets.images.iter().find(|image| {
            let configured = normalize_path(&image.path);
            configured == module_path
                || configured == root_path
                || normalize_slashes(requested_path) == image.relative_path
        })
    }
}

impl EmbedFileResolver for ConfiguredImageResolver<'_> {
    fn resolve(
        &self,
        source_path: &Path,
        requested_path: &str,
    ) -> Result<Option<Vec<u8>>, Diagnostic> {
        let Some(config) = self.configured_image(source_path, requested_path) else {
            return Ok(None);
        };
        let decoded = decode_indexed_png_file(&config.path).map_err(|error| {
            Diagnostic::new(format!(
                "failed to convert indexed PNG `{}` referenced from `{}`: {error}",
                config.path.display(),
                source_path.display()
            ))
        })?;
        let kind = match config.kind {
            AssetImageKind::Tiles => ImageKind::Tiles,
            AssetImageKind::Sprite => ImageKind::Sprite,
            AssetImageKind::Bitmap => ImageKind::Bitmap,
        };
        encode_for_target(&decoded.as_indexed(), self.target, kind)
            .map(Some)
            .map_err(|error| {
                Diagnostic::new(format!(
                    "failed to convert indexed PNG `{}` for target `{}`: {error}",
                    config.path.display(),
                    self.target
                ))
            })
    }
}

fn decode_indexed_png_file(path: &Path) -> Result<ezra::image::DecodedIndexedImage, String> {
    let bytes =
        fs::read(path).map_err(|error| format!("failed to read `{}`: {error}", path.display()))?;
    decode_indexed_png(&bytes).map_err(|error| error.to_string())
}

fn normalize_slashes(path: &str) -> String {
    path.replace('\\', "/")
        .split('/')
        .filter(|component| !component.is_empty() && *component != ".")
        .collect::<Vec<_>>()
        .join("/")
}

fn normalize_path(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                normalized.pop();
            }
            component => normalized.push(component.as_os_str()),
        }
    }
    normalized
}
