use super::*;

#[test]
fn cpm_bdos_function_9_outputs_text_and_function_0_exits() {
    let run = run_assembly_test_with_cpu_options_at(
        CpuFamily::Z80,
        include_str!("../../../examples/cpm-z80/console-output.asm"),
        &TestRunOptions {
            instruction_budget: 1_000,
            initial_ports: Vec::new(),
            initial_memory: Vec::new(),
            stack_top: 0xFF00,
        },
        0x0100,
    )
    .unwrap();

    assert!(run.halted, "{run:?}");
    assert_eq!(run.debug_output, b"Hello from EZRA on CP/M\r\n");
    assert_eq!(run.failure, None);
}

#[test]
fn cpm_bdos_function_9_outputs_dollar_terminated_strings() {
    let run = run_assembly_test_with_cpu_options_at(
        CpuFamily::Z80,
        r#"
                ld hl, 010Eh
                ex de, hl
                ld c, 9
                call 0005h
                ld c, 0
                call 0005h
            message:
                db "EZRA CP/M$"
            "#,
        &TestRunOptions {
            instruction_budget: 1_000,
            initial_ports: Vec::new(),
            initial_memory: Vec::new(),
            stack_top: 0xFF00,
        },
        0x0100,
    )
    .unwrap();

    assert!(run.halted, "{run:?}");
    assert_eq!(run.debug_output, b"EZRA CP/M");
    assert_eq!(run.failure, None);
}

#[test]
fn runs_emitted_test_pass_on_ez80_vm() {
    let program = parse_program(Path::new("game.ezra"), "fn main() { test.pass() }").unwrap();
    let asm = emit_ez80_assembly(&program).unwrap();
    let run = run_assembly_test(&asm, 100).unwrap();

    assert!(run.halted);
    assert_eq!(run.result_code, 0);
    assert_eq!(run.failure, None);
}

#[test]
fn runs_full_program_core_language_constructs_on_ez80_vm() {
    let source = r#"
            alias Byte = u8
            alias Word = u16

            const LIMIT: Byte = 4
            const EXPECTED: Word = 1 + 9 + 3 + 4 + 9

            struct Pair {
                lo: Byte
                hi: Word
            }

            global values: [Byte; LIMIT] = [1, 2, 3, 4]
            global pair: Pair = Pair { lo: 0, hi: 0 }

            fn add_word(left: Word, right: Word) -> Word {
                return left + right
            }

            fn sum_values() -> Word {
                let index: Byte = 0
                let sum: Word = 0
                while index < LIMIT {
                    sum += cast<Word>(values[index])
                    index += 1
                }
                return sum
            }

            fn main() {
                values[1] = values[1] + 7
                pair.lo = values[1]
                pair.hi = add_word(sum_values(), cast<Word>(pair.lo))

                let ptr: ptr<u8> = &values[1]
                test.assert_eq_u8(*ptr, 9, 1)
                mem.poke8(ptr + 1, 6)
                test.assert_eq_u8(values[2], 6, 2)

                if pair.hi == EXPECTED {
                    test.pass()
                }
                test.fail(3)
            }
        "#;
    let (asm, run) = compile_and_run_source(source, 12_000);

    assert!(run.halted, "{asm}");
    assert_eq!(run.result_code, 0, "{asm}");
}

#[test]
fn runs_full_program_ports_and_volatile_mmio_on_ez80_vm() {
    let source = r#"
            port DEBUG: u8 = 0x0C
            volatile mmio STATUS: ptr<u8> = 0x040270
            volatile mmio CONTROL: ptr<u8> = 0x040271

            fn main() {
                *(CONTROL) = *STATUS + 1
                out DEBUG, *CONTROL
                test.assert_eq_u8(*CONTROL, 0x43, 1)
                test.pass()
            }
        "#;
    let program = parse_program(Path::new("game.ezra"), source).unwrap();
    let asm = emit_ez80_assembly(&program).unwrap();
    let run = run_assembly_test_with_options(
        &asm,
        &TestRunOptions {
            instruction_budget: 4_000,
            initial_ports: Vec::new(),
            initial_memory: vec![(0x040270, 0x42)],
            stack_top: EZRA_STACK_TOP.get(),
        },
    )
    .unwrap();

    assert!(run.halted, "{asm}");
    assert_eq!(run.result_code, 0, "{asm}");
    assert_eq!(run.debug_output, b"C", "{asm}");
}

