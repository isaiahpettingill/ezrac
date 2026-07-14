use super::*;

#[test]
fn emits_and_runs_recursive_function_calls() {
    let source = r#"
            fn sum_to(value: u8) -> u8 {
                if value == 0 {
                    return 0
                }
                let current: u8 = value
                return current + sum_to(value - 1)
            }

            fn even(value: u8) -> bool {
                if value == 0 {
                    return true
                }
                return odd(value - 1)
            }

            fn odd(value: u8) -> bool {
                if value == 0 {
                    return false
                }
                return even(value - 1)
            }

            fn main() {
                test.assert_eq_u8(sum_to(4), 10, 1)
                test.assert_eq_u8(even(6), true, 2)
                test.assert_eq_u8(odd(6), false, 3)
                test.pass()
            }
        "#;
    let program = parse_program(Path::new("game.ezra"), source).unwrap();
    let asm = emit_ez80_assembly(&program).unwrap();
    let run = run_assembly_test(&asm, 80_000).unwrap();

    assert!(asm.contains("call _sum_to"), "{asm}");
    assert!(asm.contains("call _odd"), "{asm}");
    assert!(run.halted, "{asm}");
    assert_eq!(run.result_code, 0, "{asm}");
}

#[test]
fn emits_and_runs_recursive_function_with_stack_arguments() {
    let source = r#"
            fn stepped(value: u8, base: u8, filler: u8, step: u8) -> u8 {
                if value == 0 {
                    return base
                }
                let saved_step: u8 = step
                return saved_step + stepped(value - 1, base, filler, step)
            }

            fn main() {
                test.assert_eq_u8(stepped(3, 2, 7, 4), 14, 1)
                test.pass()
            }
        "#;
    let program = parse_program(Path::new("game.ezra"), source).unwrap();
    let asm = emit_ez80_assembly(&program).unwrap();
    let run = run_assembly_test(&asm, 12_000).unwrap();

    assert!(run.halted, "{asm}");
    assert_eq!(run.result_code, 0, "{asm}");
}

#[test]
fn omits_unused_functions_regardless_of_visibility() {
    let source = r#"
            fn used(value: u8) -> u8 {
                return value + 1
            }

            fn unused_private(value: u8) -> u8 {
                return value + 2
            }

            pub fn exported(value: u8) -> u8 {
                return value + 3
            }

            pub inline fn exported_inline(value: u8) -> u8 {
                return value + 4
            }

            fn main() {
                test.assert_eq_u8(used(4), 5, 1)
                test.pass()
            }
        "#;
    let program = parse_program(Path::new("game.ezra"), source).unwrap();
    let asm = emit_ez80_assembly(&program).unwrap();
    let run = run_assembly_test(&asm, 4_000).unwrap();

    assert!(run.halted, "{asm}");
    assert_eq!(run.result_code, 0, "{asm}");
    assert!(asm.contains("_used:"));
    assert!(!asm.contains("_exported:"));
    assert!(!asm.contains("_exported_inline:"));
    assert!(!asm.contains("_unused_private:"));
}

#[test]
fn validates_calls_in_unused_private_functions_before_omitting_them() {
    let source = r#"
            fn unused_private() {
                missing()
            }

            fn main() {
                test.pass()
            }
        "#;
    let program = parse_program(Path::new("game.ezra"), source).unwrap();
    let error = emit_ez80_assembly(&program).unwrap_err();

    assert_eq!(error.message, "unknown function `missing`");
}

#[test]
fn omits_unreachable_statements_after_terminators() {
    let source = r#"
            fn choose(flag: bool) -> u8 {
                if flag {
                    return 1
                } else {
                    return 2
                }
                test.fail(7)
                return 3
            }

            fn main() {
                test.assert_eq_u8(choose(true), 1, 1)
                test.assert_eq_u8(choose(false), 2, 2)
                test.pass()
            }
        "#;
    let program = parse_program(Path::new("game.ezra"), source).unwrap();
    let asm = emit_ez80_assembly_with_debug_comments(&program, true).unwrap();
    let run = run_assembly_test(&asm, 4_000).unwrap();

    assert!(!asm.contains("; source: test.fail(7)"), "{asm}");
    assert!(!asm.contains("; source: return 3"), "{asm}");
    assert!(run.halted, "{asm}");
    assert_eq!(run.result_code, 0, "{asm}");
}

