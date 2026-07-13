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
fn diagnostics_treat_embedded_assets_as_global_values() {
    let options = CompileOptions {
        source: PathBuf::from("embedded.ezra"),
        debug_comments: false,
        default_sdk_symbols: true,
    };
    let diagnostics = check_source_diagnostics(
        "embed data: bytes = bytes [1, 2]\nfn main() { let address: ptr<u8> = &data }\n",
        &options,
    );

    assert!(
        diagnostics
            .iter()
            .all(|diagnostic| diagnostic.message != "unknown value `data`"),
        "{diagnostics:#?}"
    );
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
    let bad_call = check_source("fn helper() { missing() }\nfn main() {}\n", &options).unwrap_err();

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
fn check_keeps_repeated_reference_diagnostics_on_their_own_ast_spans() {
    let options = CompileOptions {
        source: PathBuf::from("repeated-error.ezra"),
        debug_comments: false,
        default_sdk_symbols: true,
    };
    let source = "fn main() {\n    missing()\n    missing()\n}\n";

    let diagnostics = check_source_diagnostics(source, &options);

    assert_eq!(diagnostics.len(), 2, "{diagnostics:#?}");
    assert!(
        diagnostics
            .iter()
            .all(|diagnostic| diagnostic.message == "unknown function `missing`")
    );
    assert_eq!(
        diagnostics
            .iter()
            .map(|diagnostic| diagnostic.span.as_ref().unwrap().start.line)
            .collect::<Vec<_>>(),
        [2, 3]
    );
}

#[test]
fn check_keeps_same_statement_references_on_their_own_ast_spans() {
    let options = CompileOptions {
        source: PathBuf::from("same-statement-error.ezra"),
        debug_comments: false,
        default_sdk_symbols: true,
    };
    let source = "fn main() { let value: u8 = missing + missing }\n";

    let diagnostics = check_source_diagnostics(source, &options);
    let spans = diagnostics
        .iter()
        .filter(|diagnostic| diagnostic.message == "unknown value `missing`")
        .map(|diagnostic| diagnostic.span.as_ref().unwrap())
        .collect::<Vec<_>>();

    assert_eq!(spans.len(), 2, "{diagnostics:#?}");
    assert_eq!(spans[0].start.column, 29);
    assert_eq!(spans[1].start.column, 39);
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
    let source = "import agon.vdp\nfn main() { let color: u8 = vdp.COLOR_GREEN; test.pass() }\n";

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
    let program = parse_and_resolve_imports_with_sdk(Path::new("game.ezra"), source, &sdk).unwrap();

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
    let program = parse_and_resolve_imports_with_sdk(Path::new("game.ezra"), source, &sdk).unwrap();

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
    let program = parse_and_resolve_imports_with_sdk(Path::new("game.ezra"), source, &sdk).unwrap();

    assert!(
        program
            .declarations
            .iter()
            .any(|decl| { matches!(decl, Declaration::Const(decl) if decl.name == "ACTIVE") })
    );
    assert!(
        !program
            .declarations
            .iter()
            .any(|decl| { matches!(decl, Declaration::Const(decl) if decl.name == "INACTIVE") })
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
            .filter(
                |decl| matches!(decl, Declaration::Function(function) if function.name == "value")
            )
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
                bdos.system_reset()
            }
        "#;
    let sdk = SdkResolver {
        target: Some("cpm-2.2-z80".to_owned()),
        sdk_roots: Vec::new(),
    };
    let program = parse_and_resolve_imports_with_sdk(Path::new("game.ezra"), source, &sdk).unwrap();

    assert!(program.declarations.iter().any(|decl| {
        matches!(decl, Declaration::Const(decl) if decl.name == "bdos.CONSOLE_OUTPUT")
    }));
    assert!(program.declarations.iter().any(|decl| {
        matches!(decl, Declaration::Function(function) if function.name == "bdos.console_output")
    }));
    assert!(program.declarations.iter().any(|decl| {
        matches!(decl, Declaration::Function(function) if function.name == "cpm.bdos.system_reset")
    }));
}

#[test]
fn cpm_bdos_sdk_exposes_all_cpm_2_2_system_calls() {
    let source = r#"
            import cpm.bdos

            fn main() {
                bdos.system_reset()
                bdos.console_input()
                bdos.console_output(65)
                bdos.reader_input()
                bdos.punch_output(65)
                bdos.list_output(65)
                bdos.direct_console_io(0xFF)
                bdos.get_io_byte()
                bdos.set_io_byte(0)
                bdos.print_string(0x0080)
                bdos.read_console_buffer(0x0080)
                bdos.get_console_status()
                bdos.return_version_number()
                bdos.reset_disk_system()
                bdos.select_disk(0)
                bdos.open_file(0x005C)
                bdos.close_file(0x005C)
                bdos.search_for_first(0x005C)
                bdos.search_for_next()
                bdos.delete_file(0x005C)
                bdos.read_sequential(0x005C)
                bdos.write_sequential(0x005C)
                bdos.make_file(0x005C)
                bdos.rename_file(0x005C)
                bdos.return_login_vector()
                bdos.return_current_disk()
                bdos.set_dma_address(0x0080)
                bdos.get_allocation_vector()
                bdos.write_protect_disk()
                bdos.get_read_only_vector()
                bdos.set_file_attributes(0x005C)
                bdos.disk_parameter_block()
                bdos.get_set_user_code(0xFF)
                bdos.read_random(0x005C)
                bdos.write_random(0x005C)
                bdos.compute_file_size(0x005C)
                bdos.populate_random_record(0x005C)
                let reset_status: u8 = bdos.reset_drive(1)
                bdos.access_drive(1)
                bdos.free_drive(1)
                bdos.write_random_with_zero_fill(0x005C)
            }
        "#;
    let sdk = SdkResolver {
        target: Some("cpm-2.2-z80".to_owned()),
        sdk_roots: Vec::new(),
    };
    let program = parse_and_resolve_imports_with_sdk(Path::new("game.ezra"), source, &sdk).unwrap();

    let expected_constants = [
        "bdos.SYSTEM_RESET",
        "bdos.CONSOLE_INPUT",
        "bdos.CONSOLE_OUTPUT",
        "bdos.READER_INPUT",
        "bdos.PUNCH_OUTPUT",
        "bdos.LIST_OUTPUT",
        "bdos.DIRECT_CONSOLE_IO",
        "bdos.GET_IO_BYTE",
        "bdos.SET_IO_BYTE",
        "bdos.PRINT_STRING",
        "bdos.READ_CONSOLE_BUFFER",
        "bdos.GET_CONSOLE_STATUS",
        "bdos.RETURN_VERSION_NUMBER",
        "bdos.RESET_DISK_SYSTEM",
        "bdos.SELECT_DISK",
        "bdos.OPEN_FILE",
        "bdos.CLOSE_FILE",
        "bdos.SEARCH_FOR_FIRST",
        "bdos.SEARCH_FOR_NEXT",
        "bdos.DELETE_FILE",
        "bdos.READ_SEQUENTIAL",
        "bdos.WRITE_SEQUENTIAL",
        "bdos.MAKE_FILE",
        "bdos.RENAME_FILE",
        "bdos.RETURN_LOGIN_VECTOR",
        "bdos.RETURN_CURRENT_DISK",
        "bdos.SET_DMA_ADDRESS",
        "bdos.GET_ALLOCATION_VECTOR",
        "bdos.WRITE_PROTECT_DISK",
        "bdos.GET_READ_ONLY_VECTOR",
        "bdos.SET_FILE_ATTRIBUTES",
        "bdos.GET_DISK_PARAMETER_BLOCK",
        "bdos.GET_SET_USER_CODE",
        "bdos.READ_RANDOM",
        "bdos.WRITE_RANDOM",
        "bdos.COMPUTE_FILE_SIZE",
        "bdos.SET_RANDOM_RECORD",
        "bdos.RESET_DRIVE",
        "bdos.ACCESS_DRIVE",
        "bdos.FREE_DRIVE",
        "bdos.WRITE_RANDOM_WITH_ZERO_FILL",
    ];

    for expected in expected_constants {
        assert!(
            program
                .declarations
                .iter()
                .any(|decl| matches!(decl, Declaration::Const(decl) if decl.name == expected)),
            "missing CP/M BDOS constant {expected}"
        );
    }
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
    let program = parse_and_resolve_imports_with_sdk(Path::new("game.ezra"), source, &sdk).unwrap();

    assert!(program.declarations.iter().any(|decl| {
        matches!(decl, Declaration::Function(function) if function.name == "console.write")
    }));
    assert!(program.declarations.iter().any(|decl| {
        matches!(decl, Declaration::Function(function) if function.name == "cpm.console.exit")
    }));
    assert!(program.declarations.iter().any(|decl| {
        matches!(decl, Declaration::Function(function) if function.name == "console.key_available")
    }));
    assert!(program.declarations.iter().any(|decl| {
        matches!(decl, Declaration::Function(function) if function.name == "console.read_line")
    }));
}

#[cfg(feature = "m68k")]
#[test]
fn checks_scalar_source_for_generic_m68k_target() {
    let source = r#"
        global total: u16 = 1
        fn increment(value: u16) -> u16 { return value + total }
        fn main() {
            let result: u16 = increment(2)
            if result == 3 { total = result }
        }
    "#;
    let report = check_source_with_sdk(
        source,
        &CompileOptions {
            source: PathBuf::from("m68k.ezra"),
            debug_comments: false,
            default_sdk_symbols: false,
        },
        &SdkResolver {
            target: Some("generic-m68k-bare".to_owned()),
            sdk_roots: Vec::new(),
        },
    )
    .unwrap();

    assert!(report.has_main);
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
    let program = parse_and_resolve_imports_with_sdk(Path::new("game.ezra"), source, &sdk).unwrap();

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
            import zx.io
            import zx.keyboard
            import zx.sound
            import zx.memory
            import zx.interrupt

            fn main() {
                rom.print_char(65)
                screen.border(1)
                screen.set_attr(0, 0, screen.attr(screen.WHITE, screen.BLUE, screen.BRIGHT))
                io.write_ula(0)
                let keys: u8 = keyboard.any_key()
                sound.beeper(keys)
                memory.select_128k_bank(0, 0, 0)
                interrupt.disable()
            }
        "#;
    let sdk = SdkResolver {
        target: Some("zxspectrum-z80".to_owned()),
        sdk_roots: Vec::new(),
    };
    let program = parse_and_resolve_imports_with_sdk(Path::new("game.ezra"), source, &sdk).unwrap();

    assert!(program.declarations.iter().any(|decl| {
        matches!(decl, Declaration::Function(function) if function.name == "rom.print_char")
    }));
    assert!(program.declarations.iter().any(|decl| {
        matches!(decl, Declaration::Function(function) if function.name == "screen.border")
    }));
    for name in [
        "io.write_ula",
        "keyboard.any_key",
        "sound.ay_write",
        "memory.select_128k_bank",
        "interrupt.wait_vblank",
    ] {
        assert!(
            program.declarations.iter().any(|decl| {
                matches!(decl, Declaration::Function(function) if function.name == name)
            }),
            "missing {name}"
        );
    }
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
    let program = parse_and_resolve_imports_with_sdk(Path::new("game.ezra"), source, &sdk).unwrap();

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
    let program = parse_and_resolve_imports_with_sdk(Path::new("game.ezra"), source, &sdk).unwrap();

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
    assert!(
        program
            .declarations
            .iter()
            .any(|decl| { matches!(decl, Declaration::Const(decl) if decl.name == "input.VALUE") })
    );
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
