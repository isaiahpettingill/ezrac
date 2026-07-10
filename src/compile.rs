use std::{
    collections::{HashMap, HashSet},
    fs,
    path::{Path, PathBuf},
};

use crate::{
    asm::ez80::emitter::collect_ez80_semantic_diagnostics,
    asm::{AssemblyOptions, emit_ez80_assembly_with_options},
    ast::{
        AccessPath, AccessSegment, CfgPredicate, Declaration, EmbedSource, Expr, Function, Place,
        Program, Stmt, Type,
    },
    diagnostic::{Diagnostic, SourceLocation, diagnostic_span, source_token_spans},
    parser::parse_program,
    target::{DEFAULT_TARGET_TRIPLE, memory_model_for_cpu, parse_target_triple},
};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CompileOptions {
    pub source: PathBuf,
    pub debug_comments: bool,
    pub default_sdk_symbols: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CompileReport {
    pub imports: usize,
    pub declarations: usize,
    pub has_main: bool,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct SdkResolver {
    pub target: Option<String>,
    pub sdk_roots: Vec<PathBuf>,
}

pub fn check_source(source: &str, options: &CompileOptions) -> Result<CompileReport, Diagnostic> {
    check_source_with_sdk(source, options, &SdkResolver::default())
}

pub fn check_source_diagnostics(source: &str, options: &CompileOptions) -> Vec<Diagnostic> {
    check_source_diagnostics_with_sdk(source, options, &SdkResolver::default())
}

pub fn check_source_diagnostics_with_sdk(
    source: &str,
    options: &CompileOptions,
    sdk: &SdkResolver,
) -> Vec<Diagnostic> {
    check_source_diagnostics_with_sdk_and_overrides(source, options, sdk, &HashMap::new())
}

pub fn check_source_diagnostics_with_sdk_and_overrides(
    source: &str,
    options: &CompileOptions,
    sdk: &SdkResolver,
    source_overrides: &HashMap<PathBuf, String>,
) -> Vec<Diagnostic> {
    let root = match parse_program(&options.source, source) {
        Ok(program) => program,
        Err(error) => return vec![error],
    };
    let source_program = root.clone();
    let resolved = match resolve_program_imports(
        root,
        sdk,
        &mut Vec::new(),
        &mut HashSet::new(),
        source_overrides,
    ) {
        Ok(program) => program,
        Err(error) => return vec![locate_source_diagnostic(error, source, &options.source)],
    };
    let mut diagnostics = collect_reference_diagnostics(
        &source_program,
        &resolved,
        source,
        options.default_sdk_symbols,
    );
    for unit in &resolved.source_units {
        if normalize_path(&unit.path) == normalize_path(&source_program.source_path) {
            continue;
        }
        if let Ok(program) = parse_program(&unit.path, &unit.text) {
            diagnostics.extend(collect_reference_diagnostics(
                &program,
                &resolved,
                &unit.text,
                options.default_sdk_symbols,
            ));
        }
    }
    for diagnostic in collect_ez80_semantic_diagnostics(
        &resolved,
        AssemblyOptions {
            debug_comments: options.debug_comments,
            default_sdk_symbols: options.default_sdk_symbols,
            ..AssemblyOptions::default()
        },
    ) {
        if !diagnostics
            .iter()
            .any(|existing| diagnostic_is_covered_by(existing, &diagnostic))
        {
            diagnostics.push(diagnostic);
        }
    }
    if let Err(error) = check_source_with_sdk_and_overrides(source, options, sdk, source_overrides)
        && !diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message == error.message)
    {
        diagnostics.push(error);
    }
    diagnostics
}

fn diagnostic_is_covered_by(existing: &Diagnostic, candidate: &Diagnostic) -> bool {
    if existing.message != candidate.message {
        return false;
    }
    let (Some(existing), Some(candidate)) = (&existing.span, &candidate.span) else {
        return false;
    };
    existing.file == candidate.file
        && (candidate.start.line, candidate.start.column)
            <= (existing.start.line, existing.start.column)
        && (existing.end.line, existing.end.column) <= (candidate.end.line, candidate.end.column)
}

pub fn check_source_with_sdk(
    source: &str,
    options: &CompileOptions,
    sdk: &SdkResolver,
) -> Result<CompileReport, Diagnostic> {
    check_source_with_sdk_and_overrides(source, options, sdk, &HashMap::new())
}

fn check_source_with_sdk_and_overrides(
    source: &str,
    options: &CompileOptions,
    sdk: &SdkResolver,
    source_overrides: &HashMap<PathBuf, String>,
) -> Result<CompileReport, Diagnostic> {
    let root = parse_program(&options.source, source)?;
    let imports = root
        .declarations
        .iter()
        .filter(|decl| matches!(decl, crate::ast::Declaration::Import(_)))
        .count();
    let fallback_location = source_start_location(&options.source);
    let program = resolve_program_imports(
        root,
        sdk,
        &mut Vec::new(),
        &mut HashSet::new(),
        source_overrides,
    )
    .map_err(|error| locate_source_diagnostic(error, source, &options.source))?;
    let declarations = program.declarations.len();
    let has_main = program.main_function().is_some();

    if !has_main {
        return Err(Diagnostic::at(
            fallback_location,
            "missing required `fn main()`",
        ));
    }
    validate_main_signature(program.main_function().expect("main presence checked"))
        .map_err(|error| locate_source_diagnostic(error, source, &options.source))?;
    let assembly = emit_ez80_assembly_with_options(
        &program,
        AssemblyOptions {
            debug_comments: options.debug_comments,
            default_sdk_symbols: options.default_sdk_symbols,
            ..AssemblyOptions::default()
        },
    )
    .map_err(|error| locate_source_diagnostic(error, source, &options.source))?;
    crate::vm::assemble_ez80_subset_at(&assembly, 0)
        .map_err(|error| error.with_location_if_missing(source_start_location(&options.source)))?;

    Ok(CompileReport {
        imports,
        declarations,
        has_main,
    })
}

fn locate_source_diagnostic(error: Diagnostic, source: &str, path: &Path) -> Diagnostic {
    if error.location().is_some() {
        return error;
    }
    diagnostic_span(path, source, &error.message)
        .map(|span| error.clone().with_span_if_missing(span))
        .unwrap_or_else(|| error.with_location_if_missing(source_start_location(path)))
}

struct ReferenceDiagnostics<'a> {
    file: &'a Path,
    source: &'a str,
    occurrences: HashMap<String, usize>,
    diagnostics: Vec<Diagnostic>,
}

impl ReferenceDiagnostics<'_> {
    fn push(&mut self, name: &str, message: String) {
        let occurrence = self.occurrences.entry(name.to_owned()).or_default();
        let spans = source_token_spans(self.file, self.source, name);
        let span = spans.get(*occurrence).or_else(|| spans.last()).cloned();
        *occurrence += 1;
        let diagnostic = span
            .map(|span| Diagnostic::at_span(span, message.clone()))
            .unwrap_or_else(|| Diagnostic::new(message));
        self.diagnostics.push(diagnostic);
    }
}

fn collect_reference_diagnostics(
    source_program: &Program,
    resolved: &Program,
    source: &str,
    default_sdk_symbols: bool,
) -> Vec<Diagnostic> {
    let globals = resolved
        .declarations
        .iter()
        .filter_map(declaration_name)
        .map(str::to_owned)
        .collect::<HashSet<_>>();
    let mut output = ReferenceDiagnostics {
        file: &source_program.source_path,
        source,
        occurrences: HashMap::new(),
        diagnostics: Vec::new(),
    };
    for declaration in &source_program.declarations {
        collect_declaration_references(declaration, &globals, default_sdk_symbols, &mut output);
    }
    output.diagnostics
}

fn collect_declaration_references(
    declaration: &Declaration,
    globals: &HashSet<String>,
    default_sdk_symbols: bool,
    output: &mut ReferenceDiagnostics<'_>,
) {
    match declaration {
        Declaration::Const(decl) => {
            collect_expr_references(&decl.value, globals, default_sdk_symbols, output)
        }
        Declaration::Port(decl) => {
            collect_expr_references(&decl.value, globals, default_sdk_symbols, output)
        }
        Declaration::Mmio(decl) => {
            collect_expr_references(&decl.value, globals, default_sdk_symbols, output)
        }
        Declaration::Global(decl) => {
            collect_expr_references(&decl.value, globals, default_sdk_symbols, output)
        }
        Declaration::Function(function) => {
            let mut names = globals.clone();
            names.extend(function.params.iter().map(|param| param.name.clone()));
            collect_local_names(&function.body, &mut names);
            collect_stmt_references(&function.body, &names, default_sdk_symbols, output);
        }
        Declaration::Cfg { declaration, .. } => {
            collect_declaration_references(declaration, globals, default_sdk_symbols, output)
        }
        Declaration::Embed(_)
        | Declaration::Import(_)
        | Declaration::Alias(_)
        | Declaration::Struct(_)
        | Declaration::ExternAsmFunction(_) => {}
    }
}

