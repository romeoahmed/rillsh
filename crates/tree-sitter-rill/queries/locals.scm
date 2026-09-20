(source_file) @local.scope
(block) @local.scope
(closure) @local.scope
(recursive_closure) @local.scope
(function_body) @local.scope
(match_arm) @local.scope

((binding) @local.definition
 (#not-eq? @local.definition "_"))
(identifier) @local.reference
(record_field name: (field_identifier) @local.reference !value)
(export_field name: (field_identifier) @local.reference !value)
