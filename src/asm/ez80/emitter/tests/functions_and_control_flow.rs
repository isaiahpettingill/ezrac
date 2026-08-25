use super::*;

fn lowered_function_calls(checked: &CheckedEz80Program, function_name: &str) -> Vec<String> {
    let function = checked
        .tbir
        .lowered_program
        .declarations
        .iter()
        .find_map(|declaration| match unwrapped_declaration(declaration) {
            Declaration::Function(function) if function.name == function_name => Some(function),
            _ => None,
        })
        .unwrap_or_else(|| panic!("missing lowered function `{function_name}`"));
    let mut calls = Vec::new();
    collect_stmt_calls_with_symbols(&function.body, &mut calls, None);
    calls
}

fn debug_let_store_displacement(assembly: &str, name: &str) -> i32 {
    let marker = format!("; source: let {name}:");
    let body = assembly
        .split(&marker)
        .nth(1)
        .unwrap_or_else(|| panic!("missing debug marker for local `{name}`\n{assembly}"));
    let line = body
        .lines()
        .find(|line| {
            let trimmed = line.trim_start();
            trimmed.starts_with("ld (ix") && trimmed.ends_with("), a")
        })
        .unwrap_or_else(|| panic!("missing frame store for local `{name}`\n{body}"));
    let displacement = line
        .trim()
        .strip_prefix("ld (ix")
        .and_then(|line| line.strip_suffix("), a"))
        .unwrap_or_else(|| panic!("unexpected local store syntax `{line}`"))
        .parse::<i32>()
        .unwrap_or_else(|_| panic!("unexpected ix displacement `{line}`"));
    displacement
}

#[test]
fn rejects_missing_return_value_in_non_void_function() {
    let cases = [
        r#"
                fn answer() -> u8 {
                    let value: u8 = 1
                }

                fn main() { test.pass() }
            "#,
        r#"
                fn answer() -> u8 {
                    loop {
                        break
                        return 1
                    }
                }

                fn main() { test.pass() }
            "#,
        r#"
                fn answer(flag: bool) -> u8 {
                    loop {
                        if flag {
                            break
                        } else {
                            return 1
                        }
                    }
                }

                fn main() { test.pass() }
            "#,
    ];

    for source in cases {
        let program = parse_program(Path::new("game.ezra"), source).unwrap();
        let error = emit_ez80_assembly(&program).unwrap_err();

        assert_eq!(error.message, "missing return value in function `answer`");
    }
}

#[test]
fn rejects_empty_return_in_non_void_function() {
    let source = r#"
            fn answer() -> u8 {
                return
            }

            fn main() { test.pass() }
        "#;
    let program = parse_program(Path::new("game.ezra"), source).unwrap();
    let error = emit_ez80_assembly(&program).unwrap_err();

    assert_eq!(error.message, "missing return value in function `answer`");
}

#[test]
fn rejects_value_return_in_void_function() {
    let source = r#"
            fn main() {
                return 1
            }
        "#;
    let program = parse_program(Path::new("game.ezra"), source).unwrap();
    let error = emit_ez80_assembly(&program).unwrap_err();

    assert_eq!(error.message, "void function `main` cannot return a value");
}

#[test]
fn local_target_models_pair_aliases_and_cpu_index_registers() {
    let z80 = ez80_local_target(CpuFamily::Z80);
    let register = |target: &Target, name: &str| {
        PhysReg(
            target
                .registers
                .iter()
                .position(|register| register.name == name)
                .unwrap_or_else(|| panic!("missing register `{name}`")),
        )
    };

    assert!(z80.registers_alias(register(&z80, "b"), register(&z80, "bc")));
    assert!(z80.registers_alias(register(&z80, "c"), register(&z80, "bc")));
    assert!(!z80.registers_alias(register(&z80, "b"), register(&z80, "de")));
    assert!(z80.registers.iter().any(|register| register.name == "ix"));
    assert!(z80.registers.iter().any(|register| register.name == "iy"));
    assert!(
        z80.register_classes[EZ80_MEMORY_LOCAL_CLASS.0]
            .registers
            .is_empty()
    );

    for cpu in [CpuFamily::I8080, CpuFamily::I8085] {
        let target = ez80_local_target(cpu);
        assert!(
            !target
                .registers
                .iter()
                .any(|register| register.name == "ix")
        );
        assert!(
            !target
                .registers
                .iter()
                .any(|register| register.name == "iy")
        );
    }
}

#[test]
fn nonoverlapping_storage_locals_share_static_memory() {
    let source = r#"
            global sink: u8 = 0

            fn main() {
                let first: u8 = 1
                first += 1
                sink = first
                let second: u8 = 3
                second += 1
                sink += second
                test.assert_eq_u8(sink, 6, 1)
                test.pass()
            }
        "#;
    let program = parse_program(Path::new("game.ezra"), source).unwrap();
    let assembly = emit_ez80_assembly_with_debug_comments(&program, true).unwrap();

    assert_eq!(
        debug_let_store_displacement(&assembly, "first"),
        debug_let_store_displacement(&assembly, "second"),
        "{assembly}"
    );
    let run = run_assembly_test(&assembly, 2_000).unwrap();
    assert!(run.halted, "{assembly}");
    assert_eq!(run.result_code, 0, "{assembly}");
}

