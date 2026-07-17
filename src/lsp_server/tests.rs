use super::*;
use std::fs;

fn repository_path(relative: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(relative)
}

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
    assert!(
        !items.iter().any(|item| {
            item.get("label").and_then(Value::as_str) == Some("import agon.console")
        })
    );
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
        text: "import ez180n.console\nfn main() { ez180n.console.put_char(0, 0, 0) }\n".to_owned(),
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
fn bundled_sdk_documents_use_target_context_and_do_not_require_main() {
    let path = repository_path("toolchains/ez180n-ez80/sdk/ez180n/console.ezra");
    let document = OpenDocument {
        text: fs::read_to_string(&path).unwrap(),
        path: path.clone(),
        version: None,
    };

    let sdk = sdk_for_path(&path).unwrap();
    assert_eq!(sdk.target.as_deref(), Some("ez180n-ez80"));
    assert_eq!(
        sdk.sdk_roots,
        vec![normalize_document_path(&repository_path(
            "toolchains/ez180n-ez80/sdk"
        ))]
    );
    let diagnostics = check_document_diagnostics(&document);
    assert!(diagnostics.is_empty(), "{diagnostics:#?}");
}

#[test]
fn library_lsp_mode_checks_sdk_imports_without_requiring_main() {
    let root = std::env::temp_dir().join(format!("ezrac-lsp-library-{}", std::process::id()));
    let sdk_root = root.join("sdk");
    let source_path = root.join("src/lib.ezra");
    fs::create_dir_all(&sdk_root).unwrap();
    fs::create_dir_all(source_path.parent().unwrap()).unwrap();
    fs::write(
        root.join("Ezra.toml"),
        "[build]\ntarget = \"custom-unknown-ez80\"\n\n[lsp]\nmode = \"library\"\n\n[sdk]\npaths = [\"sdk\"]\n",
    )
    .unwrap();
    fs::write(sdk_root.join("math.ezra"), "pub const VALUE: u8 = 42\n").unwrap();
    let document = OpenDocument {
        path: source_path,
        text: "import math\npub fn answer() -> u8 { return math.VALUE }\n".to_owned(),
        version: None,
    };

    let diagnostics = check_document_diagnostics(&document);
    assert!(diagnostics.is_empty(), "{diagnostics:#?}");

    let _ = fs::remove_dir_all(root);
}

#[test]
fn multi_target_projects_create_an_lsp_context_for_every_target() {
    let root = std::env::temp_dir().join(format!("ezrac-lsp-multi-target-{}", std::process::id()));
    let source_path = root.join("main.ezra");
    fs::create_dir_all(&root).unwrap();
    fs::write(
        root.join("Ezra.toml"),
        "[build]\ntarget = [\"agonlight-mos-ez80\", \"cpm-2.2-z80\"]\n",
    )
    .unwrap();

    let sdks = sdks_for_path(&source_path).unwrap();
    assert_eq!(
        sdks.iter()
            .filter_map(|sdk| sdk.target.as_deref())
            .collect::<Vec<_>>(),
        vec!["agonlight-mos-ez80", "cpm-2.2-z80"]
    );

    let warning = diagnostic_to_lsp_source_with_severity(
        "fn main() { platform_init() }",
        &source_path,
        &Diagnostic::new("[cpm-2.2-z80] unknown function `platform_init`"),
        2,
    );
    assert_eq!(warning.severity, 2);
    assert!(warning.message.starts_with("[cpm-2.2-z80]"));

    let _ = fs::remove_dir_all(root);
}

#[test]
fn tiny_lisp_is_clean_for_every_configured_target() {
    let path = repository_path("examples/tiny-lisp/main.ezra");
    let source = fs::read_to_string(&path).unwrap();
    let options = CompileOptions {
        source: path.clone(),
        debug_comments: false,
        default_sdk_symbols: true,
    };

    for sdk in sdks_for_path(&path).unwrap() {
        let diagnostics = check_source_semantic_diagnostics_with_sdk_and_overrides(
            &source,
            &options,
            &sdk,
            &HashMap::new(),
        );
        assert!(
            diagnostics.is_empty(),
            "{}: {diagnostics:#?}",
            sdk.target.as_deref().unwrap_or_default()
        );
    }
}

#[test]
fn cpm_examples_resolve_the_built_in_sdk_from_their_project_target() {
    let path = repository_path("examples/cpm-z80/console-output.ezra");
    let document = OpenDocument {
        text: fs::read_to_string(&path).unwrap(),
        path: path.clone(),
        version: None,
    };

    let sdk = sdk_for_path(&path).unwrap();
    assert_eq!(sdk.target.as_deref(), Some("cpm-2.2-z80"));
    let diagnostics = check_document_diagnostics(&document);
    assert!(diagnostics.is_empty(), "{diagnostics:#?}");
}

#[test]
fn gameboy_examples_resolve_embeds_and_target_sdk_modules() {
    for relative in [
        "examples/gameboy/background/src/main.ezra",
        "examples/gameboy/color-input/src/main.ezra",
        "examples/gameboy/input-audio/src/main.ezra",
        "examples/gameboy/serial-hello/src/main.ezra",
        "examples/gameboy/sprite/src/main.ezra",
    ] {
        let path = repository_path(relative);
        let document = OpenDocument {
            text: fs::read_to_string(&path).unwrap(),
            path,
            version: None,
        };

        let diagnostics = check_document_diagnostics(&document);
        assert!(diagnostics.is_empty(), "{relative}: {diagnostics:#?}");
    }
}

