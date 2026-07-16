use super::*;

#[test]
fn emits_test_pass_ports() {
    let program = parse_program(
        Path::new("game.ezra"),
        "// keep this source note in assembly\nfn main() { test.pass() }",
    )
    .unwrap();
    let asm = emit_ez80_assembly(&program).unwrap();

    assert!(asm.contains("__ezra_pass:"));
    assert!(asm.contains("__ezra_fail:"));
    assert!(asm.contains("    call __ezra_pass"));
    assert!(asm.contains("out0 (0Dh), a"));
    assert!(asm.contains("out0 (0Eh), a"));
}

#[test]
fn emits_test_fail_helper_calls() {
    let source = r#"
            fn main() {
                test.fail(7)
            }
        "#;
    let program = parse_program(Path::new("game.ezra"), source).unwrap();
    let asm = emit_ez80_assembly(&program).unwrap();
    let run = run_assembly_test(&asm, 1_000).unwrap();

    assert!(asm.contains("    call __ezra_fail"), "{asm}");
    assert!(run.halted, "{asm}");
    assert_eq!(run.result_code, 7, "{asm}");
}

#[test]
fn emits_and_runs_memcpy_runtime_helper() {
    let source = r#"
            fn main() {
                asm volatile(clobber af, clobber bc, clobber de, clobber hl, clobber memory) {
                    "ld a, 41h"
                    "ld (040300h), a"
                    "ld a, 42h"
                    "ld (040301h), a"
                    "ld a, 43h"
                    "ld (040302h), a"
                    "ld hl, 040310h"
                    "ld de, 040300h"
                    "ld bc, 000003h"
                    "call __ezra_memcpy"
                }
                test.assert_eq_u8(mem.peek8(cast<ptr<u8>>(0x040310)), 0x41, 1)
                test.assert_eq_u8(mem.peek8(cast<ptr<u8>>(0x040311)), 0x42, 2)
                test.assert_eq_u8(mem.peek8(cast<ptr<u8>>(0x040312)), 0x43, 3)
                test.pass()
            }
        "#;
    let program = parse_program(Path::new("game.ezra"), source).unwrap();
    let asm = emit_ez80_assembly(&program).unwrap();
    let run = run_assembly_test(&asm, 3_000).unwrap();

    assert!(asm.contains("__ezra_memcpy:"), "{asm}");
    assert!(
            asm.contains(
                "__ezra_memcpy:\n    push de\n    push hl\n    push bc\n    pop hl\n    ld de, 000000h\n    or a\n    sbc hl, de\n    pop hl\n    pop de\n    ret z\n    ex de, hl\n    ldir\n    ret"
            ),
            "{asm}"
        );
    assert!(run.halted, "{asm}");
    assert_eq!(run.result_code, 0, "{asm}");
}

#[test]
fn emits_and_runs_memset_runtime_helper() {
    let source = r#"
            fn main() {
                asm volatile(clobber af, clobber bc, clobber de, clobber hl, clobber memory) {
                    "ld hl, 040320h"
                    "ld a, 5Ah"
                    "ld bc, 000003h"
                    "call __ezra_memset"
                }
                test.assert_eq_u8(mem.peek8(cast<ptr<u8>>(0x040320)), 0x5A, 1)
                test.assert_eq_u8(mem.peek8(cast<ptr<u8>>(0x040321)), 0x5A, 2)
                test.assert_eq_u8(mem.peek8(cast<ptr<u8>>(0x040322)), 0x5A, 3)
                test.pass()
            }
        "#;
    let program = parse_program(Path::new("game.ezra"), source).unwrap();
    let asm = emit_ez80_assembly(&program).unwrap();
    let run = run_assembly_test(&asm, 3_000).unwrap();

    assert!(asm.contains("__ezra_memset:"), "{asm}");
    assert!(
            asm.contains(
                "__ezra_memset:\n    push hl\n    push bc\n    pop hl\n    ld de, 000000h\n    or a\n    sbc hl, de\n    pop hl\n    ret z\n    ld (hl), a\n    dec bc\n    push hl\n    push bc\n    pop hl\n    ld de, 000000h\n    or a\n    sbc hl, de\n    pop hl\n    ret z\n    push hl\n    inc hl\n    ex de, hl\n    pop hl\n    ldir\n    ret"
            ),
            "{asm}"
        );
    assert!(run.halted, "{asm}");
    assert_eq!(run.result_code, 0, "{asm}");
}

