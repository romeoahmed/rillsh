(comment) @comment
(string) @string
(number) @number
(boolean) @constant.builtin
(null) @constant.builtin
[(identifier) (binding)] @variable
(field_identifier) @property
(function_declaration name: (binding) @function)
(struct_declaration name: (binding) @type)
(enum_declaration name: (binding) @type)
(variant_declaration name: (field_identifier) @constructor)
(field_expression field: (_) @property)
(command_word) @string.special
(command executable: (command_word) @function.call)
["let" "fn" "rec" "if" "then" "else" "do" "match" "of" "struct" "enum" "with" "job" "import" "as" "export"] @keyword
["+" "-" "*" "/" "==" "!=" "<" "<=" ">" ">=" "and" "or" "not" "|>" "|" "^" "=" "=>"] @operator
["(" ")" "[" "]" "{" "}"] @punctuation.bracket
["," ":" "." ";"] @punctuation.delimiter
