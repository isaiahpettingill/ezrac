use super::*;

#[test]
fn emits_and_runs_direct_port_read() {
    let source = r#"
            port PAD1_LO: u8 = 0x01
            fn main() {
                let pad: u8 = in PAD1_LO
                test.assert_eq_u8(pad, 0, 4)
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
fn emits_and_runs_inline_asm_statements() {
    let source = r#"
            fn main() {
                let ch: u8 = 0x41
                let result: u8 = 0
                asm volatile(in ch: u8 as reg8, out result: u8 as reg8, clobber a, clobber ports) {
                    "ld a, 0x41"
                    "out0 (0Ch), a"
                }
                test.pass()
            }
        "#;
    let program = parse_program(Path::new("game.ezra"), source).unwrap();
    let asm = emit_ez80_assembly(&program).unwrap();
    let run = run_assembly_test(&asm, 4_000).unwrap();

    assert!(asm.contains("    ; asm volatile"));
    assert!(asm.contains("    ; in ch: u8 as reg8"));
    assert!(asm.contains("    ; out result: u8 as reg8"));
    assert!(asm.contains("    ; clobber a, ports"));
    assert!(asm.contains("    ld a, 0x41"));
    assert!(run.halted, "{asm}");
    assert_eq!(run.result_code, 0, "{asm}");
    assert_eq!(run.debug_output, b"A", "{asm}");
}

#[test]
fn emits_and_runs_inline_asm_with_inferred_operand_classes() {
    let source = r#"
            fn main() {
                let ch: u8 = 0x53
                let result: u8 = 0
                asm volatile(in ch: u8, out result: u8, clobber a, clobber ports) {
                    "out0 (0Ch), a"
                }
                test.assert_eq_u8(result, 0x53, 1)
                test.pass()
            }
        "#;
    let program = parse_program(Path::new("game.ezra"), source).unwrap();
    let asm = emit_ez80_assembly(&program).unwrap();
    let run = run_assembly_test(&asm, 4_000).unwrap();

    assert!(asm.contains("    ; in ch: u8 as reg8"), "{asm}");
    assert!(asm.contains("    ; out result: u8 as reg8"), "{asm}");
    assert!(run.halted, "{asm}");
    assert_eq!(run.result_code, 0, "{asm}");
    assert_eq!(run.debug_output, b"S", "{asm}");
}

#[test]
fn emits_and_runs_inline_asm_reg8_and_imm_placeholders() {
    let source = r#"
            const DEBUG_PORT: u8 = 0x0C

            fn main() {
                let port: u8 = DEBUG_PORT
                let ch: u8 = 0x43
                asm volatile(in port: u8 as imm, in ch: u8 as reg8, clobber ports) {
                    "out0 ({port}), {ch}"
                }
                test.pass()
            }
        "#;
    let program = parse_program(Path::new("game.ezra"), source).unwrap();
    let asm = emit_ez80_assembly(&program).unwrap();
    let run = run_assembly_test(&asm, 4_000).unwrap();

    assert!(asm.contains("    ; in port: u8 as imm"), "{asm}");
    assert!(asm.contains("    out0 (0Ch), a"), "{asm}");
    assert!(run.halted, "{asm}");
    assert_eq!(run.result_code, 0, "{asm}");
    assert_eq!(run.debug_output, b"C", "{asm}");
}

#[test]
fn emits_and_runs_inline_asm_adc_and_sbc() {
    let source = r#"
            fn main() {
                let base: u8 = 0x40
                let result: u8 = 0
                asm volatile(in base: u8 as reg8, out result: u8 as reg8, clobber a, clobber flags) {
                    "cp 41h"
                    "adc a, 01h"
                    "cp 43h"
                    "sbc a, 00h"
                }
                test.assert_eq_u8(result, 0x41, 1)
                test.pass()
            }
        "#;
    let program = parse_program(Path::new("game.ezra"), source).unwrap();
    let asm = emit_ez80_assembly(&program).unwrap();
    let run = run_assembly_test(&asm, 4_000).unwrap();

    assert!(asm.contains("    adc a, 01h"), "{asm}");
    assert!(asm.contains("    sbc a, 00h"), "{asm}");
    assert!(run.halted, "{asm}");
    assert_eq!(run.result_code, 0, "{asm}");
}

#[test]
fn rejects_runtime_values_as_inline_asm_immediates() {
    let source = r#"
            fn main() {
                let port: u8 = 0x0C
                port = port + 1
                let ch: u8 = 0x43
                asm volatile(in port: u8 as imm, in ch: u8 as reg8, clobber ports) {
                    "out0 ({port}), {ch}"
                }
                test.pass()
            }
        "#;
    let program = parse_program(Path::new("game.ezra"), source).unwrap();
    let error = emit_ez80_assembly(&program).unwrap_err();

    assert_eq!(error.message, "unknown constant `port`");
}

#[test]
fn emits_and_runs_inline_asm_output_writeback() {
    let source = r#"
            fn main() {
                let result: u8 = 0
                asm volatile(out result: u8 as reg8, clobber a) {
                    "ld a, 07h"
                }
                test.assert_eq_u8(result, 7, 11)
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
fn emits_and_runs_inline_asm_mem_operands() {
    let source = r#"
            fn main() {
                let source: u8 = 0x2A
                let result: u8 = 0
                asm volatile(in source: u8 as mem, out result: u8 as mem, clobber a, clobber flags, clobber memory) {
                    "ld a, {source}"
                    "add a, a"
                    "ld {result}, a"
                }
                test.assert_eq_u8(result, 0x54, 1)
                test.pass()
            }
        "#;
    let program = parse_program(Path::new("game.ezra"), source).unwrap();
    let asm = emit_ez80_assembly(&program).unwrap();
    let run = run_assembly_test(&asm, 4_000).unwrap();

    assert!(asm.contains("    ; in source: u8 as mem"), "{asm}");
    assert!(asm.contains("    ; out result: u8 as mem"), "{asm}");
    assert!(asm.contains("    ; clobber a, flags, memory"), "{asm}");
    assert!(run.halted, "{asm}");
    assert_eq!(run.result_code, 0, "{asm}");
}

#[test]
fn inline_asm_memory_clobber_invalidates_local_constants() {
    let source = r#"
            fn main() {
                let value: u8 = 1
                let ptr: ptr<u8> = &value
                asm volatile(in ptr: ptr<u8> as reg24, clobber a, clobber memory) {
                    "ld a, 05h"
                    "ld (hl), a"
                }
                test.assert_eq_u8(value, 5, 1)
                test.pass()
            }
        "#;
    let program = parse_program(Path::new("game.ezra"), source).unwrap();
    let asm = emit_ez80_assembly(&program).unwrap();
    let run = run_assembly_test(&asm, 4_000).unwrap();

    assert!(asm.contains("    ld (hl), a"), "{asm}");
    assert!(run.halted, "{asm}");
    assert_eq!(run.result_code, 0, "{asm}");
}

#[test]
fn emits_and_runs_inline_asm_reg16_and_reg24_operands() {
    let source = r#"
            fn main() {
                let word: u16 = 0x1234
                let word_result: u16 = 0
                asm volatile(in word: u16 as reg16, out word_result: u16 as reg16, clobber hl, clobber flags) {
                    "inc hl"
                }

                let long: u24 = 0x040123
                let long_result: u24 = 0
                asm volatile(in long: u24 as reg24, out long_result: u24 as reg24, clobber hl, clobber flags) {
                    "inc hl"
                }

                test.assert_eq_u16(word_result, 0x1235, 1)
                test.assert_eq_u24(long_result, 0x040124, 2)
                test.pass()
            }
        "#;
    let program = parse_program(Path::new("game.ezra"), source).unwrap();
    let asm = emit_ez80_assembly(&program).unwrap();
    let run = run_assembly_test(&asm, 4_000).unwrap();

    assert!(asm.contains("    ; in word: u16 as reg16"), "{asm}");
    assert!(asm.contains("    ; out word_result: u16 as reg16"), "{asm}");
    assert!(asm.contains("    ; in long: u24 as reg24"), "{asm}");
    assert!(asm.contains("    ; out long_result: u24 as reg24"), "{asm}");
    assert!(run.halted, "{asm}");
    assert_eq!(run.result_code, 0, "{asm}");
}

#[test]
fn accepts_inline_asm_operand_alias_types() {
    let source = r#"
            alias byte = u8

            fn main() {
                let ch: byte = 0x41
                let result: byte = 0
                asm volatile(in ch: byte, out result: byte, clobber a, clobber memory) {
                    "ld a, {ch}"
                    "ld {result}, a"
                }
                test.assert_eq_u8(result, 0x41, 1)
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
fn rejects_inline_asm_operand_type_mismatch() {
    let input = r#"
            fn main() {
                let value: u16 = 0
                asm volatile(in value: u8 as reg8) {
                    "ld a, {value}"
                }
                test.pass()
            }
        "#;
    let output = r#"
            fn main() {
                let result: u8 = 0
                asm volatile(out result: u16 as reg16, clobber hl) {
                    "ld hl, 000007h"
                }
                test.pass()
            }
        "#;
    let program = parse_program(Path::new("game.ezra"), input).unwrap();
    let error = emit_ez80_assembly(&program).unwrap_err();
    assert_eq!(
        error.message,
        "inline asm input `value` declared type `u8` does not match bound type `u16`"
    );

    let program = parse_program(Path::new("game.ezra"), output).unwrap();
    let error = emit_ez80_assembly(&program).unwrap_err();
    assert_eq!(
        error.message,
        "inline asm output `result` declared type `u16` does not match bound type `u8`"
    );
}

#[test]
fn rejects_unknown_inline_asm_operand_placeholder() {
    let source = r#"
            fn main() {
                asm volatile(clobber a) {
                    "ld a, {missing}"
                }
                test.pass()
            }
        "#;
    let program = parse_program(Path::new("game.ezra"), source).unwrap();
    let error = emit_ez80_assembly(&program).unwrap_err();

    assert_eq!(
        error.message,
        "unknown inline asm operand placeholder `missing`"
    );
}

#[test]
fn rejects_duplicate_inline_asm_operands() {
    let source = r#"
            fn main() {
                let value: u8 = 0
                asm volatile(in value: u8 as reg8, out value: u8 as reg8) {
                    "ld a, 1"
                }
                test.pass()
            }
        "#;
    let program = parse_program(Path::new("game.ezra"), source).unwrap();
    let error = emit_ez80_assembly(&program).unwrap_err();

    assert_eq!(error.message, "duplicate inline asm operand `value`");
}
