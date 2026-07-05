use std::{
    collections::{HashMap, HashSet},
    fs,
    path::{Path, PathBuf},
};

use crate::{
    ast::{Declaration, EmbedSource, Expr, Function, Place, Program, Stmt, Type},
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

    validate_private_import_access(&program)?;

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
        let module_aliases = module_alias_declarations(import, &imported.declarations);
        let imported = resolve_program_imports(imported, stack, seen)?;
        declarations.extend(imported.declarations);
        declarations.extend(module_aliases);
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

fn module_alias_declarations(import: &str, declarations: &[Declaration]) -> Vec<Declaration> {
    let Some(module) = import.rsplit('.').next() else {
        return Vec::new();
    };
    declarations
        .iter()
        .filter_map(|declaration| match declaration {
            Declaration::Function(function) if function.public && function.name != "main" => {
                let mut alias = function.clone();
                alias.name = format!("{module}.{}", alias.name);
                Some(Declaration::Function(alias))
            }
            Declaration::ExternAsmFunction(function) if function.public => {
                let mut alias = function.clone();
                alias.name = format!("{module}.{}", alias.name);
                Some(Declaration::ExternAsmFunction(alias))
            }
            _ => None,
        })
        .collect()
}

fn validate_private_import_access(program: &Program) -> Result<(), Diagnostic> {
    let mut private_imports = HashMap::new();
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
        for declaration in &imported.declarations {
            let Some(name) = declaration_name(declaration) else {
                continue;
            };
            if !declaration_is_public(declaration) {
                private_imports.insert(name.to_owned(), import.clone());
            }
        }
    }

    if private_imports.is_empty() {
        return Ok(());
    }

    for declaration in &program.declarations {
        validate_declaration_private_import_access(declaration, &private_imports)?;
    }
    Ok(())
}

fn validate_declaration_private_import_access(
    declaration: &Declaration,
    private_imports: &HashMap<String, String>,
) -> Result<(), Diagnostic> {
    match declaration {
        Declaration::Const(decl) => {
            validate_type_private_import_access(&decl.ty, private_imports)?;
            validate_expr_private_import_access(&decl.value, private_imports, &HashSet::new())
        }
        Declaration::Alias(decl) => validate_type_private_import_access(&decl.ty, private_imports),
        Declaration::Port(decl) => {
            validate_expr_private_import_access(&decl.value, private_imports, &HashSet::new())
        }
        Declaration::Mmio(decl) => {
            validate_type_private_import_access(&decl.ty, private_imports)?;
            validate_expr_private_import_access(&decl.value, private_imports, &HashSet::new())
        }
        Declaration::Embed(decl) => {
            validate_embed_source_private_import_access(
                &decl.source,
                private_imports,
                &HashSet::new(),
            )?;
            if let Some(align) = &decl.align {
                validate_expr_private_import_access(align, private_imports, &HashSet::new())?;
            }
            Ok(())
        }
        Declaration::Global(decl) => {
            validate_type_private_import_access(&decl.ty, private_imports)?;
            validate_expr_private_import_access(&decl.value, private_imports, &HashSet::new())
        }
        Declaration::Struct(decl) => {
            for field in &decl.fields {
                validate_type_private_import_access(&field.ty, private_imports)?;
            }
            Ok(())
        }
        Declaration::ExternAsmFunction(function) => {
            for param in &function.params {
                validate_type_private_import_access(&param.ty, private_imports)?;
            }
            if let Some(return_type) = &function.return_type {
                validate_type_private_import_access(return_type, private_imports)?;
            }
            Ok(())
        }
        Declaration::Function(function) => {
            validate_function_private_import_access(function, private_imports)
        }
        Declaration::Import(_) => Ok(()),
    }
}

