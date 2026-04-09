//! Integration tests ported from `src/Parser.test.ts`.
//!
//! Each TypeScript `test('description', ...)` is translated to a Rust `#[test]`.

use std::collections::{BTreeMap, HashMap};

use incurs::parser::{parse, ParseOptions};
use incurs::schema::{to_kebab, FieldMeta, FieldType};
use serde_json::{json, Value};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Shorthand to build a `Vec<String>` from literal tokens.
fn argv(tokens: &[&str]) -> Vec<String> {
    tokens.iter().map(|s| s.to_string()).collect()
}

/// Build a `FieldMeta` with sensible defaults; caller can customise via the
/// returned value.
fn make_field(
    name: &'static str,
    field_type: FieldType,
    required: bool,
    default: Option<Value>,
    alias: Option<char>,
) -> FieldMeta {
    FieldMeta {
        name,
        cli_name: to_kebab(name),
        description: None,
        field_type,
        required,
        default,
        alias,
        deprecated: false,
        env_name: None,
    }
}

/// Convenience: default `ParseOptions` with no args, options, aliases, or
/// defaults.
fn empty_opts() -> ParseOptions {
    ParseOptions {
        args_fields: vec![],
        options_fields: vec![],
        aliases: HashMap::new(),
        defaults: None,
    }
}

// ---------------------------------------------------------------------------
// Tests ported from Parser.test.ts
// ---------------------------------------------------------------------------

#[test]
fn returns_empty_args_and_options_when_no_schemas() {
    let result = parse(&argv(&[]), &empty_opts()).unwrap();
    assert!(result.args.is_empty());
    assert!(result.options.is_empty());
}

#[test]
fn parses_positional_args_in_schema_key_order() {
    let opts = ParseOptions {
        args_fields: vec![
            make_field("greeting", FieldType::String, true, None, None),
            make_field("name", FieldType::String, true, None, None),
        ],
        ..empty_opts()
    };
    let result = parse(&argv(&["hello", "world"]), &opts).unwrap();
    assert_eq!(result.args["greeting"], json!("hello"));
    assert_eq!(result.args["name"], json!("world"));
}

#[test]
fn parses_flag_value_options() {
    let opts = ParseOptions {
        options_fields: vec![make_field("state", FieldType::String, true, None, None)],
        ..empty_opts()
    };
    let result = parse(&argv(&["--state", "open"]), &opts).unwrap();
    assert_eq!(result.options["state"], json!("open"));
}

#[test]
fn parses_flag_equals_value_syntax() {
    let opts = ParseOptions {
        options_fields: vec![make_field("state", FieldType::String, true, None, None)],
        ..empty_opts()
    };
    let result = parse(&argv(&["--state=closed"]), &opts).unwrap();
    assert_eq!(result.options["state"], json!("closed"));
}

#[test]
fn parses_short_alias_with_value() {
    let mut aliases = HashMap::new();
    aliases.insert("state".to_string(), 's');
    let opts = ParseOptions {
        options_fields: vec![make_field("state", FieldType::String, true, None, None)],
        aliases,
        ..empty_opts()
    };
    let result = parse(&argv(&["-s", "open"]), &opts).unwrap();
    assert_eq!(result.options["state"], json!("open"));
}

#[test]
fn parses_verbose_as_true() {
    let opts = ParseOptions {
        options_fields: vec![make_field("verbose", FieldType::Boolean, true, None, None)],
        ..empty_opts()
    };
    let result = parse(&argv(&["--verbose"]), &opts).unwrap();
    assert_eq!(result.options["verbose"], json!(true));
}

#[test]
fn parses_no_verbose_as_false() {
    let opts = ParseOptions {
        options_fields: vec![make_field("verbose", FieldType::Boolean, true, None, None)],
        ..empty_opts()
    };
    let result = parse(&argv(&["--no-verbose"]), &opts).unwrap();
    assert_eq!(result.options["verbose"], json!(false));
}

#[test]
fn parses_repeated_flags_as_array() {
    let opts = ParseOptions {
        options_fields: vec![make_field(
            "label",
            FieldType::Array(Box::new(FieldType::String)),
            false,
            None,
            None,
        )],
        ..empty_opts()
    };
    let result = parse(&argv(&["--label", "bug", "--label", "feature"]), &opts).unwrap();
    assert_eq!(result.options["label"], json!(["bug", "feature"]));
}