fn collect_local_names(stmts: &[Stmt], names: &mut HashSet<String>) {
    for stmt in stmts {
        match stmt {
            Stmt::Let { name, .. } => {
                names.insert(name.clone());
            }
            Stmt::If {
                then_body,
                else_body,
                ..
            } => {
                collect_local_names(then_body, names);
                collect_local_names(else_body, names);
            }
            Stmt::While { body, .. } | Stmt::Loop { body } => collect_local_names(body, names),
            Stmt::Asm { outputs, .. } => {
                names.extend(outputs.iter().map(|output| output.name.clone()));
            }
            _ => {}
        }
    }
}

fn collect_stmt_references(
    stmts: &[Stmt],
    names: &HashSet<String>,
    default_sdk_symbols: bool,
    output: &mut ReferenceDiagnostics<'_>,
) {
    for stmt in stmts {
        match stmt {
            Stmt::Let { value, .. } | Stmt::Return(Some(value)) => {
                collect_expr_references(value, names, default_sdk_symbols, output)
            }
            Stmt::Assign { target, value, .. } => {
                collect_place_references(target, names, default_sdk_symbols, output);
                collect_expr_references(value, names, default_sdk_symbols, output);
            }
            Stmt::If {
                condition,
                then_body,
                else_body,
            } => {
                collect_expr_references(condition, names, default_sdk_symbols, output);
                collect_stmt_references(then_body, names, default_sdk_symbols, output);
                collect_stmt_references(else_body, names, default_sdk_symbols, output);
            }
            Stmt::While { condition, body } => {
                collect_expr_references(condition, names, default_sdk_symbols, output);
                collect_stmt_references(body, names, default_sdk_symbols, output);
            }
            Stmt::Loop { body } => {
                collect_stmt_references(body, names, default_sdk_symbols, output)
            }
            Stmt::Out { port, value } => {
                push_unknown_reference(port, false, names, default_sdk_symbols, output);
                collect_expr_references(value, names, default_sdk_symbols, output);
            }
            Stmt::Expr(expr) => collect_expr_references(expr, names, default_sdk_symbols, output),
            Stmt::Break | Stmt::Continue | Stmt::Return(None) | Stmt::Asm { .. } => {}
        }
    }
}

fn collect_place_references(
    place: &Place,
    names: &HashSet<String>,
    default_sdk_symbols: bool,
    output: &mut ReferenceDiagnostics<'_>,
) {
    match place {
        Place::Ident(name) => {
            push_unknown_reference(name, false, names, default_sdk_symbols, output)
        }
        Place::Index { name, index } => {
            push_unknown_reference(name, false, names, default_sdk_symbols, output);
            collect_expr_references(index, names, default_sdk_symbols, output);
        }
        Place::Field { base, field } => {
            push_unknown_field(base, field, names, default_sdk_symbols, output)
        }
        Place::Access(path) => {
            push_unknown_access_path(path, names, default_sdk_symbols, output);
            for segment in &path.segments {
                if let AccessSegment::Index(index) = segment {
                    collect_expr_references(index, names, default_sdk_symbols, output);
                }
            }
        }
        Place::Deref(expr) => collect_expr_references(expr, names, default_sdk_symbols, output),
    }
}

fn collect_expr_references(
    expr: &Expr,
    names: &HashSet<String>,
    default_sdk_symbols: bool,
    output: &mut ReferenceDiagnostics<'_>,
) {
    match expr {
        Expr::Ident(name) | Expr::In(name) | Expr::AddressOf(name) => {
            push_unknown_reference(name, false, names, default_sdk_symbols, output)
        }
        Expr::Index { name, index } | Expr::AddressOfIndex { name, index } => {
            push_unknown_reference(name, false, names, default_sdk_symbols, output);
            collect_expr_references(index, names, default_sdk_symbols, output);
        }
        Expr::Field { base, field } | Expr::AddressOfField { base, field } => {
            push_unknown_field(base, field, names, default_sdk_symbols, output)
        }
        Expr::Access(path) | Expr::AddressOfAccess(path) => {
            push_unknown_access_path(path, names, default_sdk_symbols, output);
            for segment in &path.segments {
                if let AccessSegment::Index(index) = segment {
                    collect_expr_references(index, names, default_sdk_symbols, output);
                }
            }
        }
        Expr::StructInit { ty, fields } => {
            push_unknown_reference(ty, false, names, default_sdk_symbols, output);
            for (_, value) in fields {
                collect_expr_references(value, names, default_sdk_symbols, output);
            }
        }
        Expr::Call { path, args } => {
            push_unknown_reference(&path.join("."), true, names, default_sdk_symbols, output);
            for arg in args {
                collect_expr_references(arg, names, default_sdk_symbols, output);
            }
        }
        Expr::Unary { expr, .. } | Expr::Cast { expr, .. } | Expr::Deref(expr) => {
            collect_expr_references(expr, names, default_sdk_symbols, output)
        }
        Expr::Binary { left, right, .. } => {
            collect_expr_references(left, names, default_sdk_symbols, output);
            collect_expr_references(right, names, default_sdk_symbols, output);
        }
        Expr::Array(values) => {
            for value in values {
                collect_expr_references(value, names, default_sdk_symbols, output);
            }
        }
        Expr::Int(_) | Expr::TypedInt(_, _) | Expr::Bool(_) | Expr::Char(_) | Expr::String(_) => {}
    }
}

fn push_unknown_access_path(
    path: &AccessPath,
    names: &HashSet<String>,
    default_sdk_symbols: bool,
    output: &mut ReferenceDiagnostics<'_>,
) {
    if names.contains(&path.root) {
        return;
    }
    let mut qualified = path.root.clone();
    for segment in &path.segments {
        let AccessSegment::Field(field) = segment else {
            break;
        };
        qualified.push('.');
        qualified.push_str(field);
    }
    push_unknown_reference(&qualified, false, names, default_sdk_symbols, output);
}

fn push_unknown_field(
    base: &str,
    field: &str,
    names: &HashSet<String>,
    default_sdk_symbols: bool,
    output: &mut ReferenceDiagnostics<'_>,
) {
    if names.contains(base) {
        return;
    }
    push_unknown_reference(
        &format!("{base}.{field}"),
        false,
        names,
        default_sdk_symbols,
        output,
    );
}

fn push_unknown_reference(
    name: &str,
    function: bool,
    names: &HashSet<String>,
    default_sdk_symbols: bool,
    output: &mut ReferenceDiagnostics<'_>,
) {
    if names.contains(name)
        || default_sdk_symbols
            && matches!(
                name.split('.').next(),
                Some("test" | "debug" | "mem" | "ezra")
            )
    {
        return;
    }
    let kind = if function { "function" } else { "value" };
    output.push(name, format!("unknown {kind} `{name}`"));
}

fn source_start_location(path: &Path) -> SourceLocation {
    SourceLocation {
        file: path.to_path_buf(),
        line: 1,
        column: 1,
    }
}

pub fn load_program(path: &Path) -> Result<Program, Diagnostic> {
    load_program_with_sdk(path, &SdkResolver::default())
}

pub fn load_program_with_sdk(path: &Path, sdk: &SdkResolver) -> Result<Program, Diagnostic> {
    let source = fs::read_to_string(path).map_err(|error| {
        Diagnostic::new(format!("failed to read `{}`: {error}", path.display()))
    })?;
    parse_and_resolve_imports_with_sdk(path, &source, sdk)
}

pub fn parse_and_resolve_imports(path: &Path, source: &str) -> Result<Program, Diagnostic> {
    parse_and_resolve_imports_with_sdk(path, source, &SdkResolver::default())
}

pub fn parse_and_resolve_imports_with_sdk(
    path: &Path,
    source: &str,
    sdk: &SdkResolver,
) -> Result<Program, Diagnostic> {
    let root = parse_program(path, source)?;
    let mut stack = Vec::new();
    let mut seen = HashSet::new();
    resolve_program_imports(root, sdk, &mut stack, &mut seen, &HashMap::new())
}

pub fn resolve_import_source(
    importer: &Path,
    import: &str,
    sdk: &SdkResolver,
) -> Result<(PathBuf, String), Diagnostic> {
    read_import_source(importer, import, sdk)
}