#[test]
fn overlapping_storage_locals_use_distinct_static_memory() {
    let source = r#"
            global sink: u8 = 0

            fn main() {
                let first: u8 = 1
                first += 1
                let second: u8 = 3
                second += 1
                sink = first + second
                test.assert_eq_u8(sink, 6, 1)
                test.pass()
            }
        "#;
    let program = parse_program(Path::new("game.ezra"), source).unwrap();
    let assembly = emit_ez80_assembly_with_debug_comments(&program, true).unwrap();

    assert_ne!(
        debug_let_store_displacement(&assembly, "first"),
        debug_let_store_displacement(&assembly, "second"),
        "{assembly}"
    );
    let run = run_assembly_test(&assembly, 2_000).unwrap();
    assert!(run.halted, "{assembly}");
    assert_eq!(run.result_code, 0, "{assembly}");
}

#[test]
fn allocated_locals_assemble_and_run_on_ez80_z80_and_i8080() {
    let source = r#"
            fn main() {
                let first: u8 = 1
                first += 1
                let second: u8 = 3
                second += 1
                test.assert_eq_u8(first + second, 6, 1)
                test.pass()
            }
        "#;
    let program = parse_program(Path::new("game.ezra"), source).unwrap();

    for cpu in [CpuFamily::Ez80, CpuFamily::Z80, CpuFamily::I8080] {
        let classic = cpu != CpuFamily::Ez80;
        let stack_top = if classic {
            0xF000
        } else {
            EZRA_STACK_TOP.get()
        };
        let assembly = emit_ez80_assembly_with_options(
            &program,
            AssemblyOptions {
                cpu,
                ram_base: Address24::new(if classic { 0x2000 } else { EZRA_RAM_BASE.get() }),
                rodata_base: Address24::new(if classic {
                    0x3000
                } else {
                    EZRA_RODATA_BASE.get()
                }),
                stack_top: Address24::new(stack_top),
                section_bases: vec![(
                    ".rodata".to_owned(),
                    Address24::new(if classic {
                        0x3000
                    } else {
                        EZRA_RODATA_BASE.get()
                    }),
                )],
                ..AssemblyOptions::default()
            },
        )
        .unwrap();
        let run = crate::vm::run_assembly_test_with_cpu_options_at(
            cpu,
            &assembly,
            &TestRunOptions {
                instruction_budget: 3_000,
                initial_ports: Vec::new(),
                initial_memory: Vec::new(),
                stack_top,
            },
            if classic {
                0x0100
            } else {
                EZRA_LOAD_ADDR.get()
            },
        )
        .unwrap_or_else(|error| panic!("{} failed: {error}\n{assembly}", cpu.as_str()));

        assert!(run.halted, "{}\n{assembly}", cpu.as_str());
        assert_eq!(run.result_code, 0, "{}\n{assembly}", cpu.as_str());
    }
}

#[test]
fn plans_storage_for_constants_used_after_inline_asm_memory_clobber() {
    let source = r#"
            fn main() {
                let value: u8 = 7
                asm volatile(clobber memory) {
                    "nop"
                }
                let copied: u8 = value
                test.assert_eq_u8(copied, 7, 1)
                test.pass()
            }
        "#;
    let program = parse_program(Path::new("game.ezra"), source).unwrap();
    let assembly = emit_ez80_assembly(&program).unwrap();
    let run = run_assembly_test(&assembly, 2_000).unwrap();

    assert!(assembly.contains("    ; clobber memory"), "{assembly}");
    assert!(run.halted, "{assembly}");
    assert_eq!(run.result_code, 0, "{assembly}");
}

#[test]
fn plans_storage_for_inline_asm_output_and_later_dependent_local() {
    let source = r#"
            fn main() {
                let result: u8 = 0
                asm volatile(out result: u8 as reg8, clobber a) {
                    "ld a, 07h"
                }
                let copied: u8 = result
                test.assert_eq_u8(copied, 7, 1)
                test.pass()
            }
        "#;
    let program = parse_program(Path::new("game.ezra"), source).unwrap();
    let assembly = emit_ez80_assembly(&program).unwrap();
    let run = run_assembly_test(&assembly, 2_000).unwrap();

    assert!(
        assembly.contains("    ; out result: u8 as reg8"),
        "{assembly}"
    );
    assert!(run.halted, "{assembly}");
    assert_eq!(run.result_code, 0, "{assembly}");
}

#[test]
fn rejects_void_function_calls_used_as_values() {
    let cases = [
        r#"
                fn effect() {}

                fn main() {
                    let value: u8 = effect()
                    test.pass()
                }
            "#,
        r#"
                fn effect() {}

                fn main() {
                    if effect() {
                        test.pass()
                    }
                    test.pass()
                }
            "#,
    ];

    for source in cases {
        let program = parse_program(Path::new("game.ezra"), source).unwrap();
        let error = emit_ez80_assembly(&program).unwrap_err();

        assert_eq!(error.message, "function `effect` does not return a value");
    }
}

#[test]
fn rejects_invalid_main_signatures() {
    for (source, expected) in [
        (
            "fn main(code: u8) {}\n",
            "main function cannot take parameters",
        ),
        (
            "fn main() -> u8 { return 0 }\n",
            "main function cannot return a value",
        ),
    ] {
        let program = parse_program(Path::new("game.ezra"), source).unwrap();
        let error = emit_ez80_assembly(&program).unwrap_err();

        assert_eq!(error.message, expected);
    }
}

