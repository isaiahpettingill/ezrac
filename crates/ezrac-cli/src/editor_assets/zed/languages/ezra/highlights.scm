; Declarations and control flow
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
  "clobber"
  "as"
] @keyword.operator

[
  "pub"
  "inline"
  "naked"
  "interrupt"
  "volatile"
] @keyword.modifier

; Layout and embed directives
[
  "load"
  "entry"
  "stack"
  "region"
  "section"
  "symbol"
  "align"
  "read"
  "write"
  "execute"
  "reserved"
  "file"
  "bytes"
  "text"
  "cstr"
  "repeat"
  "reg8"
  "reg16"
  "reg24"
  "mem"
  "imm"
] @keyword

(line_comment) @comment
(string_literal) @string
(char_literal) @string.special
(integer_literal) @number
(boolean_literal) @boolean
(primitive_type) @type.builtin
(pointer_type "ptr" @type.builtin)

; Operators and delimiters
[
  "="
  "+="
  "-="
  "*="
  "/="
  "%="
  "&="
  "|="
  "^="
  "<<="
  ">>="
  "||"
  "&&"
  "|"
  "^"
  "&"
  "=="
  "!="
  "<"
  "<="
  ">"
  ">="
  "<<"
  ">>"
  "+"
  "-"
  "*"
  "/"
  "%"
  "!"
  "~"
  "->"
  ".."
] @operator

["{" "}" "[" "]" "(" ")"] @punctuation.bracket
[":" ";" "," "."] @punctuation.delimiter

; Declarations and references
(function_item name: (identifier) @function)
(struct_item name: (identifier) @type)
(const_declaration (identifier) @constant)
(alias_declaration (identifier) @type)
(port_declaration (identifier) @constant)
(mmio_declaration (identifier) @constant)
(embed_declaration (identifier) @constant)
(global_declaration (identifier) @variable)
(parameter (identifier) @variable.parameter)
(field_declaration (identifier) @property)
(field_initializer (identifier) @property)
(field_expression (identifier) @variable (identifier) @property)
(call_expression function: (path (identifier) @function))