#[test]
fn coerces_string_to_number() {
    let opts = ParseOptions {
        options_fields: vec![make_field("limit", FieldType::Number, true, None, None)],
        ..empty_opts()
    };
    let result = parse(&argv(&["--limit", "10"]), &opts).unwrap();
    assert_eq!(result.options["limit"], json!(10.0));
}

#[test]
fn coerces_string_to_boolean() {
    let opts = ParseOptions {
        options_fields: vec![make_field("dry", FieldType::Boolean, true, None, None)],
        ..empty_opts()
    };
    let result = parse(&argv(&["--dry", "true"]), &opts).unwrap();
    assert_eq!(result.options["dry"], json!(true));
}

#[test]
fn applies_default_values_for_missing_options() {
    let opts = ParseOptions {
        options_fields: vec![make_field(
            "limit",
            FieldType::Number,
            false,
            Some(json!(30)),
            None,
        )],
        ..empty_opts()
    };
    let result = parse(&argv(&[]), &opts).unwrap();
    assert_eq!(result.options["limit"], json!(30));
}

#[test]
fn allows_optional_fields_to_be_omitted() {
    let opts = ParseOptions {
        options_fields: vec![make_field("verbose", FieldType::Boolean, false, None, None)],
        ..empty_opts()
    };
    let result = parse(&argv(&[]), &opts).unwrap();
    assert!(result.options.is_empty());
}

#[test]
fn returns_error_on_unknown_flags() {
    let opts = ParseOptions {
        options_fields: vec![make_field("state", FieldType::String, true, None, None)],
        ..empty_opts()
    };
    let result = parse(&argv(&["--unknown", "val"]), &opts);
    assert!(result.is_err());
}

#[test]
fn missing_required_positional_args_produces_empty_entry() {
    // NOTE: The Rust parser does not validate required positional args.
    // The TS version throws a ValidationError. In Rust, validation is a
    // separate step (IncurSchema::from_raw). Here we verify the parser
    // simply omits the arg from the result.
    let opts = ParseOptions {
        args_fields: vec![make_field("name", FieldType::String, true, None, None)],
        ..empty_opts()
    };
    let result = parse(&argv(&[]), &opts).unwrap();
    assert!(!result.args.contains_key("name"));
}

#[test]
fn enum_value_passes_through_parser() {
    // NOTE: The Rust parser does not validate enum values. The TS version
    // throws a ValidationError on mismatch. In Rust, validation is a
    // separate step. Here we verify the parser passes through the value.
    let opts = ParseOptions {
        options_fields: vec![make_field(
            "state",
            FieldType::Enum(vec!["open".into(), "closed".into()]),
            true,
            None,
            None,
        )],
        ..empty_opts()
    };
    let result = parse(&argv(&["--state", "invalid"]), &opts).unwrap();
    assert_eq!(result.options["state"], json!("invalid"));
}

#[test]
fn stacks_boolean_short_aliases() {
    let mut aliases = HashMap::new();
    aliases.insert("verbose".to_string(), 'v');
    aliases.insert("debug".to_string(), 'D');
    let opts = ParseOptions {
        options_fields: vec![
            make_field("verbose", FieldType::Boolean, false, Some(json!(false)), None),
            make_field("debug", FieldType::Boolean, false, Some(json!(false)), None),
        ],
        aliases,
        ..empty_opts()
    };
    let result = parse(&argv(&["-vD"]), &opts).unwrap();
    assert_eq!(result.options["verbose"], json!(true));
    assert_eq!(result.options["debug"], json!(true));
}

#[test]
fn last_flag_in_stack_takes_a_value() {
    let mut aliases = HashMap::new();
    aliases.insert("verbose".to_string(), 'v');
    aliases.insert("debug".to_string(), 'D');
    aliases.insert("format".to_string(), 'f');
    let opts = ParseOptions {
        options_fields: vec![
            make_field("verbose", FieldType::Boolean, false, Some(json!(false)), None),
            make_field("debug", FieldType::Boolean, false, Some(json!(false)), None),
            make_field("format", FieldType::String, false, Some(json!("text")), None),
        ],
        aliases,
        ..empty_opts()
    };
    let result = parse(&argv(&["-vDf", "json"]), &opts).unwrap();
    assert_eq!(result.options["verbose"], json!(true));
    assert_eq!(result.options["debug"], json!(true));
    assert_eq!(result.options["format"], json!("json"));
}