fn resolve_program_imports(
    mut program: Program,
    sdk: &SdkResolver,
    stack: &mut Vec<PathBuf>,
    seen: &mut HashSet<PathBuf>,
    source_overrides: &HashMap<PathBuf, String>,
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
            source_text: program.source_text,
            source_units: Vec::new(),
            declarations: Vec::new(),
        });
    }

    program.declarations = active_declarations(program.declarations, sdk)?;

    validate_private_import_access(&program, sdk)?;

    let short_module_counts = direct_import_short_module_counts(&program);
    stack.push(path);
    let mut declarations = Vec::new();
    let mut source_units = Vec::new();
    for declaration in &program.declarations {
        let Declaration::Import(import) = declaration else {
            continue;
        };
        let (import_path, source) = read_import_source(&program.source_path, import, sdk)?;
        let source = source_override(source_overrides, &import_path).unwrap_or(source);
        let mut imported = parse_program(&import_path, &source)?;
        imported.declarations = active_declarations(imported.declarations, sdk)?;
        let short_module = import.rsplit('.').next().unwrap_or(import);
        let include_short_aliases = short_module_counts
            .get(short_module)
            .copied()
            .unwrap_or_default()
            <= 1;
        let module_aliases =
            module_alias_declarations(import, &imported.declarations, include_short_aliases);
        let imported = resolve_program_imports(imported, sdk, stack, seen, source_overrides)?;
        source_units.extend(imported.source_units.iter().cloned());
        let imported_declarations = imported
            .declarations
            .into_iter()
            .filter(|declaration| !is_entry_function(declaration))
            .collect::<Vec<_>>();
        reject_import_declaration_collisions(
            &declarations,
            &imported_declarations,
            &import_path,
            &source,
        )?;
        declarations.extend(imported_declarations);
        declarations.extend(module_aliases);
    }
    stack.pop();

    declarations.extend(
        program
            .declarations
            .into_iter()
            .filter(|decl| !matches!(decl, Declaration::Import(_))),
    );
    source_units.extend(program.source_units);

    Ok(Program {
        source_path: program.source_path,
        source_text: program.source_text,
        source_units,
        declarations,
    })
}

fn source_override(source_overrides: &HashMap<PathBuf, String>, path: &Path) -> Option<String> {
    source_overrides
        .get(path)
        .or_else(|| source_overrides.get(&normalize_path(path)))
        .cloned()
}

fn reject_import_declaration_collisions(
    existing: &[Declaration],
    incoming: &[Declaration],
    import_path: &Path,
    source: &str,
) -> Result<(), Diagnostic> {
    let existing = existing
        .iter()
        .filter_map(declaration_name)
        .collect::<HashSet<_>>();
    for declaration in incoming {
        let Some(name) = declaration_name(declaration) else {
            continue;
        };
        if existing.contains(name) {
            let message = format!("duplicate imported declaration `{name}`");
            let diagnostic = diagnostic_span(import_path, source, &message)
                .map(|span| Diagnostic::at_span(span, message.clone()))
                .unwrap_or_else(|| Diagnostic::new(message));
            return Err(diagnostic);
        }
    }
    Ok(())
}

fn validate_main_signature(main: &Function) -> Result<(), Diagnostic> {
    if !main.params.is_empty() {
        return Err(Diagnostic::new("main function cannot take parameters"));
    }
    if main.return_type.is_some() {
        return Err(Diagnostic::new("main function cannot return a value"));
    }
    Ok(())
}

fn is_entry_function(declaration: &Declaration) -> bool {
    matches!(declaration, Declaration::Function(function) if function.name == "main")
}

fn active_declarations(
    declarations: Vec<Declaration>,
    sdk: &SdkResolver,
) -> Result<Vec<Declaration>, Diagnostic> {
    let context = CfgContext::new(sdk)?;
    declarations
        .into_iter()
        .filter_map(|declaration| active_declaration(declaration, &context).transpose())
        .collect()
}

fn active_declaration(
    declaration: Declaration,
    context: &CfgContext,
) -> Result<Option<Declaration>, Diagnostic> {
    match declaration {
        Declaration::Cfg {
            predicates,
            declaration,
        } => {
            for predicate in &predicates {
                if !context.matches(predicate)? {
                    return Ok(None);
                }
            }
            active_declaration(*declaration, context)
        }
        declaration => Ok(Some(declaration)),
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct CfgContext {
    target: String,
    cpu: String,
    target_family: Option<String>,
    vendor: Option<String>,
    os: Option<String>,
    features: HashSet<String>,
    pointer_width: u16,
    address_width: u16,
    debug: bool,
}

impl CfgContext {
    fn new(sdk: &SdkResolver) -> Result<Self, Diagnostic> {
        let target = sdk.target.as_deref().unwrap_or(DEFAULT_TARGET_TRIPLE);
        let triple = parse_target_triple(target).map_err(Diagnostic::new)?;
        let parts = target.split('-').collect::<Vec<_>>();
        let cpu = triple.cpu.as_str().to_owned();
        let target_family = parts.first().map(|part| (*part).to_owned());
        let vendor = parts.get(1).map(|part| (*part).to_owned());
        let os = parts
            .iter()
            .copied()
            .find(|part| matches!(*part, "mos" | "cpm" | "baremetal"))
            .map(str::to_owned);
        let features = parts
            .iter()
            .copied()
            .filter(|part| *part != cpu)
            .map(str::to_owned)
            .collect();
        let memory = memory_model_for_cpu(triple.cpu).ok_or_else(|| {
            Diagnostic::new(format!("no target profile is implemented for CPU `{cpu}`"))
        })?;
        Ok(Self {
            target: target.to_owned(),
            cpu,
            target_family,
            vendor,
            os,
            features,
            pointer_width: memory.pointer_width_bits,
            address_width: memory.address_width_bits,
            debug: cfg!(debug_assertions),
        })
    }

    fn matches(&self, predicate: &CfgPredicate) -> Result<bool, Diagnostic> {
        match predicate {
            CfgPredicate::Target(value) => Ok(self.target == *value),
            CfgPredicate::TargetFamily(value) => Ok(self.target_family.as_deref() == Some(value)),
            CfgPredicate::Cpu(value) => Ok(self.cpu == *value),
            CfgPredicate::Vendor(value) => Ok(self.vendor.as_deref() == Some(value)),
            CfgPredicate::Os(value) => Ok(self.os.as_deref() == Some(value)),
            CfgPredicate::PointerWidth(value) => Ok(self.pointer_width == *value),
            CfgPredicate::AddressWidth(value) => Ok(self.address_width == *value),
            CfgPredicate::Feature(value) => {
                if !self.features.contains(value) {
                    return Err(Diagnostic::new(format!("unknown cfg feature `{value}`")));
                }
                Ok(true)
            }
            CfgPredicate::Debug => Ok(self.debug),
            CfgPredicate::Release => Ok(!self.debug),
            CfgPredicate::All(predicates) => {
                for predicate in predicates {
                    if !self.matches(predicate)? {
                        return Ok(false);
                    }
                }
                Ok(true)
            }
            CfgPredicate::Any(predicates) => {
                for predicate in predicates {
                    if self.matches(predicate)? {
                        return Ok(true);
                    }
                }
                Ok(false)
            }
            CfgPredicate::Not(predicate) => Ok(!self.matches(predicate)?),
        }
    }
}

fn read_import_source(
    source_path: &Path,
    import: &str,
    sdk: &SdkResolver,
) -> Result<(PathBuf, String), Diagnostic> {
    let candidates = import_file_candidates(source_path, import, sdk);
    let missing_path = candidates
        .first()
        .cloned()
        .unwrap_or_else(|| PathBuf::from(import.replace('.', "/")).with_extension("ezra"));
    for candidate in candidates {
        match fs::read_to_string(&candidate) {
            Ok(source) => return Ok((candidate, source)),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(Diagnostic::new(format!(
                    "failed to read import `{import}` at `{}`: {error}",
                    candidate.display()
                )));
            }
        }
    }
    if let Some(source) = builtin_sdk_source(sdk.target.as_deref(), import) {
        return Ok((builtin_sdk_path(import), source.to_owned()));
    }
    Err(Diagnostic::new(format!(
        "failed to read import `{import}` at `{}`: not found",
        missing_path.display()
    )))
}

fn import_file_candidates(source_path: &Path, import: &str, sdk: &SdkResolver) -> Vec<PathBuf> {
    let module_path = PathBuf::from(import.replace('.', "/")).with_extension("ezra");
    let source_dir = source_path.parent().unwrap_or_else(|| Path::new("."));
    let mut candidates = Vec::new();
    push_unique_path(&mut candidates, source_dir.join(&module_path));

    for ancestor in source_dir.ancestors().skip(1) {
        push_unique_path(&mut candidates, ancestor.join(&module_path));
    }

    if let Ok(project_root) = std::env::current_dir() {
        push_unique_path(&mut candidates, project_root.join(&module_path));
    }
    for root in &sdk.sdk_roots {
        push_unique_path(&mut candidates, root.join(&module_path));
    }
    candidates
}

fn builtin_sdk_path(import: &str) -> PathBuf {
    PathBuf::from(format!("builtin-sdk/{}.ezra", import.replace('.', "/")))
}

fn builtin_sdk_source(target: Option<&str>, import: &str) -> Option<&'static str> {
    if target.is_some_and(|target| target.starts_with("agonlight-mos-ez80")) {
        match import {
            "agon.buffers" => Some(builtin_sdk_utf8(
                include_bytes!("../toolchains/agonlight-mos-ez80/sdk/agon/buffers.ezra"),
                "agon.buffers",
            )),
            "agon.console" => Some(builtin_sdk_utf8(
                include_bytes!("../toolchains/agonlight-mos-ez80/sdk/agon/console.ezra"),
                "agon.console",
            )),
            "agon.mos" => Some(builtin_sdk_utf8(
                include_bytes!("../toolchains/agonlight-mos-ez80/sdk/agon/mos.ezra"),
                "agon.mos",
            )),
            "agon.gpio" => Some(builtin_sdk_utf8(
                include_bytes!("../toolchains/agonlight-mos-ez80/sdk/agon/gpio.ezra"),
                "agon.gpio",
            )),
            "agon.keyboard" => Some(builtin_sdk_utf8(
                include_bytes!("../toolchains/agonlight-mos-ez80/sdk/agon/keyboard.ezra"),
                "agon.keyboard",
            )),
            "agon.mouse" => Some(builtin_sdk_utf8(
                include_bytes!("../toolchains/agonlight-mos-ez80/sdk/agon/mouse.ezra"),
                "agon.mouse",
            )),
            "agon.sprites" => Some(builtin_sdk_utf8(
                include_bytes!("../toolchains/agonlight-mos-ez80/sdk/agon/sprites.ezra"),
                "agon.sprites",
            )),
            "agon.vdp" => Some(builtin_sdk_utf8(
                include_bytes!("../toolchains/agonlight-mos-ez80/sdk/agon/vdp.ezra"),
                "agon.vdp",
            )),
            _ => None,
        }
    } else if target.is_some_and(|target| target.starts_with("ez180n-ez80")) {
        match import {
            "ez180n.console" => Some(builtin_sdk_utf8(
                include_bytes!("../toolchains/ez180n-ez80/sdk/ez180n/console.ezra"),
                "ez180n.console",
            )),
            _ => None,
        }
    } else if target.is_some_and(is_ti_ce_target) {
        match import {
            "tice.os" => Some(builtin_sdk_utf8(
                include_bytes!("../toolchains/tice-ez80/sdk/tice/os.ezra"),
                "tice.os",
            )),
            "tice.lcd" => Some(builtin_sdk_utf8(
                include_bytes!("../toolchains/tice-ez80/sdk/tice/lcd.ezra"),
                "tice.lcd",
            )),
            _ => None,
        }
    } else if target.is_some_and(is_ti_z80_target) {
        match import {
            "ti.os" => Some(builtin_sdk_utf8(
                include_bytes!("../toolchains/ti-z80/sdk/ti/os.ezra"),
                "ti.os",
            )),
            "ti.lcd" => Some(builtin_sdk_utf8(
                include_bytes!("../toolchains/ti-z80/sdk/ti/lcd.ezra"),
                "ti.lcd",
            )),
            _ => None,
        }
    } else if target.is_some_and(|target| target.starts_with("zxspectrum-z80")) {
        match import {
            "zx.rom" => Some(builtin_sdk_utf8(
                include_bytes!("../toolchains/zxspectrum-z80/sdk/zx/rom.ezra"),
                "zx.rom",
            )),
            "zx.screen" => Some(builtin_sdk_utf8(
                include_bytes!("../toolchains/zxspectrum-z80/sdk/zx/screen.ezra"),
                "zx.screen",
            )),
            _ => None,
        }
    } else if target.is_some_and(|target| target.starts_with("ezra-test-")) {
        match import {
            "harness.io" => Some(builtin_sdk_utf8(
                include_bytes!("../toolchains/ezra-test-ez80/sdk/harness/io.ezra"),
                "harness.io",
            )),
            "harness.layout" => Some(builtin_sdk_utf8(
                include_bytes!("../toolchains/ezra-test-ez80/sdk/harness/layout.ezra"),
                "harness.layout",
            )),
            "harness.memory" => Some(builtin_sdk_utf8(
                include_bytes!("../toolchains/ezra-test-ez80/sdk/harness/memory.ezra"),
                "harness.memory",
            )),
            _ => None,
        }
    } else if target.is_some_and(|target| target.split('-').any(|part| part == "cpm")) {
        match import {
            "cpm.bdos" => Some(builtin_sdk_utf8(
                include_bytes!("../toolchains/cpm-2.2-z80/sdk/cpm/bdos.ezra"),
                "cpm.bdos",
            )),
            "cpm.console" => Some(builtin_sdk_utf8(
                include_bytes!("../toolchains/cpm-2.2-z80/sdk/cpm/console.ezra"),
                "cpm.console",
            )),
            "cpm.dma" => Some(builtin_sdk_utf8(
                include_bytes!("../toolchains/cpm-2.2-z80/sdk/cpm/dma.ezra"),
                "cpm.dma",
            )),
            "cpm.fcb" => Some(builtin_sdk_utf8(
                include_bytes!("../toolchains/cpm-2.2-z80/sdk/cpm/fcb.ezra"),
                "cpm.fcb",
            )),
            _ => None,
        }
    } else {
        None
    }
}

