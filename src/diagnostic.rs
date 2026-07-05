use std::path::PathBuf;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SourceLocation {
    pub file: PathBuf,
    pub line: usize,
    pub column: usize,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Diagnostic {
    pub location: Option<SourceLocation>,
    pub message: String,
}

impl Diagnostic {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            location: None,
            message: message.into(),
        }
    }

    pub fn at(location: SourceLocation, message: impl Into<String>) -> Self {
        Self {
            location: Some(location),
            message: message.into(),
        }
    }

    pub fn with_location_if_missing(mut self, location: SourceLocation) -> Self {
        if self.location.is_none() {
            self.location = Some(location);
        }
        self
    }
}

impl std::fmt::Display for Diagnostic {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if let Some(location) = &self.location {
            write!(
                f,
                "{}:{}:{}: {}",
                location.file.display(),
                location.line,
                location.column,
                self.message
            )
        } else {
            f.write_str(&self.message)
        }
    }
}

impl std::error::Error for Diagnostic {}