#[test]
fn returns_error_for_non_boolean_mid_stack() {
    let mut aliases = HashMap::new();
    aliases.insert("format".to_string(), 'f');
    aliases.insert("verbose".to_string(), 'v');
    let opts = ParseOptions {
        options_fields: vec![
            make_field("format", FieldType::String, true, None, None),
            make_field("verbose", FieldType::Boolean, false, Some(json!(false)), None),
        ],
        aliases,
        ..empty_opts()
    };
    let result = parse(&argv(&["-fv"]), &opts);
    assert!(result.is_err());
}

#[test]
fn returns_error_when_last_flag_in_stack_missing_value() {
    let mut aliases = HashMap::new();
    aliases.insert("verbose".to_string(), 'v');
    aliases.insert("format".to_string(), 'f');
    let opts = ParseOptions {
        options_fields: vec![
            make_field("verbose", FieldType::Boolean, false, Some(json!(false)), None),
            make_field("format", FieldType::String, true, None, None),
        ],
        aliases,
        ..empty_opts()
    };
    let result = parse(&argv(&["-vf"]), &opts);
    assert!(result.is_err());
}

#[test]
fn single_boolean_short_alias_works() {
    let mut aliases = HashMap::new();
    aliases.insert("verbose".to_string(), 'v');
    let opts = ParseOptions {
        options_fields: vec![make_field(
            "verbose",
            FieldType::Boolean,
            false,
            Some(json!(false)),
            None,
        )],
        aliases,
        ..empty_opts()
    };
    let result = parse(&argv(&["-v"]), &opts).unwrap();
    assert_eq!(result.options["verbose"], json!(true));
}

#[test]
fn returns_error_for_unknown_alias_in_stack() {
    let mut aliases = HashMap::new();
    aliases.insert("verbose".to_string(), 'v');
    let opts = ParseOptions {
        options_fields: vec![make_field(
            "verbose",
            FieldType::Boolean,
            false,
            Some(json!(false)),
            None,
        )],
        aliases,
        ..empty_opts()
    };
    let result = parse(&argv(&["-vx"]), &opts);
    assert!(result.is_err());
}

#[test]
fn detects_boolean_through_nested_optional_default() {
    // z.boolean().default(false).optional() -> Boolean, required=false, default=Some(false)
    let opts = ParseOptions {
        options_fields: vec![make_field(
            "verbose",
            FieldType::Boolean,
            false,
            Some(json!(false)),
            None,
        )],
        ..empty_opts()
    };
    let result = parse(&argv(&["--verbose"]), &opts).unwrap();
    assert_eq!(result.options["verbose"], json!(true));
}

#[test]
fn detects_array_through_optional() {
    let opts = ParseOptions {
        options_fields: vec![make_field(
            "label",
            FieldType::Array(Box::new(FieldType::String)),
            false,
            None,
            None,
        )],
        ..empty_opts()
    };
    let result = parse(&argv(&["--label", "bug", "--label", "fix"]), &opts).unwrap();
    assert_eq!(result.options["label"], json!(["bug", "fix"]));
}

#[test]
fn detects_array_through_default() {
    let opts = ParseOptions {
        options_fields: vec![make_field(
            "label",
            FieldType::Array(Box::new(FieldType::String)),
            false,
            Some(json!([])),
            None,
        )],
        ..empty_opts()
    };
    let result = parse(&argv(&["--label", "bug", "--label", "fix"]), &opts).unwrap();
    assert_eq!(result.options["label"], json!(["bug", "fix"]));
}

#[test]
fn count_defaults_to_zero_when_flag_not_provided() {
    let opts = ParseOptions {
        options_fields: vec![make_field(
            "verbose",
            FieldType::Count,
            false,
            Some(json!(0)),
            None,
        )],
        ..empty_opts()
    };
    let result = parse(&argv(&[]), &opts).unwrap();
    assert_eq!(result.options["verbose"], json!(0));
}

#[test]
fn count_single_flag_increments_to_one() {
    let opts = ParseOptions {
        options_fields: vec![make_field(
            "verbose",
            FieldType::Count,
            false,
            Some(json!(0)),
            None,
        )],
        ..empty_opts()
    };
    let result = parse(&argv(&["--verbose"]), &opts).unwrap();
    assert_eq!(result.options["verbose"], json!(1));
}

