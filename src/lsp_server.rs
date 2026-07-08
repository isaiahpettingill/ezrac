use std::{
    collections::BTreeMap,
    io::{self, BufRead, BufReader, Write},
    path::{Path, PathBuf},
};

use ezra::{
    compile::{CompileOptions, SdkResolver, check_source_with_sdk},
    diagnostic::{Diagnostic, SourceLocation},
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

#[derive(Clone, Copy, Serialize)]
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
                    let result = completion_items(self.documents.get(&params.text_document.uri));
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
        if let Some(document) = self.documents.get_mut(&params.text_document.uri) {
            if let Some(change) = params.content_changes.into_iter().last() {
                document.text = change.text;
                document.version = params.text_document.version;
            }
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
        let diagnostics = match check_document(document) {
            Ok(()) => Vec::new(),
            Err(error) => vec![diagnostic_to_lsp(&error)],
        };
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

fn check_document(document: &OpenDocument) -> Result<(), Diagnostic> {
    let sdk = sdk_for_path(&document.path)?;
    check_source_with_sdk(
        &document.text,
        &CompileOptions {
            source: document.path.clone(),
            debug_comments: false,
            default_sdk_symbols: true,
        },
        &sdk,
    )
    .map(|_| ())
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
            "completionProvider": { "triggerCharacters": [".", " "] }
        },
        "serverInfo": { "name": "ezrac", "version": env!("CARGO_PKG_VERSION") }
    })
}

fn completion_items(document: Option<&OpenDocument>) -> Value {
    let mut items = vec![
        completion_item("import agon.console", 15),
        completion_item("import agon.vdp", 15),
        completion_item("import agon.sprites", 15),
        completion_item("import agon.buffers", 15),
        completion_item("import agon.keyboard", 15),
        completion_item("import agon.mouse", 15),
        completion_item("import agon.gpio", 15),
        completion_item("agon.console", 9),
        completion_item("agon.vdp", 9),
        completion_item("agon.sprites", 9),
        completion_item("agon.buffers", 9),
        completion_item("agon.keyboard", 9),
        completion_item("agon.mouse", 9),
        completion_item("agon.gpio", 9),
    ];
    if let Some(document) = document {
        for line in document.text.lines() {
            let trimmed = line.trim();
            let Some(module) = trimmed.strip_prefix("import ") else {
                continue;
            };
            if let Some(short) = module
                .strip_prefix("agon.")
                .and_then(|_| module.rsplit('.').next())
            {
                items.push(completion_item(short, 9));
            }
        }
    }
    Value::Array(items)
}

fn completion_item(label: &str, kind: u8) -> Value {
    json!({ "label": label, "kind": kind })
}

fn diagnostic_to_lsp(error: &Diagnostic) -> LspDiagnostic {
    LspDiagnostic {
        range: error
            .location
            .as_ref()
            .map(source_location_to_range)
            .unwrap_or_else(default_range),
        severity: 1,
        source: "ezrac",
        message: error.message.clone(),
    }
}

fn source_location_to_range(location: &SourceLocation) -> Range {
    let line = location.line.saturating_sub(1) as u32;
    let character = location.column.saturating_sub(1) as u32;
    Range {
        start: Position { line, character },
        end: Position {
            line,
            character: character + 1,
        },
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
