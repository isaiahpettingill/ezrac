use super::*;
use crate::diagnostic::SourceLocation;

#[test]
fn parses_main_with_out() {
    let program = parse_program(
        Path::new("game.ezra"),
        "port DEBUG_CHAR: u8 = 0x0C\nfn main() { out debug.DEBUG_CHAR, 'A' }",
    )
    .unwrap();

    assert!(program.main_function().is_some());
    assert_eq!(program.declarations.len(), 2);
    let main = program.main_function().unwrap();
    assert!(matches!(
        &main.body[0],
        Stmt::Out { port, .. } if port == "debug.DEBUG_CHAR"
    ));
}

#[test]
fn parses_in_port_expression() {
    let program = parse_program(
        Path::new("game.ezra"),
        "port PAD1_LO: u8 = 0x01\nfn main() { let pad: u8 = in input.PAD1_LO }",
    )
    .unwrap();

    assert!(program.main_function().is_some());
    let main = program.main_function().unwrap();
    assert!(matches!(
        &main.body[0],
        Stmt::Let {
            value: Expr::In(port),
            ..
        } if port == "input.PAD1_LO"
    ));
}

#[test]
fn parses_port_declaration_type() {
    let program = parse_program(
        Path::new("game.ezra"),
        "port DEBUG_CHAR: byte = 0x0C\nfn main() {}",
    )
    .unwrap();

    assert!(matches!(
        &program.declarations[0],
        Declaration::Port(PortDecl {
            name,
            ty: Type::Named(ty),
            ..
        }) if name == "DEBUG_CHAR" && ty == "byte"
    ));
}

#[test]
fn parses_volatile_mmio_declaration() {
    let program = parse_program(
        Path::new("game.ezra"),
        "volatile mmio FRAMEBUFFER: ptr<u8> = 0x080000\nfn main() {}",
    )
    .unwrap();

    assert!(matches!(program.declarations[0], Declaration::Mmio(_)));
}

#[test]
fn parses_type_alias_declaration() {
    let program = parse_program(
        Path::new("game.ezra"),
        "pub alias subpx = i24\nfn main() { let x: subpx = 0 }",
    )
    .unwrap();

    assert!(matches!(program.declarations[0], Declaration::Alias(_)));
}

#[test]
fn parses_module_qualified_types_and_struct_literals() {
    let program = parse_program(
        Path::new("game.ezra"),
        r#"
            fn main() {
                let value: types.Byte = 7
                let pair: types.Pair = types.Pair { lo: value, hi: 8 }
            }
            "#,
    )
    .unwrap();

    let main = program.main_function().unwrap();
    assert!(matches!(
        &main.body[0],
        Stmt::Let {
            ty: Type::Named(name),
            ..
        } if name == "types.Byte"
    ));
    assert!(matches!(
        &main.body[1],
        Stmt::Let {
            ty: Type::Named(name),
            value: Expr::StructInit { ty, .. },
            ..
        } if name == "types.Pair" && ty == "types.Pair"
    ));
}

#[test]
fn parses_array_literal_index_and_address_of_index() {
    let program = parse_program(
            Path::new("game.ezra"),
            "global palette: [u8; 4] = [1, 2]\nfn main() { palette[1] = 3\nlet p: ptr<u8> = &palette[0] }",
        )
        .unwrap();

    assert!(program.main_function().is_some());
}