fn validate_function_private_import_access(
    function: &Function,
    private_imports: &HashMap<String, String>,
) -> Result<(), Diagnostic> {
    let mut locals = function
        .params
        .iter()
        .map(|param| param.name.clone())
        .collect::<HashSet<_>>();
    for param in &function.params {
        validate_type_private_import_access(&param.ty, private_imports)?;
    }
    if let Some(return_type) = &function.return_type {
        validate_type_private_import_access(return_type, private_imports)?;
    }
    for stmt in &function.body {
        validate_stmt_private_import_access(stmt, private_imports, &mut locals)?;
    }
    Ok(())
}

fn validate_stmt_private_import_access(
    stmt: &Stmt,
    private_imports: &HashMap<String, String>,
    locals: &mut HashSet<String>,
) -> Result<(), Diagnostic> {
    match stmt {
        Stmt::Let {
            name, ty, value, ..
        } => {
            validate_type_private_import_access(ty, private_imports)?;
            validate_expr_private_import_access(value, private_imports, locals)?;
            locals.insert(name.clone());
        }
        Stmt::Assign { target, value, .. } => {
            validate_place_private_import_access(target, private_imports, locals)?;
            validate_expr_private_import_access(value, private_imports, locals)?;
        }
        Stmt::If {
            condition,
            then_body,
            else_body,
        } => {
            validate_expr_private_import_access(condition, private_imports, locals)?;
            let mut then_locals = locals.clone();
            for stmt in then_body {
                validate_stmt_private_import_access(stmt, private_imports, &mut then_locals)?;
            }
            let mut else_locals = locals.clone();
            for stmt in else_body {
                validate_stmt_private_import_access(stmt, private_imports, &mut else_locals)?;
            }
        }
        Stmt::While { condition, body } => {
            validate_expr_private_import_access(condition, private_imports, locals)?;
            let mut body_locals = locals.clone();
            for stmt in body {
                validate_stmt_private_import_access(stmt, private_imports, &mut body_locals)?;
            }
        }
        Stmt::Loop { body } => {
            let mut body_locals = locals.clone();
            for stmt in body {
                validate_stmt_private_import_access(stmt, private_imports, &mut body_locals)?;
            }
        }
        Stmt::Return(Some(expr)) | Stmt::Expr(expr) => {
            validate_expr_private_import_access(expr, private_imports, locals)?;
        }
        Stmt::Return(None) | Stmt::Break | Stmt::Continue => {}
        Stmt::Asm {
            inputs, outputs, ..
        } => {
            for input in inputs {
                validate_type_private_import_access(&input.ty, private_imports)?;
                reject_private_import_name(&input.name, private_imports, locals)?;
            }
            for output in outputs {
                validate_type_private_import_access(&output.ty, private_imports)?;
                reject_private_import_name(&output.name, private_imports, locals)?;
            }
        }
        Stmt::Out { port, value } => {
            reject_private_import_name(port, private_imports, locals)?;
            validate_expr_private_import_access(value, private_imports, locals)?;
        }
    }
    Ok(())
}

fn validate_embed_source_private_import_access(
    source: &EmbedSource,
    private_imports: &HashMap<String, String>,
    locals: &HashSet<String>,
) -> Result<(), Diagnostic> {
    match source {
        EmbedSource::File(_) | EmbedSource::Text(_) | EmbedSource::CStr(_) => Ok(()),
        EmbedSource::Bytes(values) => {
            for value in values {
                validate_expr_private_import_access(value, private_imports, locals)?;
            }
            Ok(())
        }
        EmbedSource::Repeat { value, len } => {
            validate_expr_private_import_access(value, private_imports, locals)?;
            validate_expr_private_import_access(len, private_imports, locals)
        }
    }
}

fn validate_place_private_import_access(
    place: &Place,
    private_imports: &HashMap<String, String>,
    locals: &HashSet<String>,
) -> Result<(), Diagnostic> {
    match place {
        Place::Ident(name) => reject_private_import_name(name, private_imports, locals),
        Place::Index { name, index } => {
            reject_private_import_name(name, private_imports, locals)?;
            validate_expr_private_import_access(index, private_imports, locals)
        }
        Place::Field { base, .. } => reject_private_import_name(base, private_imports, locals),
        Place::Deref(expr) => validate_expr_private_import_access(expr, private_imports, locals),
    }
}

