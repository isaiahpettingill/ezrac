use std::{
    collections::{BTreeMap, BTreeSet},
    io::{self, BufRead, BufReader, Write},
    path::{Path, PathBuf},
};

use ezra::{
    ast::{Declaration, Expr, Function, Stmt, Type},
    compile::{
        CompileOptions, SdkResolver, builtin_sdk_modules, check_source_diagnostics_with_sdk,
        parse_and_resolve_imports_with_sdk,
    },
    diagnostic::{Diagnostic, SourcePosition, SourceSpan},
    parser::parse_program,
    project::load_nearest_project_config,
    target::DEFAULT_TARGET_TRIPLE,
};
use lsp::notification::{Notification, TextDocumentPublishDiagnostics};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

#[derive(Default)]
struct Server {
    documents: BTreeMap<String, OpenDocument>,
    shutdown_requested: bool,
}

struct OpenDocument {
    path: PathBuf,
    text: String,
    version: Option<i32>,
}

#[derive(Clone)]
struct SymbolInfo {
    label: String,
    kind: u8,
    detail: String,
}

#[derive(Default)]
struct SymbolIndex {
    symbols: BTreeMap<String, SymbolInfo>,
    modules: BTreeSet<String>,
}

#[derive(Deserialize)]
struct Message {
    id: Option<Value>,
    method: String,
    #[serde(default)]
    params: Value,
}

#[derive(Deserialize)]
struct DidOpenParams {
    #[serde(rename = "textDocument")]
    text_document: TextDocumentItem,
}

#[derive(Deserialize)]
struct TextDocumentItem {
    uri: String,
    text: String,
    version: i32,
}

#[derive(Deserialize)]
struct DidChangeParams {
    #[serde(rename = "textDocument")]
    text_document: VersionedTextDocumentIdentifier,
    #[serde(rename = "contentChanges")]
    content_changes: Vec<TextDocumentContentChangeEvent>,
}

#[derive(Deserialize)]
struct VersionedTextDocumentIdentifier {
    uri: String,
    version: Option<i32>,
}

#[derive(Deserialize)]
struct TextDocumentContentChangeEvent {
    text: String,
}

#[derive(Deserialize)]
struct DidCloseParams {
    #[serde(rename = "textDocument")]
    text_document: TextDocumentIdentifier,
}

#[derive(Deserialize)]
struct CompletionParams {
    #[serde(rename = "textDocument")]
    text_document: TextDocumentIdentifier,
    position: Position,
}

#[derive(Deserialize)]
struct HoverParams {
    #[serde(rename = "textDocument")]
    text_document: TextDocumentIdentifier,
    position: Position,
}

#[derive(Deserialize)]
struct TextDocumentIdentifier {
    uri: String,
}

#[derive(Serialize)]
struct LspDiagnostic {
    range: Range,
    severity: u8,
    source: &'static str,
    message: String,
}

#[derive(Clone, Copy, Serialize)]
struct Range {
    start: Position,
    end: Position,
}

#[derive(Clone, Copy, Deserialize, Serialize)]
struct Position {
    line: u32,
    character: u32,
}

pub fn run() -> Result<(), String> {
    Server::default().run()
}

impl Server {
    fn run(&mut self) -> Result<(), String> {
        let stdin = io::stdin();
        let mut input = BufReader::new(stdin.lock());
        let stdout = io::stdout();
        let mut output = stdout.lock();

        while let Some(raw) = read_message(&mut input)? {
            let message: Message = serde_json::from_str(&raw)
                .map_err(|error| format!("failed to parse LSP message: {error}"))?;
            if self.handle_message(message, &mut output)? {
                break;
            }
        }
        Ok(())
    }

    fn handle_message(
        &mut self,
        message: Message,
        output: &mut impl Write,
    ) -> Result<bool, String> {
        match message.method.as_str() {
            "initialize" => {
                if let Some(id) = message.id {
                    write_response(output, id, initialize_result())?;
                }
            }
            "shutdown" => {
                self.shutdown_requested = true;
                if let Some(id) = message.id {
                    write_response(output, id, Value::Null)?;
                }
            }
            "exit" => return Ok(true),
            "textDocument/didOpen" => self.did_open(message.params, output)?,
            "textDocument/didChange" => self.did_change(message.params, output)?,
            "textDocument/didClose" => self.did_close(message.params, output)?,
            "textDocument/completion" => {
                if let Some(id) = message.id {
                    let params: CompletionParams = serde_json::from_value(message.params)
                        .map_err(|error| format!("invalid completion params: {error}"))?;
                    let result = completion_items(
                        self.documents.get(&params.text_document.uri),
                        params.position,
                    );
                    write_response(output, id, result)?;
                }
            }
            "textDocument/hover" => {
                if let Some(id) = message.id {
                    let params: HoverParams = serde_json::from_value(message.params)
                        .map_err(|error| format!("invalid hover params: {error}"))?;
                    let result = hover(
                        self.documents.get(&params.text_document.uri),
                        params.position,
                    );
                    write_response(output, id, result)?;
                }
            }
            "textDocument/signatureHelp" => {
                if let Some(id) = message.id {
                    let params: CompletionParams = serde_json::from_value(message.params)
                        .map_err(|error| format!("invalid signature help params: {error}"))?;
                    let result = signature_help(
                        self.documents.get(&params.text_document.uri),
                        params.position,
                    );
                    write_response(output, id, result)?;
                }
            }
            _ => {
                if let Some(id) = message.id {
                    write_error(output, id, -32601, "method not found")?;
                }
            }
        }
        Ok(false)
    }

