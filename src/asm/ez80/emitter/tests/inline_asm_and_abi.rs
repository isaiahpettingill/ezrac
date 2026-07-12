use super::*;

#[test]
fn rejects_inline_asm_missing_required_clobbers() {
    let cases = [
        (
            r#"
                fn main() {
                    asm volatile {
                        "ld ix, 0"
                    }
                    test.pass()
                }
                "#,
            "inline asm uses `ix` without declaring clobber `ix`",
        ),
        (
            r#"
                fn main() {
                    asm volatile {
                        "out0 (0Ch), a"
                    }
                    test.pass()
                }
                "#,
            "inline asm uses ports without declaring clobber `ports`",
        ),
        (
            r#"
                fn main() {
                    asm volatile(clobber sp) {
                        "ld sp, 0F00000h"
                    }
                    test.pass()
                }
                "#,
            "inline asm clobber `sp` is only allowed in naked functions",
        ),
        (
            r#"
                fn main() {
                    asm volatile(clobber a) {
                        "xor a"
                    }
                    test.pass()
                }
                "#,
            "inline asm changes flags without declaring clobber `flags`",
        ),
        (
            r#"
                fn main() {
                    asm volatile {
                        "scf"
                    }
                    test.pass()
                }
                "#,
            "inline asm changes flags without declaring clobber `flags`",
        ),
        (
            r#"
                fn main() {
                    asm volatile {
                        "ld a, 1"
                    }
                    test.pass()
                }
                "#,
            "inline asm modifies `a` without declaring clobber `a`",
        ),
        (
            r#"
                fn main() {
                    asm volatile {
                        "ld hl, 040000h"
                    }
                    test.pass()
                }
                "#,
            "inline asm modifies `hl` without declaring clobber `hl`",
        ),
        (
            r#"
                fn main() {
                    asm volatile {
                        "set 0, b"
                    }
                    test.pass()
                }
                "#,
            "inline asm modifies `b` without declaring clobber `b`",
        ),
        (
            r#"
                fn main() {
                    asm volatile(clobber hl) {
                        "push hl"
                    }
                    test.pass()
                }
                "#,
            "inline asm modifies `sp` without declaring clobber `sp`",
        ),
        (
            r#"
                fn main() {
                    asm volatile(clobber hl) {
                        "pop hl"
                    }
                    test.pass()
                }
                "#,
            "inline asm modifies `sp` without declaring clobber `sp`",
        ),
        (
            r#"
                fn main() {
                    asm volatile {
                        "call .L_inline_sub"
                        "jr .L_inline_after"
                        ".L_inline_sub:"
                        "ret"
                        ".L_inline_after:"
                    }
                    test.pass()
                }
                "#,
            "inline asm modifies `af` without declaring clobber `af`",
        ),
        (
            r#"
                fn main() {
                    asm volatile {
                        "rst 10h"
                    }
                    test.pass()
                }
                "#,
            "inline asm modifies `af` without declaring clobber `af`",
        ),
        (
            r#"
                fn main() {
                    asm volatile {
                        "mlt bc"
                    }
                    test.pass()
                }
                "#,
            "inline asm modifies `bc` without declaring clobber `bc`",
        ),
        (
            r#"
                fn main() {
                    asm volatile(clobber bc, clobber de, clobber hl) {
                        "ldir"
                    }
                    test.pass()
                }
                "#,
            "inline asm changes flags without declaring clobber `flags`",
        ),
        (
            r#"
                fn main() {
                    asm volatile(clobber bc, clobber hl, clobber flags, clobber ports) {
                        "otir"
                    }
                    test.pass()
                }
                "#,
            "inline asm uses memory without declaring clobber `memory`",
        ),
        (
            r#"
                fn main() {
                    asm volatile(clobber a) {
                        "ld (hl), a"
                    }
                    test.pass()
                }
                "#,
            "inline asm uses memory without declaring clobber `memory`",
        ),
        (
            r#"
                fn main() {
                    let value: u8 = 1
                    asm volatile(in value: u8 as mem) {
                        "nop"
                    }
                    test.pass()
                }
                "#,
            "inline asm uses memory without declaring clobber `memory`",
        ),
    ];

    for (source, expected) in cases {
        let program = parse_program(Path::new("game.ezra"), source).unwrap();
        let error = emit_ez80_assembly(&program).unwrap_err();

        assert_eq!(error.message, expected);
    }
}

