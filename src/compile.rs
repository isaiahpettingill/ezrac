use std::{
    collections::HashSet,
    fs,
    path::{Path, PathBuf},
};

use crate::{
    ast::{Declaration, Program},
    diagnostic::Diagnostic,
    parser::parse_program,
};

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
    let root = parse_program(&options.source, source)?;
    let imports = root
        .declarations
        .iter()
        .filter(|decl| matches!(decl, crate::ast::Declaration::Import(_)))
        .count();
    let program = resolve_program_imports(root, &mut Vec::new(), &mut HashSet::new())?;
    let declarations = program.declarations.len();
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

pub fn load_program(path: &Path) -> Result<Program, Diagnostic> {
    let source = fs::read_to_string(path).map_err(|error| {
        Diagnostic::new(format!("failed to read `{}`: {error}", path.display()))
    })?;
    parse_and_resolve_imports(path, &source)
}

pub fn parse_and_resolve_imports(path: &Path, source: &str) -> Result<Program, Diagnostic> {
    let root = parse_program(path, source)?;
    let mut stack = Vec::new();
    let mut seen = HashSet::new();
    resolve_program_imports(root, &mut stack, &mut seen)
}

fn resolve_program_imports(
    program: Program,
    stack: &mut Vec<PathBuf>,
    seen: &mut HashSet<PathBuf>,
) -> Result<Program, Diagnostic> {
    let path = normalize_path(&program.source_path);
    if stack.contains(&path) {
        let mut cycle = stack
            .iter()
            .map(|path| path.display().to_string())
            .collect::<Vec<_>>();
        cycle.push(path.display().to_string());
        return Err(Diagnostic::new(format!(
            "cyclic import detected: {}",
            cycle.join(" -> ")
        )));
    }
    if !seen.insert(path.clone()) {
        return Ok(Program {
            source_path: program.source_path,
            declarations: Vec::new(),
        });
    }

    stack.push(path);
    let mut declarations = Vec::new();
    for declaration in &program.declarations {
        let Declaration::Import(import) = declaration else {
            continue;
        };
        let import_path = resolve_import_path(&program.source_path, import);
        let source = fs::read_to_string(&import_path).map_err(|error| {
            Diagnostic::new(format!(
                "failed to read import `{import}` at `{}`: {error}",
                import_path.display()
            ))
        })?;
        let imported = parse_program(&import_path, &source)?;
        let imported = resolve_program_imports(imported, stack, seen)?;
        declarations.extend(imported.declarations);
    }
    stack.pop();

    declarations.extend(
        program
            .declarations
            .into_iter()
            .filter(|decl| !matches!(decl, Declaration::Import(_))),
    );

    Ok(Program {
        source_path: program.source_path,
        declarations,
    })
}

fn resolve_import_path(source_path: &Path, import: &str) -> PathBuf {
    let base = source_path.parent().unwrap_or_else(|| Path::new("."));
    base.join(import.replace('.', "/")).with_extension("ezra")
}

fn normalize_path(path: &Path) -> PathBuf {
    path.canonicalize().unwrap_or_else(|_| path.to_path_buf())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_root(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "ezra_compile_{name}_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ))
    }

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

    #[test]
    fn resolves_imported_declarations() {
        let root = temp_root("imports");
        std::fs::create_dir_all(root.join("lib")).unwrap();
        let main_path = root.join("game.ezra");
        let lib_path = root.join("lib/math.ezra");
        std::fs::write(&lib_path, "fn add_one(v: u8) -> u8 { return v + 1 }\n").unwrap();
        let source = "import lib.math\nfn main() { let x: u8 = add_one(4); test.pass() }\n";
        std::fs::write(&main_path, source).unwrap();

        let options = CompileOptions {
            source: main_path.clone(),
            debug_comments: false,
        };
        let report = check_source(source, &options).unwrap();
        let program = load_program(&main_path).unwrap();

        assert_eq!(report.imports, 1);
        assert_eq!(report.declarations, 2);
        assert!(program.declarations.iter().any(|decl| {
            matches!(decl, Declaration::Function(function) if function.name == "add_one")
        }));

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn rejects_cyclic_imports() {
        let root = temp_root("cycle");
        std::fs::create_dir_all(&root).unwrap();
        let a_path = root.join("a.ezra");
        let b_path = root.join("b.ezra");
        std::fs::write(&a_path, "import b\nfn main() {}\n").unwrap();
        std::fs::write(&b_path, "import a\n").unwrap();

        let error = load_program(&a_path).unwrap_err();

        assert!(error.message.starts_with("cyclic import detected:"));

        let _ = std::fs::remove_dir_all(root);
    }
}