fn validate_expr_private_import_access(
    expr: &Expr,
    private_imports: &HashMap<String, String>,
    locals: &HashSet<String>,
) -> Result<(), Diagnostic> {
    match expr {
        Expr::Ident(name) | Expr::AddressOf(name) | Expr::In(name) => {
            reject_private_import_name(name, private_imports, locals)
        }
        Expr::Index { name, index } | Expr::AddressOfIndex { name, index } => {
            reject_private_import_name(name, private_imports, locals)?;
            validate_expr_private_import_access(index, private_imports, locals)
        }
        Expr::Field { base, .. } | Expr::AddressOfField { base, .. } => {
            reject_private_import_name(base, private_imports, locals)
        }
        Expr::Cast { expr, ty } => {
            validate_type_private_import_access(ty, private_imports)?;
            validate_expr_private_import_access(expr, private_imports, locals)
        }
        Expr::Unary { expr, .. } | Expr::Deref(expr) => {
            validate_expr_private_import_access(expr, private_imports, locals)
        }
        Expr::Binary { left, right, .. } => {
            validate_expr_private_import_access(left, private_imports, locals)?;
            validate_expr_private_import_access(right, private_imports, locals)
        }
        Expr::Array(values) => {
            for value in values {
                validate_expr_private_import_access(value, private_imports, locals)?;
            }
            Ok(())
        }
        Expr::StructInit { ty, fields } => {
            reject_private_import_type_name(ty, private_imports)?;
            for (_, value) in fields {
                validate_expr_private_import_access(value, private_imports, locals)?;
            }
            Ok(())
        }
        Expr::Call { path, args } => {
            if let Some(name) = path.first() {
                reject_private_import_name(name, private_imports, locals)?;
            }
            for arg in args {
                validate_expr_private_import_access(arg, private_imports, locals)?;
            }
            Ok(())
        }
        Expr::Int(_) | Expr::Char(_) | Expr::Bool(_) | Expr::String(_) => Ok(()),
    }
}

fn validate_type_private_import_access(
    ty: &Type,
    private_imports: &HashMap<String, String>,
) -> Result<(), Diagnostic> {
    match ty {
        Type::Named(name) => reject_private_import_type_name(name, private_imports),
        Type::Ptr(inner) => validate_type_private_import_access(inner, private_imports),
        Type::Array { element, len } => {
            validate_type_private_import_access(element, private_imports)?;
            validate_expr_private_import_access(
                &parse_array_len_expr(len),
                private_imports,
                &HashSet::new(),
            )
        }
    }
}

fn parse_array_len_expr(len: &str) -> Expr {
    len.parse::<i64>()
        .map(Expr::Int)
        .unwrap_or_else(|_| Expr::Ident(len.to_owned()))
}

fn reject_private_import_type_name(
    name: &str,
    private_imports: &HashMap<String, String>,
) -> Result<(), Diagnostic> {
    if let Some(import) = private_imports.get(name) {
        return Err(Diagnostic::new(format!(
            "declaration `{name}` from import `{import}` is private"
        )));
    }
    Ok(())
}

fn reject_private_import_name(
    name: &str,
    private_imports: &HashMap<String, String>,
    locals: &HashSet<String>,
) -> Result<(), Diagnostic> {
    if locals.contains(name) {
        return Ok(());
    }
    if let Some(import) = private_imports.get(name) {
        return Err(Diagnostic::new(format!(
            "declaration `{name}` from import `{import}` is private"
        )));
    }
    Ok(())
}

