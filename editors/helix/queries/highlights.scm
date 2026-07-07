; Copy to runtime/queries/ezra/highlights.scm after installing the tree-sitter grammar.
[
  "alias"
  "const"
  "embed"
  "extern"
  "fn"
  "global"
  "import"
  "layout"
  "let"
  "mmio"
  "port"
  "struct"
] @keyword

[
  "if"
  "else"
  "while"
  "loop"
  "break"
  "continue"
  "return"
] @keyword.control

[
  "asm"
  "cast"
  "in"
  "out"
] @keyword.operator

[
  "pub"
  "inline"
  "naked"
  "interrupt"
  "volatile"
] @keyword.storage.modifier

(line_comment) @comment
(string_literal) @string
(char_literal) @constant.character
(integer_literal) @constant.numeric.integer
(boolean_literal) @constant.builtin.boolean
(primitive_type) @type.builtin
(function_item name: (identifier) @function)
(call_expression function: (path (identifier) @function))
(struct_item name: (identifier) @type)
