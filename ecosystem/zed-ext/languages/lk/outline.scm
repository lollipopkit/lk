; Outline items for LK in Zed.

(function_definition
  "fn" @context
  name: (identifier) @name) @item

(macro_definition
  "macro_rules" @context
  name: (identifier) @name) @item

(struct_definition
  "struct" @context
  (type_identifier) @name) @item

(type_alias_definition
  "type" @context
  (type_identifier) @name) @item

(trait_definition
  "trait" @context
  (type_identifier) @name) @item

(impl_definition
  "impl" @context
  target: (named_type) @name) @item

(trait_method
  "fn" @context
  name: (identifier) @name) @item
