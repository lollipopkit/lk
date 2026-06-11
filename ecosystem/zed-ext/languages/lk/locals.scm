; Locals (scoping) for LK

; Function definitions create new scopes
(function_definition) @local.scope

; Let creates bindings
(let_statement
  (pattern
    (identifier_pattern
      (identifier) @local.definition)))

(define_statement
  (identifier) @local.definition)

; Function parameters
(parameter
  name: (identifier) @local.definition)

; Named parameters
(named_param
  name: (identifier) @local.definition)

; Closures
(closure
  (parameter
    name: (identifier) @local.definition))

; For loop patterns create bindings
(for_statement
  (for_pattern) @local.definition)

; Use creates references
(import_statement
  (identifier) @local.reference)