#[test]
fn emits_and_runs_u8_loop_with_assertion() {
    let source = r#"
            global total: u8 = 0
            fn main() {
                let i: u8 = 0
                while i < 4 {
                    total += 2
                    i += 1
                }
                test.assert_eq_u8(total, 8, 7)
                test.pass()
            }
        "#;
    let program = parse_program(Path::new("game.ezra"), source).unwrap();
    let asm = emit_ez80_assembly(&program).unwrap();
    let run = run_assembly_test(&asm, 1_000).unwrap();

    assert!(run.halted, "{asm}");
    assert_eq!(run.result_code, 0, "{asm}");
}

#[test]
fn emits_and_runs_loop_break_and_continue() {
    let source = r#"
            fn main() {
                let i: u8 = 0
                let total: u8 = 0
                loop {
                    i += 1
                    if i == 2 {
                        continue
                    }
                    if i == 5 {
                        break
                    }
                    total += i
                }
                test.assert_eq_u8(total, 1 + 3 + 4, 1)
                test.pass()
            }
        "#;
    let program = parse_program(Path::new("game.ezra"), source).unwrap();
    let asm = emit_ez80_assembly(&program).unwrap();
    let run = run_assembly_test(&asm, 2_000).unwrap();

    assert!(run.halted, "{asm}");
    assert_eq!(run.result_code, 0, "{asm}");
}

#[test]
fn emits_and_runs_u8_function_with_returning_if_else() {
    let source = r#"
            fn choose(flag: bool) -> u8 {
                if flag {
                    return 1
                } else {
                    return 2
                }
            }

            fn main() {
                let yes: u8 = choose(true)
                let no: u8 = choose(false)
                test.assert_eq_u8(yes, 1, 9)
                test.assert_eq_u8(no, 2, 10)
                test.pass()
            }
        "#;
    let program = parse_program(Path::new("game.ezra"), source).unwrap();
    let asm = emit_ez80_assembly(&program).unwrap();
    let run = run_assembly_test(&asm, 1_000).unwrap();

    assert!(run.halted, "{asm}");
    assert_eq!(run.result_code, 0, "{asm}");
}

#[test]
fn emits_and_runs_else_if_chains() {
    let source = r#"
            fn choose(value: u8) -> u8 {
                if value == 1 {
                    return 10
                } else if value == 2 {
                    return 20
                } else if value == 3 {
                    return 30
                } else {
                    return 40
                }
            }

            fn main() {
                test.assert_eq_u8(choose(1), 10, 1)
                test.assert_eq_u8(choose(2), 20, 2)
                test.assert_eq_u8(choose(3), 30, 3)
                test.assert_eq_u8(choose(4), 40, 4)
                test.pass()
            }
        "#;
    let program = parse_program(Path::new("game.ezra"), source).unwrap();
    let asm = emit_ez80_assembly(&program).unwrap();
    let run = run_assembly_test(&asm, 3_000).unwrap();

    assert!(run.halted, "{asm}");
    assert_eq!(run.result_code, 0, "{asm}");
}

#[test]
fn emits_and_runs_function_returning_from_loop() {
    let source = r#"
            fn answer() -> u8 {
                loop {
                    return 42
                }
            }

            fn choose(flag: bool) -> u8 {
                loop {
                    if flag {
                        return 7
                    } else {
                        return 9
                    }
                }
            }

            fn main() {
                test.assert_eq_u8(answer(), 42, 1)
                test.assert_eq_u8(choose(true), 7, 2)
                test.assert_eq_u8(choose(false), 9, 3)
                test.pass()
            }
        "#;
    let program = parse_program(Path::new("game.ezra"), source).unwrap();
    let asm = emit_ez80_assembly(&program).unwrap();
    let run = run_assembly_test(&asm, 4_000).unwrap();

    assert!(run.halted, "{asm}");
    assert_eq!(run.result_code, 0, "{asm}");
}

#[test]
fn emits_and_runs_function_returning_from_true_while() {
    let source = r#"
            fn answer() -> u8 {
                while true {
                    return 42
                }
            }

            fn choose(flag: bool) -> u8 {
                while true {
                    if flag {
                        return 7
                    } else {
                        return 9
                    }
                }
            }

            fn main() {
                test.assert_eq_u8(answer(), 42, 1)
                test.assert_eq_u8(choose(true), 7, 2)
                test.assert_eq_u8(choose(false), 9, 3)
                test.pass()
            }
        "#;
    let program = parse_program(Path::new("game.ezra"), source).unwrap();
    let asm = emit_ez80_assembly(&program).unwrap();
    let run = run_assembly_test(&asm, 4_000).unwrap();

    assert!(run.halted, "{asm}");
    assert_eq!(run.result_code, 0, "{asm}");
}