#[test]
fn count_repeated_flags_increment() {
    let opts = ParseOptions {
        options_fields: vec![make_field(
            "verbose",
            FieldType::Count,
            false,
            Some(json!(0)),
            None,
        )],
        ..empty_opts()
    };
    let result = parse(&argv(&["--verbose", "--verbose"]), &opts).unwrap();
    assert_eq!(result.options["verbose"], json!(2));
}

#[test]
fn count_stacked_alias_increments() {
    let mut aliases = HashMap::new();
    aliases.insert("verbose".to_string(), 'v');
    let opts = ParseOptions {
        options_fields: vec![make_field(
            "verbose",
            FieldType::Count,
            false,
            Some(json!(0)),
            None,
        )],
        aliases,
        ..empty_opts()
    };
    let result = parse(&argv(&["-vv"]), &opts).unwrap();
    assert_eq!(result.options["verbose"], json!(2));
}

#[test]
fn count_mixed_stacking_with_boolean() {
    let mut aliases = HashMap::new();
    aliases.insert("verbose".to_string(), 'v');
    aliases.insert("debug".to_string(), 'D');
    let opts = ParseOptions {
        options_fields: vec![
            make_field("verbose", FieldType::Count, false, Some(json!(0)), None),
            make_field("debug", FieldType::Boolean, false, Some(json!(false)), None),
        ],
        aliases,
        ..empty_opts()
    };
    let result = parse(&argv(&["-vvD"]), &opts).unwrap();
    assert_eq!(result.options["verbose"], json!(2));
    assert_eq!(result.options["debug"], json!(true));
}

#[test]
fn count_describe_works() {
    // .describe() in TS maps to the description field; it shouldn't affect
    // parsing behaviour.
    let mut aliases = HashMap::new();
    aliases.insert("verbose".to_string(), 'v');
    let opts = ParseOptions {
        options_fields: vec![{
            let mut f = make_field("verbose", FieldType::Count, false, Some(json!(0)), None);
            f.description = Some("Verbosity level");
            f
        }],
        aliases,
        ..empty_opts()
    };
    let result = parse(&argv(&["-v"]), &opts).unwrap();
    assert_eq!(result.options["verbose"], json!(1));
}

#[test]
fn parses_positional_args_and_options_together() {
    let opts = ParseOptions {
        args_fields: vec![make_field("repo", FieldType::String, true, None, None)],
        options_fields: vec![make_field("limit", FieldType::Number, true, None, None)],
        ..empty_opts()
    };
    let result = parse(&argv(&["myrepo", "--limit", "5"]), &opts).unwrap();
    assert_eq!(result.args["repo"], json!("myrepo"));
    assert_eq!(result.options["limit"], json!(5.0));
}

#[test]
fn applies_config_defaults_when_argv_omits_an_option() {
    let mut defaults = BTreeMap::new();
    defaults.insert("limit".to_string(), json!(10));
    let opts = ParseOptions {
        options_fields: vec![make_field(
            "limit",
            FieldType::Number,
            false,
            Some(json!(30)),
            None,
        )],
        defaults: Some(defaults),
        ..empty_opts()
    };
    let result = parse(&argv(&[]), &opts).unwrap();
    assert_eq!(result.options["limit"], json!(10));
}

#[test]
fn argv_overrides_config_defaults() {
    let mut defaults = BTreeMap::new();
    defaults.insert("limit".to_string(), json!(10));
    let opts = ParseOptions {
        options_fields: vec![make_field(
            "limit",
            FieldType::Number,
            false,
            Some(json!(30)),
            None,
        )],
        defaults: Some(defaults),
        ..empty_opts()
    };
    let result = parse(&argv(&["--limit", "5"]), &opts).unwrap();
    assert_eq!(result.options["limit"], json!(5.0));
}

#[test]
fn argv_arrays_replace_config_arrays() {
    let mut defaults = BTreeMap::new();
    defaults.insert("label".to_string(), json!(["ops"]));
    let opts = ParseOptions {
        options_fields: vec![make_field(
            "label",
            FieldType::Array(Box::new(FieldType::String)),
            false,
            Some(json!([])),
            None,
        )],
        defaults: Some(defaults),
        ..empty_opts()
    };
    let result = parse(&argv(&["--label", "bug", "--label", "feature"]), &opts).unwrap();
    assert_eq!(result.options["label"], json!(["bug", "feature"]));
}

