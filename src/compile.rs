use std::path::PathBuf;

use crate::{diagnostic::Diagnostic, parser::parse_program};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CompileOptions {
    pub source: PathBuf,
    pub debug_comments: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CompileReport {
    pub imports: usize,
    pub declarations: usize,
    pub has_main: bool,
}

pub fn check_source(source: &str, options: &CompileOptions) -> Result<CompileReport, Diagnostic> {
    let program = parse_program(&options.source, source)?;
    let imports = program
        .declarations
        .iter()
        .filter(|decl| matches!(decl, crate::ast::Declaration::Import(_)))
        .count();
    let declarations = program.declarations.len() - imports;
    let has_main = program.main_function().is_some();

    if !has_main {
        return Err(Diagnostic::new("missing required `fn main()`"));
    }

    Ok(CompileReport {
        imports,
        declarations,
        has_main,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_minimal_main() {
        let options = CompileOptions {
            source: PathBuf::from("game.ezra"),
            debug_comments: false,
        };

        let report = check_source("fn main() {\n}\n", &options).unwrap();

        assert!(report.has_main);
        assert_eq!(report.declarations, 1);
    }

    #[test]
    fn reports_missing_main() {
        let options = CompileOptions {
            source: PathBuf::from("game.ezra"),
            debug_comments: false,
        };

        let error = check_source("const X: u8 = 1\n", &options).unwrap_err();

        assert_eq!(error.message, "missing required `fn main()`");
    }
}
