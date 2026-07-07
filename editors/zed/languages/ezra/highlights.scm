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
] @keyword.modifier

(line_comment) @comment
(string_literal) @string
(char_literal) @string.special
(integer_literal) @number
(boolean_literal) @boolean
(primitive_type) @type.builtin
(function_item name: (identifier) @function)
(call_expression function: (path (identifier) @function))
(struct_item name: (identifier) @type)
