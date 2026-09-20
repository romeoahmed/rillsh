/**
 * @file Rill Shell's editor grammar. Execution remains owned by rill-syntax.
 */
/// <reference types="tree-sitter-cli/dsl" />
// @ts-check

const commaList = (rule, nl) =>
  seq(optional(seq(rule, repeat(seq(",", nl, rule)), optional(","))), nl);
const expressionRules = (soft) => {
  const suffix = soft ? "_soft" : "";
  const ref = ($, name) =>
    soft && !name.startsWith("_") ? alias($[name + suffix], $[name]) : $[name + suffix];
  const newline = ($) => (soft ? optional($._operator_gap) : blank());
  return {
    ["_expression" + suffix]: ($) =>
      choice(
        ref($, "_atom"),
        ref($, "call_expression"),
        ref($, "field_expression"),
        ref($, "index_expression"),
        ref($, "unary_expression"),
        ref($, "binary_expression"),
        ref($, "if_expression"),
        ref($, "match_expression"),
      ),
    ["_atom" + suffix]: ($) =>
      choice(
        $.identifier,
        $.number,
        $.string,
        $.boolean,
        $.null,
        $.unit,
        $.list,
        $.record,
        $.closure,
        $.recursive_closure,
        $.group,
        $.block,
        $.job,
      ),
    ["call_expression" + suffix]: ($) =>
      prec.left(
        9,
        seq(
          field(
            "function",
            choice(
              ref($, "call_expression"),
              ref($, "field_expression"),
              ref($, "index_expression"),
              ref($, "_atom"),
            ),
          ),
          soft ? $._soft_application_gap : $._application_gap,
          field(
            "argument",
            choice(ref($, "field_expression"), ref($, "index_expression"), ref($, "_atom")),
          ),
        ),
      ),
    ["field_expression" + suffix]: ($) =>
      prec.left(
        10,
        seq(
          field(
            "value",
            choice(ref($, "field_expression"), ref($, "index_expression"), ref($, "_atom")),
          ),
          token.immediate("."),
          field("field", $._field_name),
        ),
      ),
    ["index_expression" + suffix]: ($) =>
      prec.left(
        10,
        seq(
          field(
            "value",
            choice(ref($, "field_expression"), ref($, "index_expression"), ref($, "_atom")),
          ),
          token.immediate("["),
          repeat($._newline),
          field("index", $._expression_soft),
          repeat($._newline),
          "]",
        ),
      ),
    ["unary_expression" + suffix]: ($) =>
      prec.right(8, seq(choice("-", "not"), repeat($._newline), ref($, "_expression"))),
    ["binary_expression" + suffix]: ($) =>
      choice(
        ...[
          [7, ["*", "/"]],
          [6, ["+", "-"]],
          [5, ["==", "!=", "<", "<=", ">", ">="]],
          [4, ["and"]],
          [3, ["or"]],
          [2, ["with"]],
          [1, ["|>"]],
        ].map(([level, operators]) =>
          prec.left(
            level,
            seq(
              field("left", ref($, "_expression")),
              level === 1 ? optional($._pipeline_gap) : newline($),
              field("operator", operators.length === 1 ? operators[0] : choice(...operators)),
              repeat($._newline),
              field("right", ref($, "_expression")),
            ),
          ),
        ),
      ),
    ["if_expression" + suffix]: ($) =>
      prec.right(
        seq(
          "if",
          repeat($._newline),
          field("condition", ref($, "_expression")),
          repeat($._newline),
          "then",
          repeat($._newline),
          field("consequence", ref($, "_expression")),
          repeat($._newline),
          "else",
          repeat($._newline),
          field("alternative", ref($, "_expression")),
        ),
      ),
    ["match_expression" + suffix]: ($) =>
      seq(
        "match",
        repeat($._newline),
        field("value", ref($, "_expression")),
        repeat($._newline),
        "of",
        repeat($._newline),
        "{",
        repeat($._newline),
        commaList($.match_arm, repeat($._newline)),
        "}",
      ),
  };
};