#[test]
fn emits_and_runs_memcpy_and_memset_builtins() {
    let source = r#"
            global src: [u8; 5] = [0x11, 0x22, 0x33, 0x44, 0x55]
            global dst: [u8; 5] = [0, 0, 0, 0, 0]

            fn main() {
                mem.memcpy(&dst[1], &src[0], 3)
                test.assert_eq_u8(dst[0], 0, 1)
                test.assert_eq_u8(dst[1], 0x11, 2)
                test.assert_eq_u8(dst[2], 0x22, 3)
                test.assert_eq_u8(dst[3], 0x33, 4)
                test.assert_eq_u8(dst[4], 0, 5)

                ezra.mem.memset(&dst[2], 0x7A, 2)
                test.assert_eq_u8(dst[1], 0x11, 6)
                test.assert_eq_u8(dst[2], 0x7A, 7)
                test.assert_eq_u8(dst[3], 0x7A, 8)
                test.assert_eq_u8(dst[4], 0, 9)

                mem.memcpy(&dst[4], &src[4], 0)
                mem.memset(&dst[4], 0xEE, 0)
                test.assert_eq_u8(dst[4], 0, 10)
                mem.memset(&dst[4], 0xCC, 1)
                test.assert_eq_u8(dst[4], 0xCC, 11)
                test.pass()
            }
        "#;
    let program = parse_program(Path::new("game.ezra"), source).unwrap();
    let asm = emit_ez80_assembly(&program).unwrap();
    let run = run_assembly_test(&asm, 6_000).unwrap();

    assert!(asm.contains("    call __ezra_memcpy"), "{asm}");
    assert!(asm.contains("    call __ezra_memset"), "{asm}");
    assert!(run.halted, "{asm}");
    assert_eq!(run.result_code, 0, "{asm}");
}

#[test]
fn emits_and_runs_mul_u8_runtime_helper() {
    let expected = 17u8.wrapping_mul(15);
    let source = format!(
        r#"
            fn main() {{
                asm volatile(clobber af, clobber bc, clobber de, clobber hl, clobber memory) {{
                    "ld a, 0Fh"
                    "ld c, a"
                    "ld a, 11h"
                    "call __ezra_mul_u8"
                    "ld (040330h), a"
                }}
                test.assert_eq_u8(mem.peek8(cast<ptr<u8>>(0x040330)), {expected}, 1)
                test.pass()
            }}
        "#
    );
    let program = parse_program(Path::new("game.ezra"), &source).unwrap();
    let asm = emit_ez80_assembly(&program).unwrap();
    let run = run_assembly_test(&asm, 3_000).unwrap();

    assert!(asm.contains("__ezra_mul_u8:"), "{asm}");
    assert!(run.halted, "{asm}");
    assert_eq!(run.result_code, 0, "{asm}");
}

