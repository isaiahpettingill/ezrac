
use super::*;

fn parse(source: &str) -> ParsedAssembly {
    parse_assembly_syntax("test.asm", source).unwrap()
}

#[test]
fn semicolon_inside_string_is_data_not_a_comment() {
    let parsed = parse("db \"a;b\", 1 ; real comment\n");
    assert_eq!(parsed.items.len(), 1);
    assert!(matches!(
        &parsed.items[0].kind,
        ParsedAssemblyItem::Data { values, .. }
            if values == &vec![
                ParsedAssemblyDataValue::StringLiteral("\"a;b\"".to_owned()),
                ParsedAssemblyDataValue::Expression("1".to_owned()),
            ]
    ));
    let lowered = lower_parsed_assembly(parsed).unwrap();
    assert!(matches!(
        &lowered.items[0].kind,
        AssemblyItem::Data { values, .. }
            if values[0] == AssemblyDataValue::Bytes(b"a;b".to_vec())
    ));
}

#[test]
fn expression_precedence_works_without_whitespace() {
    let lowered = lower_parsed_assembly(parse("answer equ 1+2*3&7|8^9")).unwrap();
    let AssemblyItem::Equ { value, .. } = &lowered.items[0].kind else {
        panic!("expected equate");
    };
    assert!(matches!(
        value,
        AssemblyExpression::Binary {
            operator: AssemblyBinaryOperator::BitOr,
            ..
        }
    ));
}

#[test]
fn reserve_directive_lowers_to_a_label_aware_fill() {
    let lowered =
        lower_parsed_assembly(parse("start: db 1\nend:\n    ds 4 - (end - start)\n")).unwrap();
    let reserve = lowered
        .items
        .iter()
        .find_map(|item| match &item.kind {
            AssemblyItem::Reserve(expression) => Some(expression),
            _ => None,
        })
        .expect("expected reserve directive");
    assert!(matches!(reserve, AssemblyExpression::Binary { .. }));
}

#[test]
fn nested_macros_and_conditionals_are_structural() {
    let parsed = parse(
        "%macro outer(a)\n%if a\n%macro inner(x)\ndb x\n%endmacro\n%else\ndb 0\n%endif\n%endmacro\n",
    );
    let ParsedAssemblyItem::MacroDefinition { body, .. } = &parsed.items[0].kind else {
        panic!("expected macro");
    };
    let ParsedAssemblyItem::Conditional {
        then_items,
        else_items,
        ..
    } = &body[0].kind
    else {
        panic!("expected conditional");
    };
    assert!(matches!(
        then_items[0].kind,
        ParsedAssemblyItem::MacroDefinition { .. }
    ));
    assert!(matches!(
        else_items[0].kind,
        ParsedAssemblyItem::Data { .. }
    ));
}

#[test]
fn malformed_syntax_has_precise_location() {
    let error = parse_assembly_syntax("bad.asm", "ld a, (1 + 2\n").unwrap_err();
    let location = error.location().unwrap();
    assert_eq!(location.line, 1);
    assert!(location.column >= 7);
}

#[test]
fn label_and_instruction_on_one_line_become_two_items() {
    let parsed = parse("start: ld a, 1\n");
    assert_eq!(parsed.items.len(), 2);
    assert!(matches!(parsed.items[0].kind, ParsedAssemblyItem::Label(_)));
    assert!(matches!(
        parsed.items[1].kind,
        ParsedAssemblyItem::Instruction(_)
    ));
    assert!(parsed.items[1].location.column > parsed.items[0].location.column);
}

#[test]
fn commas_inside_parentheses_and_brackets_do_not_split_operands() {
    let parsed = parse("op (a, b), [x, y], final\n%call pair(1, 2), [3, 4]\n");
    let ParsedAssemblyItem::Instruction(instruction) = &parsed.items[0].kind else {
        panic!("expected instruction");
    };
    assert_eq!(instruction.operands, vec!["(a, b)", "[x, y]", "final"]);
    let ParsedAssemblyItem::MacroInvocation { arguments, .. } = &parsed.items[1].kind else {
        panic!("expected macro invocation");
    };
    assert_eq!(arguments, &vec!["pair(1, 2)", "[3, 4]"]);
}