#[test]
fn emits_and_runs_function_returning_from_const_true_while() {
    let source = r#"
            const RUN: bool = true
            const SHOULD_SKIP: bool = false

            fn answer() -> u8 {
                while RUN {
                    if SHOULD_SKIP {
                        return 1
                    } else {
                        return 42
                    }
                }
            }

            fn main() {
                test.assert_eq_u8(answer(), 42, 1)
                test.pass()
            }
        "#;
    let program = parse_program(Path::new("game.ezra"), source).unwrap();
    let asm = emit_ez80_assembly(&program).unwrap();
    let run = run_assembly_test(&asm, 2_000).unwrap();

    assert!(run.halted, "{asm}");
    assert_eq!(run.result_code, 0, "{asm}");
}

#[test]
fn rejects_true_while_with_break_as_missing_return() {
    let cases = [
        r#"
                fn answer() -> u8 {
                    while true {
                        break
                        return 1
                    }
                }

                fn main() { test.pass() }
            "#,
        r#"
                const RUN: bool = false

                fn answer() -> u8 {
                    while RUN {
                        return 1
                    }
                }

                fn main() { test.pass() }
            "#,
    ];

    for source in cases {
        let program = parse_program(Path::new("game.ezra"), source).unwrap();
        let error = emit_ez80_assembly(&program).unwrap_err();

        assert_eq!(error.message, "missing return value in function `answer`");
    }
}

#[test]
fn emits_and_runs_user_function_returning_u8() {
    let source = r#"
            fn answer() -> u8 {
                return 42
            }

            fn main() {
                let x: u8 = answer()
                test.assert_eq_u8(x, 42, 9)
                test.pass()
            }
        "#;
    let program = parse_program(Path::new("game.ezra"), source).unwrap();
    let asm = emit_ez80_assembly(&program).unwrap();
    let run = run_assembly_test(&asm, 1_000).unwrap();

    assert!(run.halted, "{asm}");
    assert_eq!(run.result_code, 0, "{asm}");
}

#[test]
fn emits_and_runs_two_result_user_functions_with_a_hidden_return_slot() {
    let source = r#"
            fn pair(value: u8) -> u8, u16 {
                return value + 1, 0x1234
            }

            fn main() {
                let first: u8, second: u16 = pair(4)
                test.assert_eq_u8(first, 5, 1)
                test.assert_eq_u16(second, 0x1234, 2)
                test.pass()
            }
        "#;

    for cpu in [CpuFamily::Ez80, CpuFamily::Z80] {
        let program = parse_program(Path::new("two_result.ezra"), source).unwrap();
        let asm = emit_ez80_assembly_with_options(
            &program,
            AssemblyOptions {
                cpu,
                stack_top: Address24::new(0xF000),
                ram_base: Address24::new(0x2000),
                ..AssemblyOptions::default()
            },
        )
        .unwrap_or_else(|error| panic!("{cpu:?}: {error}"));
        let run = crate::vm::run_assembly_test_with_cpu_options_at(
            cpu,
            &asm,
            &TestRunOptions {
                instruction_budget: 8_000,
                initial_ports: Vec::new(),
                initial_memory: Vec::new(),
                stack_top: 0xF000,
            },
            0x0100,
        )
        .unwrap();

        assert!(run.halted, "{cpu:?}: {asm}");
        assert_eq!(run.result_code, 0, "{cpu:?}: {asm}");
        assert!(asm.contains("call _pair"), "{cpu:?}: {asm}");
    }
}

#[test]
fn emits_and_runs_direct_two_result_forwarding_and_recursive_forwarding() {
    let source = r#"
            fn pair(value: u8) -> u8, u16 {
                return value + 1, 0x1200 + cast<u16>(value)
            }

            fn forward(value: u8) -> u8, u16 {
                return pair(value)
            }

            fn recursive(value: u8) -> u8, u16 {
                if value == 0 {
                    return 7, 0x0109
                }
                return recursive(value - 1)
            }

            fn main() {
                let first: u8, second: u16 = forward(4)
                test.assert_eq_u8(first, 5, 1)
                test.assert_eq_u16(second, 0x1204, 2)
                let recursive_first: u8, recursive_second: u16 = recursive(4)
                test.assert_eq_u8(recursive_first, 7, 3)
                test.assert_eq_u16(recursive_second, 0x0109, 4)
                test.pass()
            }
        "#;
    let program = parse_program(Path::new("two_result_forwarding.ezra"), source).unwrap();
    let asm = emit_ez80_assembly(&program).unwrap();
    let run = run_assembly_test(&asm, 20_000).unwrap();

    assert!(run.halted, "{asm}");
    assert_eq!(run.result_code, 0, "{asm}");
}

#[test]
fn rejects_scalar_calls_to_two_result_user_functions() {
    let source = r#"
            fn pair() -> u8, u8 {
                return 1, 2
            }

            fn main() {
                let value: u8 = pair()
            }
        "#;
    let program = parse_program(Path::new("two_result_scalar.ezra"), source).unwrap();
    let error = emit_ez80_assembly(&program).unwrap_err();

    assert_eq!(
        error.message,
        "two-result function `pair` requires a two-destination call"
    );
}