export default grammar({
  name: "rill",
  extras: ($) => [/[ \t\r]+/, $.comment],
  externals: ($) => [
    $._application_gap,
    $._soft_application_gap,
    $._pipeline_gap,
    $._operator_gap,
    $.command_word,
    $._command_gap,
    $._parameter_gap,
  ],
  word: ($) => $.identifier,
  conflicts: ($) => [
    [$.match_expression_soft],
    [$.match_expression],
    [$.enum_declaration],
    [$.field_declarations],
    [$._body],
    [$.binary_expression_soft],
    [$.export_declaration],
    [$.list_pattern],
    [$.record_pattern],
    [$.list],
    [$.record],
    [$._separator, $.recursive_closure],
    [$._parameter, $._key],
    [$._field_name, $.null],
    [$._field_name, $.boolean],
    [$._parameter, $._field_name],
  ],
  rules: {
    source_file: ($) =>
      seq(
        repeat($._separator),
        optional(
          seq($._statement, repeat(seq(repeat1($._separator), $._statement)), repeat($._separator)),
        ),
      ),
    _body: ($) =>
      seq(
        repeat($._separator),
        $._statement,
        repeat(seq(repeat1($._separator), $._statement)),
        repeat($._separator),
      ),
    _separator: ($) => choice($._newline, ";"),
    _newline: (_) => "\n",
    comment: (_) => token(seq("#", /[^\n]*/)),
    _statement: ($) =>
      choice(
        $.let_declaration,
        $.function_declaration,
        $.recursive_group,
        $.struct_declaration,
        $.enum_declaration,
        $.import_declaration,
        $.export_declaration,
        $.command_pipeline,
        $._expression,
      ),
    let_declaration: ($) =>
      seq(
        "let",
        field("pattern", $._pattern),
        "=",
        repeat($._newline),
        field("value", $._expression),
      ),
    function_declaration: ($) =>
      seq("fn", field("name", alias($.identifier, $.binding)), $.function_body),
    function_body: ($) =>
      seq(
        repeat1(seq($._parameter_gap, field("parameter", $._parameter))),
        "=",
        repeat($._newline),
        field("body", $._expression),
      ),
    recursive_group: ($) =>
      seq(
        "rec",
        "{",
        repeat($._separator),
        $.function_declaration,
        repeat(seq(repeat1($._separator), $.function_declaration)),
        repeat($._separator),
        "}",
      ),
    struct_declaration: ($) =>
      seq("struct", field("name", alias($.identifier, $.binding)), $.field_declarations),
    enum_declaration: ($) =>
      seq(
        "enum",
        field("name", alias($.identifier, $.binding)),
        "{",
        repeat($._newline),
        commaList($.variant_declaration, repeat($._newline)),
        "}",
      ),
    variant_declaration: ($) =>
      seq(field("name", alias($.identifier, $.field_identifier)), optional($.field_declarations)),
    field_declarations: ($) =>
      seq(
        "{",
        repeat($._newline),
        commaList(alias($.identifier, $.field_identifier), repeat($._newline)),
        "}",
      ),
    import_declaration: ($) =>
      seq("import", field("path", $.string), "as", field("name", alias($.identifier, $.binding))),
    export_declaration: ($) =>
      seq("export", "{", repeat($._newline), commaList($.export_field, repeat($._newline)), "}"),
    export_field: ($) =>
      seq(field("name", $._key), optional(seq(":", field("value", $.qualified_name)))),
    qualified_name: ($) =>
      seq($.identifier, repeat(seq(".", alias($.identifier, $.field_identifier)))),
    _parameter: ($) =>
      choice(
        alias($.identifier, $.binding),
        $.unit,
        $.group_pattern,
        $.list_pattern,
        $.record_pattern,
        $.number,
        $.negative_pattern,
        $.string,
        $.boolean,
        $.null,
        alias($.parameter_constructor, $.constructor_pattern),
      ),
    parameter_constructor: ($) =>
      seq(
        $.identifier,
        repeat1(seq(".", alias($.identifier, $.field_identifier))),
        optional($.record_pattern),
      ),
    _pattern: ($) =>
      choice(
        alias($.identifier, $.binding),
        $.unit,
        $.group_pattern,
        $.list_pattern,
        $.record_pattern,
        $.constructor_pattern,
        $.number,
        $.negative_pattern,
        $.string,
        $.boolean,
        $.null,
      ),
    negative_pattern: ($) => seq("-", $.number),
    group_pattern: ($) => seq("(", repeat($._newline), $._pattern, repeat($._newline), ")"),
    list_pattern: ($) =>
      seq(
        "[",
        repeat($._newline),
        commaList($._pattern, repeat($._newline)),
        optional($.rest_pattern),
        repeat($._newline),
        "]",
      ),
    record_pattern: ($) =>
      seq(
        "{",
        repeat($._newline),
        commaList($.pattern_field, repeat($._newline)),
        optional($.rest_pattern),
        repeat($._newline),
        "}",
      ),
    pattern_field: ($) =>
      choice(
        seq(field("name", $._key), ":", field("pattern", $._pattern)),
        field("name", alias($.identifier, $.binding)),
      ),
    rest_pattern: ($) => seq("..", optional(alias($.identifier, $.binding))),
    constructor_pattern: ($) =>
      prec(
        1,
        choice(
          seq($.qualified_name, $.record_pattern),
          seq($.identifier, repeat1(seq(".", alias($.identifier, $.field_identifier)))),
        ),
      ),
    match_arm: ($) =>
      seq(
        field("pattern", $._pattern),
        optional(seq("if", $._expression)),
        repeat($._newline),
        "=>",
        repeat($._newline),
        field("body", $._expression),
      ),
    unit: ($) => seq("(", repeat($._newline), ")"),
    group: ($) => seq("(", repeat($._newline), $._expression_soft, repeat($._newline), ")"),
    list: ($) =>
      seq("[", repeat($._newline), commaList($._expression_soft, repeat($._newline)), "]"),
    record: ($) => seq("{", repeat($._newline), commaList($.record_field, repeat($._newline)), "}"),
    record_field: ($) =>
      seq(
        field("name", $._key),
        optional(seq(":", repeat($._newline), field("value", $._expression_soft))),
      ),
    closure: ($) =>
      seq(
        "{",
        repeat($._newline),
        field("parameter", $._parameter),
        repeat(seq($._parameter_gap, field("parameter", $._parameter))),
        repeat($._newline),
        "=>",
        field("body", optional($._body)),
        "}",
      ),
    recursive_closure: ($) =>
      seq(
        "rec",
        "{",
        repeat($._newline),
        field("name", alias($.identifier, $.binding)),
        repeat1(seq($._parameter_gap, field("parameter", $._parameter))),
        repeat($._newline),
        "=>",
        field("body", optional($._body)),
        "}",
      ),
    block: ($) => seq("do", "{", repeat($._separator), optional($._body), "}"),
    job: ($) => seq("job", "{", repeat($._newline), $.command_pipeline, repeat($._newline), "}"),
    command_pipeline: ($) =>
      prec.left(
        seq($.command, repeat(seq(optional($._pipeline_gap), "|", repeat($._newline), $.command))),
      ),
    command: ($) =>
      seq(
        "^",
        field("executable", $._command_argument),
        repeat(
          choice(
            seq($._command_gap, choice($._command_argument, $.spread_argument)),
            $.redirection,
          ),
        ),
      ),
    _command_argument: ($) => choice($.command_word, $.string, $.substitution),
    substitution: ($) =>
      seq(
        "$",
        choice(
          $.identifier,
          seq("(", repeat($._newline), $._expression_soft, repeat($._newline), ")"),
        ),
      ),
    spread_argument: ($) => seq("...", $.substitution),
    redirection: ($) =>
      choice("2>&1", seq(choice("<", ">", ">>", "2>", "2>>"), $._command_argument)),
    identifier: (_) => /[A-Za-z_][A-Za-z0-9_]*/,
    _field_name: ($) =>
      choice(
        alias($.identifier, $.field_identifier),
        "let",
        "fn",
        "rec",
        "if",
        "then",
        "else",
        "do",
        "match",
        "of",
        "struct",
        "enum",
        "with",
        "job",
        "import",
        "as",
        "export",
        "true",
        "false",
        "null",
        "and",
        "or",
        "not",
      ),
    _key: ($) => choice($._field_name, $.string),
    number: (_) => /[0-9]([0-9_]*[0-9])?(\.[0-9]([0-9_]*[0-9])?)?([eE][+-]?[0-9]([0-9_]*[0-9])?)?/,
    string: (_) =>
      token(
        choice(
          seq('"', repeat(choice(/[^"\\]/, /\\["\\nrt]/, /\\u\{[0-9a-fA-F]{1,6}\}/)), '"'),
          seq("'", /[^']*/, "'"),
        ),
      ),
    boolean: (_) => choice("true", "false"),
    null: (_) => "null",
    ...expressionRules(false),
    ...expressionRules(true),
  },
});