    fn did_open(&mut self, params: Value, output: &mut impl Write) -> Result<(), String> {
        let params: DidOpenParams = serde_json::from_value(params)
            .map_err(|error| format!("invalid didOpen params: {error}"))?;
        let uri = params.text_document.uri;
        let path = uri_to_path(&uri)?;
        self.documents.insert(
            uri.clone(),
            OpenDocument {
                path,
                text: params.text_document.text,
                version: Some(params.text_document.version),
            },
        );
        self.publish_diagnostics(&uri, output)
    }

    fn did_change(&mut self, params: Value, output: &mut impl Write) -> Result<(), String> {
        let params: DidChangeParams = serde_json::from_value(params)
            .map_err(|error| format!("invalid didChange params: {error}"))?;
        if let Some(document) = self.documents.get_mut(&params.text_document.uri)
            && let Some(change) = params.content_changes.into_iter().last()
        {
            document.text = change.text;
            document.version = params.text_document.version;
        }
        self.publish_diagnostics(&params.text_document.uri, output)
    }

    fn did_close(&mut self, params: Value, output: &mut impl Write) -> Result<(), String> {
        let params: DidCloseParams = serde_json::from_value(params)
            .map_err(|error| format!("invalid didClose params: {error}"))?;
        self.documents.remove(&params.text_document.uri);
        write_notification(
            output,
            TextDocumentPublishDiagnostics::METHOD,
            json!({ "uri": params.text_document.uri, "diagnostics": [] }),
        )
    }

    fn publish_diagnostics(&self, uri: &str, output: &mut impl Write) -> Result<(), String> {
        let Some(document) = self.documents.get(uri) else {
            return Ok(());
        };
        let diagnostics = check_document_diagnostics(document)
            .iter()
            .map(|diagnostic| diagnostic_to_lsp(document, diagnostic))
            .collect::<Vec<_>>();
        write_notification(
            output,
            TextDocumentPublishDiagnostics::METHOD,
            json!({
                "uri": uri,
                "version": document.version,
                "diagnostics": diagnostics,
            }),
        )
    }
}

fn check_document_diagnostics(document: &OpenDocument) -> Vec<Diagnostic> {
    let sdk = match sdk_for_path(&document.path) {
        Ok(sdk) => sdk,
        Err(error) => return vec![error],
    };
    check_source_diagnostics_with_sdk(
        &document.text,
        &CompileOptions {
            source: document.path.clone(),
            debug_comments: false,
            default_sdk_symbols: true,
        },
        &sdk,
    )
}

fn sdk_for_path(path: &Path) -> Result<SdkResolver, Diagnostic> {
    let project = load_nearest_project_config(path)?;
    Ok(SdkResolver {
        target: project
            .as_ref()
            .and_then(|project| project.target.clone())
            .or_else(|| Some(DEFAULT_TARGET_TRIPLE.to_owned())),
        sdk_roots: project
            .as_ref()
            .map(|project| project.sdk_paths.clone())
            .unwrap_or_default(),
    })
}

fn initialize_result() -> Value {
    json!({
        "capabilities": {
            "textDocumentSync": 1,
            "completionProvider": { "triggerCharacters": completion_trigger_characters() },
            "hoverProvider": true,
            "signatureHelpProvider": {
                "triggerCharacters": ["(", ","],
                "retriggerCharacters": [","]
            }
        },
        "serverInfo": { "name": "ezrac", "version": env!("CARGO_PKG_VERSION") }
    })
}

fn completion_items(document: Option<&OpenDocument>, position: Position) -> Value {
    let prefix = document
        .map(|document| completion_prefix(&document.text, position))
        .unwrap_or_default();
    let import_context =
        document.is_some_and(|document| is_import_completion(&document.text, position));
    let sdk = document.and_then(|document| sdk_for_path(&document.path).ok());
    let mut items = if import_context {
        import_completion_items(sdk.as_ref())
    } else {
        standard_completion_items()
    };
    if let Some(document) = document {
        let index = symbol_index(document);
        for module in &index.modules {
            items.push(completion_item(module, 9, "module"));
        }
        if !import_context {
            for symbol in index.symbols.values() {
                if should_show_symbol_completion(&symbol.label) {
                    items.push(json!({
                        "label": symbol.label,
                        "kind": symbol.kind,
                        "detail": symbol.detail,
                    }));
                }
            }
        }
    }
    let items = items
        .into_iter()
        .filter(|item| {
            prefix.is_empty()
                || item
                    .get("label")
                    .and_then(Value::as_str)
                    .is_some_and(|label| label.starts_with(&prefix))
        })
        .fold(BTreeMap::<String, Value>::new(), |mut items, item| {
            if let Some(label) = item.get("label").and_then(Value::as_str) {
                items.entry(label.to_owned()).or_insert(item);
            }
            items
        })
        .into_values()
        .map(|item| completion_text_edit(item, prefix.as_str(), position, import_context))
        .collect::<Vec<_>>();
    json!({ "isIncomplete": true, "items": items })
}

fn completion_text_edit(
    mut item: Value,
    prefix: &str,
    position: Position,
    import_context: bool,
) -> Value {
    let Some(label) = item.get("label").and_then(Value::as_str).map(str::to_owned) else {
        return item;
    };
    let new_text = if import_context {
        label.strip_prefix("import ").unwrap_or(&label)
    } else {
        &label
    };
    let start = Position {
        line: position.line,
        character: position.character.saturating_sub(utf16_len(prefix)),
    };
    if let Some(object) = item.as_object_mut() {
        object.insert(
            "textEdit".to_owned(),
            json!({
                "range": { "start": start, "end": position },
                "newText": new_text,
            }),
        );
    }
    item
}

fn is_import_completion(source: &str, position: Position) -> bool {
    let Some(line) = source.lines().nth(position.line as usize) else {
        return false;
    };
    let end = byte_index_for_character(line, position.character as usize);
    let before_cursor = line[..end].trim_start();
    before_cursor == "import" || before_cursor.starts_with("import ")
}