#[test]
fn kebab_case_config_keys_map_to_snake_case_field_names() {
    // In TS this tests camelCase; Rust uses snake_case field names.
    // The config file might use kebab-case keys ("save-dev") which the parser
    // should normalise to snake_case ("save_dev") when merging defaults.
    let mut defaults = BTreeMap::new();
    defaults.insert("save-dev".to_string(), json!(true));
    let opts = ParseOptions {
        options_fields: vec![make_field(
            "save_dev",
            FieldType::Boolean,
            false,
            Some(json!(false)),
            None,
        )],
        defaults: Some(defaults),
        ..empty_opts()
    };
    let result = parse(&argv(&[]), &opts).unwrap();
    assert_eq!(result.options["save_dev"], json!(true));
}

#[test]
fn returns_error_on_unknown_config_option_keys() {
    let mut defaults = BTreeMap::new();
    defaults.insert("missing".to_string(), json!(true));
    let opts = ParseOptions {
        options_fields: vec![make_field(
            "save_dev",
            FieldType::Boolean,
            false,
            Some(json!(false)),
            None,
        )],
        defaults: Some(defaults),
        ..empty_opts()
    };
    let result = parse(&argv(&[]), &opts);
    assert!(result.is_err());
}

#[test]
fn invalid_config_defaults_error_when_argv_does_not_override() {
    // In TS, this is a ValidationError for a type mismatch ("oops" for a
    // number field). The Rust parser should also error.
    let mut defaults = BTreeMap::new();
    defaults.insert("limit".to_string(), json!("oops"));
    let opts = ParseOptions {
        options_fields: vec![make_field("limit", FieldType::Number, true, None, None)],
        defaults: Some(defaults),
        ..empty_opts()
    };
    let result = parse(&argv(&[]), &opts);
    assert!(result.is_err());
}

#[test]
fn argv_overrides_invalid_config_defaults() {
    let mut defaults = BTreeMap::new();
    defaults.insert("limit".to_string(), json!("oops"));
    let opts = ParseOptions {
        options_fields: vec![make_field("limit", FieldType::Number, true, None, None)],
        defaults: Some(defaults),
        ..empty_opts()
    };
    let result = parse(&argv(&["--limit", "5"]), &opts).unwrap();
    assert_eq!(result.options["limit"], json!(5.0));
}

#[test]
fn defaults_with_no_options_schema_throws_on_non_empty_defaults() {
    let mut defaults = BTreeMap::new();
    defaults.insert("limit".to_string(), json!(10));
    let opts = ParseOptions {
        options_fields: vec![],
        defaults: Some(defaults),
        ..empty_opts()
    };
    let result = parse(&argv(&[]), &opts);
    assert!(result.is_err());
}

#[test]
fn defaults_with_no_options_schema_and_empty_defaults_is_noop() {
    let opts = ParseOptions {
        options_fields: vec![],
        defaults: Some(BTreeMap::new()),
        ..empty_opts()
    };
    let result = parse(&argv(&[]), &opts).unwrap();
    assert!(result.options.is_empty());
}

#[test]
fn config_array_defaults_are_used_when_argv_omits_the_option() {
    let mut defaults = BTreeMap::new();
    defaults.insert("label".to_string(), json!(["bug", "feature"]));
    let opts = ParseOptions {
        options_fields: vec![make_field(
            "label",
            FieldType::Array(Box::new(FieldType::String)),
            false,
            Some(json!([])),
            None,
        )],
        defaults: Some(defaults),
        ..empty_opts()
    };
    let result = parse(&argv(&[]), &opts).unwrap();
    assert_eq!(result.options["label"], json!(["bug", "feature"]));
}

#[test]
fn refined_option_schemas_validate_only_merged_winning_values() {
    // In TS, `.refine()` validates post-merge. The Rust parser has no
    // refinement step, but we verify that argv values override defaults and
    // that both values are present after parsing.
    let mut defaults = BTreeMap::new();
    defaults.insert("min".to_string(), json!("oops"));
    let opts = ParseOptions {
        options_fields: vec![
            make_field("min", FieldType::Number, true, None, None),
            make_field("max", FieldType::Number, true, None, None),
        ],
        defaults: Some(defaults),
        ..empty_opts()
    };
    let result = parse(&argv(&["--min", "1", "--max", "3"]), &opts).unwrap();
    assert_eq!(result.options["min"], json!(1.0));
    assert_eq!(result.options["max"], json!(3.0));
}