#[test]
fn parses_pointer_dereference_expression_and_assignment() {
    EzraParser::parse(Rule::assign_stmt, "*p = 7").unwrap();
    EzraParser::parse(Rule::assign_stmt, "*p += 7").unwrap();
    EzraParser::parse(Rule::assign_stmt, "*(p + 1) ^= 7").unwrap();
    EzraParser::parse(Rule::assign_stmt, "*(SCRATCH) = 7").unwrap();
    EzraParser::parse(Rule::assign_stmt, "*module.PTR = 7").unwrap();
    EzraParser::parse(Rule::assign_stmt, "*pointers[0] = 7").unwrap();
    EzraParser::parse(Rule::assign_stmt, "*next_ptr() = 7").unwrap();
    EzraParser::parse(Rule::assign_stmt, "*(byte_ptr) = [4, 5, 6]").unwrap();
    EzraParser::parse(Rule::stmt, "*p += 7").unwrap();
    EzraParser::parse(Rule::stmt, "*module.PTR += 7").unwrap();
    EzraParser::parse(Rule::stmt, "*next_ptr() += 7").unwrap();
    EzraParser::parse(Rule::stmt, "*(byte_ptr) = [4, 5, 6]").unwrap();
    assert!(EzraParser::parse(Rule::expr_stmt, "*p = 7").is_err());
    let program = parse_program(
            Path::new("game.ezra"),
            "global bytes: [u8; 2] = [0, 0]\nconst PTR: ptr<u8> = &bytes[0]\nfn main() { let p: ptr<u8> = &bytes[0]; *p = 7; let x: u8 = *(p + 1); let y: u8 = *PTR }",
        )
        .unwrap();

    assert!(program.main_function().is_some());
}

#[test]
fn parses_newline_separated_deref_assignment_without_semicolon() {
    let program = parse_program(
        Path::new("game.ezra"),
        r#"
                global screen: [u8; 2] = [0, 0]

                fn main() {
                    let p: ptr<u8> = &screen[0]
                    *p = 7
                    *(p + 1) = 8
                }
            "#,
    )
    .unwrap();

    assert!(program.main_function().is_some());
}

#[test]
fn parses_else_if_as_nested_if() {
    let program = parse_program(
        Path::new("game.ezra"),
        r#"
            fn main() {
                if false {
                    test.fail(1)
                } else if true {
                    test.pass()
                } else {
                    test.fail(2)
                }
            }
            "#,
    )
    .unwrap();
    let main = program.main_function().unwrap();
    let Stmt::If { else_body, .. } = &main.body[0] else {
        panic!("unexpected statement shape: {:?}", main.body[0]);
    };

    assert_eq!(else_body.len(), 1);
    assert!(matches!(else_body[0], Stmt::If { .. }));
}

#[test]
fn parses_logical_function_call_operands() {
    let program = parse_program(
            Path::new("game.ezra"),
            "fn bump(value: bool) -> bool { return value }\nfn main() { let value: bool = false && bump(true); }",
        )
        .unwrap();
    let main = program.main_function().unwrap();
    let Stmt::Let {
        value:
            Expr::Binary {
                left,
                op: BinaryOp::And,
                right,
            },
        ..
    } = &main.body[0]
    else {
        panic!("unexpected statement shape: {:?}", main.body[0]);
    };

    assert_eq!(**left, Expr::Bool(false));
    assert!(matches!(**right, Expr::Call { ref path, .. } if path == &["bump"]));
}

#[test]
fn parses_inline_asm_statements() {
    let program = parse_program(
        Path::new("game.ezra"),
        r#"
            fn main() {
                asm volatile {
                    "ld a, 0x41"
                    "out0 (0Ch), a"
                }
            }
            "#,
    )
    .unwrap();

    let main = program.main_function().unwrap();
    assert!(matches!(
        &main.body[0],
        Stmt::Asm {
            volatile: true,
            inputs,
            outputs,
            clobbers,
            lines
        } if lines == &["ld a, 0x41", "out0 (0Ch), a"]
            && inputs.is_empty()
            && outputs.is_empty()
            && clobbers.is_empty()
    ));
}