fn import_completion_items(sdk: Option<&SdkResolver>) -> Vec<Value> {
    available_modules(sdk)
        .into_iter()
        .flat_map(|module| {
            [
                completion_item(&format!("import {module}"), 15, "target SDK import"),
                completion_item(&module, 9, "module"),
            ]
        })
        .collect()
}

fn standard_completion_items() -> Vec<Value> {
    let mut items = Vec::new();
    for keyword in KEYWORDS {
        items.push(completion_item(keyword, 14, "keyword"));
    }
    for ty in PRIMITIVE_TYPES {
        items.push(completion_item(ty, 25, "primitive type"));
    }
    items
}

fn should_show_symbol_completion(_label: &str) -> bool {
    true
}

fn completion_item(label: &str, kind: u8, detail: &str) -> Value {
    json!({ "label": label, "kind": kind, "detail": detail })
}

fn hover(document: Option<&OpenDocument>, position: Position) -> Value {
    let Some(document) = document else {
        return Value::Null;
    };
    let Some(symbol) = symbol_at_position(&document.text, position) else {
        return Value::Null;
    };
    let index = symbol_index(document);
    if let Some(info) = index.symbols.get(&symbol) {
        return hover_markdown(&format!("```ezra\n{}\n```", info.detail));
    }
    if index.modules.contains(&symbol) {
        let members = module_members(&index, &symbol);
        let body = if members.is_empty() {
            format!("module `{symbol}`")
        } else {
            format!("module `{symbol}`\n\nMembers:\n{}", members.join("\n"))
        };
        return hover_markdown(&body);
    }
    Value::Null
}

fn hover_markdown(value: &str) -> Value {
    json!({ "contents": { "kind": "markdown", "value": value } })
}

fn signature_help(document: Option<&OpenDocument>, position: Position) -> Value {
    let Some(document) = document else {
        return Value::Null;
    };
    let Some((name, active_parameter)) = call_at_position(&document.text, position) else {
        return Value::Null;
    };
    let index = symbol_index(document);
    let Some(symbol) = index.symbols.get(&name) else {
        return Value::Null;
    };
    let Some(open) = symbol.detail.find('(') else {
        return Value::Null;
    };
    let Some(close) = symbol.detail.rfind(')') else {
        return Value::Null;
    };
    let params = symbol.detail[open + 1..close]
        .split(", ")
        .filter(|param| !param.is_empty())
        .map(|param| json!({ "label": param }))
        .collect::<Vec<_>>();
    json!({
        "signatures": [{
            "label": symbol.detail,
            "parameters": params,
        }],
        "activeSignature": 0,
        "activeParameter": active_parameter.min(params.len().saturating_sub(1)),
    })
}

fn call_at_position(source: &str, position: Position) -> Option<(String, usize)> {
    let line = source.lines().nth(position.line as usize)?;
    let end = byte_index_for_character(line, position.character as usize);
    let before = source
        .lines()
        .take(position.line as usize)
        .chain(std::iter::once(&line[..end]))
        .collect::<Vec<_>>()
        .join("\n");
    let mut calls = Vec::<(usize, usize)>::new();
    let mut quote = None;
    let mut escaped = false;
    let mut chars = before.char_indices().peekable();
    while let Some((index, ch)) = chars.next() {
        if let Some(end_quote) = quote {
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == end_quote {
                quote = None;
            }
            continue;
        }
        if ch == '/' && chars.peek().is_some_and(|(_, next)| *next == '/') {
            chars.next();
            while chars.next().is_some_and(|(_, next)| next != '\n') {}
            continue;
        }
        match ch {
            '\'' | '"' => quote = Some(ch),
            '(' => calls.push((index, 0)),
            ')' => {
                calls.pop();
            }
            ',' => {
                if let Some((_, active_parameter)) = calls.last_mut() {
                    *active_parameter += 1;
                }
            }
            _ => {}
        }
    }
    let (open, active_parameter) = calls.last().copied()?;
    let before_open = before[..open].trim_end();
    let end = before_open.len();
    let start = before_open
        .char_indices()
        .rev()
        .take_while(|(_, ch)| is_symbol_char(*ch))
        .last()
        .map(|(index, _)| index)
        .unwrap_or(end);
    (start < end).then(|| (before_open[start..end].to_owned(), active_parameter))
}

fn symbol_index(document: &OpenDocument) -> SymbolIndex {
    let sdk = sdk_for_path(&document.path).ok();
    let mut index = SymbolIndex::default();
    add_builtin_modules(&mut index, sdk.as_ref());
    match parse_program(&document.path, &document.text) {
        Ok(program) => add_program_symbols(&mut index, &program.declarations),
        Err(_) => add_recovery_symbols(&mut index, &document.text),
    }
    if let Some(sdk) = sdk.as_ref() {
        match parse_and_resolve_imports_with_sdk(&document.path, &document.text, sdk) {
            Ok(program) => add_program_symbols(&mut index, &program.declarations),
            Err(_) => add_recovery_import_symbols(&mut index, document, sdk),
        }
    }
    index
}

fn add_recovery_import_symbols(
    index: &mut SymbolIndex,
    document: &OpenDocument,
    sdk: &SdkResolver,
) {
    let imports = document
        .text
        .lines()
        .filter_map(|line| {
            let line = line.split("//").next()?.trim();
            let module = line.strip_prefix("import ")?.trim();
            let end = module
                .char_indices()
                .take_while(|(_, ch)| is_symbol_char(*ch))
                .last()
                .map(|(index, ch)| index + ch.len_utf8())?;
            Some(module[..end].to_owned())
        })
        .collect::<BTreeSet<_>>();
    for import in imports {
        let source = format!("import {import}\nfn main() {{}}\n");
        if let Ok(program) = parse_and_resolve_imports_with_sdk(&document.path, &source, sdk) {
            add_program_symbols(index, &program.declarations);
        }
    }
}

