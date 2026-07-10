use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    io::{self, BufRead, BufReader, Write},
    path::{Path, PathBuf},
};

use ezra::{
    ast::{Declaration, Expr, Function, Stmt, Type},
    compile::{
        CompileOptions, SdkResolver, builtin_sdk_modules,
        check_source_diagnostics_with_sdk_and_overrides, parse_and_resolve_imports_with_sdk,
        resolve_import_source,
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
    published_diagnostic_uris: BTreeSet<String>,
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
    definitions: BTreeMap<String, DefinitionInfo>,
}

#[derive(Clone)]
struct DefinitionInfo {
    uri: String,
    range: Range,
}

#[derive(Deserialize)]
struct Message {
    id: Option<Value>,
    #[serde(default)]
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
struct DocumentParams {
    #[serde(rename = "textDocument")]
    text_document: TextDocumentIdentifier,
}

#[derive(Deserialize)]
struct WorkspaceSymbolParams {
    #[serde(default)]
    query: String,
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
        if message.method.is_empty() {
            return Ok(false);
        }
        match message.method.as_str() {
            "initialize" => {
                if let Some(id) = message.id {
                    write_response(output, id, initialize_result())?;
                }
            }
            "initialized" => register_file_watchers(output)?,
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
            "textDocument/definition" => {
                if let Some(id) = message.id {
                    let params: CompletionParams = serde_json::from_value(message.params)
                        .map_err(|error| format!("invalid definition params: {error}"))?;
                    let result = definition(
                        self.documents.get(&params.text_document.uri),
                        params.position,
                    );
                    write_response(output, id, result)?;
                }
            }
            "textDocument/documentSymbol" => {
                if let Some(id) = message.id {
                    let params: DocumentParams = serde_json::from_value(message.params)
                        .map_err(|error| format!("invalid document symbol params: {error}"))?;
                    let result = document_symbols(
                        self.documents.get(&params.text_document.uri),
                        &params.text_document.uri,
                    );
                    write_response(output, id, result)?;
                }
            }
            "workspace/symbol" => {
                if let Some(id) = message.id {
                    let params: WorkspaceSymbolParams = serde_json::from_value(message.params)
                        .map_err(|error| format!("invalid workspace symbol params: {error}"))?;
                    write_response(output, id, self.workspace_symbols(&params.query))?;
                }
            }
            "textDocument/semanticTokens/full" => {
                if let Some(id) = message.id {
                    let params: DocumentParams = serde_json::from_value(message.params)
                        .map_err(|error| format!("invalid semantic token params: {error}"))?;
                    let result = semantic_tokens(self.documents.get(&params.text_document.uri));
                    write_response(output, id, result)?;
                }
            }
            "workspace/didChangeWatchedFiles" => {
                self.publish_workspace_diagnostics(output)?;
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
        self.publish_workspace_diagnostics(output)
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
        self.publish_workspace_diagnostics(output)
    }

    fn did_close(&mut self, params: Value, output: &mut impl Write) -> Result<(), String> {
        let params: DidCloseParams = serde_json::from_value(params)
            .map_err(|error| format!("invalid didClose params: {error}"))?;
        self.documents.remove(&params.text_document.uri);
        self.publish_workspace_diagnostics(output)
    }

    fn publish_workspace_diagnostics(&mut self, output: &mut impl Write) -> Result<(), String> {
        let source_overrides = self
            .documents
            .values()
            .map(|document| {
                (
                    normalize_document_path(&document.path),
                    document.text.clone(),
                )
            })
            .collect::<HashMap<_, _>>();
        let mut roots = BTreeSet::new();
        let mut configuration_errors = Vec::new();
        for document in self.documents.values() {
            match project_source_path(&document.path) {
                Ok(path) => {
                    roots.insert(normalize_document_path(&path));
                }
                Err(error) => configuration_errors.push((document.path.clone(), error)),
            }
        }

        let mut diagnostics = configuration_errors;
        for path in roots {
            let source = source_overrides
                .get(&path)
                .cloned()
                .or_else(|| std::fs::read_to_string(&path).ok());
            let Some(source) = source else {
                diagnostics.push((
                    path.clone(),
                    Diagnostic::new(format!("failed to read `{}`", path.display())),
                ));
                continue;
            };
            let sdk = match sdk_for_path(&path) {
                Ok(sdk) => sdk,
                Err(error) => {
                    diagnostics.push((path.clone(), error));
                    continue;
                }
            };
            diagnostics.extend(
                check_source_diagnostics_with_sdk_and_overrides(
                    &source,
                    &CompileOptions {
                        source: path.clone(),
                        debug_comments: false,
                        default_sdk_symbols: true,
                    },
                    &sdk,
                    &source_overrides,
                )
                .into_iter()
                .map(|diagnostic| {
                    let diagnostic_path = diagnostic
                        .span
                        .as_ref()
                        .map(|span| span.file.clone())
                        .unwrap_or_else(|| path.clone());
                    (diagnostic_path, diagnostic)
                }),
            );
        }

        let mut publications = self
            .documents
            .iter()
            .map(|(uri, document)| {
                (
                    uri.clone(),
                    (document.text.clone(), document.version, Vec::new()),
                )
            })
            .collect::<BTreeMap<_, _>>();
        for (path, diagnostic) in diagnostics {
            let normalized = normalize_document_path(&path);
            let (uri, source, version) = self
                .documents
                .iter()
                .find(|(_, document)| normalize_document_path(&document.path) == normalized)
                .map(|(uri, document)| (uri.clone(), document.text.clone(), document.version))
                .unwrap_or_else(|| {
                    (
                        path_to_uri(&path),
                        std::fs::read_to_string(&path).unwrap_or_default(),
                        None,
                    )
                });
            publications
                .entry(uri)
                .or_insert_with(|| (source.clone(), version, Vec::new()))
                .2
                .push(diagnostic_to_lsp_source(&source, &path, &diagnostic));
        }

        let current = publications.keys().cloned().collect::<BTreeSet<_>>();
        for uri in self
            .published_diagnostic_uris
            .difference(&current)
            .cloned()
            .collect::<Vec<_>>()
        {
            write_notification(
                output,
                TextDocumentPublishDiagnostics::METHOD,
                json!({ "uri": uri, "diagnostics": [] }),
            )?;
        }
        for (uri, (_, version, diagnostics)) in publications {
            write_notification(
                output,
                TextDocumentPublishDiagnostics::METHOD,
                json!({ "uri": uri, "version": version, "diagnostics": diagnostics }),
            )?;
        }
        self.published_diagnostic_uris = current;
        Ok(())
    }

    fn workspace_symbols(&self, query: &str) -> Value {
        let mut symbols = BTreeMap::new();
        for (uri, document) in &self.documents {
            for symbol in document_symbol_values(document, uri) {
                let Some(name) = symbol.get("name").and_then(Value::as_str) else {
                    continue;
                };
                if query.is_empty() || name.contains(query) {
                    symbols
                        .entry((uri.clone(), name.to_owned()))
                        .or_insert(symbol);
                }
            }
        }
        Value::Array(symbols.into_values().collect())
    }
}

#[cfg(test)]
fn check_document_diagnostics(document: &OpenDocument) -> Vec<Diagnostic> {
    let sdk = match sdk_for_path(&document.path) {
        Ok(sdk) => sdk,
        Err(error) => return vec![error],
    };
    ezra::compile::check_source_diagnostics_with_sdk(
        &document.text,
        &CompileOptions {
            source: document.path.clone(),
            debug_comments: false,
            default_sdk_symbols: true,
        },
        &sdk,
    )
}

fn project_source_path(path: &Path) -> Result<PathBuf, Diagnostic> {
    let project = load_nearest_project_config(path)?;
    Ok(project
        .and_then(|project| {
            (project.input_kind.as_deref() != Some("assembly"))
                .then_some(project.input)
                .flatten()
        })
        .unwrap_or_else(|| path.to_path_buf()))
}

fn normalize_document_path(path: &Path) -> PathBuf {
    path.canonicalize().unwrap_or_else(|_| {
        if path.is_absolute() {
            path.to_path_buf()
        } else {
            std::env::current_dir().unwrap_or_default().join(path)
        }
    })
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
            "definitionProvider": true,
            "documentSymbolProvider": true,
            "workspaceSymbolProvider": true,
            "semanticTokensProvider": {
                "legend": {
                    "tokenTypes": ["namespace", "type", "function", "variable", "keyword", "number", "string", "comment", "operator"],
                    "tokenModifiers": []
                },
                "full": true
            },
            "signatureHelpProvider": {
                "triggerCharacters": ["(", ","],
                "retriggerCharacters": [","]
            }
        },
        "serverInfo": { "name": "ezrac", "version": env!("CARGO_PKG_VERSION") }
    })
}