/// Return the built-in SDK modules available for the selected target.
///
/// Keep this list derived from `builtin_sdk_source` so consumers such as the
/// LSP cannot advertise a module that import resolution would reject.
pub fn builtin_sdk_modules(target: Option<&str>) -> Vec<&'static str> {
    const MODULES: &[&str] = &[
        "agon.buffers",
        "agon.console",
        "agon.mos",
        "agon.gpio",
        "agon.keyboard",
        "agon.mouse",
        "agon.sprites",
        "agon.vdp",
        "ez180n.console",
        "tice.os",
        "tice.lcd",
        "ti.os",
        "ti.lcd",
        "zx.rom",
        "zx.screen",
        "harness.io",
        "harness.layout",
        "harness.memory",
        "cpm.bdos",
        "cpm.console",
        "cpm.dma",
        "cpm.fcb",
    ];

    MODULES
        .iter()
        .copied()
        .filter(|module| builtin_sdk_source(target, module).is_some())
        .collect()
}

fn is_ti_ce_target(target: &str) -> bool {
    target.starts_with("ti84plusce-ez80") || target.starts_with("ti83premiumce-ez80")
}

fn is_ti_z80_target(target: &str) -> bool {
    target.starts_with("ti83-z80")
        || target.starts_with("ti83plus-z80")
        || target.starts_with("ti84-z80")
        || target.starts_with("ti84plus-z80")
}