#[test]
fn bundled_gameboy_sdk_modules_are_diagnostic_clean() {
    for module in ["audio", "color", "input", "serial", "sprites", "video"] {
        let path = repository_path(&format!("toolchains/gameboy-lr35902/sdk/gb/{module}.ezra"));
        let document = OpenDocument {
            text: fs::read_to_string(&path).unwrap(),
            path,
            version: None,
        };

        let diagnostics = check_document_diagnostics(&document);
        assert!(diagnostics.is_empty(), "gb.{module}: {diagnostics:#?}");
    }
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
        text: format!("fn add(left: u8, right: u8) -> u8 {{ return left + right }}\n{call_line}\n"),
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
fn watched_project_files_republish_layout_diagnostics_and_ignore_unrelated_files() {
    let root = std::env::temp_dir().join(format!("ezrac-lsp-watched-files-{}", std::process::id()));
    fs::create_dir_all(root.join("src")).unwrap();
    let main_path = root.join("src/main.ezra");
    let layout_path = root.join("layout.ezralayout");
    fs::write(
        root.join("Ezra.toml"),
        "[layout]\nfile = \"layout.ezralayout\"\n",
    )
    .unwrap();
    fs::write(&main_path, "fn main() {}\n").unwrap();
    fs::write(&layout_path, "this is not an EZRA layout\n").unwrap();
    let main_uri = path_to_uri(&main_path);
    let layout_uri = path_to_uri(&layout_path);
    let mut server = Server::default();
    server.documents.insert(
        main_uri,
        OpenDocument {
            path: main_path,
            text: "fn main() {}\n".to_owned(),
            version: Some(1),
        },
    );

    let mut output = Vec::new();
    server
        .handle_message(
            Message {
                id: None,
                method: "workspace/didChangeWatchedFiles".to_owned(),
                params: json!({ "changes": [{ "uri": layout_uri, "type": 2 }] }),
            },
            &mut output,
        )
        .unwrap();
    let output = String::from_utf8(output).unwrap();
    assert!(output.contains("publishDiagnostics"));
    assert!(output.contains("layout.ezralayout"), "{output}");

    fs::write(root.join("Ezra.toml"), "[layout\n").unwrap();
    let mut output = Vec::new();
    server
        .handle_message(
            Message {
                id: None,
                method: "workspace/didChangeWatchedFiles".to_owned(),
                params: json!({
                    "changes": [{ "uri": path_to_uri(&root.join("Ezra.toml")), "type": 2 }]
                }),
            },
            &mut output,
        )
        .unwrap();
    let output = String::from_utf8(output).unwrap();
    assert!(output.contains("publishDiagnostics"));
    assert!(output.contains("failed to parse"), "{output}");

    let mut output = Vec::new();
    server
        .handle_message(
            Message {
                id: None,
                method: "workspace/didChangeWatchedFiles".to_owned(),
                params: json!({
                    "changes": [{ "uri": "file:///tmp/unrelated.txt", "type": 2 }]
                }),
            },
            &mut output,
        )
        .unwrap();
    assert!(output.is_empty());

    let _ = fs::remove_dir_all(root);
}

#[test]
fn watched_import_changes_republish_dependent_project_diagnostics() {
    let root =
        std::env::temp_dir().join(format!("ezrac-lsp-watched-import-{}", std::process::id()));
    fs::create_dir_all(root.join("src/lib")).unwrap();
    fs::write(
        root.join("Ezra.toml"),
        "[build]\ntarget = \"custom-unknown-ez80\"\n",
    )
    .unwrap();
    let main_path = root.join("src/main.ezra");
    let import_path = root.join("src/lib/math.ezra");
    let main_source = "import lib.math\nfn main() { math.increment() }\n";
    fs::write(&main_path, main_source).unwrap();
    fs::write(&import_path, "pub fn increment() {}\n").unwrap();
    let mut server = Server::default();
    server.documents.insert(
        path_to_uri(&main_path),
        OpenDocument {
            path: main_path,
            text: main_source.to_owned(),
            version: Some(1),
        },
    );

    fs::write(&import_path, "pub fn increment(\n").unwrap();
    let import_uri = path_to_uri(&import_path);
    let mut output = Vec::new();
    server
        .handle_message(
            Message {
                id: None,
                method: "workspace/didChangeWatchedFiles".to_owned(),
                params: json!({ "changes": [{ "uri": import_uri, "type": 2 }] }),
            },
            &mut output,
        )
        .unwrap();
    let output = String::from_utf8(output).unwrap();

    assert!(output.contains("publishDiagnostics"));
    assert!(output.contains("lib/math.ezra"), "{output}");

    let _ = fs::remove_dir_all(root);
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
    let root = std::env::temp_dir().join(format!("ezrac-lsp-definition-{}", std::process::id()));
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
    let path = std::env::temp_dir().join(format!("ezrac-lsp-symbols-{}.ezra", std::process::id()));
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
