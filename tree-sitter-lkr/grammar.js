/// grammar.js — Tree-sitter grammar for LKR
///
/// LKR is a Rust-like scripting language with first-class named parameters,
/// pattern matching, structs, closures, and optional concurrency primitives.

module.exports = grammar({
  name: 'lkr',

  extras: $ => [
    /\s/,
    $.line_comment,
    $.block_comment,
  ],

  conflicts: $ => [
    [$._expression, $.pattern],
    [$.struct_literal, $.map_expression],
    [$.struct_literal, $.block],
    [$.map_expression, $.block],
    [$.primary_expression, $.map_entry],
    [$.call_expression, $.closure],
    [$.field_access, $.closure],
    [$.optional_field_access, $.closure],
    [$.index_access, $.closure],
    [$.optional_index_access, $.closure],
    [$.closure, $.binary_expression],
    [$.pattern, $.range_pattern],
    [$._type, $.named_type],
    [$.parameter, $.union_type],
    [$.parenthesized_expression, $.if_statement],
    [$.parenthesized_expression, $.while_statement],
    [$.parenthesized_expression, $._argument_list],
    [$.or_pattern],
    [$.or_pattern, $.guarded_pattern],
    [$.index_access, $.list_expression],
    [$.index_access, $.match_arm],
    [$.if_statement],
    [$.range_expression, $.guarded_pattern],
    [$.optional_type, $.union_type],
    [$.union_type],
    [$.function_type, $.optional_type],
    [$.function_type, $.union_type],
    [$.guarded_pattern],
    [$.optional_type],
    [$._expression, $._expression],
    [$._statement, $.import_statement],
    [$.identifier, $.type_identifier],
    [$._full_expression, $.call_expression],
    [$._full_expression, $.binary_expression],
    [$._full_expression, $.nullish_coalescing_expression],
    [$._full_expression, $.range_expression],
    [$._full_expression, $.ternary_expression],
    [$.program, $._statement],
  ],

  precedences: $ => [
    ['binary_or', 'binary_and', 'binary_comparison', 'binary_range', 'binary_add', 'binary_mul', 'binary_unary', 'binary_nullish', 'binary_ternary'],
  ],

  word: $ => $._word_identifier,

  rules: {
    program: $ => repeat(
      choice(
        $.import_statement,
        $._statement,
      ),
    ),

    // ── Comments ──────────────────────────────────────────────────────
    line_comment: $ => /\/\/[^\n]*/,
    block_comment: $ => /\/\*[\s\S]*?\*\//,

    // ── Identifiers ────────────────────────────────────────────────────
    identifier: $ => /[a-zA-Z_][a-zA-Z0-9_-]*/,
    _word_identifier: $ => /[a-zA-Z_][a-zA-Z0-9_]*/,
    type_identifier: $ => /[a-zA-Z_][a-zA-Z0-9_-]*/,

    // ── Literals ───────────────────────────────────────────────────────
    integer_literal: $ => /[0-9]+/,
    float_literal: $ => /[0-9]+\.[0-9]+([eE][+-]?[0-9]+)?/,
    boolean_literal: $ => choice('true', 'false'),
    nil_literal: $ => 'nil',

    string_literal: $ => choice(
      $.double_string,
      $.single_string,
      $.raw_string,
    ),

    double_string: $ => seq(
      '"',
      repeat(choice(
        /[^"\\$]+/,
        $.escape_sequence,
        $.string_interpolation,
      )),
      '"',
    ),

    single_string: $ => seq(
      "'",
      repeat(choice(
        /[^'\\$]+/,
        $.escape_sequence,
        $.string_interpolation,
      )),
      "'",
    ),

    raw_string: $ => seq(
      'r',
      repeat('#'),
      '"',
      repeat(choice(
        /[^"\\]+/,
        /\\./,
      )),
      '"',
      repeat('#'),
    ),

    string_interpolation: $ => seq(
      '${',
      $._full_expression,
      '}',
    ),

    escape_sequence: $ => /\\[nrt\\"'$0]/,

    // ── Expressions ────────────────────────────────────────────────────
    // Separate 'expression' (used in most contexts) from '_full_expression'
    // (used in template strings and other positions where ternary is allowed)
    _full_expression: $ => choice(
      $._expression,
      $.ternary_expression,
    ),

    _expression: $ => choice(
      $.primary_expression,
      $.unary_expression,
      $.binary_expression,
      $.nullish_coalescing_expression,
      $.range_expression,
      $.closure,
      $.match_expression,
      $.spawn_expression,
      $.chan_expression,
      $.send_expression,
      $.recv_expression,
      $.select_expression,
    ),

    expression: $ => $._expression,

    primary_expression: $ => choice(
      $.identifier,
      $.integer_literal,
      $.float_literal,
      $.boolean_literal,
      $.nil_literal,
      $.string_literal,
      $.list_expression,
      $.map_expression,
      $.struct_literal,
      $.parenthesized_expression,
      $.call_expression,
      $.field_access,
      $.index_access,
      $.optional_field_access,
      $.optional_index_access,
    ),

    parenthesized_expression: $ => seq('(', $._full_expression, ')'),

    // Postfix
    call_expression: $ => seq(
      field('function', $._expression),
      '(',
      optional($._argument_list),
      ')',
    ),

    _argument_list: $ => seq(
      $._full_expression,
      repeat(seq(',', $._full_expression)),
      optional(','),
      optional(seq(',', $.named_argument, repeat(seq(',', $.named_argument)), optional(','))),
    ),

    named_argument: $ => seq(
      field('name', $.identifier),
      ':',
      field('value', $._full_expression),
    ),

    field_access: $ => prec.left(20, seq(
      field('object', $._expression),
      '.',
      field('field', choice($.identifier, $.integer_literal)),
    )),

    optional_field_access: $ => prec.left(20, seq(
      field('object', $._expression),
      '?.',
      field('field', choice($.identifier, $.integer_literal)),
    )),

    index_access: $ => prec.left(20, seq(
      field('object', $._expression),
      '[',
      field('index', $._full_expression),
      ']',
    )),

    optional_index_access: $ => prec.left(20, seq(
      field('object', $._expression),
      '?[',
      field('index', $._full_expression),
      ']',
    )),

    // Collections
    list_expression: $ => seq(
      '[',
      optional(seq(
        $._full_expression,
        repeat(seq(',', $._full_expression)),
        optional(','),
      )),
      ']',
    ),

    map_expression: $ => seq(
      '{',
      optional(seq(
        $.map_entry,
        repeat(seq(',', $.map_entry)),
        optional(','),
      )),
      '}',
    ),

    map_entry: $ => seq(
      field('key', choice($._full_expression, $.identifier)),
      ':',
      field('value', $._full_expression),
    ),

    struct_literal: $ => seq(
      field('name', $.type_identifier),
      '{',
      optional(seq(
        $.struct_field_init,
        repeat(seq(',', $.struct_field_init)),
        optional(','),
      )),
      '}',
    ),

    struct_field_init: $ => seq(
      field('name', $.identifier),
      optional(seq(':', field('value', $._full_expression))),
    ),

    // ── Unary ─────────────────────────────────────────────────────────
    unary_expression: $ => prec.left('binary_unary', seq(
      field('operator', '!'),
      field('operand', $._expression),
    )),

    // ── Binary ─────────────────────────────────────────────────────────
    binary_expression: $ => choice(
      prec.left('binary_mul', seq(field('left', $._expression), field('operator', choice('*', '/', '%')), field('right', $._expression))),
      prec.left('binary_add', seq(field('left', $._expression), field('operator', choice('+', '-')), field('right', $._expression))),
      prec.left('binary_comparison', seq(field('left', $._expression), field('operator', choice('==', '!=', '<', '>', '<=', '>=')), field('right', $._expression))),
      prec.left('binary_and', seq(field('left', $._expression), '&&', field('right', $._expression))),
      prec.left('binary_or', seq(field('left', $._expression), '||', field('right', $._expression))),
    ),

    // ── Nullish coalescing ────────────────────────────────────────────
    nullish_coalescing_expression: $ => prec.left('binary_nullish', seq(
      field('left', $._expression),
      '??',
      field('right', $._expression),
    )),

    // ── Range ──────────────────────────────────────────────────────────
    range_expression: $ => choice(
      prec.left('binary_range', seq(field('left', $._expression), '..', optional(field('right', $._expression)))),
      prec.left('binary_range', seq(field('left', $._expression), '..=', field('right', $._expression))),
    ),

    // ── Ternary ────────────────────────────────────────────────────────
    ternary_expression: $ => prec.right('binary_ternary', seq(
      field('condition', $._expression),
      '?',
      field('consequence', $._full_expression),
      ':',
      field('alternative', $._full_expression),
    )),

    // ── Closures ────────────────────────────────────────────────────────
    closure: $ => prec('binary_or', seq(
      '|',
      optional($._parameter_list),
      '|',
      field('body', $._full_expression),
    )),

    _parameter_list: $ => seq(
      $.parameter,
      repeat(seq(',', $.parameter)),
      optional(','),
    ),

    parameter: $ => seq(
      field('name', $.identifier),
      optional(seq(':', field('type', $._type))),
    ),

    // ── Match ──────────────────────────────────────────────────────────
    match_expression: $ => seq(
      'match',
      field('value', $._expression),
      '{',
      repeat($.match_arm),
      '}',
    ),

    match_arm: $ => seq(
      field('pattern', $.pattern),
      '=>',
      field('value', $._full_expression),
      optional(choice(',', ';')),
    ),

    // ── Patterns ────────────────────────────────────────────────────────
    pattern: $ => choice(
      $.wildcard_pattern,
      $.literal_pattern,
      $.identifier_pattern,
      $.list_pattern,
      $.map_pattern,
      $.or_pattern,
      $.guarded_pattern,
      $.range_pattern,
    ),

    wildcard_pattern: $ => '_',

    literal_pattern: $ => choice(
      $.integer_literal,
      $.float_literal,
      $.string_literal,
      $.boolean_literal,
      $.nil_literal,
    ),

    identifier_pattern: $ => $.identifier,

    list_pattern: $ => seq(
      '[',
      optional(seq(
        $.pattern,
        repeat(seq(',', $.pattern)),
        optional(','),
        optional(seq('..', $.identifier)),
      )),
      ']',
    ),

    map_pattern: $ => seq(
      '{',
      optional(seq(
        $.map_pattern_entry,
        repeat(seq(',', $.map_pattern_entry)),
        optional(','),
        optional(seq('..', $.identifier)),
      )),
      '}',
    ),

    map_pattern_entry: $ => seq(
      choice($.string_literal, $.identifier),
      ':',
      $.pattern,
    ),

    or_pattern: $ => seq(
      $.pattern,
      '|',
      $.pattern,
      repeat(seq('|', $.pattern)),
    ),

    guarded_pattern: $ => seq(
      $.pattern,
      'if',
      $._expression,
    ),

    range_pattern: $ => choice(
      seq(field('left', $.literal_pattern), '..', field('right', $.literal_pattern)),
      seq(field('left', $.literal_pattern), '..=', field('right', choice($.literal_pattern, $.identifier_pattern))),
    ),

    // ── Concurrency ────────────────────────────────────────────────────
    spawn_expression: $ => seq('spawn', '(', $._full_expression, ')'),

    chan_expression: $ => seq('chan', '(', optional($._full_expression), optional(seq(',', $._full_expression)), ')'),

    send_expression: $ => seq('send', '(', $._full_expression, ',', $._full_expression, ')'),

    recv_expression: $ => seq('recv', '(', $._full_expression, ')'),

    select_expression: $ => seq(
      'select',
      '{',
      repeat1($.select_case),
      '}',
    ),

    select_case: $ => choice(
      seq('case', $._full_expression, '<=', $._full_expression, '=>', $._full_expression, optional(';')),
      seq('case', $._full_expression, '=>', $._full_expression, optional(';')),
      seq('default', '=>', $._full_expression, optional(';')),
    ),

    // ── Types ──────────────────────────────────────────────────────────
    _type: $ => choice(
      $.primitive_type,
      $.list_type,
      $.map_type,
      $.function_type,
      $.optional_type,
      $.union_type,
      $.named_type,
      $.type_identifier,
    ),

    type: $ => $._type,

    primitive_type: $ => choice('Int', 'Float', 'String', 'Bool', 'Nil', 'Any'),

    list_type: $ => seq('List', '<', $._type, '>'),

    map_type: $ => seq('Map', '<', $._type, ',', $._type, '>'),

    function_type: $ => seq('(', optional(seq($._type, repeat(seq(',', $._type)))), ')', '->', $._type),

    optional_type: $ => seq($._type, '?'),

    union_type: $ => seq($._type, '|', $._type, repeat(seq('|', $._type))),

    named_type: $ => seq($.type_identifier, optional(seq('<', $._type, repeat(seq(',', $._type)), '>'))),

    // ── Statements ─────────────────────────────────────────────────────
    _statement: $ => choice(
      $.import_statement,
      $.let_statement,
      $.define_statement,
      $.assignment_statement,
      $.compound_assignment_statement,
      $.if_statement,
      $.while_statement,
      $.for_statement,
      $.function_definition,
      $.struct_definition,
      $.return_statement,
      $.break_statement,
      $.continue_statement,
      $.expression_statement,
      $.block,
    ),

    statement: $ => $._statement,

    import_statement: $ => choice(
      seq('import', $.identifier, optional(seq('as', $.identifier)), ';'),
      seq('import', $.string_literal, optional(seq('as', $.identifier)), ';'),
      seq('import', '{', $.import_item, repeat(seq(',', $.import_item)), optional(','), '}', 'from', choice($.identifier, $.string_literal), ';'),
      seq('import', '*', 'as', $.identifier, 'from', choice($.identifier, $.string_literal), ';'),
    ),

    import_item: $ => seq($.identifier, optional(seq('as', $.identifier))),

    let_statement: $ => seq(
      'let',
      $.pattern,
      optional(seq(':', $._type)),
      '=',
      $._full_expression,
      ';',
    ),

    define_statement: $ => seq(
      $.identifier,
      ':=',
      $._full_expression,
      ';',
    ),

    assignment_statement: $ => seq(
      $.identifier,
      '=',
      $._full_expression,
      ';',
    ),

    compound_assignment_statement: $ => seq(
      $.identifier,
      choice('+=', '-=', '*=', '/=', '%='),
      $._full_expression,
      ';',
    ),

    if_statement: $ => seq(
      'if',
      choice(
        seq('(', $._full_expression, ')'),
        $._full_expression,
      ),
      $._statement,
      optional(seq('else', $._statement)),
    ),

    while_statement: $ => seq(
      'while',
      choice(
        seq('(', $._full_expression, ')'),
        $._full_expression,
      ),
      $._statement,
    ),

    for_statement: $ => seq(
      'for',
      $.for_pattern,
      'in',
      $._full_expression,
      $._statement,
    ),

    for_pattern: $ => choice(
      $.identifier,
      '_',
      seq('(', $.for_pattern, repeat(seq(',', $.for_pattern)), ')'),
      seq('[', $.for_pattern, repeat(seq(',', $.for_pattern)), optional(seq('..', $.identifier)), ']'),
      seq('{', $.for_pattern_entry, repeat(seq(',', $.for_pattern_entry)), '}'),
    ),

    for_pattern_entry: $ => seq(
      choice($.string_literal, $.identifier),
      ':',
      $.for_pattern,
    ),

    function_definition: $ => seq(
      'fn',
      field('name', $.identifier),
      '(',
      optional($.function_params),
      ')',
      optional(seq('->', $._type)),
      $.block,
    ),

    function_params: $ => seq(
      $.parameter,
      repeat(seq(',', $.parameter)),
      optional(','),
      optional(seq(',', $.named_params_block)),
    ),

    named_params_block: $ => seq(
      '{',
      $.named_param,
      repeat(seq(',', $.named_param)),
      optional(','),
      '}',
    ),

    named_param: $ => seq(
      field('name', $.identifier),
      optional(seq(':', $._type)),
      optional(seq('=', $._full_expression)),
    ),

    struct_definition: $ => seq(
      'struct',
      $.type_identifier,
      '{',
      optional(seq(
        $.struct_field,
        repeat(seq(',', $.struct_field)),
        optional(','),
      )),
      '}',
    ),

    struct_field: $ => seq(
      $.identifier,
      optional(seq(':', $._type)),
    ),

    return_statement: $ => seq('return', optional($._full_expression), ';'),

    break_statement: $ => seq('break', ';'),

    continue_statement: $ => seq('continue', ';'),

    expression_statement: $ => seq($._full_expression, ';'),

    block: $ => seq(
      '{',
      repeat($._statement),
      '}',
    ),
  },
});