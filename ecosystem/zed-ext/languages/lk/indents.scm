; Indentation rules for LK

[
  (block)
  (map_expression)
  (list_expression)
  (struct_literal)
  (match_expression)
  (select_expression)
] @indent

[
  "}"
  "]"
] @outdent

; Match arms
(match_arm) @indent

; Select cases
(select_case) @indent

; Function parameters
(function_params) @indent

; Named params block
(named_params_block) @indent