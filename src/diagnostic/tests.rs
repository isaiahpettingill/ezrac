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