#[test]
fn emits_and_runs_user_function_with_u8_parameters() {
    let source = r#"
            fn inc(v: u8) -> u8 {
                return v + 1
            }

            fn add(a: u8, b: u8) -> u8 {
                return a + b
            }

            fn mix(a: u8, b: u8, c: u8) -> u8 {
                return a + b + c
            }

            fn main() {
                let x: u8 = inc(4)
                let y: u8 = add(x, 6)
                let z: u8 = mix(y, 2, 3)
                test.assert_eq_u8(z, 16, 8)
                test.pass()
            }
        "#;
    let program = parse_program(Path::new("game.ezra"), source).unwrap();
    let asm = emit_ez80_assembly(&program).unwrap();
    let run = run_assembly_test(&asm, 2_000).unwrap();

    assert!(run.halted, "{asm}");
    assert_eq!(run.result_code, 0, "{asm}");
}

#[test]
fn emits_and_runs_tbir_expanded_return_functions() {
    let source = r#"
            inline fn pressed(pad: u16, button: u16) -> bool {
                return (pad & button) != 0
            }

            fn main() {
                let pad: u16 = 0x0011
                test.assert_eq_u8(pressed(pad, 0x0010), true, 1)
                test.assert_eq_u8(pressed(pad, 0x0002), false, 2)
                test.pass()
            }
        "#;
    let program = parse_program(Path::new("game.ezra"), source).unwrap();
    let options = AssemblyOptions::default();
    let checked = CheckedEz80Program::from_program(&program, &options).unwrap();
    assert!(!lowered_function_calls(&checked, "main").contains(&"pressed".to_owned()));
    let asm = emit_ez80_assembly_from_checked(&program, &checked, options).unwrap();
    let run = run_assembly_test(&asm, 3_000).unwrap();

    assert!(run.halted, "{asm}");
    assert_eq!(run.result_code, 0, "{asm}");
    assert!(!asm.contains("call _pressed"), "{asm}");
    assert!(!asm.contains("_pressed:"), "{asm}");
}

#[test]
fn emits_and_runs_inline_functions_with_local_prefix() {
    let source = r#"
            inline fn score(value: u8) -> u8 {
                let caller: u8 = value + 1
                let doubled: u8 = caller * 2
                return doubled + 1
            }

            fn main() {
                let caller: u8 = 3
                test.assert_eq_u8(score(caller), 9, 1)
                test.pass()
            }
        "#;
    let program = parse_program(Path::new("game.ezra"), source).unwrap();
    let asm = emit_ez80_assembly(&program).unwrap();
    let run = run_assembly_test(&asm, 4_000).unwrap();

    assert!(run.halted, "{asm}");
    assert_eq!(run.result_code, 0, "{asm}");
    assert!(!asm.contains("call _score"), "{asm}");
    assert!(!asm.contains("_score:"), "{asm}");
}

#[test]
fn emits_and_runs_void_inline_functions() {
    let source = r#"
            port DEBUG: u8 = 0x0C

            inline fn send(value: u8) {
                out DEBUG, value
            }

            fn main() {
                send('A')
                test.pass()
            }
        "#;
    let program = parse_program(Path::new("game.ezra"), source).unwrap();
    let asm = emit_ez80_assembly(&program).unwrap();
    let run = run_assembly_test(&asm, 4_000).unwrap();

    assert!(run.halted, "{asm}");
    assert_eq!(run.result_code, 0, "{asm}");
    assert!(!asm.contains("call _send"), "{asm}");
    assert!(!asm.contains("_send:"), "{asm}");
}

#[test]
fn emits_and_runs_void_inline_functions_with_final_return() {
    let source = r#"
            global value: u8 = 0

            inline fn store(value_arg: u8) {
                value = value_arg
                return
            }

            fn main() {
                store(7)
                test.assert_eq_u8(value, 7, 1)
                test.pass()
            }
        "#;
    let program = parse_program(Path::new("game.ezra"), source).unwrap();
    let asm = emit_ez80_assembly(&program).unwrap();
    let run = run_assembly_test(&asm, 4_000).unwrap();

    assert!(run.halted, "{asm}");
    assert_eq!(run.result_code, 0, "{asm}");
    assert!(!asm.contains("call _store"), "{asm}");
    assert!(!asm.contains("_store:"), "{asm}");
}

#[test]
fn void_inline_functions_keep_helper_calls_reachable() {
    let source = r#"
            port DEBUG: u8 = 0x0C

            fn add_one(value: u8) -> u8 {
                return value + 1
            }

            inline fn send_next(value: u8) {
                let next: u8 = add_one(value)
                out DEBUG, next
            }

            fn main() {
                send_next(4)
                test.pass()
            }
        "#;
    let program = parse_program(Path::new("game.ezra"), source).unwrap();
    let asm = emit_ez80_assembly(&program).unwrap();
    let run = run_assembly_test(&asm, 4_000).unwrap();

    assert!(run.halted, "{asm}");
    assert_eq!(run.result_code, 0, "{asm}");
    assert!(asm.contains("_add_one:"), "{asm}");
    assert!(asm.contains("call _add_one"), "{asm}");
    assert!(!asm.contains("_send_next:"), "{asm}");
    assert!(!asm.contains("call _send_next"), "{asm}");
}