#[test]
fn runs_full_program_imports_visibility_embeds_and_assets_on_ez80_vm() {
    let root = temp_root("imports_assets");
    std::fs::create_dir_all(root.join("lib")).unwrap();
    let main_path = root.join("game.ezra");
    std::fs::write(
        root.join("lib/assets.ezra"),
        r#"
                const PRIVATE_OFFSET: u8 = 1
                pub const PUBLIC_OFFSET: u8 = PRIVATE_OFFSET
                pub embed sprite: bytes = bytes [0x41, 0x42, 0x43]

                pub fn second() -> u8 {
                    return *(sprite.ptr + PUBLIC_OFFSET)
                }
            "#,
    )
    .unwrap();
    std::fs::write(
        &main_path,
        r#"
                import lib.assets

                fn main() {
                    test.assert_eq_u24(assets.sprite.len, 3, 1)
                    test.assert_eq_u8(assets.second(), 0x42, 2)
                    test.assert_eq_u8(*(lib.assets.sprite.ptr + 2), 0x43, 3)
                    test.pass()
                }
            "#,
    )
    .unwrap();
    let program = load_program(&main_path).unwrap();
    let asm = emit_ez80_assembly(&program).unwrap();
    let run = run_assembly_test(&asm, 8_000).unwrap();

    let _ = std::fs::remove_dir_all(root);
    assert!(run.halted, "{asm}");
    assert_eq!(run.result_code, 0, "{asm}");
}

#[test]
fn runs_full_program_naked_and_interrupt_functions_on_ez80_vm() {
    let source = r#"
            naked fn raw_debug() {
                asm volatile(clobber a, clobber ports) {
                    "ld a, 0x4E"
                    "out0 (0Ch), a"
                    "ret"
                }
            }

            interrupt fn irq_debug() {
                debug.char('I')
            }

            fn main() {
                raw_debug()
                irq_debug()
                test.pass()
            }
        "#;
    let (asm, run) = compile_and_run_source(source, 6_000);

    assert!(asm.contains("    reti"), "{asm}");
    assert!(run.halted, "{asm}");
    assert_eq!(run.result_code, 0, "{asm}");
    assert_eq!(run.debug_output, b"NI", "{asm}");
}

#[test]
fn runs_full_program_with_custom_layout_addresses_on_ez80_vm() {
    let source = r#"
            global value: u8 = 0x3A

            fn main() {
                test.assert_eq_u8(value, 0x3A, 1)
                test.pass()
            }
        "#;
    let program = parse_program(Path::new("game.ezra"), source).unwrap();
    let load_addr = Address24::new(0x040000);
    let entry_addr = Address24::new(0x040040);
    let code_base = Address24::new(0x040040);
    let asm = emit_ez80_assembly_with_options(
        &program,
        AssemblyOptions {
            load_addr,
            entry_addr,
            code_base,
            ram_base: Address24::new(0x050000),
            ..AssemblyOptions::default()
        },
    )
    .unwrap();
    let run = run_assembly_test_with_options_at(
        &asm,
        &TestRunOptions {
            instruction_budget: 4_000,
            initial_ports: Vec::new(),
            initial_memory: Vec::new(),
            stack_top: EZRA_STACK_TOP.get(),
        },
        load_addr.get(),
    )
    .unwrap();

    assert!(asm.contains("ld (050000h), a"), "{asm}");
    assert!(
        !asm.contains(&format!("{:06X}h", EZRA_RAM_BASE.get())),
        "{asm}"
    );
    assert!(run.halted, "{asm}");
    assert_eq!(run.result_code, 0, "{asm}");
}

#[test]
fn runs_inline_asm_nop_on_ez80_vm() {
    let source = r#"
            fn main() {
                asm volatile {
                    "nop"
                }
                test.pass()
            }
        "#;
    let program = parse_program(Path::new("game.ezra"), source).unwrap();
    let asm = emit_ez80_assembly(&program).unwrap();
    let run = run_assembly_test(&asm, 1_000).unwrap();

    assert!(asm.contains("    nop"), "{asm}");
    assert!(run.halted, "{asm}");
    assert_eq!(run.result_code, 0, "{asm}");
}