#[test]
fn rejects_duplicate_inline_asm_clobbers() {
    let source = r#"
            fn main() {
                asm volatile(clobber a, clobber a) {
                    "ld a, 1"
                }
                test.pass()
            }
        "#;
    let program = parse_program(Path::new("game.ezra"), source).unwrap();
    let error = emit_ez80_assembly(&program).unwrap_err();

    assert_eq!(error.message, "duplicate inline asm clobber `a`");
}

#[test]
fn rejects_unknown_inline_asm_clobbers() {
    let error = validate_inline_asm_clobbers(
        &["scratch".to_owned()],
        &["nop".to_owned()],
        false,
        AssemblerCpu::Ez80,
    )
    .unwrap_err();

    assert_eq!(error.message, "unknown inline asm clobber `scratch`");
}

#[test]
fn accepts_inline_asm_declared_flags_clobbers() {
    for clobber in ["flags", "f", "af"] {
        let source = format!(
            r#"
                fn main() {{
                    asm volatile(clobber a, clobber {clobber}) {{
                        "xor a"
                    }}
                    test.pass()
                }}
                "#
        );
        let program = parse_program(Path::new("game.ezra"), &source).unwrap();
        let asm = emit_ez80_assembly(&program).unwrap();
        let run = run_assembly_test(&asm, 1_000).unwrap();

        assert!(run.halted, "{asm}");
        assert_eq!(run.result_code, 0, "{asm}");
    }
}

#[test]
fn accepts_inline_asm_declared_register_clobbers() {
    let cases = [
        "asm volatile(clobber af, clobber flags) { \"xor a\" }",
        "asm volatile(clobber b, clobber c) { \"ld bc, 1234h\" }",
        "asm volatile(clobber h, clobber l) { \"ld hl, 040000h\" }",
        "asm volatile(clobber de, clobber hl) { \"ex de, hl\" }",
        "asm volatile(clobber bc) { \"ld b, 11h\" \"ld c, 0Fh\" \"mlt bc\" }",
        "asm volatile(clobber d, clobber e) { \"ld d, 02h\" \"ld e, 03h\" \"mlt de\" }",
        "asm volatile(clobber h, clobber l) { \"ld h, 04h\" \"ld l, 05h\" \"mlt hl\" }",
        "asm volatile(clobber b) { \"set 0, b\" \"res 0, b\" }",
    ];

    for asm_stmt in cases {
        let source = format!(
            r#"
                fn main() {{
                    {asm_stmt}
                    test.pass()
                }}
                "#
        );
        let program = parse_program(Path::new("game.ezra"), &source).unwrap();
        let asm = emit_ez80_assembly(&program).unwrap();
        let run = run_assembly_test(&asm, 1_000).unwrap();

        assert!(run.halted, "{asm}");
        assert_eq!(run.result_code, 0, "{asm}");
    }
}

#[test]
fn normal_inline_asm_preserves_callee_saved_index_registers() {
    let source = r#"
            fn main() {
                asm volatile(clobber ix, clobber iy) {
                    "ld ix, 012345h"
                    "ld iy, 06789Ah"
                }
                test.pass()
            }
        "#;
    let program = parse_program(Path::new("game.ezra"), source).unwrap();
    let asm = emit_ez80_assembly(&program).unwrap();
    let main = asm.split("_main:").nth(1).unwrap();
    let main = main.split("section .header").next().unwrap();
    let run = run_assembly_test(&asm, 1_000).unwrap();

    assert!(
            main.contains(
                "    push ix\n    push iy\n    ld ix, 012345h\n    ld iy, 06789Ah\n    pop iy\n    pop ix"
            ),
            "{asm}"
        );
    assert!(run.halted, "{asm}");
    assert_eq!(run.result_code, 0, "{asm}");
}