#[test]
fn explicit_inline_preserves_typed_argument_order_and_inlines_nested_helpers() {
    let source = r#"
            global sequence: u8 = 0

            fn next() -> u8 {
                sequence += 1
                return sequence
            }

            inline fn decimal_pair(first: u8, second: u8) -> u8 {
                return first * 10 + second
            }

            inline fn nested_pair(first: u8, second: u8) -> u8 {
                return decimal_pair(first, second)
            }

            fn main() {
                test.assert_eq_u8(nested_pair(next(), next()), 12, 1)
                test.assert_eq_u8(sequence, 2, 2)
                test.pass()
            }
        "#;
    let program = parse_program(Path::new("game.ezra"), source).unwrap();
    let asm = emit_ez80_assembly(&program).unwrap();
    let run = run_assembly_test(&asm, 6_000).unwrap();

    assert!(run.halted, "{asm}");
    assert_eq!(run.result_code, 0, "{asm}");
    assert_eq!(asm.matches("call _next").count(), 2, "{asm}");
    assert!(!asm.contains("call _decimal_pair"), "{asm}");
    assert!(!asm.contains("call _nested_pair"), "{asm}");
    assert!(!asm.contains("_decimal_pair:"), "{asm}");
    assert!(!asm.contains("_nested_pair:"), "{asm}");
}

#[test]
fn tbir_preserves_unsafe_condition_inline_calls_as_calls() {
    let source = r#"
            global checks: u8 = 0

            inline fn ready() -> bool {
                checks += 1
                return checks < 3
            }

            fn main() {
                let short_circuit: bool = false && ready()
                while ready() {}
                test.assert_eq_u8(checks, 3, 1)
                test.pass()
            }
        "#;
    let program = parse_program(Path::new("game.ezra"), source).unwrap();
    let options = AssemblyOptions::default();
    let checked = CheckedEz80Program::from_program(&program, &options).unwrap();
    assert_eq!(
        lowered_function_calls(&checked, "main")
            .iter()
            .filter(|name| name.as_str() == "ready")
            .count(),
        2
    );
    let asm = emit_ez80_assembly_from_checked(&program, &checked, options).unwrap();
    let run = run_assembly_test(&asm, 6_000).unwrap();

    assert!(run.halted, "{asm}");
    assert_eq!(run.result_code, 0, "{asm}");
    assert!(asm.contains("_ready:"), "{asm}");
    assert_eq!(asm.matches("call _ready").count(), 2, "{asm}");
}

#[test]
fn recursive_inline_functions_fall_back_to_calls() {
    let source = r#"
            inline fn self_call(value: u8) -> u8 {
                if value == 0 { return 0 }
                return self_call(value - 1) + 1
            }

            fn main() {
                test.assert_eq_u8(self_call(3), 3, 1)
                test.pass()
            }
        "#;
    let program = parse_program(Path::new("game.ezra"), source).unwrap();
    let asm = emit_ez80_assembly(&program).unwrap();
    let run = run_assembly_test(&asm, 4_000).unwrap();

    assert!(run.halted, "{asm}");
    assert_eq!(run.result_code, 0, "{asm}");
    assert!(asm.contains("_self_call:"), "{asm}");
    assert!(asm.contains("call _self_call"), "{asm}");
}

#[test]
fn tbir_rejects_recursive_inline_wrappers_but_keeps_safe_tail_calls() {
    let source = r#"
            inline fn count_down(value: u8) -> u8 {
                return count_down_impl(value)
            }

            fn count_down_impl(value: u8) -> u8 {
                if value == 0 {
                    return 0
                }
                return count_down(value - 1) + 1
            }

            fn main() {
                test.assert_eq_u8(count_down(4), 4, 1)
                test.pass()
            }
        "#;
    let program = parse_program(Path::new("game.ezra"), source).unwrap();
    let options = AssemblyOptions::default();
    let checked = CheckedEz80Program::from_program(&program, &options).unwrap();
    assert!(lowered_function_calls(&checked, "main").contains(&"count_down".to_owned()));
    assert!(lowered_function_calls(&checked, "count_down").contains(&"count_down_impl".to_owned()));
    let asm = emit_ez80_assembly_from_checked(&program, &checked, options).unwrap();
    let run = run_assembly_test(&asm, 20_000).unwrap();

    assert!(run.halted, "{asm}");
    assert_eq!(run.result_code, 0, "{asm}");
    assert!(asm.contains("_count_down_impl:"), "{asm}");
    assert!(asm.contains("_count_down:"), "{asm}");
    assert!(asm.contains("jp _count_down_impl"), "{asm}");
    assert!(asm.contains("call _count_down"), "{asm}");
}

#[test]
fn inline_return_functions_keep_helper_calls_reachable() {
    let source = r#"
            fn add_one(value: u8) -> u8 {
                return value + 1
            }

            inline fn add_two(value: u8) -> u8 {
                return add_one(value) + 1
            }

            fn main() {
                test.assert_eq_u8(add_two(5), 7, 1)
                test.pass()
            }
        "#;
    let program = parse_program(Path::new("game.ezra"), source).unwrap();
    let asm = emit_ez80_assembly(&program).unwrap();
    let run = run_assembly_test(&asm, 4_000).unwrap();

    assert!(run.halted, "{asm}");
    assert_eq!(run.result_code, 0, "{asm}");
    assert!(asm.contains("_add_one:"), "{asm}");
    assert!(asm.contains("call _add_one"), "{asm}");
    assert!(!asm.contains("_add_two:"), "{asm}");
    assert!(!asm.contains("call _add_two"), "{asm}");
}