/// Keep completion useful while the user is in the middle of an edit that
/// makes the document temporarily unparsable. This deliberately captures only
/// simple declarations; the full parser remains the source of truth whenever
/// it succeeds.
fn add_recovery_symbols(index: &mut SymbolIndex, source: &str) {
    for raw_line in source.lines() {
        let line = raw_line.split("//").next().unwrap_or_default().trim();
        add_recovery_function_symbols(index, line);
        for keyword in [
            "let", "const", "global", "port", "mmio", "embed", "alias", "struct",
        ] {
            let Some(rest) = line
                .strip_prefix(keyword)
                .and_then(|rest| rest.strip_prefix(' '))
            else {
                continue;
            };
            let Some(name) = recovery_identifier(rest) else {
                continue;
            };
            add_symbol(
                index,
                SymbolInfo {
                    label: name.to_owned(),
                    kind: if keyword == "struct" { 23 } else { 6 },
                    detail: format!("{keyword} {name}"),
                },
            );
        }
    }
}

fn add_recovery_function_symbols(index: &mut SymbolIndex, line: &str) {
    let Some((_, rest)) = line.split_once("fn ") else {
        return;
    };
    let Some(name) = recovery_identifier(rest) else {
        return;
    };
    let signature = rest
        .split_once('{')
        .map(|(signature, _)| signature)
        .unwrap_or(rest)
        .trim();
    add_symbol(
        index,
        SymbolInfo {
            label: name.to_owned(),
            kind: 3,
            detail: format!("fn {signature}"),
        },
    );

    let Some((_, params)) = rest.split_once('(') else {
        return;
    };
    for parameter in params.split(',') {
        let Some(name) = recovery_identifier(parameter.trim()) else {
            continue;
        };
        add_symbol(
            index,
            SymbolInfo {
                label: name.to_owned(),
                kind: 6,
                detail: format!("parameter {name}"),
            },
        );
    }
}

fn recovery_identifier(text: &str) -> Option<&str> {
    let end = text
        .char_indices()
        .take_while(|(_, ch)| ch.is_ascii_alphanumeric() || *ch == '_')
        .last()
        .map(|(index, ch)| index + ch.len_utf8())?;
    let identifier = &text[..end];
    identifier
        .chars()
        .next()
        .filter(|ch| ch.is_ascii_alphabetic() || *ch == '_')
        .map(|_| identifier)
}

fn add_builtin_modules(index: &mut SymbolIndex, sdk: Option<&SdkResolver>) {
    for module in available_modules(sdk) {
        for prefix in module_prefixes(&module) {
            index.modules.insert(prefix);
        }
        index.modules.insert(module.to_owned());
    }
}

fn available_modules(sdk: Option<&SdkResolver>) -> Vec<String> {
    let Some(sdk) = sdk else {
        return Vec::new();
    };

    let mut modules = builtin_sdk_modules(sdk.target.as_deref())
        .into_iter()
        .map(str::to_owned)
        .collect::<BTreeSet<_>>();
    for root in &sdk.sdk_roots {
        collect_sdk_modules(root, root, &mut modules);
    }
    modules.into_iter().collect()
}

fn collect_sdk_modules(root: &Path, directory: &Path, modules: &mut BTreeSet<String>) {
    let Ok(entries) = std::fs::read_dir(directory) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_sdk_modules(root, &path, modules);
            continue;
        }
        if path.extension().and_then(|extension| extension.to_str()) != Some("ezra") {
            continue;
        }
        let Ok(relative) = path.strip_prefix(root) else {
            continue;
        };
        let module = relative
            .with_extension("")
            .to_string_lossy()
            .replace(['\\', '/'], ".");
        if !module.is_empty() {
            modules.insert(module);
        }
    }
}

fn add_program_symbols(index: &mut SymbolIndex, declarations: &[Declaration]) {
    for declaration in declarations {
        if let Some(info) = declaration_symbol(declaration) {
            add_symbol(index, info);
        }
        if let Declaration::Function(function) = declaration {
            add_function_locals(index, function);
        }
    }
}

fn add_function_locals(index: &mut SymbolIndex, function: &Function) {
    for param in &function.params {
        add_symbol(
            index,
            SymbolInfo {
                label: param.name.clone(),
                kind: 6,
                detail: format!("{}: {}", param.name, type_text(&param.ty)),
            },
        );
    }
    add_stmt_locals(index, &function.body);
}

fn add_stmt_locals(index: &mut SymbolIndex, stmts: &[Stmt]) {
    for stmt in stmts {
        match stmt {
            Stmt::Let { name, ty, .. } => add_symbol(
                index,
                SymbolInfo {
                    label: name.clone(),
                    kind: 6,
                    detail: format!("let {name}: {}", type_text(ty)),
                },
            ),
            Stmt::If {
                then_body,
                else_body,
                ..
            } => {
                add_stmt_locals(index, then_body);
                add_stmt_locals(index, else_body);
            }
            Stmt::While { body, .. } | Stmt::Loop { body } => add_stmt_locals(index, body),
            Stmt::Asm {
                inputs, outputs, ..
            } => {
                for input in inputs {
                    add_symbol(
                        index,
                        SymbolInfo {
                            label: input.name.clone(),
                            kind: 6,
                            detail: format!("asm input {}: {}", input.name, type_text(&input.ty)),
                        },
                    );
                }
                for output in outputs {
                    add_symbol(
                        index,
                        SymbolInfo {
                            label: output.name.clone(),
                            kind: 6,
                            detail: format!(
                                "asm output {}: {}",
                                output.name,
                                type_text(&output.ty)
                            ),
                        },
                    );
                }
            }
            _ => {}
        }
    }
}