#[test]
fn omits_unreachable_statements_after_nonbreaking_loop() {
    let source = r#"
            fn exit_loop() {
                loop {
                    return
                }
                test.fail(7)
            }

            fn main() {
                exit_loop()
                test.fail(8)
            }
        "#;
    let program = parse_program(Path::new("game.ezra"), source).unwrap();
    let asm = emit_ez80_assembly_with_debug_comments(&program, true).unwrap();
    let run = run_assembly_test(&asm, 4_000).unwrap();

    assert!(!asm.contains("; source: test.fail(7)"), "{asm}");
    assert!(run.halted, "{asm}");
    assert_eq!(run.result_code, 8, "{asm}");
}

#[test]
fn validates_unreachable_statements_before_omitting_them() {
    let source = r#"
            fn done() {
                return;
                let value: u8 = 0x100
            }

            fn main() {
                done()
                test.pass()
            }
        "#;
    let program = parse_program(Path::new("game.ezra"), source).unwrap();
    let error = emit_ez80_assembly(&program).unwrap_err();

    assert_eq!(error.message, "value 256 is outside u8 range");
}

#[test]
fn omits_constant_dead_if_and_while_branches() {
    let source = r#"
            const RUN_COLD: bool = false

            fn cold() {
                test.fail(9)
            }

            fn choose() -> u8 {
                if RUN_COLD {
                    cold()
                    return 9
                } else {
                    return 4
                }
            }

            fn main() {
                while false {
                    test.fail(7)
                }
                test.assert_eq_u8(choose(), 4, 1)
                test.pass()
            }
        "#;
    let program = parse_program(Path::new("game.ezra"), source).unwrap();
    let asm = emit_ez80_assembly_with_debug_comments(&program, true).unwrap();
    let run = run_assembly_test(&asm, 4_000).unwrap();

    assert!(!asm.contains("_cold:"), "{asm}");
    assert!(!asm.contains("; source: cold()"), "{asm}");
    assert!(!asm.contains("; source: test.fail(7)"), "{asm}");
    assert!(!asm.contains("; source: return 9"), "{asm}");
    assert!(run.halted, "{asm}");
    assert_eq!(run.result_code, 0, "{asm}");
}

#[test]
fn omits_constant_true_while_condition_checks() {
    let source = r#"
            const KEEP_RUNNING: bool = true

            fn main() {
                let count: u8 = 0
                while KEEP_RUNNING {
                    count += 1
                    if count == 3 {
                        break
                    }
                }
                test.assert_eq_u8(count, 3, 1)
                test.pass()
            }
        "#;
    let program = parse_program(Path::new("game.ezra"), source).unwrap();
    let asm = emit_ez80_assembly_with_debug_comments(&program, true).unwrap();
    let run = run_assembly_test(&asm, 4_000).unwrap();
    let while_body = asm
        .split("; source: while KEEP_RUNNING")
        .nth(1)
        .and_then(|tail| tail.split("; source: test.assert_eq_u8").next())
        .unwrap();

    assert!(!while_body.contains("    jp z, .L_endwhile"), "{asm}");
    assert!(while_body.contains("    jp .L_while"), "{asm}");
    assert!(run.halted, "{asm}");
    assert_eq!(run.result_code, 0, "{asm}");
}

#[test]
fn omits_unreachable_statements_after_const_true_while_return() {
    let source = r#"
            const KEEP_RUNNING: bool = true

            fn done() {
                while KEEP_RUNNING {
                    return
                }
                test.fail(7)
            }

            fn choose() -> u8 {
                if KEEP_RUNNING {
                    return 5
                }
                test.fail(8)
                return 9
            }

            fn main() {
                done()
                test.assert_eq_u8(choose(), 5, 1)
                test.pass()
            }
        "#;
    let program = parse_program(Path::new("game.ezra"), source).unwrap();
    let asm = emit_ez80_assembly_with_debug_comments(&program, true).unwrap();
    let run = run_assembly_test(&asm, 4_000).unwrap();

    assert!(!asm.contains("; source: test.fail(7)"), "{asm}");
    assert!(!asm.contains("; source: test.fail(8)"), "{asm}");
    assert!(!asm.contains("; source: return 9"), "{asm}");
    assert!(run.halted, "{asm}");
    assert_eq!(run.result_code, 0, "{asm}");
}

#[test]
fn validates_constant_dead_branches_before_omitting_them() {
    let source = r#"
            fn main() {
                if false {
                    let value: u8 = 0x100
                }
                test.pass()
            }
        "#;
    let program = parse_program(Path::new("game.ezra"), source).unwrap();
    let error = emit_ez80_assembly(&program).unwrap_err();

    assert_eq!(error.message, "value 256 is outside u8 range");
}