#[test]
fn rewrites_and_runs_direct_tail_recursion_as_a_loop() {
    let source = r#"
            fn count(value: u8, total: u8) -> u8 {
                if value == 0 {
                    return total
                }
                return count(value - 1, total + 1)
            }

            fn main() {
                test.assert_eq_u8(count(40, 2), 42, 1)
                test.pass()
            }
        "#;
    let program = parse_program(Path::new("game.ezra"), source).unwrap();
    let asm = emit_ez80_assembly(&program).unwrap();
    let run = run_assembly_test(&asm, 20_000).unwrap();

    assert!(run.halted, "{asm}");
    assert_eq!(run.result_code, 0, "{asm}");
    assert!(asm.contains("_count:"), "{asm}");
    assert_eq!(asm.matches("call _count").count(), 1, "{asm}");
}

#[test]
fn emits_and_runs_tbir_approved_sibling_tail_call() {
    let source = r#"
            fn finish(first: u8, second: u8) -> u8 {
                return first * 10 + second
            }

            fn swap(first: u8, second: u8) -> u8 {
                return finish(second, first)
            }

            fn main() {
                test.assert_eq_u8(swap(1, 2), 21, 1)
                test.pass()
            }
        "#;
    let program = parse_program(Path::new("game.ezra"), source).unwrap();
    let options = AssemblyOptions::default();
    let checked = CheckedEz80Program::from_program(&program, &options).unwrap();
    assert!(lowered_function_calls(&checked, "swap").contains(&"finish".to_owned()));
    let asm = emit_ez80_assembly_from_checked(&program, &checked, options).unwrap();
    let run = run_assembly_test(&asm, 5_000).unwrap();

    assert!(run.halted, "{asm}");
    assert_eq!(run.result_code, 0, "{asm}");
    assert!(asm.contains("jp _finish"), "{asm}");
    assert!(!asm.contains("call _finish"), "{asm}");
}

#[test]
fn emits_and_runs_wide_third_argument_after_byte_second_argument() {
    let expected = 0x10u32 + 0x12 + 0x000345;
    let source = format!(
        r#"
            fn mixed(first: u8, second: u8, third: u24) -> u24 {{
                return cast<u24>(first) + cast<u24>(second) + third
            }}

            fn main() {{
                test.assert_eq_u24(mixed(0x10, 0x12, 0x000345), 0x{expected:06X}, 1)
                test.pass()
            }}
        "#
    );
    let program = parse_program(Path::new("game.ezra"), &source).unwrap();
    let asm = emit_ez80_assembly(&program).unwrap();
    let run = run_assembly_test(&asm, 4_000).unwrap();

    assert!(run.halted, "{asm}");
    assert_eq!(run.result_code, 0, "{asm}");
    assert!(asm.contains("call _mixed"), "{asm}");
}

#[test]
fn emits_and_runs_user_function_calls_with_explicit_casts() {
    let source = r#"
            fn low(value: u8) -> u8 {
                return value
            }

            fn wide(value: u16) -> u16 {
                return value
            }

            fn main() {
                let small: u8 = 0x12
                let big: u16 = 0x1234
                test.assert_eq_u16(wide(cast<u16>(small)), 0x0012, 1)
                test.assert_eq_u8(low(cast<u8>(big)), 0x34, 2)
                test.pass()
            }
        "#;
    let program = parse_program(Path::new("game.ezra"), source).unwrap();
    let asm = emit_ez80_assembly(&program).unwrap();
    let run = run_assembly_test(&asm, 2_000).unwrap();

    assert!(run.halted, "{asm}");
    assert_eq!(run.result_code, 0, "{asm}");
}

#[test]
fn emits_and_runs_user_function_with_wide_register_parameters() {
    let expected_pair = (0x010000u32 + 0x000123) & 0x00FF_FFFF;
    let expected_three = (0x000100u32 + 0x000020 + 0x000003) & 0x00FF_FFFF;
    let source = format!(
        r#"
            fn add_pair(a: u24, b: u24) -> u24 {{
                return a + b
            }}

            fn add_three(a: u24, b: u24, c: u24) -> u24 {{
                return a + b + c
            }}

            fn add_count(base: u24, count: u8) -> u24 {{
                return base + cast<u24>(count)
            }}

            fn main() {{
                let pair: u24 = add_pair(0x010000, 0x000123)
                let three: u24 = add_three(0x000100, 0x000020, 0x000003)
                let mixed: u24 = add_count(0x000200, 5)
                test.assert_eq_u24(pair, 0x{expected_pair:06X}, 1)
                test.assert_eq_u24(three, 0x{expected_three:06X}, 2)
                test.assert_eq_u24(mixed, 0x000205, 3)
                test.pass()
            }}
            "#
    );
    let program = parse_program(Path::new("game.ezra"), &source).unwrap();
    let asm = emit_ez80_assembly(&program).unwrap();
    let run = run_assembly_test(&asm, 4_000).unwrap();

    assert!(run.halted, "{asm}");
    assert_eq!(run.result_code, 0, "{asm}");
}