fn builtin_sdk_utf8(bytes: &'static [u8], module: &str) -> &'static str {
    std::str::from_utf8(bytes)
        .unwrap_or_else(|_| panic!("built-in SDK module `{module}` is not UTF-8"))
}

fn push_unique_path(paths: &mut Vec<PathBuf>, path: PathBuf) {
    if !paths.iter().any(|candidate| candidate == &path) {
        paths.push(path);
    }
}

fn normalize_path(path: &Path) -> PathBuf {
    path.canonicalize().unwrap_or_else(|_| path.to_path_buf())
}

fn direct_import_short_module_counts(program: &Program) -> HashMap<String, usize> {
    let mut counts = HashMap::new();
    for declaration in &program.declarations {
        let Declaration::Import(import) = declaration else {
            continue;
        };
        let short_module = import.rsplit('.').next().unwrap_or(import);
        *counts.entry(short_module.to_owned()).or_insert(0) += 1;
    }
    counts
}

fn module_alias_declarations(
    import: &str,
    declarations: &[Declaration],
    include_short_aliases: bool,
) -> Vec<Declaration> {
    let Some(short_module) = import.rsplit('.').next() else {
        return Vec::new();
    };
    let mut prefixes = Vec::new();
    if include_short_aliases {
        prefixes.push(short_module.to_owned());
    }
    if short_module != import || !include_short_aliases {
        prefixes.push(import.to_owned());
    }
    declarations
        .iter()
        .flat_map(|declaration| {
            prefixes
                .iter()
                .filter_map(|prefix| alias_declaration(declaration, prefix))
                .collect::<Vec<_>>()
        })
        .collect()
}

fn alias_declaration(declaration: &Declaration, prefix: &str) -> Option<Declaration> {
    match declaration {
        Declaration::Alias(decl) if decl.public => {
            let mut alias = decl.clone();
            alias.name = format!("{prefix}.{}", alias.name);
            Some(Declaration::Alias(alias))
        }
        Declaration::Const(decl) if decl.public => {
            let mut alias = decl.clone();
            alias.name = format!("{prefix}.{}", alias.name);
            Some(Declaration::Const(alias))
        }
        Declaration::Port(decl) if decl.public => {
            let mut alias = decl.clone();
            alias.name = format!("{prefix}.{}", alias.name);
            Some(Declaration::Port(alias))
        }
        Declaration::Mmio(decl) if decl.public => {
            let mut alias = decl.clone();
            alias.name = format!("{prefix}.{}", alias.name);
            Some(Declaration::Mmio(alias))
        }
        Declaration::Embed(decl) if decl.public => {
            let mut alias = decl.clone();
            alias.name = format!("{prefix}.{}", alias.name);
            Some(Declaration::Embed(alias))
        }
        Declaration::Global(decl) if decl.public => {
            let mut alias = decl.clone();
            alias.name = format!("{prefix}.{}", alias.name);
            Some(Declaration::Global(alias))
        }
        Declaration::Struct(decl) if decl.public => {
            let mut alias = decl.clone();
            alias.name = format!("{prefix}.{}", alias.name);
            Some(Declaration::Struct(alias))
        }
        Declaration::Function(function) if function.public && function.name != "main" => {
            let mut alias = function.clone();
            alias.name = format!("{prefix}.{}", alias.name);
            Some(Declaration::Function(alias))
        }
        Declaration::ExternAsmFunction(function) if function.public => {
            let mut alias = function.clone();
            alias.name = format!("{prefix}.{}", alias.name);
            Some(Declaration::ExternAsmFunction(alias))
        }
        _ => None,
    }
}

fn validate_private_import_access(program: &Program, sdk: &SdkResolver) -> Result<(), Diagnostic> {
    let mut private_imports = HashMap::new();
    let mut seen_imports = HashSet::new();
    for declaration in &program.declarations {
        let Declaration::Import(import) = declaration else {
            continue;
        };
        collect_private_imports(
            &program.source_path,
            import,
            sdk,
            &mut private_imports,
            &mut seen_imports,
        )?;
    }

    if private_imports.is_empty() {
        return Ok(());
    }

    for declaration in &program.declarations {
        validate_declaration_private_import_access(declaration, &private_imports)?;
    }
    Ok(())
}

fn collect_private_imports(
    source_path: &Path,
    import: &str,
    sdk: &SdkResolver,
    private_imports: &mut HashMap<String, String>,
    seen: &mut HashSet<PathBuf>,
) -> Result<(), Diagnostic> {
    let (import_path, source) = read_import_source(source_path, import, sdk)?;
    let normalized = normalize_path(&import_path);
    if !seen.insert(normalized) {
        return Ok(());
    }
    let imported = parse_program(&import_path, &source)?;
    let short_module = import.rsplit('.').next().unwrap_or(import);
    for declaration in &imported.declarations {
        let Some(name) = declaration_name(declaration) else {
            continue;
        };
        if !declaration_is_public(declaration) {
            private_imports.insert(name.to_owned(), import.to_owned());
            private_imports.insert(format!("{short_module}.{name}"), import.to_owned());
            if short_module != import {
                private_imports.insert(format!("{import}.{name}"), import.to_owned());
            }
        }
    }
    for declaration in &imported.declarations {
        let Declaration::Import(nested) = declaration else {
            continue;
        };
        collect_private_imports(&import_path, nested, sdk, private_imports, seen)?;
    }

    Ok(())
}

fn validate_declaration_private_import_access(
    declaration: &Declaration,
    private_imports: &HashMap<String, String>,
) -> Result<(), Diagnostic> {
    match declaration {
        Declaration::Cfg { declaration, .. } => {
            validate_declaration_private_import_access(declaration, private_imports)
        }
        Declaration::Const(decl) => {
            validate_type_private_import_access(&decl.ty, private_imports)?;
            validate_expr_private_import_access(&decl.value, private_imports, &HashSet::new())
        }
        Declaration::Alias(decl) => validate_type_private_import_access(&decl.ty, private_imports),
        Declaration::Port(decl) => {
            validate_type_private_import_access(&decl.ty, private_imports)?;
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
        Place::Field { base, field } => {
            reject_private_import_name(&format!("{base}.{field}"), private_imports, locals)?;
            reject_private_import_name(base, private_imports, locals)
        }
        Place::Access(path) => validate_access_private_import_access(path, private_imports, locals),
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
        Expr::Field { base, field } | Expr::AddressOfField { base, field } => {
            reject_private_import_name(&format!("{base}.{field}"), private_imports, locals)?;
            reject_private_import_name(base, private_imports, locals)
        }
        Expr::Access(path) | Expr::AddressOfAccess(path) => {
            validate_access_private_import_access(path, private_imports, locals)
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
        Expr::TypedInt(_, ty) => validate_type_private_import_access(ty, private_imports),
        Expr::Int(_) | Expr::Char(_) | Expr::Bool(_) | Expr::String(_) => Ok(()),
    }
}

fn validate_access_private_import_access(
    path: &AccessPath,
    private_imports: &HashMap<String, String>,
    locals: &HashSet<String>,
) -> Result<(), Diagnostic> {
    reject_private_import_name(&path.root, private_imports, locals)?;
    for segment in &path.segments {
        if let AccessSegment::Index(index) = segment {
            validate_expr_private_import_access(index, private_imports, locals)?;
        }
    }
    Ok(())
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
            validate_expr_private_import_access(len, private_imports, &HashSet::new())
        }
    }
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
        Declaration::Cfg { declaration, .. } => declaration_name(declaration),
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
        Declaration::Cfg { declaration, .. } => declaration_is_public(declaration),
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
            default_sdk_symbols: true,
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
            default_sdk_symbols: true,
        };

        let error = check_source("const X: u8 = 1\n", &options).unwrap_err();

        assert_eq!(error.message, "missing required `fn main()`");
        assert_eq!(
            error.location(),
            Some(source_start_location(&options.source))
        );
    }

    #[test]
    fn rejects_invalid_main_signatures() {
        let options = CompileOptions {
            source: PathBuf::from("game.ezra"),
            debug_comments: false,
            default_sdk_symbols: true,
        };

        let with_param = check_source("fn main(code: u8) {}\n", &options).unwrap_err();
        let with_return = check_source("fn main() -> u8 { return 0 }\n", &options).unwrap_err();

        assert_eq!(with_param.message, "main function cannot take parameters");
        assert_eq!(with_return.message, "main function cannot return a value");
    }

    #[test]
    fn check_rejects_semantic_errors_in_function_bodies() {
        let options = CompileOptions {
            source: PathBuf::from("game.ezra"),
            debug_comments: false,
            default_sdk_symbols: true,
        };

        let mismatch = check_source("fn main() { let x: u8 = 0x0100 }\n", &options).unwrap_err();
        let bad_call =
            check_source("fn helper() { missing() }\nfn main() {}\n", &options).unwrap_err();

        assert_eq!(mismatch.message, "value 256 is outside u8 range");
        assert_eq!(
            mismatch.location(),
            Some(SourceLocation {
                file: options.source.clone(),
                line: 1,
                column: 25,
            })
        );
        assert_eq!(mismatch.span.as_ref().unwrap().end.column, 31);
        assert_eq!(bad_call.message, "unknown function `missing`");
        assert_eq!(
            bad_call.location(),
            Some(SourceLocation {
                file: options.source.clone(),
                line: 1,
                column: 15,
            })
        );
        assert_eq!(bad_call.span.as_ref().unwrap().end.column, 22);
    }

    #[test]
    fn check_collects_multiple_reference_diagnostics_with_distinct_spans() {
        let options = CompileOptions {
            source: PathBuf::from("multi-error.ezra"),
            debug_comments: false,
            default_sdk_symbols: true,
        };
        let source = "fn main() {\n    missing_one()\n    missing_two()\n}\n";

        let diagnostics = check_source_diagnostics(source, &options);

        assert_eq!(diagnostics.len(), 2, "{diagnostics:#?}");
        assert_eq!(diagnostics[0].message, "unknown function `missing_one`");
        assert_eq!(diagnostics[1].message, "unknown function `missing_two`");
        let first = diagnostics[0].span.as_ref().unwrap();
        let second = diagnostics[1].span.as_ref().unwrap();
        assert_eq!((first.start.line, first.start.column), (2, 5));
        assert_eq!((first.end.line, first.end.column), (2, 16));
        assert_eq!((second.start.line, second.start.column), (3, 5));
        assert_eq!((second.end.line, second.end.column), (3, 16));
    }

    #[test]
    fn check_collects_independent_body_diagnostics_with_statement_spans() {
        let options = CompileOptions {
            source: PathBuf::from("body-errors.ezra"),
            debug_comments: false,
            default_sdk_symbols: true,
        };
        let source = "fn helper() { test.pass(1) }\nfn main() {\n    let value: u8 = true\n}\n";

        let diagnostics = check_source_diagnostics(source, &options);

        let arity = diagnostics
            .iter()
            .find(|diagnostic| diagnostic.message == "test.pass requires no arguments")
            .unwrap();
        assert_eq!(arity.span.as_ref().unwrap().start.line, 1);
        let type_error = diagnostics
            .iter()
            .find(|diagnostic| diagnostic.message.contains("type mismatch"))
            .unwrap();
        assert_eq!(type_error.span.as_ref().unwrap().start.line, 3);
        assert!(diagnostics.len() >= 2, "{diagnostics:#?}");
    }

    #[test]
    fn multi_diagnostics_resolve_qualified_imported_values() {
        let options = CompileOptions {
            source: PathBuf::from("qualified.ezra"),
            debug_comments: false,
            default_sdk_symbols: true,
        };
        let sdk = SdkResolver {
            target: Some("agonlight-mos-ez80".to_owned()),
            sdk_roots: Vec::new(),
        };
        let source =
            "import agon.vdp\nfn main() { let color: u8 = vdp.COLOR_GREEN; test.pass() }\n";

        let diagnostics = check_source_diagnostics_with_sdk(source, &options, &sdk);

        assert!(diagnostics.is_empty(), "{diagnostics:#?}");
    }

    #[test]
    fn diagnostics_use_unsaved_import_source_overrides() {
        let root = temp_root("source_overrides");
        std::fs::create_dir_all(root.join("lib")).unwrap();
        let main_path = root.join("main.ezra");
        let import_path = root.join("lib/math.ezra");
        let source = "import lib.math\nfn main() { lib.math.increment(1) }\n";
        std::fs::write(&main_path, source).unwrap();
        std::fs::write(
            &import_path,
            "pub fn increment(value: u8) -> u8 { return value + 1 }\n",
        )
        .unwrap();
        let overrides = HashMap::from([(import_path.clone(), "pub fn increment(".to_owned())]);

        let diagnostics = check_source_diagnostics_with_sdk_and_overrides(
            source,
            &CompileOptions {
                source: main_path,
                debug_comments: false,
                default_sdk_symbols: true,
            },
            &SdkResolver::default(),
            &overrides,
        );

        assert_eq!(diagnostics.len(), 1);
        assert_eq!(diagnostics[0].span.as_ref().unwrap().file, import_path);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn semantic_diagnostics_preserve_imported_module_provenance() {
        let root = temp_root("import_diagnostic_provenance");
        std::fs::create_dir_all(root.join("lib")).unwrap();
        let main_path = root.join("main.ezra");
        let import_path = root.join("lib/broken.ezra");
        let source = "import lib.broken\nfn main() {}\n";
        std::fs::write(&main_path, source).unwrap();
        std::fs::write(
            &import_path,
            "pub fn helper() {\n    missing_one()\n    missing_two()\n}\n",
        )
        .unwrap();

        let diagnostics = check_source_diagnostics(
            source,
            &CompileOptions {
                source: main_path,
                debug_comments: false,
                default_sdk_symbols: true,
            },
        );

        let imported = diagnostics
            .iter()
            .filter(|diagnostic| {
                diagnostic
                    .span
                    .as_ref()
                    .is_some_and(|span| span.file == import_path)
            })
            .collect::<Vec<_>>();
        assert_eq!(imported.len(), 2, "{diagnostics:#?}");
        assert_eq!(imported[0].message, "unknown function `missing_one`");
        assert_eq!(imported[1].message, "unknown function `missing_two`");
        assert_eq!(imported[0].span.as_ref().unwrap().start.line, 2);
        assert_eq!(imported[1].span.as_ref().unwrap().start.line, 3);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn required_diagnostics_report_messages_and_locations() {
        let cases = [
            (
                "type mismatch",
                "fn main() { let ordered: bool = false < true }\n",
                "type mismatch",
            ),
            (
                "unknown identifier",
                "fn main() { missing() }\n",
                "unknown function `missing`",
            ),
            (
                "duplicate declaration",
                "const VALUE: u8 = 1\nglobal VALUE: u8 = 2\nfn main() {}\n",
                "duplicate declaration `VALUE`",
            ),
            (
                "invalid cast",
                r#"
                fn main() {
                    let raw: u16 = 0x1234
                    let p: ptr<u8> = cast<ptr<u8>>(raw)
                }
                "#,
                "integer-to-pointer casts require u24 or ptr24",
            ),
            (
                "pointer arithmetic on non-pointers",
                r#"
                global left: u8 = 0
                global right: u8 = 0
                fn main() {
                    let lp: ptr<u8> = &left
                    let rp: ptr<u8> = &right
                    let bad: ptr<u8> = lp + rp
                }
                "#,
                "pointer arithmetic requires exactly one pointer operand",
            ),
            (
                "array index out of bounds",
                r#"
                global bytes: [u8; 2] = [1, 2]
                fn main() { let value: u8 = bytes[2] }
                "#,
                "array index 2 is out of bounds for `bytes` length 2",
            ),
            (
                "struct field does not exist",
                r#"
                struct Entity { x: u8 }
                global player: Entity = Entity { x: 1 }
                fn main() { let value: u8 = player.y }
                "#,
                "struct `Entity` has no field `y`",
            ),
            (
                "inline asm output type mismatch",
                r#"
                fn main() {
                    let result: u8 = 0
                    asm volatile(out result: u16 as reg16, clobber hl) {
                        "ld hl, 000007h"
                    }
                }
                "#,
                "inline asm output `result` declared type `u16` does not match bound type `u8`",
            ),
            (
                "inline asm undeclared clobber",
                r#"
                fn main() {
                    asm(clobber made_up) {
                        "nop"
                    }
                }
                "#,
                "unknown inline asm clobber `made_up`",
            ),
        ];

        for (label, source, expected) in cases {
            let options = CompileOptions {
                source: PathBuf::from(format!("{label}.ezra")),
                debug_comments: false,
                default_sdk_symbols: true,
            };
            let error = match check_source(source, &options) {
                Ok(_) => panic!("{label}: expected diagnostic"),
                Err(error) => error,
            };

            assert_eq!(error.message, expected, "{label}");
            assert!(error.location().is_some(), "{label}: {error:?}");
        }
    }

    #[test]
    fn cfg_filters_declarations_before_semantic_checks() {
        let source = r#"
            @cfg(cpu("z80"))
            fn main() {
                missing_on_inactive_target()
            }

            @cfg(cpu("ez80"))
            fn main() {
                test.pass()
            }
        "#;
        let sdk = SdkResolver {
            target: Some("ti84plusce-ez80".to_owned()),
            sdk_roots: Vec::new(),
        };
        let program =
            parse_and_resolve_imports_with_sdk(Path::new("game.ezra"), source, &sdk).unwrap();

        assert_eq!(program.declarations.len(), 1);
        assert_eq!(program.main_function().unwrap().body.len(), 1);
        emit_ez80_assembly_with_options(
            &program,
            AssemblyOptions {
                default_sdk_symbols: true,
                ..AssemblyOptions::default()
            },
        )
        .unwrap();
    }

    #[test]
    fn cfg_skips_inactive_imports_before_file_loading() {
        let source = r#"
            @cfg(cpu("z80"))
            import missing.module

            fn main() {
                test.pass()
            }
        "#;
        let sdk = SdkResolver {
            target: Some("ti84plusce-ez80".to_owned()),
            sdk_roots: Vec::new(),
        };
        let program =
            parse_and_resolve_imports_with_sdk(Path::new("game.ezra"), source, &sdk).unwrap();

        assert!(program.main_function().is_some());
    }

    #[test]
    fn cfg_evaluates_target_predicates_and_multiple_attributes() {
        let source = r#"
            @cfg(all(target("agonlight-mos-ez80"), target_family("agonlight"), cpu("ez80")))
            @cfg(all(os("mos"), pointer_width(24), address_width(24), feature("mos")))
            const ACTIVE: u8 = 1

            @cfg(any(cpu("z80"), not(target("agonlight-mos-ez80"))))
            const INACTIVE: u8 = missing_symbol

            fn main() {
                test.pass()
            }
        "#;
        let sdk = SdkResolver {
            target: Some("agonlight-mos-ez80".to_owned()),
            sdk_roots: Vec::new(),
        };
        let program =
            parse_and_resolve_imports_with_sdk(Path::new("game.ezra"), source, &sdk).unwrap();

        assert!(
            program
                .declarations
                .iter()
                .any(|decl| { matches!(decl, Declaration::Const(decl) if decl.name == "ACTIVE") })
        );
        assert!(
            !program.declarations.iter().any(|decl| {
                matches!(decl, Declaration::Const(decl) if decl.name == "INACTIVE")
            })
        );
    }

    #[test]
    fn cfg_filters_imported_declarations_and_aliases() {
        let root = temp_root("cfg_imports");
        std::fs::create_dir_all(root.join("lib")).unwrap();
        let main_path = root.join("game.ezra");
        let lib_path = root.join("lib/utils.ezra");
        std::fs::write(
            &lib_path,
            r#"
                @cfg(cpu("z80"))
                pub fn value() -> u8 { return missing_symbol }

                @cfg(cpu("ez80"))
                pub fn value() -> u8 { return 7 }
            "#,
        )
        .unwrap();
        std::fs::write(
            &main_path,
            r#"
                import lib.utils

                fn main() {
                    test.assert_eq_u8(utils.value(), 7, 1)
                    test.pass()
                }
            "#,
        )
        .unwrap();
        let sdk = SdkResolver {
            target: Some("ti84plusce-ez80".to_owned()),
            sdk_roots: Vec::new(),
        };
        let program = load_program_with_sdk(&main_path, &sdk).unwrap();

        assert_eq!(
            program
                .declarations
                .iter()
                .filter(|decl| matches!(decl, Declaration::Function(function) if function.name == "value"))
                .count(),
            1
        );
        assert!(program.declarations.iter().any(|decl| {
            matches!(decl, Declaration::Function(function) if function.name == "utils.value")
        }));

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn cfg_rejects_unknown_predicates_and_features() {
        let unknown_predicate = parse_program(
            Path::new("game.ezra"),
            r#"
                @cfg(board("agon"))
                fn main() {}
            "#,
        )
        .unwrap_err();
        assert_eq!(unknown_predicate.message, "unknown cfg predicate `board`");

        let unknown_feature = parse_and_resolve_imports_with_sdk(
            Path::new("game.ezra"),
            r#"
                @cfg(feature("sprites"))
                fn main() {}
            "#,
            &SdkResolver {
                target: Some("agonlight-mos-ez80".to_owned()),
                sdk_roots: Vec::new(),
            },
        )
        .unwrap_err();
        assert_eq!(unknown_feature.message, "unknown cfg feature `sprites`");
    }

    #[test]
    fn cpm_z80_target_uses_builtin_bdos_sdk() {
        let source = r#"
            import cpm.bdos

            fn main() {
                bdos.console_output(65)
                bdos.exit()
            }
        "#;
        let sdk = SdkResolver {
            target: Some("cpm-2.2-z80".to_owned()),
            sdk_roots: Vec::new(),
        };
        let program =
            parse_and_resolve_imports_with_sdk(Path::new("game.ezra"), source, &sdk).unwrap();

        assert!(program.declarations.iter().any(|decl| {
            matches!(decl, Declaration::Const(decl) if decl.name == "bdos.CONSOLE_OUTPUT")
        }));
        assert!(program.declarations.iter().any(|decl| {
            matches!(decl, Declaration::Function(function) if function.name == "bdos.console_output")
        }));
        assert!(program.declarations.iter().any(|decl| {
            matches!(decl, Declaration::Function(function) if function.name == "cpm.bdos.exit")
        }));
    }

    #[test]
    fn cpm_z80_target_uses_builtin_console_sdk() {
        let source = r#"
            import cpm.console

            fn main() {
                console.write(65)
                console.newline()
                console.exit()
            }
        "#;
        let sdk = SdkResolver {
            target: Some("cpm-2.2-z80".to_owned()),
            sdk_roots: Vec::new(),
        };
        let program =
            parse_and_resolve_imports_with_sdk(Path::new("game.ezra"), source, &sdk).unwrap();

        assert!(program.declarations.iter().any(|decl| {
            matches!(decl, Declaration::Function(function) if function.name == "console.write")
        }));
        assert!(program.declarations.iter().any(|decl| {
            matches!(decl, Declaration::Function(function) if function.name == "cpm.console.exit")
        }));
        assert!(program.declarations.iter().any(|decl| {
            matches!(decl, Declaration::Function(function) if function.name == "console.key_available")
        }));
    }

    #[test]
    fn cpm_z80_target_uses_builtin_fcb_and_dma_sdks() {
        let source = r#"
            import cpm.dma
            import cpm.fcb

            global file_control_block: [u8; 36] = [0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]

            fn main() {
                fcb.init(&file_control_block[0], fcb.DRIVE_DEFAULT)
                fcb.set_name_char(&file_control_block[0], 0, 'R')
                dma.reset_default()
            }
        "#;
        let sdk = SdkResolver {
            target: Some("cpm-2.2-z80".to_owned()),
            sdk_roots: Vec::new(),
        };
        let program =
            parse_and_resolve_imports_with_sdk(Path::new("game.ezra"), source, &sdk).unwrap();

        assert!(program.declarations.iter().any(|decl| {
            matches!(decl, Declaration::Function(function) if function.name == "fcb.init")
        }));
        assert!(program.declarations.iter().any(|decl| {
            matches!(decl, Declaration::Function(function) if function.name == "dma.reset_default")
        }));
        assert!(program.declarations.iter().any(|decl| {
            matches!(decl, Declaration::Const(decl) if decl.name == "cpm.fcb.DRIVE_DEFAULT")
        }));
    }

    #[test]
    fn zxspectrum_target_uses_builtin_zx_sdk() {
        let source = r#"
            import zx.rom
            import zx.screen

            fn main() {
                rom.print_char(65)
                screen.border(1)
            }
        "#;
        let sdk = SdkResolver {
            target: Some("zxspectrum-z80".to_owned()),
            sdk_roots: Vec::new(),
        };
        let program =
            parse_and_resolve_imports_with_sdk(Path::new("game.ezra"), source, &sdk).unwrap();

        assert!(program.declarations.iter().any(|decl| {
            matches!(decl, Declaration::Function(function) if function.name == "rom.print_char")
        }));
        assert!(program.declarations.iter().any(|decl| {
            matches!(decl, Declaration::Function(function) if function.name == "screen.border")
        }));
    }

    #[test]
    fn ti_ce_targets_use_builtin_tice_sdk() {
        let source = r#"
            import tice.os
            import tice.lcd

            fn main() {
                lcd.set_first_pixel(3)
                let key: u8 = os.wait_key()
            }
        "#;

        for target in ["ti84plusce-ez80", "ti83premiumce-ez80"] {
            let sdk = SdkResolver {
                target: Some(target.to_owned()),
                sdk_roots: Vec::new(),
            };
            let program =
                parse_and_resolve_imports_with_sdk(Path::new("game.ezra"), source, &sdk).unwrap();

            assert!(program.declarations.iter().any(|decl| {
                matches!(decl, Declaration::Function(function) if function.name == "lcd.set_first_pixel")
            }));
            assert!(program.declarations.iter().any(|decl| {
                matches!(decl, Declaration::Function(function) if function.name == "os.wait_key")
            }));
        }
    }

    #[test]
    fn ti_z80_targets_use_builtin_ti_sdk() {
        let source = r#"
            import ti.os
            import ti.lcd

            fn main() {
                lcd.set_first_byte(3)
                let value: u8 = os.zero()
            }
        "#;

        for target in ["ti83-z80", "ti83plus-z80", "ti84-z80", "ti84plus-z80"] {
            let sdk = SdkResolver {
                target: Some(target.to_owned()),
                sdk_roots: Vec::new(),
            };
            let program =
                parse_and_resolve_imports_with_sdk(Path::new("game.ezra"), source, &sdk).unwrap();

            assert!(program.declarations.iter().any(|decl| {
                matches!(decl, Declaration::Function(function) if function.name == "lcd.set_first_byte")
            }));
            assert!(program.declarations.iter().any(|decl| {
                matches!(decl, Declaration::Function(function) if function.name == "os.zero")
            }));
        }
    }

    #[test]
    fn cpm_8080_target_uses_builtin_console_sdk() {
        let source = r#"
            import cpm.console

            fn main() {
                console.write(65)
                console.exit()
            }
        "#;
        let sdk = SdkResolver {
            target: Some("cpm-2.2-i8080".to_owned()),
            sdk_roots: Vec::new(),
        };
        let program =
            parse_and_resolve_imports_with_sdk(Path::new("game.ezra"), source, &sdk).unwrap();

        assert!(program.declarations.iter().any(|decl| {
            matches!(decl, Declaration::Function(function) if function.name == "console.write")
        }));
        assert!(program.declarations.iter().any(|decl| {
            matches!(decl, Declaration::Function(function) if function.name == "cpm.console.exit")
        }));
        assert!(program.declarations.iter().any(|decl| {
            matches!(decl, Declaration::Function(function) if function.name == "console.key_available")
        }));
    }

    #[test]
    fn cpm_8085_target_uses_builtin_console_sdk() {
        let source = r#"
            import cpm.console

            fn main() {
                console.write(65)
                console.exit()
            }
        "#;
        let sdk = SdkResolver {
            target: Some("cpm-2.2-i8085".to_owned()),
            sdk_roots: Vec::new(),
        };
        let program =
            parse_and_resolve_imports_with_sdk(Path::new("game.ezra"), source, &sdk).unwrap();

        assert!(program.declarations.iter().any(|decl| {
            matches!(decl, Declaration::Function(function) if function.name == "console.write")
        }));
        assert!(program.declarations.iter().any(|decl| {
            matches!(decl, Declaration::Function(function) if function.name == "cpm.console.exit")
        }));
        assert!(program.declarations.iter().any(|decl| {
            matches!(decl, Declaration::Function(function) if function.name == "console.key_available")
        }));
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
            default_sdk_symbols: true,
        };
        let report = check_source(source, &options).unwrap();
        let program = load_program(&main_path).unwrap();

        assert_eq!(report.imports, 1);
        assert_eq!(report.declarations, 4);
        assert!(program.declarations.iter().any(|decl| {
            matches!(decl, Declaration::Function(function) if function.name == "add_one")
        }));
        assert!(program.declarations.iter().any(|decl| {
            matches!(decl, Declaration::Function(function) if function.name == "math.add_one")
        }));
        assert!(program.declarations.iter().any(|decl| {
            matches!(decl, Declaration::Function(function) if function.name == "lib.math.add_one")
        }));

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn resolves_imports_from_project_root_ancestor() {
        let root = temp_root("project_root_imports");
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::create_dir_all(root.join("sdk")).unwrap();
        let main_path = root.join("src/game.ezra");
        std::fs::write(
            root.join("sdk/input.ezra"),
            "pub const VALUE: u8 = 0x2A\npub fn read() -> u8 { return VALUE }\n",
        )
        .unwrap();
        let source = "import sdk.input\nfn main() { let x: u8 = input.read(); test.pass() }\n";
        std::fs::write(&main_path, source).unwrap();

        let options = CompileOptions {
            source: main_path.clone(),
            debug_comments: false,
            default_sdk_symbols: true,
        };
        let report = check_source(source, &options).unwrap();
        let program = load_program(&main_path).unwrap();

        assert_eq!(report.imports, 1);
        assert!(program.declarations.iter().any(|decl| {
            matches!(decl, Declaration::Const(decl) if decl.name == "input.VALUE")
        }));
        assert!(program.declarations.iter().any(|decl| {
            matches!(decl, Declaration::Function(function) if function.name == "input.read")
        }));

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn full_import_paths_disambiguate_colliding_short_module_names() {
        let root = temp_root("colliding_short_modules");
        std::fs::create_dir_all(root.join("lib")).unwrap();
        std::fs::create_dir_all(root.join("sdk")).unwrap();
        let main_path = root.join("game.ezra");
        std::fs::write(
            root.join("lib/math.ezra"),
            "pub fn add(v: u8) -> u8 { return v + 1 }\n",
        )
        .unwrap();
        std::fs::write(
            root.join("sdk/math.ezra"),
            "pub fn sub(v: u8) -> u8 { return v - 1 }\n",
        )
        .unwrap();
        let source = r#"
            import lib.math
            import sdk.math
            fn main() {
                let a: u8 = lib.math.add(4)
                let b: u8 = sdk.math.sub(4)
                test.pass()
            }
        "#;
        std::fs::write(&main_path, source).unwrap();

        let options = CompileOptions {
            source: main_path.clone(),
            debug_comments: false,
            default_sdk_symbols: true,
        };
        let report = check_source(source, &options).unwrap();
        let program = load_program(&main_path).unwrap();

        assert_eq!(report.imports, 2);
        assert!(program.declarations.iter().any(|decl| {
            matches!(decl, Declaration::Function(function) if function.name == "lib.math.add")
        }));
        assert!(program.declarations.iter().any(|decl| {
            matches!(decl, Declaration::Function(function) if function.name == "sdk.math.sub")
        }));
        assert!(!program.declarations.iter().any(|decl| {
            matches!(decl, Declaration::Function(function) if function.name == "math.add")
        }));
        assert!(!program.declarations.iter().any(|decl| {
            matches!(decl, Declaration::Function(function) if function.name == "math.sub")
        }));

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn duplicate_imported_declarations_report_the_conflicting_module() {
        let root = temp_root("duplicate_imported_declarations");
        std::fs::create_dir_all(root.join("lib")).unwrap();
        let main_path = root.join("game.ezra");
        let second_path = root.join("lib/second.ezra");
        std::fs::write(root.join("lib/first.ezra"), "pub const VALUE: u8 = 1\n").unwrap();
        std::fs::write(&second_path, "pub const VALUE: u8 = 2\n").unwrap();
        let source = "import lib.first\nimport lib.second\nfn main() {}\n";
        std::fs::write(&main_path, source).unwrap();

        let error = load_program(&main_path).unwrap_err();

        assert_eq!(error.message, "duplicate imported declaration `VALUE`");
        assert_eq!(
            error.location(),
            Some(SourceLocation {
                file: second_path,
                line: 1,
                column: 11,
            })
        );

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn rejects_ambiguous_short_module_aliases() {
        let root = temp_root("ambiguous_short_modules");
        std::fs::create_dir_all(root.join("lib")).unwrap();
        std::fs::create_dir_all(root.join("sdk")).unwrap();
        let main_path = root.join("game.ezra");
        std::fs::write(
            root.join("lib/math.ezra"),
            "pub fn add(v: u8) -> u8 { return v + 1 }\n",
        )
        .unwrap();
        std::fs::write(
            root.join("sdk/math.ezra"),
            "pub fn sub(v: u8) -> u8 { return v - 1 }\n",
        )
        .unwrap();
        let source = r#"
            import lib.math
            import sdk.math
            fn main() {
                let a: u8 = math.add(4)
                test.pass()
            }
        "#;
        std::fs::write(&main_path, source).unwrap();

        let options = CompileOptions {
            source: main_path,
            debug_comments: false,
            default_sdk_symbols: true,
        };
        let error = check_source(source, &options).unwrap_err();

        assert_eq!(error.message, "unknown function `math.add`");

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn root_source_must_define_main_entry() {
        let root = temp_root("root_main");
        std::fs::create_dir_all(root.join("lib")).unwrap();
        let main_path = root.join("game.ezra");
        let lib_path = root.join("lib/app.ezra");
        std::fs::write(&lib_path, "fn main() { test.fail(1) }\n").unwrap();
        let source = "import lib.app\n";
        std::fs::write(&main_path, source).unwrap();

        let options = CompileOptions {
            source: main_path.clone(),
            debug_comments: false,
            default_sdk_symbols: true,
        };
        let error = check_source(source, &options).unwrap_err();

        assert_eq!(error.message, "missing required `fn main()`");

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn imported_main_does_not_conflict_with_root_main() {
        let root = temp_root("imported_main");
        std::fs::create_dir_all(root.join("lib")).unwrap();
        let main_path = root.join("game.ezra");
        let lib_path = root.join("lib/app.ezra");
        std::fs::write(&lib_path, "fn main() { test.fail(1) }\n").unwrap();
        std::fs::write(&main_path, "import lib.app\nfn main() { test.pass() }\n").unwrap();

        let program = load_program(&main_path).unwrap();
        let main_count = program
            .declarations
            .iter()
            .filter(|declaration| is_entry_function(declaration))
            .count();

        assert_eq!(main_count, 1);

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

        std::fs::write(
            &lib_path,
            "global secret: u8 = 7\npub fn shown(v: u8) -> u8 { return v }\n",
        )
        .unwrap();
        std::fs::write(
            &main_path,
            "import lib.math\nfn main() { let x: u8 = math.secret; test.pass() }\n",
        )
        .unwrap();

        let error = load_program(&main_path).unwrap_err();

        assert_eq!(
            error.message,
            "declaration `math.secret` from import `lib.math` is private"
        );

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn rejects_access_to_transitive_private_imported_declarations() {
        let root = temp_root("transitive_private_imports");
        std::fs::create_dir_all(root.join("lib")).unwrap();
        let main_path = root.join("game.ezra");
        let api_path = root.join("lib/api.ezra");
        let secret_path = root.join("lib/secret.ezra");
        std::fs::write(
            &api_path,
            "import secret\npub fn shown(v: u8) -> u8 { return v }\n",
        )
        .unwrap();
        std::fs::write(
            &secret_path,
            "fn hidden(v: u8) -> u8 { return v + 1 }\nglobal secret: u8 = 7\n",
        )
        .unwrap();
        std::fs::write(
            &main_path,
            "import lib.api\nfn main() { let x: u8 = hidden(4); test.pass() }\n",
        )
        .unwrap();

        let error = load_program(&main_path).unwrap_err();

        assert_eq!(
            error.message,
            "declaration `hidden` from import `secret` is private"
        );

        std::fs::write(
            &main_path,
            "import lib.api\nfn main() { let x: u8 = secret.secret; test.pass() }\n",
        )
        .unwrap();

        let error = load_program(&main_path).unwrap_err();

        assert_eq!(
            error.message,
            "declaration `secret.secret` from import `secret` is private"
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

        std::fs::write(
            &main_path,
            "import lib.types\nfn main() { let x: types.Secret = types.Secret { value: 1 }; test.pass() }\n",
        )
        .unwrap();

        let error = load_program(&main_path).unwrap_err();

        assert_eq!(
            error.message,
            "declaration `types.Secret` from import `lib.types` is private"
        );

        std::fs::write(
            &main_path,
            "import lib.types\nfn main() { let x: lib.types.Secret = lib.types.Secret { value: 1 }; test.pass() }\n",
        )
        .unwrap();

        let error = load_program(&main_path).unwrap_err();

        assert_eq!(
            error.message,
            "declaration `lib.types.Secret` from import `lib.types` is private"
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
