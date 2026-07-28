use super::*;

#[test]
fn semantic_diagnostic_spans_select_relevant_tokens() {
    let file = Path::new("game.ezra");
    let source = "const VALUE: u8 = 1\nglobal VALUE: u8 = 2\nfn main() { missing() }\n";
    let duplicate = diagnostic_span(file, source, "duplicate declaration `VALUE`").unwrap();
    assert_eq!((duplicate.start.line, duplicate.start.column), (2, 8));
    assert_eq!((duplicate.end.line, duplicate.end.column), (2, 13));
    let unknown = diagnostic_span(file, source, "unknown function `missing`").unwrap();
    assert_eq!((unknown.start.line, unknown.start.column), (3, 13));
    assert_eq!((unknown.end.line, unknown.end.column), (3, 20));
}

#[test]
fn value_diagnostic_spans_match_u32_and_i32_literals() {
    let file = Path::new("game.ezra");
    let source = "const HIGH: u32 = 4294967295u32\nconst LOW: i32 = 2147483648i32\n";

    let high = diagnostic_span(file, source, "value 4294967295 is outside u32 range").unwrap();
    assert_eq!((high.start.line, high.start.column), (1, 19));
    assert_eq!((high.end.line, high.end.column), (1, 32));

    let low = diagnostic_span(file, source, "value 2147483648 is outside i32 range").unwrap();
    assert_eq!((low.start.line, low.start.column), (2, 18));
    assert_eq!((low.end.line, low.end.column), (2, 31));
}