#[test]
fn emits_and_runs_user_function_with_spilled_parameters() {
    let expected_mixed = 0x000100u32 + 5 + 0x000020 + 7;
    let source = format!(
        r#"
            fn add_four(a: u8, b: u8, c: u8, d: u8) -> u8 {{
                return a + b + c + d
            }}

            fn wide_third(a: u24, b: u8, c: u24) -> u24 {{
                return a + cast<u24>(b) + c
            }}

            fn wide_third_with_extra(a: u24, b: u8, c: u24, d: u8) -> u24 {{
                return a + cast<u24>(b) + c + cast<u24>(d)
            }}

            fn main() {{
                test.assert_eq_u8(add_four(1, 2, 3, 4), 10, 1)
                test.assert_eq_u24(wide_third(0x000100, 5, 0x000020), 0x000125, 2)
                test.assert_eq_u24(wide_third_with_extra(0x000100, 5, 0x000020, 7), 0x{expected_mixed:06X}, 3)
                test.pass()
            }}
        "#
    );
    let program = parse_program(Path::new("game.ezra"), &source).unwrap();
    let asm = emit_ez80_assembly(&program).unwrap();
    let run = run_assembly_test(&asm, 6_000).unwrap();

    assert!(run.halted, "{asm}");
    assert_eq!(run.result_code, 0, "{asm}");
}

#[test]
fn emits_and_runs_typed_function_pointer_calls_on_16_and_24_bit_targets() {
    let source = r#"
            global callback: ptr<fn(u8, u8)u8> = &add
            global result: u8 = 0

            fn add(left: u8, right: u8) -> u8 {
                return left + right
            }

            fn main() {
                let local: ptr<fn(u8, u8)u8> = callback
                result = local(20, 22)
                test.assert_eq_u8(result, 42, 1)
                test.pass()
            }
        "#;
    let program = parse_program(Path::new("game.ezra"), source).unwrap();

    for cpu in [CpuFamily::Ez80, CpuFamily::Z80, CpuFamily::I8080] {
        let classic = cpu != CpuFamily::Ez80;
        let stack_top = if classic {
            0xF000
        } else {
            EZRA_STACK_TOP.get()
        };
        let rodata_base = if classic {
            0x3000
        } else {
            EZRA_RODATA_BASE.get()
        };
        let asm = emit_ez80_assembly_with_options(
            &program,
            AssemblyOptions {
                cpu,
                ram_base: Address24::new(if classic { 0x2000 } else { EZRA_RAM_BASE.get() }),
                rodata_base: Address24::new(rodata_base),
                stack_top: Address24::new(stack_top),
                section_bases: vec![(".rodata".to_owned(), Address24::new(rodata_base))],
                ..AssemblyOptions::default()
            },
        )
        .unwrap_or_else(|error| panic!("{} failed to emit: {error}", cpu.as_str()));
        let has_function_pointer_load = match cpu {
            CpuFamily::I8080 => asm.contains("call .L_fn_ptr_capture"),
            _ => asm.contains("ld hl, _add"),
        };
        assert!(has_function_pointer_load, "{asm}");
        let run = crate::vm::run_assembly_test_with_cpu_options_at(
            cpu,
            &asm,
            &TestRunOptions {
                instruction_budget: 6_000,
                initial_ports: Vec::new(),
                initial_memory: Vec::new(),
                stack_top,
            },
            if classic {
                0x0100
            } else {
                EZRA_LOAD_ADDR.get()
            },
        )
        .unwrap_or_else(|error| panic!("{} failed: {error}\n{asm}", cpu.as_str()));
        assert!(run.halted, "{} run={:?}`n{asm}", cpu.as_str(), run);
        assert_eq!(run.result_code, 0, "{}\n{asm}", cpu.as_str());
    }
}

#[test]
fn emits_typed_function_pointer_trampolines_for_wide_third_arguments() {
    let source = r#"
            global callback: ptr<fn(u8, u8, u16)u8> = &add

            fn add(first: u8, second: u8, third: u16) -> u8 {
                return first + second + cast<u8>(third)
            }

            fn main() {
                test.assert_eq_u8(callback(1, 2, 3), 6, 1)
                test.pass()
            }
        "#;
    let program = parse_program(Path::new("game.ezra"), source).unwrap();
    let asm = emit_ez80_assembly(&program).unwrap();
    let run = run_assembly_test(&asm, 6_000).unwrap();

    // Frame CPUs pass the blocked argument combination on the stack; the
    // indirect-call trampoline jumps straight to the target function.
    assert!(run.halted, "{asm}");
    assert_eq!(run.result_code, 0, "{asm}");
}

#[test]
fn validates_typed_function_pointer_call_arguments_and_results() {
    for (source, expected) in [
        (
            "global callback: ptr<fn(u8)u8> = &identity fn identity(value: u8) -> u8 { return value } fn main() { callback(1, 2) }",
            "function pointer `callback` expects 1 arguments but got 2",
        ),
        (
            "global callback: ptr<fn(u8)u8> = &identity fn identity(value: u8) -> u8 { return value } fn main() { callback(true) }",
            "type mismatch",
        ),
        (
            "global callback: ptr<fn(u8)u8> = &identity fn identity(value: u8) -> u8 { return value } fn main() { let result: u16 = callback(1) }",
            "widening without cast",
        ),
    ] {
        let program = parse_program(Path::new("game.ezra"), source).unwrap();
        let error = emit_ez80_assembly(&program).unwrap_err();
        assert!(error.message.contains(expected), "{}: {}", expected, error);
    }
}