#[test]
fn parses_inline_asm_clobbers() {
    let program = parse_program(
        Path::new("game.ezra"),
        r#"
            fn main() {
                asm volatile(clobber a, clobber ports, clobber memory) {
                    "ld a, 0x41"
                    "out0 (0Ch), a"
                }
            }
            "#,
    )
    .unwrap();

    let main = program.main_function().unwrap();
    assert!(matches!(
        &main.body[0],
        Stmt::Asm {
            volatile: true,
            inputs,
            outputs,
            clobbers,
            lines
        } if inputs.is_empty()
            && outputs.is_empty()
            && clobbers == &["a", "ports", "memory"]
            && lines.len() == 2
    ));
}

#[test]
fn parses_i8086_inline_asm_clobbers_and_pointer_register_class() {
    let program = parse_program(
        Path::new("game.ezra"),
        r#"
            alias Word = u16
            fn main() {
                asm(in address: ptr<u8> as reg16, in word: Word as reg16, clobber ax, clobber bx, clobber ds, clobber flags) {
                    "mov bx,{address}"
                }
            }
        "#,
    )
    .unwrap();

    let main = program.main_function().unwrap();
    assert!(matches!(
        &main.body[0],
        Stmt::Asm { inputs, clobbers, .. }
            if inputs[0].class == "reg16"
                && inputs[1].ty == Type::Named("Word".to_owned())
                && inputs[1].class == "reg16"
                && clobbers == &["ax", "bx", "ds", "flags"]
    ));
}

#[test]
fn parses_inline_asm_input_and_output_operands() {
    let program = parse_program(
            Path::new("game.ezra"),
            r#"
            fn main() {
                asm volatile(in ch: u8 as reg8, out result: u8 as reg8, in addr: ptr<u8> as reg24, clobber a) {
                    "ld a, {ch}"
                    "ld {result}, a"
                    "ld hl, {addr}"
                }
            }
            "#,
        )
        .unwrap();

    let main = program.main_function().unwrap();
    assert!(matches!(
        &main.body[0],
        Stmt::Asm {
            volatile: true,
            inputs,
            outputs,
            clobbers,
            lines
        } if inputs.len() == 2
            && inputs[0].name == "ch"
            && inputs[0].ty == Type::Named("u8".to_owned())
            && inputs[0].class == "reg8"
            && inputs[1].name == "addr"
            && inputs[1].ty == Type::Ptr(Box::new(Type::Named("u8".to_owned())))
            && inputs[1].class == "reg24"
            && outputs.len() == 1
            && outputs[0].name == "result"
            && outputs[0].ty == Type::Named("u8".to_owned())
            && outputs[0].class == "reg8"
            && clobbers == &["a"]
            && lines.len() == 3
    ));
}

#[test]
fn parses_inline_asm_operands_with_inferred_classes() {
    let program = parse_program(
        Path::new("game.ezra"),
        r#"
            fn main() {
                asm volatile(in ch: u8, in word: u16, in addr: ptr<u8>, out result: u8, clobber a) {
                    "ld a, {ch}"
                    "ld hl, {word}"
                    "ld hl, {addr}"
                }
            }
            "#,
    )
    .unwrap();

    let main = program.main_function().unwrap();
    assert!(matches!(
        &main.body[0],
        Stmt::Asm {
            inputs,
            outputs,
            ..
        } if inputs.len() == 3
            && inputs[0].class == "reg8"
            && inputs[1].class == "reg16"
            && inputs[2].class == "reg24"
            && outputs.len() == 1
            && outputs[0].class == "reg8"
    ));
}

#[test]
fn rejects_unknown_inline_asm_clobbers() {
    let error = parse_program(
        Path::new("game.ezra"),
        r#"
            fn main() {
                asm(clobber made_up) {
                    "xor a"
                }
            }
            "#,
    )
    .unwrap_err();

    assert_eq!(error.message, "unknown inline asm clobber `made_up`");
}