#[test]
fn omits_private_functions_only_called_from_unreachable_statements() {
    let source = r#"
            fn unreachable_private() {
                test.fail(7)
            }

            fn done() {
                return;
                unreachable_private()
            }

            fn main() {
                done()
                test.pass()
            }
        "#;
    let program = parse_program(Path::new("game.ezra"), source).unwrap();
    let asm = emit_ez80_assembly_with_debug_comments(&program, true).unwrap();
    let run = run_assembly_test(&asm, 4_000).unwrap();

    assert!(!asm.contains("_unreachable_private:"), "{asm}");
    assert!(!asm.contains("; source: unreachable_private()"), "{asm}");
    assert!(run.halted, "{asm}");
    assert_eq!(run.result_code, 0, "{asm}");
}

#[test]
fn propagates_local_scalar_constants_until_assignment() {
    let source = r#"
            fn copied() -> u8 {
                let base: u8 = 4
                let derived: u8 = base + 3
                return derived
            }

            fn assigned() -> u8 {
                let value: u8 = 4
                value = value + 1
                return value
            }

            fn main() {
                test.assert_eq_u8(copied(), 7, 1)
                test.assert_eq_u8(assigned(), 5, 2)
                test.pass()
            }
        "#;
    let program = parse_program(Path::new("game.ezra"), source).unwrap();
    let asm = emit_ez80_assembly(&program).unwrap();
    let run = run_assembly_test(&asm, 4_000).unwrap();
    let copied = asm
        .split("_copied:")
        .nth(1)
        .and_then(|tail| tail.split("_assigned:").next())
        .unwrap();
    let assigned = asm
        .split("_assigned:")
        .nth(1)
        .and_then(|tail| tail.split("section .header").next())
        .unwrap();

    assert!(copied.contains("    ld a, 07h\n    ret"), "{asm}");
    assert!(assigned.contains("    ld a, (040"), "{asm}");
    assert!(run.halted, "{asm}");
    assert_eq!(run.result_code, 0, "{asm}");
}

#[test]
fn propagates_local_pointer_constants_until_assignment() {
    let source = r#"
            global byte: u8 = 0

            fn copied_ptr() -> u24 {
                let base: ptr<u8> = &byte
                let copied: ptr<u8> = base
                return cast<u24>(copied)
            }

            fn copied_raw() -> u24 {
                let raw: ptr24 = cast<ptr24>(&byte)
                return cast<u24>(raw)
            }

            fn assigned_ptr() -> u24 {
                let value: ptr<u8> = &byte
                value = value + 1
                return cast<u24>(value)
            }

            fn main() {
                test.assert_eq_u24(copied_ptr(), cast<u24>(&byte), 1)
                test.assert_eq_u24(copied_raw(), cast<u24>(&byte), 2)
                test.assert_eq_u24(assigned_ptr(), cast<u24>(&byte) + 1, 3)
                test.pass()
            }
        "#;
    let program = parse_program(Path::new("game.ezra"), source).unwrap();
    let asm = emit_ez80_assembly(&program).unwrap();
    let run = run_assembly_test(&asm, 6_000).unwrap();
    let copied_ptr = asm
        .split("_copied_ptr:")
        .nth(1)
        .and_then(|tail| tail.split("_copied_raw:").next())
        .unwrap();
    let copied_raw = asm
        .split("_copied_raw:")
        .nth(1)
        .and_then(|tail| tail.split("_assigned_ptr:").next())
        .unwrap();
    let assigned_ptr = asm
        .split("_assigned_ptr:")
        .nth(1)
        .and_then(|tail| tail.split("section .header").next())
        .unwrap();

    assert!(copied_ptr.contains("    ld hl, 040"), "{asm}");
    assert!(copied_ptr.contains("    ret"), "{asm}");
    assert!(copied_raw.contains("    ld hl, 040"), "{asm}");
    assert!(copied_raw.contains("    ret"), "{asm}");
    assert!(assigned_ptr.contains("    ld hl, (040"), "{asm}");
    assert!(run.halted, "{asm}");
    assert_eq!(run.result_code, 0, "{asm}");
}

#[test]
fn rejects_local_shadowing() {
    let source = r#"
            global score: u8 = 0

            fn bump(value: u8) {
                let value: u8 = 1
            }

            fn main() {
                let score: u8 = 1
                test.pass()
            }
        "#;
    let program = parse_program(Path::new("game.ezra"), source).unwrap();
    let error = emit_ez80_assembly(&program).unwrap_err();

    assert_eq!(error.message, "local `score` shadows an existing name");
}