fn declaration_symbol(declaration: &Declaration) -> Option<SymbolInfo> {
    match declaration {
        Declaration::Const(decl) => Some(SymbolInfo {
            label: decl.name.clone(),
            kind: 21,
            detail: format!("const {}: {}", decl.name, type_text(&decl.ty)),
        }),
        Declaration::Alias(decl) => Some(SymbolInfo {
            label: decl.name.clone(),
            kind: 25,
            detail: format!("alias {} = {}", decl.name, type_text(&decl.ty)),
        }),
        Declaration::Port(decl) => Some(SymbolInfo {
            label: decl.name.clone(),
            kind: 21,
            detail: format!("port {}: {}", decl.name, type_text(&decl.ty)),
        }),
        Declaration::Mmio(decl) => Some(SymbolInfo {
            label: decl.name.clone(),
            kind: 21,
            detail: format!("mmio {}: {}", decl.name, type_text(&decl.ty)),
        }),
        Declaration::Embed(decl) => Some(SymbolInfo {
            label: decl.name.clone(),
            kind: 21,
            detail: format!("embed {}: bytes", decl.name),
        }),
        Declaration::Global(decl) => Some(SymbolInfo {
            label: decl.name.clone(),
            kind: 6,
            detail: format!("global {}: {}", decl.name, type_text(&decl.ty)),
        }),
        Declaration::Struct(decl) => Some(SymbolInfo {
            label: decl.name.clone(),
            kind: 23,
            detail: format!(
                "struct {} {{ {} }}",
                decl.name,
                decl.fields
                    .iter()
                    .map(|field| format!("{}: {}", field.name, type_text(&field.ty)))
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        }),
        Declaration::ExternAsmFunction(function) => Some(SymbolInfo {
            label: function.name.clone(),
            kind: 3,
            detail: function_signature(
                "extern asm fn",
                &function.name,
                &function.params,
                &function.return_type,
            ),
        }),
        Declaration::Function(function) => Some(SymbolInfo {
            label: function.name.clone(),
            kind: 3,
            detail: function_signature(
                "fn",
                &function.name,
                &function.params,
                &function.return_type,
            ),
        }),
        Declaration::Cfg { declaration, .. } => declaration_symbol(declaration),
        Declaration::Import(_) => None,
    }
}

fn add_symbol(index: &mut SymbolIndex, info: SymbolInfo) {
    for module in module_prefixes(&info.label) {
        index.modules.insert(module);
    }
    index.symbols.insert(info.label.clone(), info);
}

fn module_prefixes(label: &str) -> Vec<String> {
    let mut prefixes = Vec::new();
    let mut parts = label.split('.').collect::<Vec<_>>();
    while parts.len() > 1 {
        parts.pop();
        prefixes.push(parts.join("."));
    }
    prefixes
}

fn function_signature(
    prefix: &str,
    name: &str,
    params: &[ezra::ast::Param],
    return_type: &Option<Type>,
) -> String {
    let params = params
        .iter()
        .map(|param| format!("{}: {}", param.name, type_text(&param.ty)))
        .collect::<Vec<_>>()
        .join(", ");
    match return_type {
        Some(ty) => format!("{prefix} {name}({params}) -> {}", type_text(ty)),
        None => format!("{prefix} {name}({params})"),
    }
}

fn type_text(ty: &Type) -> String {
    match ty {
        Type::Named(name) => name.clone(),
        Type::Ptr(inner) => format!("ptr<{}>", type_text(inner)),
        Type::Array { element, len } => format!("[{}; {}]", type_text(element), expr_text(len)),
    }
}

fn expr_text(expr: &Expr) -> String {
    match expr {
        Expr::Int(value) => value.to_string(),
        Expr::TypedInt(value, ty) => format!("{}{}", value, type_text(ty)),
        Expr::Bool(value) => value.to_string(),
        Expr::Char(value) => format!("'{}'", *value as char),
        Expr::String(value) => format!("\"{value}\""),
        Expr::Ident(name) | Expr::In(name) | Expr::AddressOf(name) => name.clone(),
        Expr::Access(path) | Expr::AddressOfAccess(path) => access_path_text(path),
        _ => "...".to_owned(),
    }
}

fn access_path_text(path: &ezra::ast::AccessPath) -> String {
    let mut text = path.root.clone();
    for segment in &path.segments {
        match segment {
            ezra::ast::AccessSegment::Field(field) => {
                text.push('.');
                text.push_str(field);
            }
            ezra::ast::AccessSegment::Index(_) => text.push_str("[...]"),
        }
    }
    text
}

fn module_members(index: &SymbolIndex, module: &str) -> Vec<String> {
    let prefix = format!("{module}.");
    index
        .symbols
        .values()
        .filter(|symbol| symbol.label.starts_with(&prefix))
        .map(|symbol| format!("- `{}`", symbol.detail))
        .collect()
}

fn diagnostic_symbol(message: &str) -> Option<&str> {
    message
        .split('`')
        .nth(1)
        .filter(|symbol| !symbol.is_empty())
}

fn completion_prefix(source: &str, position: Position) -> String {
    let Some(line) = source.lines().nth(position.line as usize) else {
        return String::new();
    };
    let end = byte_index_for_character(line, position.character as usize);
    let prefix = &line[..end];
    prefix
        .chars()
        .rev()
        .take_while(|ch| is_symbol_char(*ch))
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect()
}

fn symbol_at_position(source: &str, position: Position) -> Option<String> {
    let line = source.lines().nth(position.line as usize)?;
    let cursor = byte_index_for_character(line, position.character as usize);
    let bytes = line.as_bytes();
    let mut start = cursor.min(bytes.len());
    while start > 0 && is_symbol_char(bytes[start - 1] as char) {
        start -= 1;
    }
    let mut end = cursor.min(bytes.len());
    while end < bytes.len() && is_symbol_char(bytes[end] as char) {
        end += 1;
    }
    (start < end).then(|| line[start..end].to_owned())
}

fn byte_index_for_character(line: &str, character: usize) -> usize {
    let mut utf16_offset = 0usize;
    for (index, ch) in line.char_indices() {
        if utf16_offset >= character {
            return index;
        }
        let next = utf16_offset + ch.len_utf16();
        if character < next {
            return index;
        }
        utf16_offset = next;
    }
    line.len()
}

fn utf16_len(text: &str) -> u32 {
    text.encode_utf16().count() as u32
}

fn is_symbol_char(ch: char) -> bool {
    ch == '.' || ch == '_' || ch.is_ascii_alphanumeric()
}

fn range_for_symbol(source: &str, symbol: &str) -> Option<Range> {
    for (line_index, line) in source.lines().enumerate() {
        let mut search_from = 0;
        while let Some(offset) = line[search_from..].find(symbol) {
            let start = search_from + offset;
            let end = start + symbol.len();
            let before = start
                .checked_sub(1)
                .and_then(|index| line.as_bytes().get(index))
                .copied()
                .map(char::from);
            let after = line.as_bytes().get(end).copied().map(char::from);
            if before.is_none_or(|ch| !is_symbol_char(ch))
                && after.is_none_or(|ch| !is_symbol_char(ch))
            {
                return Some(Range {
                    start: Position {
                        line: line_index as u32,
                        character: utf16_len(&line[..start]),
                    },
                    end: Position {
                        line: line_index as u32,
                        character: utf16_len(&line[..end]),
                    },
                });
            }
            search_from = end;
        }
    }
    None
}

fn completion_trigger_characters() -> Vec<String> {
    let mut triggers = vec![
        ".".to_owned(),
        "_".to_owned(),
        "(".to_owned(),
        ",".to_owned(),
    ];
    for ch in 'a'..='z' {
        triggers.push(ch.to_string());
    }
    for ch in 'A'..='Z' {
        triggers.push(ch.to_string());
    }
    triggers
}

const KEYWORDS: &[&str] = &[
    "import",
    "const",
    "alias",
    "port",
    "mmio",
    "embed",
    "global",
    "struct",
    "extern",
    "asm",
    "fn",
    "layout",
    "load",
    "entry",
    "stack",
    "region",
    "section",
    "symbol",
    "let",
    "if",
    "else",
    "while",
    "loop",
    "break",
    "continue",
    "return",
    "out",
    "in",
    "cast",
    "file",
    "text",
    "cstr",
    "repeat",
    "align",
    "read",
    "write",
    "execute",
    "reserved",
    "pub",
    "inline",
    "naked",
    "interrupt",
    "volatile",
    "as",
    "clobber",
];

const PRIMITIVE_TYPES: &[&str] = &["u8", "i8", "u16", "i16", "u24", "i24", "ptr", "bytes"];

fn diagnostic_to_lsp(document: &OpenDocument, error: &Diagnostic) -> LspDiagnostic {
    LspDiagnostic {
        range: error
            .span
            .as_ref()
            .filter(|span| span.file == document.path)
            .map(|span| source_span_to_range(&document.text, span))
            .unwrap_or_else(|| diagnostic_fallback_range(document, &error.message)),
        severity: 1,
        source: "ezrac",
        message: error.message.clone(),
    }
}

fn diagnostic_fallback_range(document: &OpenDocument, message: &str) -> Range {
    if let Some(symbol) = diagnostic_symbol(message)
        && let Some(range) = range_for_symbol(&document.text, symbol)
    {
        return range;
    }

    let preferred_line = if message.contains("main function") {
        document
            .text
            .lines()
            .position(|line| line.contains("fn main"))
    } else if message.contains("type mismatch")
        || message.contains("outside")
        || message.contains("invalid")
    {
        document.text.lines().position(|line| {
            line.contains("let ")
                || line.contains("return ")
                || line.contains(" = ")
                || line.contains("out ")
        })
    } else {
        None
    };
    preferred_line
        .map(|line| range_for_line(&document.text, line))
        .unwrap_or_else(|| {
            document
                .text
                .lines()
                .position(|line| !line.trim().is_empty())
                .map(|line| range_for_line(&document.text, line))
                .unwrap_or_else(default_range)
        })
}

fn range_for_line(source: &str, line_index: usize) -> Range {
    let line = source.lines().nth(line_index).unwrap_or_default();
    Range {
        start: Position {
            line: line_index as u32,
            character: 0,
        },
        end: Position {
            line: line_index as u32,
            character: utf16_len(line).max(1),
        },
    }
}

fn source_span_to_range(source: &str, span: &SourceSpan) -> Range {
    let start = source_position_to_lsp(source, &span.start);
    let end = source_position_to_lsp(source, &span.end);
    Range { start, end }
}

fn source_position_to_lsp(source: &str, location: &SourcePosition) -> Position {
    let line_index = location.line.saturating_sub(1);
    let source_line = source.lines().nth(line_index).unwrap_or_default();
    let scalar_column = location.column.saturating_sub(1);
    let byte = source_line
        .char_indices()
        .nth(scalar_column)
        .map(|(index, _)| index)
        .unwrap_or(source_line.len());
    Position {
        line: line_index as u32,
        character: utf16_len(&source_line[..byte]),
    }
}

fn default_range() -> Range {
    Range {
        start: Position {
            line: 0,
            character: 0,
        },
        end: Position {
            line: 0,
            character: 1,
        },
    }
}

fn read_message(input: &mut impl BufRead) -> Result<Option<String>, String> {
    let mut content_length = None;
    loop {
        let mut line = String::new();
        let read = input
            .read_line(&mut line)
            .map_err(|error| format!("failed to read LSP header: {error}"))?;
        if read == 0 {
            return Ok(None);
        }
        let header = line.trim_end_matches(['\r', '\n']);
        if header.is_empty() {
            break;
        }
        if let Some(value) = header.strip_prefix("Content-Length:") {
            content_length = Some(
                value
                    .trim()
                    .parse::<usize>()
                    .map_err(|error| format!("invalid Content-Length: {error}"))?,
            );
        }
    }
    let len = content_length.ok_or_else(|| "missing Content-Length header".to_owned())?;
    let mut buffer = vec![0; len];
    input
        .read_exact(&mut buffer)
        .map_err(|error| format!("failed to read LSP body: {error}"))?;
    String::from_utf8(buffer)
        .map(Some)
        .map_err(|error| format!("LSP body is not UTF-8: {error}"))
}

fn write_response(output: &mut impl Write, id: Value, result: Value) -> Result<(), String> {
    write_json(
        output,
        &json!({ "jsonrpc": "2.0", "id": id, "result": result }),
    )
}

fn write_error(output: &mut impl Write, id: Value, code: i32, message: &str) -> Result<(), String> {
    write_json(
        output,
        &json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } }),
    )
}

