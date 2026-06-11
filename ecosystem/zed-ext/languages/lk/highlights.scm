; Highlights for LK in Zed.

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
  "use"
  "from"
  "as"
  "struct"
  "type"
  "trait"
  "impl"
  "export"
  "macro_rules"
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
(line_comment) @comment
(block_comment) @comment

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
"." @punctuation
"?." @punctuation

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

(macro_definition
  name: (identifier) @function.macro)

(macro_export_item
  (identifier) @function.macro)

(macro_invocation
  name: (identifier) @function.macro)

(attribute
  (identifier) @attribute)

(type_alias_definition
  (type_identifier) @type)

(trait_definition
  (type_identifier) @type)

(trait_method
  name: (identifier) @function.method)

(impl_definition
  trait: (type_identifier) @type
  target: (named_type) @type)

; Named parameters
(named_param
  name: (identifier) @variable.parameter)

(parameter
  name: (identifier) @variable.parameter)

; Struct fields
(struct_field
  (identifier) @property)

(struct_field_init
  name: (identifier) @property)

; Named arguments
(named_argument
  name: (identifier) @variable.parameter)

; Use items
(import_item
  (identifier) @namespace)

(import_statement
  (identifier) @namespace)

; Field access
(field_access
  field: (identifier) @property)

(optional_field_access
  field: (identifier) @property)

; Pattern identifiers
(identifier_pattern) @variable

; Wildcard pattern
(wildcard_pattern) @character.special

; Range patterns
(range_pattern) @operator