fn declaration_name(declaration: &Declaration) -> Option<&str> {
    match declaration {
        Declaration::Import(_) => None,
        Declaration::Const(decl) => Some(&decl.name),
        Declaration::Alias(decl) => Some(&decl.name),
        Declaration::Port(decl) => Some(&decl.name),
        Declaration::Mmio(decl) => Some(&decl.name),
        Declaration::Embed(decl) => Some(&decl.name),
        Declaration::Global(decl) => Some(&decl.name),
        Declaration::Struct(decl) => Some(&decl.name),
        Declaration::ExternAsmFunction(decl) => Some(&decl.name),
        Declaration::Function(decl) => Some(&decl.name),
    }
}

fn declaration_is_public(declaration: &Declaration) -> bool {
    match declaration {
        Declaration::Import(_) => true,
        Declaration::Const(decl) => decl.public,
        Declaration::Alias(decl) => decl.public,
        Declaration::Port(decl) => decl.public,
        Declaration::Mmio(decl) => decl.public,
        Declaration::Embed(decl) => decl.public,
        Declaration::Global(decl) => decl.public,
        Declaration::Struct(decl) => decl.public,
        Declaration::ExternAsmFunction(decl) => decl.public,
        Declaration::Function(decl) => decl.public,
    }
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
        std::fs::write(&lib_path, "pub fn add_one(v: u8) -> u8 { return v + 1 }\n").unwrap();
        let source = "import lib.math\nfn main() { let x: u8 = add_one(4); test.pass() }\n";
        std::fs::write(&main_path, source).unwrap();

        let options = CompileOptions {
            source: main_path.clone(),
            debug_comments: false,
        };
        let report = check_source(source, &options).unwrap();
        let program = load_program(&main_path).unwrap();

        assert_eq!(report.imports, 1);
        assert_eq!(report.declarations, 3);
        assert!(program.declarations.iter().any(|decl| {
            matches!(decl, Declaration::Function(function) if function.name == "add_one")
        }));
        assert!(program.declarations.iter().any(|decl| {
            matches!(decl, Declaration::Function(function) if function.name == "math.add_one")
        }));

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn rejects_access_to_private_imported_declarations() {
        let root = temp_root("private_imports");
        std::fs::create_dir_all(root.join("lib")).unwrap();
        let main_path = root.join("game.ezra");
        let lib_path = root.join("lib/math.ezra");
        std::fs::write(
            &lib_path,
            "fn hidden(v: u8) -> u8 { return v + 1 }\npub fn shown(v: u8) -> u8 { return v }\n",
        )
        .unwrap();
        std::fs::write(
            &main_path,
            "import lib.math\nfn main() { let x: u8 = hidden(4); test.pass() }\n",
        )
        .unwrap();

        let error = load_program(&main_path).unwrap_err();

        assert_eq!(
            error.message,
            "declaration `hidden` from import `lib.math` is private"
        );

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn rejects_private_imported_types_in_annotations() {
        let root = temp_root("private_types");
        std::fs::create_dir_all(root.join("lib")).unwrap();
        let main_path = root.join("game.ezra");
        let lib_path = root.join("lib/types.ezra");
        std::fs::write(
            &lib_path,
            "alias Hidden = u8\nstruct Secret { value: u8 }\npub alias Shown = u8\n",
        )
        .unwrap();
        std::fs::write(
            &main_path,
            "import lib.types\nfn main() { let x: Hidden = 1; test.pass() }\n",
        )
        .unwrap();

        let error = load_program(&main_path).unwrap_err();

        assert_eq!(
            error.message,
            "declaration `Hidden` from import `lib.types` is private"
        );

        std::fs::write(
            &main_path,
            "import lib.types\nfn main() { let x: Secret = Secret { value: 1 }; test.pass() }\n",
        )
        .unwrap();

        let error = load_program(&main_path).unwrap_err();

        assert_eq!(
            error.message,
            "declaration `Secret` from import `lib.types` is private"
        );

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn rejects_private_imported_declarations_in_embeds() {
        let root = temp_root("private_embed_exprs");
        std::fs::create_dir_all(root.join("lib")).unwrap();
        let main_path = root.join("game.ezra");
        let lib_path = root.join("lib/data.ezra");
        std::fs::write(
            &lib_path,
            "const SECRET: u8 = 0x41\npub const SHOWN: u8 = 4\n",
        )
        .unwrap();
        std::fs::write(
            &main_path,
            "import lib.data\nembed blob: bytes = bytes [SECRET]\nfn main() { test.pass() }\n",
        )
        .unwrap();

        let error = load_program(&main_path).unwrap_err();

        assert_eq!(
            error.message,
            "declaration `SECRET` from import `lib.data` is private"
        );

        std::fs::write(
            &main_path,
            "import lib.data\nembed blob: bytes = repeat(0, SECRET)\nfn main() { test.pass() }\n",
        )
        .unwrap();

        let error = load_program(&main_path).unwrap_err();

        assert_eq!(
            error.message,
            "declaration `SECRET` from import `lib.data` is private"
        );

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn rejects_private_imported_declarations_in_inline_asm_operands() {
        let root = temp_root("private_asm_operands");
        std::fs::create_dir_all(root.join("lib")).unwrap();
        let main_path = root.join("game.ezra");
        let lib_path = root.join("lib/hw.ezra");
        std::fs::write(
            &lib_path,
            "const SECRET: u8 = 0x41\nstruct Hidden { value: u8 }\n",
        )
        .unwrap();
        std::fs::write(
            &main_path,
            r#"
            import lib.hw
            fn main() {
                asm volatile(in SECRET: u8 as imm) {
                    "ld a, {SECRET}"
                }
                test.pass()
            }
            "#,
        )
        .unwrap();

        let error = load_program(&main_path).unwrap_err();

        assert_eq!(
            error.message,
            "declaration `SECRET` from import `lib.hw` is private"
        );

        std::fs::write(
            &main_path,
            r#"
            import lib.hw
            fn main() {
                asm volatile(in ptr: ptr<Hidden> as reg24) {
                    "ld hl, {ptr}"
                }
                test.pass()
            }
            "#,
        )
        .unwrap();

        let error = load_program(&main_path).unwrap_err();

        assert_eq!(
            error.message,
            "declaration `Hidden` from import `lib.hw` is private"
        );

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn allows_public_imported_types_in_annotations() {
        let root = temp_root("public_types");
        std::fs::create_dir_all(root.join("lib")).unwrap();
        let main_path = root.join("game.ezra");
        let lib_path = root.join("lib/types.ezra");
        std::fs::write(&lib_path, "pub alias Shown = u8\n").unwrap();
        std::fs::write(
            &main_path,
            "import lib.types\nfn main() { let x: Shown = 1; test.pass() }\n",
        )
        .unwrap();

        let program = load_program(&main_path).unwrap();

        assert!(
            program
                .declarations
                .iter()
                .any(|decl| { matches!(decl, Declaration::Alias(alias) if alias.name == "Shown") })
        );

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn allows_public_imported_declarations_to_use_private_helpers() {
        let root = temp_root("private_helpers");
        std::fs::create_dir_all(root.join("lib")).unwrap();
        let main_path = root.join("game.ezra");
        let lib_path = root.join("lib/math.ezra");
        std::fs::write(
            &lib_path,
            "fn hidden(v: u8) -> u8 { return v + 1 }\npub fn shown(v: u8) -> u8 { return hidden(v) }\n",
        )
        .unwrap();
        std::fs::write(
            &main_path,
            "import lib.math\nfn main() { let x: u8 = shown(4); test.pass() }\n",
        )
        .unwrap();

        let program = load_program(&main_path).unwrap();

        assert!(program.declarations.iter().any(|decl| {
            matches!(decl, Declaration::Function(function) if function.name == "hidden")
        }));
        assert!(program.declarations.iter().any(|decl| {
            matches!(decl, Declaration::Function(function) if function.name == "shown")
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