fn write_notification(output: &mut impl Write, method: &str, params: Value) -> Result<(), String> {
    write_json(
        output,
        &json!({ "jsonrpc": "2.0", "method": method, "params": params }),
    )
}

fn write_json(output: &mut impl Write, value: &Value) -> Result<(), String> {
    let body = serde_json::to_vec(value)
        .map_err(|error| format!("failed to encode LSP message: {error}"))?;
    write!(output, "Content-Length: {}\r\n\r\n", body.len())
        .map_err(|error| format!("failed to write LSP header: {error}"))?;
    output
        .write_all(&body)
        .map_err(|error| format!("failed to write LSP body: {error}"))?;
    output
        .flush()
        .map_err(|error| format!("failed to flush LSP output: {error}"))
}

fn uri_to_path(uri: &str) -> Result<PathBuf, String> {
    lsp::Uri(uri.to_owned())
        .to_file_path()
        .ok_or_else(|| format!("unsupported file URI `{uri}`"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn built_in_completion_modules_follow_project_target() {
        let sdk = SdkResolver {
            target: Some("ez180n-ez80".to_owned()),
            sdk_roots: Vec::new(),
        };
        let modules = available_modules(Some(&sdk));

        assert_eq!(modules, vec!["ez180n.console"]);
        let items = import_completion_items(Some(&sdk));
        assert!(items.iter().any(|item| {
            item.get("label").and_then(Value::as_str) == Some("import ez180n.console")
        }));
        assert!(!items.iter().any(|item| {
            item.get("label").and_then(Value::as_str) == Some("import agon.console")
        }));
    }

    #[test]
    fn custom_sdk_roots_contribute_modules_for_custom_targets() {
        let root = std::env::temp_dir().join(format!("ezrac-lsp-sdk-{}", std::process::id()));
        let nested = root.join("graphics");
        fs::create_dir_all(&nested).unwrap();
        fs::write(root.join("math.ezra"), "fn helper() {}\n").unwrap();
        fs::write(nested.join("screen.ezra"), "fn helper() {}\n").unwrap();
        fs::write(root.join("README.md"), "not a module").unwrap();

        let sdk = SdkResolver {
            target: Some("custom-fantasy-ez80".to_owned()),
            sdk_roots: vec![root.clone()],
        };
        assert_eq!(
            available_modules(Some(&sdk)),
            vec!["graphics.screen", "math"]
        );

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn completion_includes_members_of_target_sdk_modules() {
        let root = std::env::temp_dir().join(format!("ezrac-lsp-members-{}", std::process::id()));
        fs::create_dir_all(root.join("src")).unwrap();
        fs::write(
            root.join("Ezra.toml"),
            "[build]\ntarget = \"ez180n-ez80\"\n",
        )
        .unwrap();
        let document = OpenDocument {
            path: root.join("src/main.ezra"),
            text: "import ez180n.console\nfn main() { ez180n.console.put_char(0, 0, 0) }\n"
                .to_owned(),
            version: None,
        };

        let items = completion_items(
            Some(&document),
            Position {
                line: 1,
                character: 15,
            },
        );
        assert!(items["items"].as_array().is_some_and(|items| {
            items.iter().any(|item| {
                item.get("label").and_then(Value::as_str) == Some("ez180n.console.put_char")
            })
        }));

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn typechecking_diagnostics_are_returned_for_type_mismatches() {
        let document = OpenDocument {
            path: PathBuf::from("type-error.ezra"),
            text: "fn main() { let value: u8 = true }".to_owned(),
            version: None,
        };

        let error = check_document_diagnostics(&document)
            .into_iter()
            .next()
            .unwrap();
        assert!(error.message.contains("type mismatch"), "{error}");
    }

    #[test]
    fn compiler_diagnostics_publish_multiple_exact_ranges() {
        let document = OpenDocument {
            path: PathBuf::from("multi-error.ezra"),
            text: "fn main() {\n    missing_one()\n    missing_two()\n}\n".to_owned(),
            version: None,
        };

        let diagnostics = check_document_diagnostics(&document);
        let diagnostics = diagnostics
            .iter()
            .map(|diagnostic| diagnostic_to_lsp(&document, diagnostic))
            .collect::<Vec<_>>();

        assert_eq!(diagnostics.len(), 2);
        assert_eq!(diagnostics[0].range.start.line, 1);
        assert_eq!(diagnostics[0].range.start.character, 4);
        assert_eq!(diagnostics[0].range.end.character, 15);
        assert_eq!(diagnostics[1].range.start.line, 2);
        assert_eq!(diagnostics[1].range.start.character, 4);
        assert_eq!(diagnostics[1].range.end.character, 15);
    }

    #[test]
    fn completion_replaces_typed_prefix_instead_of_appending_full_label() {
        let item = completion_text_edit(
            completion_item("console.put_char", 3, "function"),
            "console.",
            Position {
                line: 2,
                character: 8,
            },
            false,
        );
        assert_eq!(item["textEdit"]["newText"], "console.put_char");
        assert_eq!(item["textEdit"]["range"]["start"]["character"], 0);
        assert_eq!(item["textEdit"]["range"]["end"]["character"], 8);

        let import = completion_text_edit(
            completion_item("import ez180n.console", 15, "target SDK import"),
            "import",
            Position {
                line: 0,
                character: 6,
            },
            true,
        );
        assert_eq!(import["textEdit"]["newText"], "ez180n.console");
    }

    #[test]
    fn signature_help_tracks_call_parameter_index() {
        let call_line = "fn main() { add(1, ) }";
        let cursor = call_line.rfind(')').unwrap();
        let document = OpenDocument {
            path: PathBuf::from("signature.ezra"),
            text: format!(
                "fn add(left: u8, right: u8) -> u8 {{ return left + right }}\n{call_line}\n"
            ),
            version: None,
        };
        let result = signature_help(
            Some(&document),
            Position {
                line: 1,
                character: utf16_len(&call_line[..cursor]),
            },
        );
        assert_eq!(result["activeParameter"], 1);
        assert_eq!(
            result["signatures"][0]["parameters"][1]["label"],
            "right: u8"
        );
    }

    #[test]
    fn signature_help_tracks_the_outer_call_after_a_nested_call() {
        let call_line = "fn main() { outer(inner(1), ) }";
        let cursor = call_line.rfind(')').unwrap();
        let document = OpenDocument {
            path: PathBuf::from("nested-signature.ezra"),
            text: format!(
                "fn inner(value: u8) -> u8 {{ return value }}\nfn outer(first: u8, second: u8) {{}}\n{call_line}\n"
            ),
            version: None,
        };
        let result = signature_help(
            Some(&document),
            Position {
                line: 2,
                character: utf16_len(&call_line[..cursor]),
            },
        );
        assert_eq!(result["activeParameter"], 1);
        assert!(
            result["signatures"][0]["label"]
                .as_str()
                .is_some_and(|label| label.starts_with("fn outer("))
        );
    }

    #[test]
    fn diagnostics_without_compiler_locations_use_relevant_source_lines() {
        let document = OpenDocument {
            path: PathBuf::from("diagnostic.ezra"),
            text: "fn main() {\n    let value: u8 = true\n}\n".to_owned(),
            version: None,
        };
        let diagnostic = diagnostic_to_lsp(
            &document,
            &Diagnostic::new("type mismatch: expected u8, found bool"),
        );
        assert_eq!(diagnostic.range.start.line, 1);
        assert_eq!(diagnostic.range.end.line, 1);
        assert!(diagnostic.range.end.character > diagnostic.range.start.character);
    }

    #[test]
    fn completion_keeps_locals_when_an_if_or_while_is_incomplete() {
        for (source, position) in [
            (
                "fn main() {\n    let counter: u8 = 0\n    if co\n}\n",
                Position {
                    line: 2,
                    character: 9,
                },
            ),
            (
                "fn main() {\n    let counter: u8 = 0\n    while co\n}\n",
                Position {
                    line: 2,
                    character: 12,
                },
            ),
        ] {
            let document = OpenDocument {
                path: PathBuf::from("incomplete-control-flow.ezra"),
                text: source.to_owned(),
                version: None,
            };
            let items = completion_items(Some(&document), position);
            assert!(items["items"].as_array().is_some_and(|items| {
                items
                    .iter()
                    .any(|item| item.get("label").and_then(Value::as_str) == Some("counter"))
            }));
        }
    }

    #[test]
    fn completion_keeps_imported_members_when_control_flow_is_incomplete() {
        let root = std::env::temp_dir().join(format!(
            "ezrac-lsp-incomplete-import-{}",
            std::process::id()
        ));
        fs::create_dir_all(root.join("src")).unwrap();
        fs::write(
            root.join("Ezra.toml"),
            "[build]\ntarget = \"ez180n-ez80\"\n",
        )
        .unwrap();
        let line = "    while ez180n.console.pu";
        let document = OpenDocument {
            path: root.join("src/main.ezra"),
            text: format!("import ez180n.console\nfn main() {{\n{line}\n}}\n"),
            version: None,
        };
        let items = completion_items(
            Some(&document),
            Position {
                line: 2,
                character: utf16_len(line),
            },
        );
        assert!(items["items"].as_array().is_some_and(|items| {
            items.iter().any(|item| {
                item.get("label").and_then(Value::as_str) == Some("ez180n.console.put_char")
            })
        }));

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn lsp_positions_use_utf16_code_units() {
        assert_eq!(byte_index_for_character("😀co", 2), "😀".len());
        assert_eq!(byte_index_for_character("😀co", 4), "😀co".len());

        let line = "    \"😀\" co";
        let document = OpenDocument {
            path: PathBuf::from("utf16-completion.ezra"),
            text: format!("fn main() {{\n    let counter: u8 = 0\n{line}\n}}\n"),
            version: None,
        };
        let items = completion_items(
            Some(&document),
            Position {
                line: 2,
                character: utf16_len(line),
            },
        );
        let counter = items["items"]
            .as_array()
            .and_then(|items| {
                items
                    .iter()
                    .find(|item| item.get("label").and_then(Value::as_str) == Some("counter"))
            })
            .unwrap();
        assert_eq!(counter["textEdit"]["range"]["start"]["character"], 9);
        assert_eq!(counter["textEdit"]["range"]["end"]["character"], 11);

        let range = source_span_to_range(
            "😀x",
            &SourceSpan {
                file: PathBuf::from("utf16.ezra"),
                start: SourcePosition { line: 1, column: 2 },
                end: SourcePosition { line: 1, column: 3 },
            },
        );
        assert_eq!(range.start.character, 2);
        assert_eq!(range.end.character, 3);
    }
}