#[test]
fn emits_and_runs_div_mod_u8_runtime_helpers() {
    let expected_div = 23u8 / 5;
    let expected_mod = 23u8 % 5;
    let source = format!(
        r#"
            fn main() {{
                asm volatile(clobber af, clobber bc, clobber de, clobber hl, clobber memory) {{
                    "ld a, 05h"
                    "ld c, a"
                    "ld a, 17h"
                    "call __ezra_div_u8"
                    "ld (040340h), a"
                    "ld a, 05h"
                    "ld c, a"
                    "ld a, 17h"
                    "call __ezra_mod_u8"
                    "ld (040341h), a"
                    "ld a, 00h"
                    "ld c, a"
                    "ld a, 17h"
                    "call __ezra_div_u8"
                    "ld (040342h), a"
                    "ld a, 00h"
                    "ld c, a"
                    "ld a, 17h"
                    "call __ezra_mod_u8"
                    "ld (040343h), a"
                }}
                test.assert_eq_u8(mem.peek8(cast<ptr<u8>>(0x040340)), {expected_div}, 1)
                test.assert_eq_u8(mem.peek8(cast<ptr<u8>>(0x040341)), {expected_mod}, 2)
                test.assert_eq_u8(mem.peek8(cast<ptr<u8>>(0x040342)), 0, 3)
                test.assert_eq_u8(mem.peek8(cast<ptr<u8>>(0x040343)), 0, 4)
                test.pass()
            }}
        "#
    );
    let program = parse_program(Path::new("game.ezra"), &source).unwrap();
    let asm = emit_ez80_assembly(&program).unwrap();
    let run = run_assembly_test(&asm, 3_000).unwrap();

    assert!(asm.contains("__ezra_div_u8:"), "{asm}");
    assert!(asm.contains("__ezra_mod_u8:"), "{asm}");
    assert!(run.halted, "{asm}");
    assert_eq!(run.result_code, 0, "{asm}");
}

#[test]
fn emits_required_assembly_sections() {
    let program = parse_program(
        Path::new("game.ezra"),
        "// keep this source note in assembly\nfn main() { test.pass() }",
    )
    .unwrap();
    let asm = emit_ez80_assembly(&program).unwrap();

    for section in [
        "section .header",
        "section .text",
        "section .rodata",
        "section .data",
        "section .bss",
        "section .assets",
        "section .scratch",
    ] {
        assert!(asm.contains(section), "{asm}");
    }
    assert!(asm.starts_with("; generated by ezrac\n"), "{asm}");
    assert!(asm.contains("; EZRA generated assembly for ez80"), "{asm}");
    assert!(
        asm.contains(";   keep this source note in assembly"),
        "{asm}"
    );
    let start = asm
        .split_once("__ezra_start:")
        .map(|(_, tail)| tail)
        .expect("assembly should contain startup label");
    let di = start
        .find("    di")
        .expect("startup should disable interrupts");
    let stack = start
        .find("    ld sp, F00000h")
        .expect("startup should initialize the stack");
    assert!(di < stack, "{asm}");
}

#[test]
fn emits_source_comments_in_debug_mode() {
    let source = r#"
            fn main() {
                let x: u8 = 4
                x += 1
                test.assert_eq_u8(x, 5, 1)
                test.pass()
            }
        "#;
    let program = parse_program(Path::new("game.ezra"), source).unwrap();
    let plain = emit_ez80_assembly(&program).unwrap();
    let asm = emit_ez80_assembly_with_debug_comments(&program, true).unwrap();
    let run = run_assembly_test(&asm, 1_000).unwrap();

    assert!(!plain.contains("; source:"), "{plain}");
    assert!(asm.contains("; source: let x: u8 = 4"), "{asm}");
    assert!(asm.contains("; source: x += 1"), "{asm}");
    assert!(
        asm.contains("; source: test.assert_eq_u8(x, 5, 1)"),
        "{asm}"
    );
    assert!(run.halted, "{asm}");
    assert_eq!(run.result_code, 0, "{asm}");
}

#[test]
fn peephole_removes_adjacent_duplicate_register_loads() {
    let asm = peephole_cleanup(
        r#"
section .text
    ld a, 01h
    ld a, 01h
    ld hl, 040000h
    ld hl, 040000h
    ld e, 02h
    ld e, 02h
    ld iy, 040000h
    ld iy, 040000h
    ld b, a
"#,
    );

    assert_eq!(asm.matches("    ld a, 01h").count(), 1, "{asm}");
    assert_eq!(asm.matches("    ld hl, 040000h").count(), 1, "{asm}");
    assert_eq!(asm.matches("    ld e, 02h").count(), 1, "{asm}");
    assert_eq!(asm.matches("    ld iy, 040000h").count(), 1, "{asm}");
    assert!(asm.contains("    ld b, a"), "{asm}");
}

