; Highlights for LK (used by Neovim, Helix, etc.)

; Keywords
[
  "if"
  "else"
  "while"
  "for"
  "in"
  "let"
  "return"
  "break"
  "continue"
  "fn"
  "match"
  "import"
  "from"
  "as"
  "struct"
  "select"
  "case"
  "default"
] @keyword

; Statement keywords
"spawn" @keyword.function
"chan" @keyword.function

; Boolean literals
[
  (boolean_literal)
] @boolean

; Nil
(nil_literal) @constant.builtin

; Numeric literals
(integer_literal) @number
(float_literal) @number

; Strings
(string_literal) @string
(double_string) @string
(single_string) @string
(raw_string) @string
(string_interpolation) @none
(escape_sequence) @string.escape

; Comments
(line_comment) @comment.line
(block_comment) @comment.block

; Operators
[
  "||"
  "&&"
  "=="
  "!="
  "<="
  ">="
  "<"
  ">"
  "+"
  "-"
  "*"
  "/"
  "%"
  "="
  "!"
  "??"
  ".."
  "..="
  "+="
  "-="
  "*="
  "/="
  "%="
  "=>"
  "->"
] @operator

; Punctuation
[
  "("
  ")"
  "{"
  "}"
  "["
  "]"
  ","
  ";"
  ":"
  "|"
] @punctuation

; Dot access
"." @punctuation.delimiter
"?." @punctuation.delimiter

; Types
(primitive_type) @type.builtin
(type_identifier) @type
(list_type) @type
(map_type) @type
(function_type) @type
(optional_type) @type
(union_type) @type
(named_type) @type

; Function definition
(function_definition
  name: (identifier) @function)

; Named parameters
(named_param
  name: (identifier) @parameter)

(parameter
  name: (identifier) @parameter)

; Struct fields
(struct_field
  (identifier) @field)

(struct_field_init
  name: (identifier) @field)

; Named arguments
(named_argument
  name: (identifier) @parameter)

; Import items
(import_item
  (identifier) @namespace)

(import_statement
  (identifier) @namespace)

; Field access
(field_access
  field: (identifier) @field)

(optional_field_access
  field: (identifier) @field)

; Pattern identifiers
(identifier_pattern) @variable

; Wildcard pattern
(wildcard_pattern) @character.special

; Range patterns
(range_pattern) @operator