#[test]
fn reports_locations_for_ast_build_errors() {
    let error = parse_program(
        Path::new("game.ezra"),
        "const bad: ptr<u8> = \"\\q\"\nfn main() {}",
    )
    .unwrap_err();

    assert_eq!(error.message, "unknown escape `\\q`");
    assert_eq!(
        error.location(),
        Some(SourceLocation {
            file: Path::new("game.ezra").to_path_buf(),
            line: 1,
            column: 1,
        })
    );
}

#[test]
fn rejects_multibyte_character_literals() {
    let error =
        parse_program(Path::new("game.ezra"), "const bad: u8 = 'é'\nfn main() {}").unwrap_err();

    assert_eq!(
        error.message,
        "character literal must contain exactly one byte"
    );
    assert_eq!(
        error.location(),
        Some(SourceLocation {
            file: Path::new("game.ezra").to_path_buf(),
            line: 1,
            column: 1,
        })
    );
}

#[test]
fn rejects_incompatible_inline_asm_input_classes() {
    let error = parse_program(
        Path::new("game.ezra"),
        r#"
            fn main() {
                asm(in wide: u16 as reg8) {
                    "ld a, {wide}"
                }
            }
            "#,
    )
    .unwrap_err();

    assert_eq!(
        error.message,
        "inline asm operand class `reg8` is incompatible with type `Named(\"u16\")`"
    );
}

#[test]
fn rejects_incompatible_inline_asm_output_classes() {
    let error = parse_program(
        Path::new("game.ezra"),
        r#"
            fn main() {
                asm(out wide: u24 as reg8) {
                    "ld {wide}, a"
                }
            }
            "#,
    )
    .unwrap_err();

    assert_eq!(
        error.message,
        "inline asm operand class `reg8` is incompatible with type `Named(\"u24\")`"
    );
}

#[test]
fn parses_extern_asm_function_declarations() {
    let program = parse_program(
        Path::new("game.ezra"),
        r#"
            pub extern asm fn memcpy_fast(dst: ptr<u8>, src: ptr<u8>, len: u24)
            extern asm fn read_status() -> u8
            fn main() {}
            "#,
    )
    .unwrap();

    assert!(matches!(
        &program.declarations[0],
        Declaration::ExternAsmFunction(function)
            if function.public
                && function.name == "memcpy_fast"
                && function.params.len() == 3
                && function.return_type.is_none()
    ));
    assert!(matches!(
        &program.declarations[1],
        Declaration::ExternAsmFunction(function)
            if !function.public
                && function.name == "read_status"
                && function.return_type == Some(Type::Named("u8".to_owned()))
    ));
}

#[test]
fn parses_inline_attribute_spellings_with_public_in_either_order() {
    let program = parse_program(
        Path::new("game.ezra"),
        r#"
            pub @inline fn explicit_after_pub() {}
            @inline pub fn explicit_before_pub() {}
            pub inline fn legacy() {}
            interrupt pub fn exported_irq() {}
            fn main() {}
            "#,
    )
    .unwrap();

    for (index, name) in ["explicit_after_pub", "explicit_before_pub", "legacy"]
        .into_iter()
        .enumerate()
    {
        assert!(matches!(
            &program.declarations[index],
            Declaration::Function(function)
                if function.public
                    && function.attrs == ["inline"]
                    && function.name == name
        ));
    }
    assert!(matches!(
        &program.declarations[3],
        Declaration::Function(function)
            if function.public
                && function.attrs == ["interrupt"]
                && function.name == "exported_irq"
    ));
}

#[test]
fn normalizes_mixed_inline_spellings_as_duplicate_attributes() {
    let program = parse_program(
        Path::new("game.ezra"),
        r#"
            inline @inline fn legacy_first() {}
            @inline inline fn explicit_first() {}
            fn main() {}
            "#,
    )
    .unwrap();

    for declaration in &program.declarations[..2] {
        assert!(matches!(
            declaration,
            Declaration::Function(function) if function.attrs == ["inline", "inline"]
        ));
    }
}