#[test]
fn peephole_preserves_volatile_sensitive_operations() {
    let asm = peephole_cleanup(
        r#"
section .text
    ld a, (040000h)
    ld a, (040000h)
    ld (040000h), a
    ld (040000h), a
    in0 a, (01h)
    in0 a, (01h)
    out0 (0Ch), a
    out0 (0Ch), a
"#,
    );

    assert_eq!(asm.matches("    ld a, (040000h)").count(), 2, "{asm}");
    assert_eq!(asm.matches("    ld (040000h), a").count(), 2, "{asm}");
    assert_eq!(asm.matches("    in0 a, (01h)").count(), 2, "{asm}");
    assert_eq!(asm.matches("    out0 (0Ch), a").count(), 2, "{asm}");
}

#[test]
fn rejects_duplicate_top_level_declarations() {
    let source = r#"
            const VALUE: u8 = 1
            global VALUE: u8 = 2
            fn main() { test.pass() }
        "#;
    let program = parse_program(Path::new("game.ezra"), source).unwrap();
    let error = emit_ez80_assembly(&program).unwrap_err();

    assert_eq!(error.message, "duplicate declaration `VALUE`");
}

#[test]
fn rejects_duplicate_function_parameters() {
    let source = r#"
            fn add(value: u8, value: u8) -> u8 {
                return value
            }

            fn main() { test.pass() }
        "#;
    let program = parse_program(Path::new("game.ezra"), source).unwrap();
    let error = emit_ez80_assembly(&program).unwrap_err();

    assert_eq!(
        error.message,
        "function `add` has duplicate parameter `value`"
    );
}

#[test]
fn rejects_duplicate_struct_fields() {
    let source = r#"
            struct Pair {
                value: u8
                value: u16
            }
            fn main() { test.pass() }
        "#;
    let program = parse_program(Path::new("game.ezra"), source).unwrap();
    let error = emit_ez80_assembly(&program).unwrap_err();

    assert_eq!(error.message, "duplicate struct field `value`");
}

#[test]
fn rejects_array_and_struct_function_values() {
    let cases = [
        (
            r#"
                fn bad(values: [u8; 2]) {}
                fn main() { test.pass() }
                "#,
            "function `bad` parameter `values` type `[u8; 2]` is an array; pass it by pointer",
        ),
        (
            r#"
                fn bad() -> [u8; 2] {
                    return [1, 2]
                }
                fn main() { test.pass() }
                "#,
            "function `bad` return type `[u8; 2]` is an array; pass it by pointer",
        ),
        (
            r#"
                struct Pair { x: u8 }
                fn bad(value: Pair) {}
                fn main() { test.pass() }
                "#,
            "function `bad` parameter `value` type `Pair` is a struct; pass it by pointer",
        ),
        (
            r#"
                struct Pair { x: u8 }
                fn bad() -> Pair {
                    return Pair { x: 1 }
                }
                fn main() { test.pass() }
                "#,
            "function `bad` return type `Pair` is a struct; pass it by pointer",
        ),
        (
            r#"
                alias Bytes = [u8; 2]
                fn bad(values: Bytes) {}
                fn main() { test.pass() }
                "#,
            "function `bad` parameter `values` type `Bytes` is an array; pass it by pointer",
        ),
        (
            r#"
                struct Pair { x: u8 }
                alias AliasPair = Pair
                fn bad(value: AliasPair) {}
                fn main() { test.pass() }
                "#,
            "function `bad` parameter `value` type `AliasPair` is a struct; pass it by pointer",
        ),
    ];

    for (source, expected) in cases {
        let program = parse_program(Path::new("game.ezra"), source).unwrap();
        let error = emit_ez80_assembly(&program).unwrap_err();

        assert_eq!(error.message, expected);
    }
}