#[test]
fn naked_inline_asm_keeps_declared_index_clobbers_raw() {
    let source = r#"
            naked fn raw_entry() {
                asm volatile(clobber ix, clobber iy) {
                    "ld ix, 012345h"
                    "ld iy, 06789Ah"
                    "ret"
                }
            }

            fn main() {
                raw_entry()
                test.pass()
            }
        "#;
    let program = parse_program(Path::new("game.ezra"), source).unwrap();
    let asm = emit_ez80_assembly(&program).unwrap();
    let raw_entry = asm.split("_raw_entry:").nth(1).unwrap();
    let raw_entry = raw_entry.split("_main:").next().unwrap();
    let run = run_assembly_test(&asm, 1_000).unwrap();

    assert!(!raw_entry.contains("    push ix"), "{asm}");
    assert!(!raw_entry.contains("    push iy"), "{asm}");
    assert!(raw_entry.contains("    ld ix, 012345h"), "{asm}");
    assert!(raw_entry.contains("    ld iy, 06789Ah"), "{asm}");
    assert!(run.halted, "{asm}");
    assert_eq!(run.result_code, 0, "{asm}");
}

#[test]
fn accepts_inline_asm_declared_call_clobbers() {
    let source = r#"
            fn main() {
                asm volatile(clobber af, clobber bc, clobber de, clobber hl) {
                    "call .L_inline_sub"
                    "jr .L_inline_after"
                    ".L_inline_sub:"
                    "ret"
                    ".L_inline_after:"
                }
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
fn emits_and_runs_inline_asm_ldir_with_declared_clobbers() {
    let source = r#"
            global src: [u8; 3] = [0x41, 0x42, 0x43]
            global dst: [u8; 3] = [0, 0, 0]

            fn main() {
                asm volatile(clobber bc, clobber de, clobber hl, clobber flags, clobber memory) {
                    "ld hl, 040000h"
                    "ld de, 040003h"
                    "ld bc, 000003h"
                    "ldir"
                }
                test.assert_eq_u8(dst[0], 0x41, 1)
                test.assert_eq_u8(dst[1], 0x42, 2)
                test.assert_eq_u8(dst[2], 0x43, 3)
                test.pass()
            }
        "#;
    let program = parse_program(Path::new("game.ezra"), source).unwrap();
    let asm = emit_ez80_assembly(&program).unwrap();
    let run = run_assembly_test(&asm, 4_000).unwrap();

    assert!(asm.contains("    ldir"), "{asm}");
    assert!(run.halted, "{asm}");
    assert_eq!(run.result_code, 0, "{asm}");
}

#[test]
fn emits_and_runs_inline_asm_cpir_with_declared_clobbers() {
    let source = r#"
            global bytes: [u8; 3] = [0x11, 0x42, 0x33]
            global remaining: u8 = 0

            fn main() {
                asm volatile(clobber a, clobber bc, clobber hl, clobber flags, clobber memory) {
                    "ld a, 42h"
                    "ld hl, 040000h"
                    "ld bc, 000003h"
                    "cpir"
                    "ld a, c"
                    "ld (040003h), a"
                }
                test.assert_eq_u8(remaining, 1, 1)
                test.pass()
            }
        "#;
    let program = parse_program(Path::new("game.ezra"), source).unwrap();
    let asm = emit_ez80_assembly(&program).unwrap();
    let run = run_assembly_test(&asm, 4_000).unwrap();

    assert!(asm.contains("    cpir"), "{asm}");
    assert!(run.halted, "{asm}");
    assert_eq!(run.result_code, 0, "{asm}");
}

#[test]
fn emits_and_runs_inline_asm_otir_with_declared_clobbers() {
    let source = r#"
            global bytes: [u8; 2] = [0x11, 0x42]

            fn main() {
                asm volatile(clobber bc, clobber hl, clobber flags, clobber memory, clobber ports) {
                    "ld hl, 040000h"
                    "ld bc, 000220h"
                    "otir"
                }
                test.pass()
            }
        "#;
    let program = parse_program(Path::new("game.ezra"), source).unwrap();
    let asm = emit_ez80_assembly(&program).unwrap();
    let run = run_assembly_test(&asm, 4_000).unwrap();

    assert!(asm.contains("    otir"), "{asm}");
    assert!(run.halted, "{asm}");
    assert_eq!(run.result_code, 0, "{asm}");
    assert_eq!(run.ports[0x20], 0x42, "{asm}");
}

#[test]
fn emits_and_runs_naked_asm_functions_without_epilogue() {
    let source = r#"
            naked fn raw_debug() {
                asm volatile(clobber a, clobber ports) {
                    "ld a, 0x42"
                    "out0 (0Ch), a"
                    "ret"
                }
            }

            fn main() {
                raw_debug()
                test.pass()
            }
        "#;
    let program = parse_program(Path::new("game.ezra"), source).unwrap();
    let asm = emit_ez80_assembly(&program).unwrap();
    let run = run_assembly_test(&asm, 4_000).unwrap();

    let raw_debug = asm.split("_raw_debug:").nth(1).unwrap();
    let raw_debug = raw_debug.split("_main:").next().unwrap();
    assert_eq!(raw_debug.matches("    ret").count(), 1, "{asm}");
    assert!(run.halted, "{asm}");
    assert_eq!(run.result_code, 0, "{asm}");
    assert_eq!(run.debug_output, b"B", "{asm}");
}

#[test]
fn emits_naked_asm_functions_with_sp_clobber() {
    let source = r#"
            naked fn raw_entry() {
                asm volatile(clobber af, clobber bc, clobber de, clobber hl, clobber sp) {
                    "ld sp, 0F00000h"
                    "call _main"
                    "jp $"
                }
            }

            fn main() {
                test.pass()
            }
        "#;
    let program = parse_program(Path::new("game.ezra"), source).unwrap();
    let asm = emit_ez80_assembly(&program).unwrap();
    let raw_entry = asm.split("_raw_entry:").nth(1).unwrap();
    let raw_entry = raw_entry.split("_main:").next().unwrap();

    assert!(raw_entry.contains("    ld sp, 0F00000h"), "{asm}");
    assert!(raw_entry.contains("    call _main"), "{asm}");
    assert!(raw_entry.contains("    jp $"), "{asm}");
    let run = run_assembly_test(&asm, 4_000).unwrap();
    assert!(run.halted, "{asm}");
    assert_eq!(run.result_code, 0, "{asm}");
}

#[test]
fn emits_and_runs_interrupt_functions_with_reti() {
    let source = r#"
            interrupt fn vblank_irq() {
                debug.char('I')
            }

            fn main() {
                vblank_irq()
                test.pass()
            }
        "#;
    let program = parse_program(Path::new("game.ezra"), source).unwrap();
    let asm = emit_ez80_assembly(&program).unwrap();
    let run = run_assembly_test(&asm, 4_000).unwrap();

    let irq = asm.split("_vblank_irq:").nth(1).unwrap();
    let irq = irq.split("_main:").next().unwrap();
    assert!(irq.contains("    push af"), "{asm}");
    assert!(irq.contains("    push ix"), "{asm}");
    assert!(irq.contains("    push iy"), "{asm}");
    assert!(irq.contains("    pop iy"), "{asm}");
    assert!(irq.contains("    pop ix"), "{asm}");
    assert!(irq.contains("    pop af"), "{asm}");
    assert!(irq.contains("    reti"), "{asm}");
    assert!(run.halted, "{asm}");
    assert_eq!(run.result_code, 0, "{asm}");
    assert_eq!(run.debug_output, b"I", "{asm}");
}

#[test]
fn emits_interrupt_epilogue_for_explicit_return() {
    let source = r#"
            interrupt fn vblank_irq() {
                debug.char('R')
                if true {
                    return
                }
                debug.char('X')
            }

            fn main() {
                vblank_irq()
                test.pass()
            }
        "#;
    let program = parse_program(Path::new("game.ezra"), source).unwrap();
    let asm = emit_ez80_assembly(&program).unwrap();
    let run = run_assembly_test(&asm, 4_000).unwrap();

    let irq = asm.split("_vblank_irq:").nth(1).unwrap();
    let irq = irq.split("_main:").next().unwrap();
    let return_site = irq
        .split("out0 (0Ch), a")
        .nth(1)
        .expect("debug output in interrupt handler");
    assert!(return_site.contains("    pop iy"), "{asm}");
    assert!(return_site.contains("    pop ix"), "{asm}");
    assert!(return_site.contains("    pop hl"), "{asm}");
    assert!(return_site.contains("    pop af"), "{asm}");
    assert!(return_site.contains("    reti"), "{asm}");
    assert!(run.halted, "{asm}");
    assert_eq!(run.result_code, 0, "{asm}");
    assert_eq!(run.debug_output, b"R", "{asm}");
}

#[test]
fn rejects_interrupt_calls_to_non_interrupt_functions() {
    let source = r#"
            fn helper() {
                debug.char('H')
            }

            interrupt fn vblank_irq() {
                helper()
            }

            fn main() {
                test.pass()
            }
        "#;
    let program = parse_program(Path::new("game.ezra"), source).unwrap();
    let error = emit_ez80_assembly(&program).unwrap_err();

    assert_eq!(
        error.message,
        "interrupt function `vblank_irq` cannot call non-interrupt function `helper`"
    );
}

#[test]
fn emits_and_runs_naked_interrupt_functions() {
    let source = r#"
            naked interrupt fn raw_irq() {
                asm volatile(clobber a, clobber ports) {
                    "ld a, 0x4E"
                    "out0 (0Ch), a"
                    "reti"
                }
            }

            fn main() {
                raw_irq()
                test.pass()
            }
        "#;
    let program = parse_program(Path::new("game.ezra"), source).unwrap();
    let asm = emit_ez80_assembly(&program).unwrap();
    let run = run_assembly_test(&asm, 4_000).unwrap();

    let raw_irq = asm.split("_raw_irq:").nth(1).unwrap();
    let raw_irq = raw_irq.split("_main:").next().unwrap();
    assert!(!raw_irq.contains("    push af"), "{asm}");
    assert!(raw_irq.contains("    reti"), "{asm}");
    assert!(run.halted, "{asm}");
    assert_eq!(run.result_code, 0, "{asm}");
    assert_eq!(run.debug_output, b"N", "{asm}");
}

#[test]
fn rejects_duplicate_function_attributes() {
    let cases = [
        (
            r#"
                inline inline fn invalid() {}
                fn main() { test.pass() }
                "#,
            "duplicate attribute `inline` on function `invalid`",
        ),
        (
            r#"
                naked naked fn invalid() {
                    asm { "ret" }
                }
                fn main() { test.pass() }
                "#,
            "duplicate attribute `naked` on function `invalid`",
        ),
        (
            r#"
                interrupt interrupt fn invalid() {}
                fn main() { test.pass() }
                "#,
            "duplicate attribute `interrupt` on function `invalid`",
        ),
    ];

    for (source, expected) in cases {
        let program = parse_program(Path::new("game.ezra"), source).unwrap();
        let error = emit_ez80_assembly(&program).unwrap_err();

        assert_eq!(error.message, expected);
    }
}

#[test]
fn rejects_interrupt_function_parameters() {
    let source = r#"
            interrupt fn invalid(code: u8) {
                debug.char(code)
            }

            fn main() {
                test.pass()
            }
        "#;
    let program = parse_program(Path::new("game.ezra"), source).unwrap();
    let error = emit_ez80_assembly(&program).unwrap_err();

    assert_eq!(
        error.message,
        "interrupt function `invalid` cannot take parameters"
    );
}

#[test]
fn rejects_naked_interrupt_function_parameters() {
    let source = r#"
            naked interrupt fn invalid(code: u8) {
                asm volatile {
                    "reti"
                }
            }

            fn main() {
                test.pass()
            }
        "#;
    let program = parse_program(Path::new("game.ezra"), source).unwrap();
    let error = emit_ez80_assembly(&program).unwrap_err();

    assert_eq!(
        error.message,
        "interrupt function `invalid` cannot take parameters"
    );
}

#[test]
fn rejects_interrupt_function_return_values() {
    let source = r#"
            interrupt fn invalid() -> u8 {
                return 1
            }

            fn main() {
                test.pass()
            }
        "#;
    let program = parse_program(Path::new("game.ezra"), source).unwrap();
    let error = emit_ez80_assembly(&program).unwrap_err();

    assert_eq!(
        error.message,
        "interrupt function `invalid` cannot return a value"
    );
}

#[test]
fn rejects_naked_interrupt_function_return_values() {
    let source = r#"
            naked interrupt fn invalid() -> u8 {
                asm volatile {
                    "reti"
                }
            }

            fn main() {
                test.pass()
            }
        "#;
    let program = parse_program(Path::new("game.ezra"), source).unwrap();
    let error = emit_ez80_assembly(&program).unwrap_err();

    assert_eq!(
        error.message,
        "interrupt function `invalid` cannot return a value"
    );
}

#[test]
fn rejects_non_asm_statements_in_naked_functions() {
    let source = r#"
            naked fn invalid() {
                let value: u8 = 1
            }

            fn main() {
                test.pass()
            }
        "#;
    let program = parse_program(Path::new("game.ezra"), source).unwrap();
    let error = emit_ez80_assembly(&program).unwrap_err();

    assert_eq!(
        error.message,
        "naked function `invalid` may contain only asm blocks"
    );
}

#[test]
fn rejects_operand_asm_in_naked_functions() {
    let source = r#"
            naked fn invalid() {
                asm volatile(in value: u8 as reg8) {
                    "ret"
                }
            }

            fn main() {
                test.pass()
            }
        "#;
    let program = parse_program(Path::new("game.ezra"), source).unwrap();
    let error = emit_ez80_assembly(&program).unwrap_err();

    assert_eq!(
        error.message,
        "naked function `invalid` asm blocks cannot use operands"
    );
}

#[test]
fn emits_calls_to_extern_asm_functions_without_bodies() {
    let source = r#"
            extern asm fn raw_add(a: u8, b: u8) -> u8

            fn main() {
                let value: u8 = raw_add(0x17, 0x2B)
                test.assert_eq_u8(value, 0x42, 1)
                debug.char(value)
                test.pass()
            }
        "#;
    let program = parse_program(Path::new("game.ezra"), source).unwrap();
    let asm = emit_ez80_assembly(&program).unwrap();
    let linked = format!("{asm}\n_raw_add:\n    add a, b\n    ret\n");
    let run = run_assembly_test(&linked, 4_000).unwrap();

    assert!(asm.contains("    call _raw_add"), "{asm}");
    assert!(!asm.contains("_raw_add:"), "{asm}");
    assert!(run.halted, "{linked}");
    assert_eq!(run.result_code, 0, "{linked}");
    assert_eq!(run.debug_output, b"B", "{linked}");
}

#[test]
fn rejects_extern_asm_signatures_that_need_internal_arg_slots() {
    let source = r#"
            extern asm fn raw_mixed(first: u8, second: u8, third: u24) -> u24

            fn main() {
                test.pass()
            }
        "#;
    let program = parse_program(Path::new("game.ezra"), source).unwrap();
    let error = emit_ez80_assembly(&program).unwrap_err();

    assert_eq!(
        error.message,
        "extern asm function `raw_mixed` cannot use a byte second argument followed by a wide third argument"
    );
}

#[test]
fn rejects_duplicate_extern_asm_parameters() {
    let source = r#"
            extern asm fn raw_dup(value: u8, value: u8) -> u8

            fn main() {
                test.pass()
            }
        "#;
    let program = parse_program(Path::new("game.ezra"), source).unwrap();
    let error = emit_ez80_assembly(&program).unwrap_err();

    assert_eq!(
        error.message,
        "function `raw_dup` has duplicate parameter `value`"
    );
}

#[test]
fn emits_and_runs_extern_asm_stack_arguments() {
    let source = r#"
            extern asm fn raw_add4(a: u8, b: u8, c: u8, d: u8) -> u8

            fn main() {
                test.assert_eq_u8(raw_add4(1, 2, 3, 4), 10, 1)
                test.pass()
            }
        "#;
    let program = parse_program(Path::new("game.ezra"), source).unwrap();
    let asm = emit_ez80_assembly(&program).unwrap();
    let linked = format!(
        "{asm}\n_raw_add4:\n    add a, b\n    add a, c\n    ld b, a\n    ld hl, 000003h\n    add hl, sp\n    ld a, (hl)\n    add a, b\n    ret\n"
    );
    let run = run_assembly_test(&linked, 4_000).unwrap();

    assert!(asm.contains("    call _raw_add4"), "{asm}");
    assert!(!asm.contains("_raw_add4:"), "{asm}");
    assert!(run.halted, "{linked}");
    assert_eq!(run.result_code, 0, "{linked}");
}