fn register_file_watchers(output: &mut impl Write) -> Result<(), String> {
    write_json(
        output,
        &json!({
            "jsonrpc": "2.0",
            "id": "ezrac-register-watchers",
            "method": "client/registerCapability",
            "params": {
                "registrations": [{
                    "id": "ezrac-workspace-files",
                    "method": "workspace/didChangeWatchedFiles",
                    "registerOptions": {
                        "watchers": [
                            { "globPattern": "**/Ezra.toml" },
                            { "globPattern": "**/*.ezra" },
                            { "globPattern": "**/*.ezralayout" }
                        ]
                    }
                }]
            }
        }),
    )
}

fn completion_items(document: Option<&OpenDocument>, position: Position) -> Value {
    let mut prefix = document
        .map(|document| completion_prefix(&document.text, position))
        .unwrap_or_default();
    let import_context =
        document.is_some_and(|document| is_import_completion(&document.text, position));
    let sdk = document.and_then(|document| sdk_for_path(&document.path).ok());
    let cfg_value = document.and_then(|document| cfg_value_completion(&document.text, position));
    if let Some((value_prefix, _)) = &cfg_value {
        prefix.clone_from(value_prefix);
    }
    let mut items = if import_context {
        import_completion_items(sdk.as_ref())
    } else if let Some((_, values)) = cfg_value {
        values
    } else {
        standard_completion_items()
    };
    if let Some(document) = document {
        let index = symbol_index(document);
        for module in &index.modules {
            items.push(completion_item(module, 9, "module"));
        }
        if !import_context {
            items.extend(field_completion_items(document, &prefix, position));
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

fn cfg_value_completion(source: &str, position: Position) -> Option<(String, Vec<Value>)> {
    let line = source.lines().nth(position.line as usize)?;
    let end = byte_index_for_character(line, position.character as usize);
    let before = &line[..end];
    let quote = before.rfind('"')?;
    let call = before[..quote].trim_end();
    let name = call
        .strip_suffix('(')?
        .trim_end()
        .split(|ch: char| !ch.is_ascii_alphanumeric() && ch != '_')
        .next_back()?;
    let values: &[&str] = match name {
        "target" => DOCUMENTED_TARGETS,
        "cpu" => &["ez80", "z80", "z80n", "z180", "i8080", "i8085"],
        "pointer_width" | "address_width" => &["16", "24"],
        _ => return None,
    };
    let prefix = before[quote + 1..].to_owned();
    Some((
        prefix,
        values
            .iter()
            .map(|value| completion_item(value, 12, "cfg value"))
            .collect(),
    ))
}

fn field_completion_items(document: &OpenDocument, prefix: &str, position: Position) -> Vec<Value> {
    let Some(root) = prefix.strip_suffix('.') else {
        return Vec::new();
    };
    let mut recovered = document.text.clone();
    let program = parse_program(&document.path, &document.text).or_else(|_| {
        let offset = source_offset(&document.text, position);
        recovered.insert_str(offset, "__ezra_completion");
        parse_program(&document.path, &recovered)
    });
    let Ok(program) = program else {
        return Vec::new();
    };
    let mut binding_types = BTreeMap::<String, String>::new();
    for declaration in &program.declarations {
        if let Declaration::Global(declaration) = declaration {
            binding_types.insert(declaration.name.clone(), type_text(&declaration.ty));
        }
        if let Declaration::Function(function) = declaration {
            binding_types.extend(
                function
                    .params
                    .iter()
                    .map(|param| (param.name.clone(), type_text(&param.ty))),
            );
            collect_binding_types(&function.body, &mut binding_types);
        }
    }
    let Some(struct_name) = binding_types.get(root) else {
        return Vec::new();
    };
    program
        .declarations
        .iter()
        .find_map(|declaration| match declaration {
            Declaration::Struct(declaration) if declaration.name == *struct_name => {
                Some(&declaration.fields)
            }
            _ => None,
        })
        .into_iter()
        .flatten()
        .map(|field| {
            completion_item(
                &format!("{root}.{}", field.name),
                5,
                &format!("{}: {}", field.name, type_text(&field.ty)),
            )
        })
        .collect()
}

fn source_offset(source: &str, position: Position) -> usize {
    let mut offset = 0;
    for (line_index, line) in source.split_inclusive('\n').enumerate() {
        if line_index == position.line as usize {
            let line = line.strip_suffix('\n').unwrap_or(line);
            return offset + byte_index_for_character(line, position.character as usize);
        }
        offset += line.len();
    }
    source.len()
}

fn collect_binding_types(stmts: &[Stmt], output: &mut BTreeMap<String, String>) {
    for stmt in stmts {
        match stmt {
            Stmt::Let { name, ty, .. } => {
                output.insert(name.clone(), type_text(ty));
            }
            Stmt::If {
                then_body,
                else_body,
                ..
            } => {
                collect_binding_types(then_body, output);
                collect_binding_types(else_body, output);
            }
            Stmt::While { body, .. } | Stmt::Loop { body } => {
                collect_binding_types(body, output);
            }
            _ => {}
        }
    }
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
    for predicate in CFG_PREDICATES {
        items.push(completion_item(predicate, 3, "cfg predicate"));
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

fn definition(document: Option<&OpenDocument>, position: Position) -> Value {
    let Some(document) = document else {
        return Value::Null;
    };
    let Some(symbol) = symbol_at_position(&document.text, position) else {
        return Value::Null;
    };
    let index = symbol_index(document);
    index
        .definitions
        .get(&symbol)
        .map(|definition| {
            json!({
                "uri": definition.uri,
                "range": definition.range,
            })
        })
        .unwrap_or(Value::Null)
}

fn document_symbols(document: Option<&OpenDocument>, uri: &str) -> Value {
    document
        .map(|document| Value::Array(document_symbol_values(document, uri)))
        .unwrap_or_else(|| Value::Array(Vec::new()))
}

fn document_symbol_values(document: &OpenDocument, uri: &str) -> Vec<Value> {
    let index = symbol_index(document);
    index
        .symbols
        .values()
        .filter_map(|symbol| {
            let definition = index.definitions.get(&symbol.label)?;
            (definition.uri == uri || definition.uri == path_to_uri(&document.path)).then(|| {
                json!({
                    "name": symbol.label,
                    "kind": symbol.kind,
                    "location": { "uri": uri, "range": definition.range },
                    "containerName": "EZRA",
                })
            })
        })
        .collect()
}

fn semantic_tokens(document: Option<&OpenDocument>) -> Value {
    let Some(document) = document else {
        return json!({ "data": [] });
    };
    let index = symbol_index(document);
    let mut tokens = Vec::<(u32, u32, u32, u32)>::new();
    for (line_index, line) in document.text.lines().enumerate() {
        collect_line_semantic_tokens(line, line_index as u32, &index, &mut tokens);
    }
    let mut data = Vec::with_capacity(tokens.len() * 5);
    let mut previous_line = 0;
    let mut previous_start = 0;
    for (line, start, len, kind) in tokens {
        let delta_line = line - previous_line;
        let delta_start = if delta_line == 0 {
            start - previous_start
        } else {
            start
        };
        data.extend([delta_line, delta_start, len, kind, 0]);
        previous_line = line;
        previous_start = start;
    }
    json!({ "data": data })
}

fn collect_line_semantic_tokens(
    line: &str,
    line_index: u32,
    index: &SymbolIndex,
    tokens: &mut Vec<(u32, u32, u32, u32)>,
) {
    let mut chars = line.char_indices().peekable();
    while let Some((start, ch)) = chars.next() {
        if ch.is_whitespace() {
            continue;
        }
        if ch == '/' && chars.peek().is_some_and(|(_, next)| *next == '/') {
            tokens.push((
                line_index,
                utf16_len(&line[..start]),
                utf16_len(&line[start..]),
                7,
            ));
            break;
        }
        if matches!(ch, '"' | '\'') {
            let quote = ch;
            let mut end = start + ch.len_utf8();
            let mut escaped = false;
            for (offset, next) in chars.by_ref() {
                end = offset + next.len_utf8();
                if escaped {
                    escaped = false;
                } else if next == '\\' {
                    escaped = true;
                } else if next == quote {
                    break;
                }
            }
            tokens.push((
                line_index,
                utf16_len(&line[..start]),
                utf16_len(&line[start..end]),
                6,
            ));
            continue;
        }
        if ch.is_ascii_alphanumeric() || ch == '_' {
            let mut end = start + ch.len_utf8();
            while let Some((offset, next)) = chars.peek().copied() {
                if !next.is_ascii_alphanumeric() && next != '_' {
                    break;
                }
                chars.next();
                end = offset + next.len_utf8();
            }
            let word = &line[start..end];
            let kind = if KEYWORDS.contains(&word) {
                4
            } else if PRIMITIVE_TYPES.contains(&word) {
                1
            } else if word.chars().next().is_some_and(|ch| ch.is_ascii_digit()) {
                5
            } else if index.modules.contains(word) {
                0
            } else {
                index
                    .symbols
                    .get(word)
                    .map_or(3, |symbol| match symbol.kind {
                        3 => 2,
                        23 | 25 => 1,
                        _ => 3,
                    })
            };
            tokens.push((line_index, utf16_len(&line[..start]), utf16_len(word), kind));
            continue;
        }
        if "+-*/%=&|^!<>".contains(ch) {
            tokens.push((
                line_index,
                utf16_len(&line[..start]),
                ch.len_utf16() as u32,
                8,
            ));
        }
    }
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
        Ok(program) => {
            add_program_symbols(&mut index, &program.declarations);
            add_source_definitions(
                &mut index,
                &document.path,
                &document.text,
                &program.declarations,
            );
        }
        Err(_) => add_recovery_symbols(&mut index, &document.text),
    }
    if let Some(sdk) = sdk.as_ref() {
        match parse_and_resolve_imports_with_sdk(&document.path, &document.text, sdk) {
            Ok(program) => add_program_symbols(&mut index, &program.declarations),
            Err(_) => add_recovery_import_symbols(&mut index, document, sdk),
        }
        add_import_definitions(
            &mut index,
            &document.path,
            &document.text,
            sdk,
            &mut BTreeSet::new(),
        );
    }
    index
}

fn add_source_definitions(
    index: &mut SymbolIndex,
    path: &Path,
    source: &str,
    declarations: &[Declaration],
) {
    let mut names = BTreeSet::new();
    collect_definition_names(declarations, &mut names);
    let uri = path_to_uri(path);
    for name in names {
        if let Some(range) = range_for_symbol(source, &name) {
            index.definitions.insert(
                name,
                DefinitionInfo {
                    uri: uri.clone(),
                    range,
                },
            );
        }
    }
}

fn collect_definition_names(declarations: &[Declaration], names: &mut BTreeSet<String>) {
    for declaration in declarations {
        if let Some(symbol) = declaration_symbol(declaration) {
            names.insert(symbol.label);
        }
        if let Declaration::Function(function) = declaration {
            names.extend(function.params.iter().map(|param| param.name.clone()));
            collect_stmt_definition_names(&function.body, names);
        }
    }
}

fn collect_stmt_definition_names(stmts: &[Stmt], names: &mut BTreeSet<String>) {
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
                collect_stmt_definition_names(then_body, names);
                collect_stmt_definition_names(else_body, names);
            }
            Stmt::While { body, .. } | Stmt::Loop { body } => {
                collect_stmt_definition_names(body, names)
            }
            Stmt::Asm {
                inputs, outputs, ..
            } => {
                names.extend(inputs.iter().map(|input| input.name.clone()));
                names.extend(outputs.iter().map(|output| output.name.clone()));
            }
            _ => {}
        }
    }
}

fn add_import_definitions(
    index: &mut SymbolIndex,
    importer: &Path,
    source: &str,
    sdk: &SdkResolver,
    seen: &mut BTreeSet<PathBuf>,
) {
    for import in source_imports(source) {
        let Ok((path, imported_source)) = resolve_import_source(importer, &import, sdk) else {
            continue;
        };
        if path.to_string_lossy().starts_with('<') || !seen.insert(path.clone()) {
            continue;
        }
        let Ok(program) = parse_program(&path, &imported_source) else {
            continue;
        };
        let Some(definition_path) = navigable_import_path(&path, &imported_source) else {
            continue;
        };
        let short = import.rsplit('.').next().unwrap_or(&import);
        let module_definition = DefinitionInfo {
            uri: path_to_uri(&definition_path),
            range: range_for_line(&imported_source, 0),
        };
        index
            .definitions
            .insert(import.clone(), module_definition.clone());
        index
            .definitions
            .entry(short.to_owned())
            .or_insert(module_definition);
        for declaration in &program.declarations {
            let Some(symbol) = declaration_symbol(declaration) else {
                continue;
            };
            let Some(range) = range_for_symbol(&imported_source, &symbol.label) else {
                continue;
            };
            let definition = DefinitionInfo {
                uri: path_to_uri(&definition_path),
                range,
            };
            index
                .definitions
                .entry(symbol.label.clone())
                .or_insert_with(|| definition.clone());
            if declaration_is_public(declaration) {
                index
                    .definitions
                    .insert(format!("{short}.{}", symbol.label), definition.clone());
                index
                    .definitions
                    .insert(format!("{import}.{}", symbol.label), definition);
            }
        }
        add_import_definitions(index, &path, &imported_source, sdk, seen);
    }
}

fn navigable_import_path(path: &Path, source: &str) -> Option<PathBuf> {
    let Ok(relative) = path.strip_prefix("builtin-sdk") else {
        return Some(path.to_path_buf());
    };
    let cache_path = std::env::temp_dir()
        .join("ezrac-builtin-sdk")
        .join(env!("CARGO_PKG_VERSION"))
        .join(relative);
    if std::fs::read_to_string(&cache_path).ok().as_deref() != Some(source) {
        std::fs::create_dir_all(cache_path.parent()?).ok()?;
        std::fs::write(&cache_path, source).ok()?;
    }
    Some(cache_path)
}

fn source_imports(source: &str) -> Vec<String> {
    source
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
        .collect()
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
    for symbol in LAYOUT_SYMBOLS {
        add_symbol(
            index,
            SymbolInfo {
                label: (*symbol).to_owned(),
                kind: 21,
                detail: format!("layout symbol {symbol}"),
            },
        );
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

const CFG_PREDICATES: &[&str] = &[
    "target",
    "target_family",
    "cpu",
    "vendor",
    "os",
    "pointer_width",
    "address_width",
    "feature",
    "debug",
    "release",
    "all",
    "any",
    "not",
];

const LAYOUT_SYMBOLS: &[&str] = &[
    "EZRA_LOAD_ADDR",
    "EZRA_ENTRY_ADDR",
    "EZRA_CODE_BASE",
    "EZRA_STACK_TOP",
    "EZRA_RAM_BASE",
    "EZRA_VRAM_BASE",
    "EZRA_AUDIO_BASE",
    "EZRA_ASSET_BASE",
    "EZRA_RODATA_BASE",
];

const DOCUMENTED_TARGETS: &[&str] = &[
    "agonlight-mos-ez80",
    "custom-unknown-ez80",
    "ez180n-ez80",
    "ezra-test-flat-ez80",
    "ezra-test-split-ez80",
    "ti84plusce-ez80",
    "ti83premiumce-ez80",
    "zxspectrum-z80",
    "ti83-z80",
    "ti83plus-z80",
    "ti84-z80",
    "ti84plus-z80",
    "cpm-2.2-z80",
    "cpm-2.2-i8080",
    "cpm-2.2-i8085",
    "bare-z80",
    "bare-z80n",
    "bare-z180",
    "bare-i8080",
    "bare-i8085",
    "bare-ez80",
];

#[cfg(test)]
fn diagnostic_to_lsp(document: &OpenDocument, error: &Diagnostic) -> LspDiagnostic {
    diagnostic_to_lsp_source(&document.text, &document.path, error)
}

fn diagnostic_to_lsp_source(source: &str, path: &Path, error: &Diagnostic) -> LspDiagnostic {
    let fallback_document = OpenDocument {
        path: path.to_path_buf(),
        text: source.to_owned(),
        version: None,
    };
    LspDiagnostic {
        range: error
            .span
            .as_ref()
            .filter(|span| normalize_document_path(&span.file) == normalize_document_path(path))
            .map(|span| source_span_to_range(source, span))
            .unwrap_or_else(|| diagnostic_fallback_range(&fallback_document, &error.message)),
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

fn path_to_uri(path: &Path) -> String {
    let absolute = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    let normalized = absolute.to_string_lossy().replace('\\', "/");
    let normalized = normalized
        .strip_prefix("//?/UNC/")
        .map(|path| format!("//{path}"))
        .or_else(|| normalized.strip_prefix("//?/").map(str::to_owned))
        .unwrap_or(normalized);
    let mut encoded = String::with_capacity(normalized.len());
    for byte in normalized.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' | b'/' | b':' => {
                encoded.push(char::from(byte))
            }
            _ => encoded.push_str(&format!("%{byte:02X}")),
        }
    }
    if encoded.starts_with('/') {
        format!("file://{encoded}")
    } else {
        format!("file:///{encoded}")
    }
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
    fn completion_includes_fields_and_cfg_values_during_incomplete_edits() {
        let field_line = "fn main(player: Player) { player.";
        let document = OpenDocument {
            path: PathBuf::from("field-completion.ezra"),
            text: format!("struct Player {{ x: u8 y: u8 }}\n{field_line}\n}}\n"),
            version: None,
        };
        let fields = completion_items(
            Some(&document),
            Position {
                line: 1,
                character: utf16_len(field_line),
            },
        );
        let labels = fields["items"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|item| item["label"].as_str())
            .collect::<BTreeSet<_>>();
        assert!(labels.contains("player.x"), "{labels:#?}");
        assert!(labels.contains("player.y"), "{labels:#?}");

        let cfg_line = "@cfg(target(\"agon";
        let cfg_document = OpenDocument {
            path: PathBuf::from("cfg-completion.ezra"),
            text: format!("{cfg_line}\nfn main() {{}}\n"),
            version: None,
        };
        let targets = completion_items(
            Some(&cfg_document),
            Position {
                line: 0,
                character: utf16_len(cfg_line),
            },
        );
        let agon = targets["items"]
            .as_array()
            .unwrap()
            .iter()
            .find(|item| item["label"] == "agonlight-mos-ez80")
            .unwrap();
        assert_eq!(agon["textEdit"]["newText"], "agonlight-mos-ez80");
        assert_eq!(agon["textEdit"]["range"]["start"]["character"], 13);
    }

    #[test]
    fn layout_symbols_are_available_for_completion_and_hover() {
        let line = "fn main() { let address: u24 = EZRA_RAM_BASE }";
        let document = OpenDocument {
            path: PathBuf::from("layout-symbol.ezra"),
            text: format!("{line}\n"),
            version: None,
        };
        let completion = completion_items(
            Some(&document),
            Position {
                line: 0,
                character: 38,
            },
        );
        assert!(
            completion["items"]
                .as_array()
                .is_some_and(|items| { items.iter().any(|item| item["label"] == "EZRA_RAM_BASE") })
        );
        let hover = hover(
            Some(&document),
            Position {
                line: 0,
                character: 39,
            },
        );
        assert!(hover.to_string().contains("layout symbol EZRA_RAM_BASE"));
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

    #[test]
    fn initialize_advertises_navigation_and_semantic_tokens() {
        let capabilities = &initialize_result()["capabilities"];
        assert_eq!(capabilities["definitionProvider"], true);
        assert_eq!(capabilities["documentSymbolProvider"], true);
        assert_eq!(capabilities["workspaceSymbolProvider"], true);
        assert_eq!(capabilities["semanticTokensProvider"]["full"], true);
    }

    #[test]
    fn initialized_registers_project_file_watchers_and_responses_are_ignored() {
        let mut server = Server::default();
        let mut output = Vec::new();
        server
            .handle_message(
                Message {
                    id: None,
                    method: "initialized".to_owned(),
                    params: Value::Null,
                },
                &mut output,
            )
            .unwrap();
        let output = String::from_utf8(output).unwrap();
        assert!(output.contains("client/registerCapability"));
        assert!(output.contains("**/Ezra.toml"));
        assert!(output.contains("**/*.ezra"));
        assert!(output.contains("**/*.ezralayout"));

        let mut output = Vec::new();
        server
            .handle_message(
                Message {
                    id: Some(json!("ezrac-register-watchers")),
                    method: String::new(),
                    params: Value::Null,
                },
                &mut output,
            )
            .unwrap();
        assert!(output.is_empty());
    }

    #[test]
    fn workspace_diagnostics_use_unsaved_imports_and_publish_the_import_uri() {
        let root = std::env::temp_dir().join(format!(
            "ezrac-lsp-workspace-diagnostics-{}",
            std::process::id()
        ));
        fs::create_dir_all(root.join("src/lib")).unwrap();
        fs::write(
            root.join("Ezra.toml"),
            "[build]\ninput = \"src/main.ezra\"\ntarget = \"ez80\"\n",
        )
        .unwrap();
        let main_path = root.join("src/main.ezra");
        let import_path = root.join("src/lib/math.ezra");
        let main_source = "import lib.math\nfn main() { let value: u8 = lib.math.increment(1) }\n";
        fs::write(&main_path, main_source).unwrap();
        fs::write(
            &import_path,
            "pub fn increment(value: u8) -> u8 { return value + 1 }\n",
        )
        .unwrap();
        let main_uri = path_to_uri(&main_path);
        let import_uri = path_to_uri(&import_path);
        let mut server = Server::default();
        server.documents.insert(
            main_uri.clone(),
            OpenDocument {
                path: main_path,
                text: main_source.to_owned(),
                version: Some(1),
            },
        );
        server.documents.insert(
            import_uri.clone(),
            OpenDocument {
                path: import_path,
                text: "pub fn increment(".to_owned(),
                version: Some(2),
            },
        );

        let mut output = Vec::new();
        server.publish_workspace_diagnostics(&mut output).unwrap();
        let output = String::from_utf8(output).unwrap();
        assert!(output.contains(&format!("\"uri\":\"{import_uri}\"")));
        assert!(output.contains("diagnostics"));
        assert!(!output.contains("missing required `fn main()`"));

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn definition_finds_local_and_imported_declarations() {
        let root =
            std::env::temp_dir().join(format!("ezrac-lsp-definition-{}", std::process::id()));
        fs::create_dir_all(root.join("src/lib")).unwrap();
        fs::write(root.join("Ezra.toml"), "[build]\ntarget = \"ez80\"\n").unwrap();
        let imported_path = root.join("src/lib/math.ezra");
        fs::write(
            &imported_path,
            "pub fn increment(value: u8) -> u8 { return value + 1 }\n",
        )
        .unwrap();
        let source = "import lib.math\nfn helper(value: u8) -> u8 { return value }\nfn main() { helper(lib.math.increment(1)) }\n";
        let document = OpenDocument {
            path: root.join("src/main.ezra"),
            text: source.to_owned(),
            version: None,
        };

        let helper = definition(
            Some(&document),
            Position {
                line: 2,
                character: 14,
            },
        );
        assert_eq!(helper["range"]["start"]["line"], 1);
        assert_eq!(helper["range"]["start"]["character"], 3);

        let imported = definition(
            Some(&document),
            Position {
                line: 2,
                character: 25,
            },
        );
        assert_eq!(imported["uri"], path_to_uri(&imported_path));
        assert_eq!(imported["range"]["start"]["line"], 0);
        assert_eq!(imported["range"]["start"]["character"], 7);

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn definition_materializes_bundled_sdk_sources_for_navigation() {
        let root =
            std::env::temp_dir().join(format!("ezrac-lsp-sdk-definition-{}", std::process::id()));
        fs::create_dir_all(root.join("src")).unwrap();
        fs::write(
            root.join("Ezra.toml"),
            "[build]\ntarget = \"ez180n-ez80\"\n",
        )
        .unwrap();
        let line = "fn main() { ez180n.console.put_char(0, 0, 0) }";
        let document = OpenDocument {
            path: root.join("src/main.ezra"),
            text: format!("import ez180n.console\n{line}\n"),
            version: None,
        };

        let result = definition(
            Some(&document),
            Position {
                line: 1,
                character: 30,
            },
        );
        let path = uri_to_path(result["uri"].as_str().unwrap()).unwrap();
        assert!(path.exists(), "{}", path.display());
        assert!(
            fs::read_to_string(path)
                .unwrap()
                .contains("pub fn put_char")
        );

        let module = definition(
            Some(&document),
            Position {
                line: 0,
                character: 12,
            },
        );
        assert!(
            uri_to_path(module["uri"].as_str().unwrap())
                .unwrap()
                .exists()
        );

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn symbols_and_semantic_tokens_cover_an_open_document() {
        let path =
            std::env::temp_dir().join(format!("ezrac-lsp-symbols-{}.ezra", std::process::id()));
        let uri = path_to_uri(&path);
        let document = OpenDocument {
            path,
            text: "const LIMIT: u8 = 3\nfn run(value: u8) { let current: u8 = value + LIMIT }\n"
                .to_owned(),
            version: None,
        };
        let symbols = document_symbols(Some(&document), &uri);
        let names = symbols
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|symbol| symbol["name"].as_str())
            .collect::<BTreeSet<_>>();
        assert!(names.contains("LIMIT"));
        assert!(names.contains("run"));
        assert!(names.contains("value"));
        assert!(names.contains("current"));

        let data = semantic_tokens(Some(&document))["data"]
            .as_array()
            .unwrap()
            .iter()
            .map(|value| value.as_u64().unwrap())
            .collect::<Vec<_>>();
        assert!(!data.is_empty());
        assert_eq!(data.len() % 5, 0);
        let kinds = data.iter().skip(3).step_by(5).copied().collect::<Vec<_>>();
        assert!(kinds.contains(&4));
        assert!(kinds.contains(&2));
        assert!(kinds.contains(&5));
    }
}