#[test]
fn rejects_duplicate_function_visibility() {
    let error = parse_program(
        Path::new("game.ezra"),
        r#"
            pub inline pub fn invalid() {}
            fn main() {}
            "#,
    )
    .unwrap_err();

    assert_eq!(error.message, "duplicate visibility `pub` on function");
}

#[test]
fn parses_string_literal_pointer_values() {
    let program = parse_program(
        Path::new("game.ezra"),
        "global title: ptr<u8> = \"EZRA\"\nfn main() { let text: ptr<u8> = \"OK\" }",
    )
    .unwrap();

    assert!(program.main_function().is_some());
}

#[test]
fn parses_scalar_address_of_expression() {
    let program = parse_program(
        Path::new("game.ezra"),
        "global value: u16 = 0\nfn main() { let p: ptr<u16> = &value }",
    )
    .unwrap();

    assert!(program.main_function().is_some());
}

#[test]
fn parses_struct_declaration_literals_and_fields() {
    EzraParser::parse(Rule::field_expr, "player.x").unwrap();
    EzraParser::parse(Rule::expr, "player.x").unwrap();
    EzraParser::parse(Rule::expr, "&player.x").unwrap();
    EzraParser::parse(Rule::expr, "test.assert_eq_u24(player.x, 0x010000, 1)").unwrap();
    EzraParser::parse(Rule::stmt, "test.assert_eq_u24(player.x, 0x010000, 1);").unwrap();
    let program = parse_program(
            Path::new("game.ezra"),
            "struct Entity { x: u24 y: u24 sprite: u8 }\nglobal player: Entity = Entity { x: 1, sprite: 2 }\nfn main() { player.y = player.x + 3 }",
        )
        .unwrap();

    assert!(matches!(program.declarations[0], Declaration::Struct(_)));
    assert!(program.main_function().is_some());
}

#[test]
fn parses_chained_access_paths() {
    EzraParser::parse(Rule::expr, "matrix[row][col]").unwrap();
    EzraParser::parse(Rule::expr, "points[i].x").unwrap();
    EzraParser::parse(Rule::expr, "outer.inner.x").unwrap();
    EzraParser::parse(Rule::expr, "&outer.inner.x").unwrap();
    EzraParser::parse(Rule::expr, "&packets[i].bytes[j]").unwrap();
    EzraParser::parse(Rule::stmt, "points[i].x += 1;").unwrap();
    EzraParser::parse(Rule::stmt, "big.padding[299] = 1;").unwrap();

    let program = parse_program(
            Path::new("game.ezra"),
            "struct Point { x: u8 }\nglobal points: [Point; 2] = []\nfn main() { let i: u8 = 1; points[i].x = 3 }",
        )
        .unwrap();

    assert!(program.main_function().is_some());

    let program = parse_program(
            Path::new("game.ezra"),
            "struct Inner { x: u8 }\nstruct Outer { inner: Inner }\nglobal outer: Outer = Outer { inner: Inner { x: 1 } }\nfn main() { let x: u8 = outer.inner.x; let p: ptr<u8> = &outer.inner.x }",
        )
        .unwrap();
    let main = program.main_function().unwrap();
    assert!(matches!(
        &main.body[0],
        Stmt::Let {
            value: Expr::Access(_),
            ..
        }
    ));
    assert!(matches!(
        &main.body[1],
        Stmt::Let {
            value: Expr::AddressOfAccess(_),
            ..
        }
    ));
}

#[test]
fn parses_embed_byte_declarations() {
    let program = parse_program(
        Path::new("game.ezra"),
        r#"
            embed palette: bytes = bytes [0x11, 0x22] section .rodata align 16
            embed blob: bytes = file("assets/blob.bin")
            embed title: bytes = cstr("OK")
            embed blank: bytes = repeat(0, 4)
            fn main() {}
            "#,
    )
    .unwrap();

    assert!(matches!(program.declarations[0], Declaration::Embed(_)));
    assert!(program.main_function().is_some());
}
