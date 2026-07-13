use super::*;

#[test]
fn ez80_flat_harness_target_runs_and_captures_output() {
    let root = temp_root("ez80_flat_harness");
    std::fs::create_dir_all(&root).unwrap();
    let source_path = root.join("game.ezra");
    std::fs::write(
        &source_path,
        r#"
                fn main() {
                    debug.char('O')
                    debug.char('K')
                    test.pass()
                }
            "#,
    )
    .unwrap();

    let run = run_source_with_command_options(&CommandOptions {
        path: source_path.to_string_lossy().into_owned(),
        debug_comments: false,
        default_sdk_symbols: true,
        layout_path: None,
        target: Some("ezra-test-flat-ez80".to_owned()),
    })
    .unwrap();

    assert!(run.halted, "{run:?}");
    assert_eq!(run.result_code, 0);
    assert_eq!(run.debug_output, b"OK");

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn ez80_harness_target_reports_execution_traps() {
    let root = temp_root("ez80_harness_trap");
    std::fs::create_dir_all(&root).unwrap();
    let source_path = root.join("game.ezra");
    std::fs::write(
        &source_path,
        r#"
                naked fn main() {
                    asm volatile {
                        "jp 030000h"
                    }
                }
            "#,
    )
    .unwrap();

    let error = test_source_with_command_options(&CommandOptions {
        path: source_path.to_string_lossy().into_owned(),
        debug_comments: false,
        default_sdk_symbols: true,
        layout_path: None,
        target: Some("ezra-test-flat-ez80".to_owned()),
    })
    .unwrap_err();

    assert!(
        error.contains("test executed outside mapped memory at 0x030000"),
        "{error}"
    );

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn ez80_harness_preserves_port_output_ordering() {
    let root = temp_root("ez80_port_order");
    std::fs::create_dir_all(&root).unwrap();
    let source_path = root.join("game.ezra");
    std::fs::write(
        &source_path,
        r#"
                port DEBUG: u8 = 0x0C

                fn main() {
                    out DEBUG, 65
                    out DEBUG, 66
                    out DEBUG, 67
                    test.pass()
                }
            "#,
    )
    .unwrap();

    let run = run_source_with_command_options(&CommandOptions {
        path: source_path.to_string_lossy().into_owned(),
        debug_comments: false,
        default_sdk_symbols: true,
        layout_path: None,
        target: Some("ezra-test-flat-ez80".to_owned()),
    })
    .unwrap();

    assert_eq!(run.debug_output, b"ABC");
    assert_eq!(run.result_code, 0);

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn ez80_harness_preserves_inline_asm_memory_clobber_barrier() {
    let root = temp_root("ez80_asm_memory_barrier");
    std::fs::create_dir_all(&root).unwrap();
    let source_path = root.join("game.ezra");
    std::fs::write(
        &source_path,
        r#"
                global value: u8 = 1

                fn main() {
                    let before: u8 = value
                    asm volatile(clobber memory, clobber a) {
                        "ld a, 02h"
                        "ld (050000h), a"
                    }
                    let after: u8 = value
                    test.assert_eq_u8(before, 1, 1)
                    test.assert_eq_u8(after, 2, 2)
                    test.pass()
                }
            "#,
    )
    .unwrap();

    test_source_with_command_options(&CommandOptions {
        path: source_path.to_string_lossy().into_owned(),
        debug_comments: false,
        default_sdk_symbols: true,
        layout_path: None,
        target: Some("ezra-test-flat-ez80".to_owned()),
    })
    .unwrap();

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn ez80_harness_preserves_volatile_memory_ordering() {
    let root = temp_root("ez80_volatile_order");
    std::fs::create_dir_all(&root).unwrap();
    let source_path = root.join("game.ezra");
    std::fs::write(
        &source_path,
        r#"
                volatile mmio DEVICE: ptr<u8> = 0x050020

                fn main() {
                    *DEVICE = 1
                    *DEVICE = *DEVICE + 1
                    test.assert_eq_u8(*DEVICE, 2, 1)
                    test.pass()
                }
            "#,
    )
    .unwrap();

    test_source_with_command_options(&CommandOptions {
        path: source_path.to_string_lossy().into_owned(),
        debug_comments: false,
        default_sdk_symbols: true,
        layout_path: None,
        target: Some("ezra-test-flat-ez80".to_owned()),
    })
    .unwrap();

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn ez80_flat_harness_runs_complex_sdk_fixture_and_raw_artifacts() {
    let root = temp_root("flat_complex_fixture");
    std::fs::create_dir_all(&root).unwrap();
    let source_path = copy_fixture(&root, "flat_complex.ezra");

    let options = CommandOptions {
        path: source_path.to_string_lossy().into_owned(),
        debug_comments: false,
        default_sdk_symbols: true,
        layout_path: None,
        target: Some("ezra-test-flat-ez80".to_owned()),
    };
    let run = run_source_with_command_options(&options).unwrap();
    assert_eq!(run.result_code, 0, "{run:?}");
    assert_eq!(run.debug_output, b"FLAT");
    assert_eq!(run.ports[0x0D], 0);
    assert_eq!(run.ports[0x0E], 1);

    let outputs = build_source_with_command_options(&options).unwrap();
    assert_eq!(outputs.executable.extension().unwrap(), "bin");
    let executable = std::fs::read(&outputs.executable).unwrap();
    assert!(!executable.starts_with(b"MOS"), "{executable:02X?}");
    let map = std::fs::read_to_string(outputs.map).unwrap();
    assert!(map.contains(".text        0x010040"), "{map}");
    assert!(map.contains(".data        0x050000"), "{map}");
    assert!(map.contains(".assets      0x0C0000"), "{map}");
    assert!(map.contains("banner"), "{map}");

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn ez80_split_harness_runs_complex_sdk_fixture_and_split_artifacts() {
    let root = temp_root("split_complex_fixture");
    std::fs::create_dir_all(&root).unwrap();
    let source_path = copy_fixture(&root, "split_complex.ezra");

    let options = CommandOptions {
        path: source_path.to_string_lossy().into_owned(),
        debug_comments: false,
        default_sdk_symbols: true,
        layout_path: None,
        target: Some("ezra-test-split-ez80".to_owned()),
    };
    let run = run_source_with_command_options(&options).unwrap();
    assert_eq!(run.result_code, 0, "{run:?}");
    assert_eq!(run.debug_output, b"SPLIT");

    let outputs = build_source_with_command_options(&options).unwrap();
    assert_eq!(outputs.executable.extension().unwrap(), "bin");
    let map = std::fs::read_to_string(outputs.map).unwrap();
    assert!(map.contains(".text        0x020040"), "{map}");
    assert!(map.contains(".data        0x100000"), "{map}");
    assert!(map.contains(".assets      0x180000"), "{map}");
    assert!(map.contains("palette"), "{map}");

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn parses_test_port_metadata() {
    let metadata = parse_test_metadata(
        r#"
                // port 0x01 = 0x10
                // test: port 2 = 0b00100000
                // mem 0x040123 = 0x6C
                // test: mem 262436 = 0b01101101
                fn main() { test.pass() }
            "#,
    )
    .unwrap();

    assert_eq!(metadata.initial_ports, vec![(0x01, 0x10), (0x02, 0x20)]);
    assert_eq!(
        metadata.initial_memory,
        vec![(0x040123, 0x6C), (0x040124, 0x6D)]
    );

    let error = parse_test_metadata("// port 0x100 = 0").unwrap_err();
    assert!(error.contains("outside u8 range"), "{error}");

    let error = parse_test_metadata("// mem 0x1000000 = 0").unwrap_err();
    assert!(error.contains("outside u24 range"), "{error}");
}

#[test]
fn test_command_uses_port_metadata() {
    let root = temp_root("test_metadata");
    std::fs::create_dir_all(&root).unwrap();
    let source_path = root.join("game.ezra");
    std::fs::write(
        &source_path,
        r#"
                // port 0x01 = 0x10
                port PAD: u8 = 0x01
                fn main() {
                    let pad: u8 = in PAD
                    test.assert_eq_u8(pad, 0x10, 1)
                    test.pass()
                }
            "#,
    )
    .unwrap();

    test_source(source_path.to_str().unwrap()).unwrap();

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn test_command_uses_memory_metadata() {
    let root = temp_root("test_memory_metadata");
    std::fs::create_dir_all(&root).unwrap();
    let source_path = root.join("game.ezra");
    std::fs::write(
        &source_path,
        r#"
                // mem 0x040123 = 0x6C
                fn main() {
                    let byte: ptr<u8> = cast<ptr<u8>>(0x040123)
                    test.assert_eq_u8(*byte, 0x6C, 1)
                    test.pass()
                }
            "#,
    )
    .unwrap();

    test_source(source_path.to_str().unwrap()).unwrap();

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn commands_run_z80_source_on_emulator() {
    let root = temp_root("z80_source_test");
    std::fs::create_dir_all(&root).unwrap();
    let source_path = root.join("game.ezra");
    std::fs::write(
        &source_path,
        r#"
                fn main() {
                    let i: u8 = 0
                    let sum: u8 = 0
                    while i < 5 {
                        sum += i
                        i += 1
                    }
                    test.assert_eq_u8(sum, 10, 1)
                    test.pass()
                }
            "#,
    )
    .unwrap();

    test_source_with_command_options(&CommandOptions {
        path: source_path.to_string_lossy().into_owned(),
        debug_comments: false,
        default_sdk_symbols: true,
        layout_path: None,
        target: Some("zxspectrum-z80".to_owned()),
    })
    .unwrap();

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn test_command_runs_cpm_8080_and_8085_source_targets() {
    let root = temp_root("test_cpm_intel_targets");
    std::fs::create_dir_all(&root).unwrap();
    for (target, extra_asm) in [
        ("cpm-2.2-i8080", ""),
        ("cpm-2.2-i8085", "asm volatile { \"rim\" \"sim\" }"),
    ] {
        let source_path = root.join(format!("{target}.ezra"));
        std::fs::write(
            &source_path,
            format!(
                r#"
                        import cpm.bdos

                        fn main() {{
                            {extra_asm}
                            bdos.system_reset()
                        }}
                    "#
            ),
        )
        .unwrap();

        test_source_with_command_options(&CommandOptions {
            path: source_path.to_string_lossy().into_owned(),
            debug_comments: false,
            default_sdk_symbols: false,
            layout_path: None,
            target: Some(target.to_owned()),
        })
        .unwrap();
    }

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn test_command_reports_stack_overflow() {
    let root = temp_root("stack_overflow_test");
    std::fs::create_dir_all(&root).unwrap();
    let source_path = root.join("game.ezra");
    std::fs::write(
        &source_path,
        r#"
                naked fn main() {
                    asm volatile(clobber sp, clobber hl) {
                        "ld sp, 0EF0000h"
                        "ld hl, 012345h"
                        "push hl"
                    }
                }
            "#,
    )
    .unwrap();

    let error = test_source(source_path.to_str().unwrap()).unwrap_err();

    assert!(
        error.contains("test stack overflowed into non-stack memory at SP=0xEEFFFD"),
        "{error}"
    );

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn test_command_reports_execution_outside_mapped_memory() {
    let root = temp_root("outside_mapped_test");
    std::fs::create_dir_all(&root).unwrap();
    let source_path = root.join("game.ezra");
    std::fs::write(
        &source_path,
        r#"
                naked fn main() {
                    asm volatile {
                        "jp 020000h"
                    }
                }
            "#,
    )
    .unwrap();

    let error = test_source(source_path.to_str().unwrap()).unwrap_err();

    assert!(
        error.contains("test executed outside mapped memory at 0x020000"),
        "{error}"
    );

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn test_command_reports_nonzero_test_result_code() {
    let root = temp_root("nonzero_test_result");
    std::fs::create_dir_all(&root).unwrap();
    let source_path = root.join("game.ezra");
    std::fs::write(
        &source_path,
        r#"
                fn main() {
                    test.fail(37)
                }
            "#,
    )
    .unwrap();

    let error = test_source(source_path.to_str().unwrap()).unwrap_err();

    assert_eq!(error, "test failed with code 37");

    let _ = std::fs::remove_dir_all(root);
}
