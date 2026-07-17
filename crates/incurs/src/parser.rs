//! Argv and environment variable parser for the incurs framework.
//!
//! Ported from `src/Parser.ts`. Takes raw argv tokens and parses them against
//! [`FieldMeta`] metadata, producing a [`ParseResult`] with coerced values.

use std::collections::{BTreeMap, HashMap};

use serde_json::Value;

use crate::errors::ParseError;
use crate::schema::{FieldMeta, FieldType, to_kebab, to_snake};

/// Options controlling how [`parse`] interprets argv tokens.
pub struct ParseOptions {
    /// Field metadata for positional args (order matters).
    pub args_fields: Vec<FieldMeta>,
    /// Field metadata for named options.
    pub options_fields: Vec<FieldMeta>,
    /// Map of option names (snake_case) to single-char aliases.
    pub aliases: HashMap<String, char>,
    /// Config-backed default values, merged under argv.
    pub defaults: Option<BTreeMap<String, Value>>,
}

/// The result of parsing argv tokens.
#[derive(Debug, Clone)]
pub struct ParseResult {
    /// Parsed positional arguments.
    pub args: BTreeMap<String, Value>,
    /// Parsed named options.
    pub options: BTreeMap<String, Value>,
}

/// The result of extracting CLI-level global options from argv.
#[derive(Debug, Clone)]
pub struct ParseGlobalsResult {
    /// Parsed global option values.
    pub parsed: BTreeMap<String, Value>,
    /// Tokens not consumed as globals, preserved for command parsing.
    pub rest: Vec<String>,
}

// ---------------------------------------------------------------------------
// Internal lookup tables
// ---------------------------------------------------------------------------

/// Pre-computed lookup tables for fast option resolution.
struct OptionNames {
    /// Known option names (snake_case).
    known: HashMap<String, ()>,
    /// kebab-case -> snake_case mapping.
    kebab_to_snake: HashMap<String, String>,
    /// alias char -> snake_case name.
    alias_to_name: HashMap<char, String>,
    /// snake_case -> FieldType lookup.
    field_types: HashMap<String, FieldType>,
}

impl OptionNames {
    fn build(fields: &[FieldMeta], aliases: &HashMap<String, char>) -> Self {
        let mut known = HashMap::new();
        let mut kebab_to_snake = HashMap::new();
        let mut alias_to_name = HashMap::new();
        let mut field_types = HashMap::new();

        for field in fields {
            let snake = field.name.to_string();
            known.insert(snake.clone(), ());
            field_types.insert(snake.clone(), field.field_type.clone());

            let kebab = to_kebab(&snake);
            if kebab != snake {
                kebab_to_snake.insert(kebab, snake.clone());
            }

            if let Some(alias_char) = field.alias {
                alias_to_name.insert(alias_char, snake.clone());
            }
        }

        // Aliases from the explicit map override field-level aliases.
        for (name, &ch) in aliases {
            alias_to_name.insert(ch, name.clone());
        }

        OptionNames {
            known,
            kebab_to_snake,
            alias_to_name,
            field_types,
        }
    }

    /// Resolve a raw long-option name (kebab or snake) to its canonical snake_case name.
    fn normalize(&self, raw: &str) -> Option<String> {
        let name = self
            .kebab_to_snake
            .get(raw)
            .cloned()
            .unwrap_or_else(|| to_snake(raw));
        if self.known.contains_key(&name) {
            Some(name)
        } else {
            None
        }
    }

    fn is_boolean(&self, name: &str) -> bool {
        matches!(self.field_types.get(name), Some(FieldType::Boolean))
    }

    fn is_count(&self, name: &str) -> bool {
        matches!(self.field_types.get(name), Some(FieldType::Count))
    }

    fn is_array(&self, name: &str) -> bool {
        matches!(self.field_types.get(name), Some(FieldType::Array(_)))
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Sets an option value, collecting into arrays for array-typed fields.
fn set_option(raw: &mut BTreeMap<String, Value>, name: &str, value: &str, names: &OptionNames) {
    if names.is_array(name) {
        match raw.get_mut(name) {
            Some(Value::Array(arr)) => {
                arr.push(Value::String(value.to_string()));
            }
            _ => {
                raw.insert(
                    name.to_string(),
                    Value::Array(vec![Value::String(value.to_string())]),
                );
            }
        }
    } else {
        raw.insert(name.to_string(), Value::String(value.to_string()));
    }
}

/// Coerces a `Value` to match the expected `FieldType`.
fn coerce(value: Value, field_type: &FieldType) -> Value {
    match field_type {
        FieldType::Number => match &value {
            Value::String(s) => s
                .parse::<i64>()
                .map(Value::from)
                .or_else(|_| {
                    s.parse::<f64>().map(|number| {
                        serde_json::Number::from_f64(number)
                            .map(Value::Number)
                            .unwrap_or(value.clone())
                    })
                })
                .unwrap_or(value),
            _ => value,
        },
        FieldType::Boolean => match &value {
            Value::String(s) => Value::Bool(s == "true" || s == "1"),
            _ => value,
        },
        FieldType::Array(inner) => match value {
            Value::Array(arr) => Value::Array(arr.into_iter().map(|v| coerce(v, inner)).collect()),
            _ => value,
        },
        _ => value,
    }
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Parses raw argv tokens against schema metadata.
///
/// Supports:
/// - `--key value` and `--key=value` long options
/// - `--no-flag` boolean negation
/// - `-f value` short aliases
/// - `-abc` stacked short aliases (all but last must be boolean/count)
/// - `-vvv` count flag incrementing
/// - `--tag x --tag y` array collection
/// - Positional arguments assigned to `args_fields` in order
/// - Coercion from strings to numbers/booleans based on field type
pub fn parse(argv: &[String], options: &ParseOptions) -> Result<ParseResult, ParseError> {
    let names = OptionNames::build(&options.options_fields, &options.aliases);

    let mut positionals: Vec<String> = Vec::new();
    let mut raw_options: BTreeMap<String, Value> = BTreeMap::new();

    let mut i = 0;
    while i < argv.len() {
        let token = &argv[i];

        if token.starts_with("--no-") && token.len() > 5 {
            // --no-flag negation
            let raw_name = &token[5..];
            let name = names.normalize(raw_name).ok_or_else(|| ParseError {
                message: format!("Unknown flag: {}", token),
                cause: None,
            })?;
            raw_options.insert(name, Value::Bool(false));
            i += 1;
        } else if token.starts_with("--") {
            let rest = &token[2..];
            if let Some(eq_idx) = rest.find('=') {
                // --flag=value
                let raw_name = &rest[..eq_idx];
                let name = names.normalize(raw_name).ok_or_else(|| ParseError {
                    message: format!("Unknown flag: --{}", raw_name),
                    cause: None,
                })?;
                let val = &rest[eq_idx + 1..];
                set_option(&mut raw_options, &name, val, &names);
                i += 1;
            } else {
                // --flag [value]
                let name = names.normalize(rest).ok_or_else(|| ParseError {
                    message: format!("Unknown flag: {}", token),
                    cause: None,
                })?;
                if names.is_count(&name) {
                    let prev = raw_options.get(&name).and_then(|v| v.as_u64()).unwrap_or(0);
                    raw_options.insert(name, Value::Number((prev + 1).into()));
                    i += 1;
                } else if names.is_boolean(&name) {
                    raw_options.insert(name, Value::Bool(true));
                    i += 1;
                } else {
                    let value = argv.get(i + 1).ok_or_else(|| ParseError {
                        message: format!("Missing value for flag: {}", token),
                        cause: None,
                    })?;
                    set_option(&mut raw_options, &name, value, &names);
                    i += 2;
                }
            }
        } else if token.starts_with('-') && !token.starts_with("--") && token.len() >= 2 {
            // -f or -abc (stacked short aliases)
            let chars: Vec<char> = token[1..].chars().collect();
            for (j, &ch) in chars.iter().enumerate() {
                let name = names.alias_to_name.get(&ch).ok_or_else(|| ParseError {
                    message: format!("Unknown flag: -{}", ch),
                    cause: None,
                })?;
                let is_last = j == chars.len() - 1;

                if !is_last {
                    // Non-last chars in a stack must be boolean or count.
                    if names.is_count(name) {
                        let prev = raw_options.get(name).and_then(|v| v.as_u64()).unwrap_or(0);
                        raw_options.insert(name.clone(), Value::Number((prev + 1).into()));
                    } else if names.is_boolean(name) {
                        raw_options.insert(name.clone(), Value::Bool(true));
                    } else {
                        return Err(ParseError {
                            message: format!(
                                "Non-boolean flag -{} must be last in a stacked alias",
                                ch
                            ),
                            cause: None,
                        });
                    }
                } else if names.is_count(name) {
                    let prev = raw_options.get(name).and_then(|v| v.as_u64()).unwrap_or(0);
                    raw_options.insert(name.clone(), Value::Number((prev + 1).into()));
                } else if names.is_boolean(name) {
                    raw_options.insert(name.clone(), Value::Bool(true));
                } else {
                    let value = argv.get(i + 1).ok_or_else(|| ParseError {
                        message: format!("Missing value for flag: -{}", ch),
                        cause: None,
                    })?;
                    set_option(&mut raw_options, name, value, &names);
                    i += 1;
                }
            }
            i += 1;
        } else {
            positionals.push(token.clone());
            i += 1;
        }
    }

    // Assign positionals to args fields in order. A final array field is
    // variadic and collects all remaining positional values.
    let mut args: BTreeMap<String, Value> = BTreeMap::new();
    for (idx, field) in options.args_fields.iter().enumerate() {
        if matches!(field.field_type, FieldType::Array(_)) {
            if idx != options.args_fields.len() - 1 {
                return Err(ParseError {
                    message: format!(
                        "Variadic arg \"{}\" must be the last key in the args schema",
                        field.name
                    ),
                    cause: None,
                });
            }
            let values = positionals[idx..]
                .iter()
                .cloned()
                .map(Value::String)
                .collect::<Vec<_>>();
            if !values.is_empty() {
                args.insert(field.name.to_string(), Value::Array(values));
            }
        } else if let Some(val) = positionals.get(idx) {
            args.insert(field.name.to_string(), Value::String(val.clone()));
        }

        if !args.contains_key(field.name)
            && let Some(default) = &field.default
        {
            args.insert(field.name.to_string(), default.clone());
        }
    }

    // Coerce raw option values to match field types.
    for field in &options.options_fields {
        if let Some(val) = raw_options.remove(field.name) {
            let coerced = coerce(val, &field.field_type);
            raw_options.insert(field.name.to_string(), coerced);
        }
    }

    // Merge defaults (defaults < argv — argv wins).
    if let Some(defaults) = &options.defaults {
        for (key, default_val) in defaults {
            // Normalise kebab-case config keys to snake_case.
            let normalised = to_snake(key);

            // Reject unknown config keys.
            let field = options.options_fields.iter().find(|f| f.name == normalised);
            if field.is_none() {
                return Err(ParseError {
                    message: format!("Unknown config option: {}", key),
                    cause: None,
                });
            }
            let field = field.unwrap();

            // Only merge when argv didn't already set the value.
            if !raw_options.contains_key(&normalised) {
                // Validate that the default value is compatible with the field
                // type. Reject obviously wrong types (e.g. string for number).
                let valid = match &field.field_type {
                    FieldType::Number => default_val.is_number() || default_val.is_null(),
                    FieldType::Boolean => default_val.is_boolean() || default_val.is_null(),
                    FieldType::Array(_) => default_val.is_array() || default_val.is_null(),
                    _ => true,
                };
                if !valid {
                    return Err(ParseError {
                        message: format!(
                            "Invalid config default for \"{}\": expected {}, got {}",
                            key,
                            field.field_type.display_name(),
                            default_val
                        ),
                        cause: None,
                    });
                }
                raw_options.insert(normalised, default_val.clone());
            }
        }
    }

    // Merge field-level defaults for fields not yet set.
    for field in &options.options_fields {
        if !raw_options.contains_key(field.name) {
            if let Some(default_val) = &field.default {
                raw_options.insert(field.name.to_string(), default_val.clone());
            }
        }
    }

    // Coerce args too.
    for field in &options.args_fields {
        if let Some(val) = args.remove(field.name) {
            let coerced = coerce(val, &field.field_type);
            args.insert(field.name.to_string(), coerced);
        }
    }

    validate_fields(&args, &options.args_fields, "argument")?;
    validate_fields(&raw_options, &options.options_fields, "option")?;

    Ok(ParseResult {
        args,
        options: raw_options,
    })
}

/// Validates parsed values against required fields and declared field types.
pub fn validate_fields(
    values: &BTreeMap<String, Value>,
    fields: &[FieldMeta],
    kind: &str,
) -> Result<(), ParseError> {
    for field in fields {
        let Some(value) = values.get(field.name) else {
            if field.required {
                return Err(ParseError {
                    message: format!("Missing required {kind}: {}", field.cli_name),
                    cause: None,
                });
            }
            continue;
        };

        if !value_matches_type(value, &field.field_type) {
            let expected = match &field.field_type {
                FieldType::Enum(values) => values.join(" | "),
                field_type => field_type.display_name(),
            };
            return Err(ParseError {
                message: format!(
                    "Invalid value for {kind} {}: expected {expected}, received {value}",
                    field.cli_name
                ),
                cause: None,
            });
        }
    }
    Ok(())
}

/// Coerces structured object values according to their declared field types.
pub fn coerce_fields(
    mut values: BTreeMap<String, Value>,
    fields: &[FieldMeta],
) -> BTreeMap<String, Value> {
    for field in fields {
        if let Some(value) = values.remove(field.name) {
            values.insert(field.name.to_string(), coerce(value, &field.field_type));
        } else if let Some(default) = &field.default {
            values.insert(field.name.to_string(), default.clone());
        }
    }
    values
}

/// Returns field-level validation details for structured transport inputs.
pub fn field_errors(
    values: &BTreeMap<String, Value>,
    fields: &[FieldMeta],
) -> Vec<crate::errors::FieldError> {
    let mut errors = Vec::new();
    for field in fields {
        let Some(value) = values.get(field.name) else {
            if field.required {
                errors.push(crate::errors::FieldError {
                    path: field.name.to_string(),
                    expected: field.field_type.display_name(),
                    received: "undefined".to_string(),
                    message: "Required".to_string(),
                });
            }
            continue;
        };
        if !value_matches_type(value, &field.field_type) {
            errors.push(crate::errors::FieldError {
                path: field.name.to_string(),
                expected: match &field.field_type {
                    FieldType::Enum(values) => values.join(" | "),
                    field_type => field_type.display_name(),
                },
                received: value.to_string(),
                message: format!("Invalid value for {}", field.cli_name),
            });
        }
    }
    errors
}

/// Extracts known CLI-level global options while preserving command tokens and
/// unknown flags for the regular command parser.
pub fn parse_globals(
    argv: &[String],
    fields: &[FieldMeta],
    aliases: &HashMap<String, char>,
) -> Result<ParseGlobalsResult, ParseError> {
    let names = OptionNames::build(fields, aliases);
    let mut parsed = BTreeMap::new();
    let mut rest = Vec::new();
    let mut i = 0;

    while i < argv.len() {
        let token = &argv[i];
        if token == "--" {
            rest.extend_from_slice(&argv[i..]);
            break;
        }

        if token.starts_with("--no-") && token.len() > 5 {
            if let Some(name) = names.normalize(&token[5..]) {
                parsed.insert(name, Value::Bool(false));
            } else {
                rest.push(token.clone());
            }
            i += 1;
            continue;
        }

        if let Some(raw) = token.strip_prefix("--") {
            if let Some((raw_name, value)) = raw.split_once('=') {
                if let Some(name) = names.normalize(raw_name) {
                    set_option(&mut parsed, &name, value, &names);
                } else {
                    rest.push(token.clone());
                }
                i += 1;
                continue;
            }

            let Some(name) = names.normalize(raw) else {
                rest.push(token.clone());
                i += 1;
                continue;
            };
            if names.is_count(&name) {
                let count = parsed.get(&name).and_then(Value::as_u64).unwrap_or(0);
                parsed.insert(name, Value::Number((count + 1).into()));
                i += 1;
            } else if names.is_boolean(&name) {
                parsed.insert(name, Value::Bool(true));
                i += 1;
            } else {
                let value = argv.get(i + 1).ok_or_else(|| ParseError {
                    message: format!("Missing value for flag: {token}"),
                    cause: None,
                })?;
                set_option(&mut parsed, &name, value, &names);
                i += 2;
            }
            continue;
        }

        if token.starts_with('-') && token.len() >= 2 {
            let chars = token[1..].chars().collect::<Vec<_>>();
            if chars.iter().any(|ch| !names.alias_to_name.contains_key(ch)) {
                rest.push(token.clone());
                i += 1;
                continue;
            }
            for (index, ch) in chars.iter().enumerate() {
                let name = names.alias_to_name.get(ch).expect("checked above");
                let last = index == chars.len() - 1;
                if names.is_count(name) {
                    let count = parsed.get(name).and_then(Value::as_u64).unwrap_or(0);
                    parsed.insert(name.clone(), Value::Number((count + 1).into()));
                } else if names.is_boolean(name) {
                    parsed.insert(name.clone(), Value::Bool(true));
                } else if !last {
                    return Err(ParseError {
                        message: format!("Non-boolean flag -{ch} must be last in a stacked alias"),
                        cause: None,
                    });
                } else {
                    let value = argv.get(i + 1).ok_or_else(|| ParseError {
                        message: format!("Missing value for flag: -{ch}"),
                        cause: None,
                    })?;
                    set_option(&mut parsed, name, value, &names);
                    i += 1;
                }
            }
            i += 1;
            continue;
        }

        rest.push(token.clone());
        i += 1;
    }

    for field in fields {
        if let Some(value) = parsed.remove(field.name) {
            let value = coerce(value, &field.field_type);
            if !value_matches_type(&value, &field.field_type) {
                return Err(ParseError {
                    message: format!(
                        "Invalid value for --{}: expected {}",
                        field.cli_name,
                        field.field_type.display_name()
                    ),
                    cause: None,
                });
            }
            parsed.insert(field.name.to_string(), value);
        } else if let Some(default) = &field.default {
            parsed.insert(field.name.to_string(), default.clone());
        } else if field.required {
            return Err(ParseError {
                message: format!("Missing required global option: --{}", field.cli_name),
                cause: None,
            });
        }
    }

    Ok(ParseGlobalsResult { parsed, rest })
}

/// Splits and validates CLI-level global values from structured HTTP input.
pub fn parse_global_input(
    mut input: BTreeMap<String, Value>,
    fields: &[FieldMeta],
) -> Result<(Value, BTreeMap<String, Value>), ParseError> {
    let mut globals = serde_json::Map::new();
    for field in fields {
        let value = input
            .remove(field.name)
            .or_else(|| input.remove(&field.cli_name));
        if let Some(value) = value {
            let value = coerce(value, &field.field_type);
            if !value_matches_type(&value, &field.field_type) {
                return Err(ParseError {
                    message: format!(
                        "Invalid value for global option --{}: expected {}",
                        field.cli_name,
                        field.field_type.display_name()
                    ),
                    cause: None,
                });
            }
            globals.insert(field.name.to_string(), value);
        } else if let Some(default) = &field.default {
            globals.insert(field.name.to_string(), default.clone());
        } else if field.required {
            return Err(ParseError {
                message: format!("Missing required global option: --{}", field.cli_name),
                cause: None,
            });
        }
    }
    Ok((Value::Object(globals), input))
}

fn value_matches_type(value: &Value, field_type: &FieldType) -> bool {
    match field_type {
        FieldType::String => value.is_string(),
        FieldType::Number | FieldType::Count => value.is_number(),
        FieldType::Boolean => value.is_boolean(),
        FieldType::Array(_) => value.is_array(),
        FieldType::Enum(values) => value
            .as_str()
            .is_some_and(|value| values.iter().any(|candidate| candidate == value)),
        FieldType::Value => true,
    }
}

/// Parses environment variables against field metadata.
///
/// Each field with an `env_name` is looked up in `source`. Values are coerced
/// from strings to the field's declared type.
pub fn parse_env(
    fields: &[FieldMeta],
    source: &HashMap<String, String>,
) -> BTreeMap<String, Value> {
    let mut result = BTreeMap::new();

    for field in fields {
        let env_key = field
            .env_name
            .map(|s| s.to_string())
            .unwrap_or_else(|| field.name.to_uppercase());

        if let Some(raw) = source.get(&env_key) {
            let value = coerce_env(raw, &field.field_type);
            result.insert(field.name.to_string(), value);
        }
    }

    result
}

/// Coerces a raw env-var string to the expected field type.
fn coerce_env(value: &str, field_type: &FieldType) -> Value {
    match field_type {
        FieldType::Number => value
            .parse::<f64>()
            .ok()
            .and_then(|n| serde_json::Number::from_f64(n).map(Value::Number))
            .unwrap_or_else(|| Value::String(value.to_string())),
        FieldType::Boolean => Value::Bool(value == "true" || value == "1"),
        _ => Value::String(value.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Helper to build a simple `FieldMeta`.
    fn field(name: &'static str, ft: FieldType) -> FieldMeta {
        FieldMeta {
            name,
            cli_name: to_kebab(name),
            description: None,
            field_type: ft,
            required: false,
            default: None,
            alias: None,
            deprecated: false,
            env_name: None,
        }
    }

    fn argv(tokens: &[&str]) -> Vec<String> {
        tokens.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn test_long_option_with_value() {
        let opts = ParseOptions {
            args_fields: vec![],
            options_fields: vec![field("output", FieldType::String)],
            aliases: HashMap::new(),
            defaults: None,
        };
        let result = parse(&argv(&["--output", "json"]), &opts).unwrap();
        assert_eq!(result.options["output"], Value::String("json".into()));
    }

    #[test]
    fn test_long_option_equals() {
        let opts = ParseOptions {
            args_fields: vec![],
            options_fields: vec![field("output", FieldType::String)],
            aliases: HashMap::new(),
            defaults: None,
        };
        let result = parse(&argv(&["--output=json"]), &opts).unwrap();
        assert_eq!(result.options["output"], Value::String("json".into()));
    }

    #[test]
    fn test_no_flag_negation() {
        let opts = ParseOptions {
            args_fields: vec![],
            options_fields: vec![field("verbose", FieldType::Boolean)],
            aliases: HashMap::new(),
            defaults: None,
        };
        let result = parse(&argv(&["--no-verbose"]), &opts).unwrap();
        assert_eq!(result.options["verbose"], Value::Bool(false));
    }

    #[test]
    fn test_boolean_flag_without_value() {
        let opts = ParseOptions {
            args_fields: vec![],
            options_fields: vec![field("verbose", FieldType::Boolean)],
            aliases: HashMap::new(),
            defaults: None,
        };
        let result = parse(&argv(&["--verbose"]), &opts).unwrap();
        assert_eq!(result.options["verbose"], Value::Bool(true));
    }

    #[test]
    fn test_count_flag() {
        let opts = ParseOptions {
            args_fields: vec![],
            options_fields: vec![{
                let mut f = field("verbose", FieldType::Count);
                f.alias = Some('v');
                f
            }],
            aliases: HashMap::new(),
            defaults: None,
        };
        let result = parse(&argv(&["-vvv"]), &opts).unwrap();
        assert_eq!(result.options["verbose"], Value::Number(3.into()));
    }

    #[test]
    fn test_short_alias() {
        let opts = ParseOptions {
            args_fields: vec![],
            options_fields: vec![{
                let mut f = field("output", FieldType::String);
                f.alias = Some('o');
                f
            }],
            aliases: HashMap::new(),
            defaults: None,
        };
        let result = parse(&argv(&["-o", "json"]), &opts).unwrap();
        assert_eq!(result.options["output"], Value::String("json".into()));
    }

    #[test]
    fn test_stacked_boolean_aliases() {
        let opts = ParseOptions {
            args_fields: vec![],
            options_fields: vec![
                {
                    let mut f = field("all", FieldType::Boolean);
                    f.alias = Some('a');
                    f
                },
                {
                    let mut f = field("long_list", FieldType::Boolean);
                    f.alias = Some('l');
                    f
                },
            ],
            aliases: HashMap::new(),
            defaults: None,
        };
        let result = parse(&argv(&["-al"]), &opts).unwrap();
        assert_eq!(result.options["all"], Value::Bool(true));
        assert_eq!(result.options["long_list"], Value::Bool(true));
    }

    #[test]
    fn test_stacked_non_boolean_last() {
        let opts = ParseOptions {
            args_fields: vec![],
            options_fields: vec![
                {
                    let mut f = field("verbose", FieldType::Boolean);
                    f.alias = Some('v');
                    f
                },
                {
                    let mut f = field("output", FieldType::String);
                    f.alias = Some('o');
                    f
                },
            ],
            aliases: HashMap::new(),
            defaults: None,
        };
        let result = parse(&argv(&["-vo", "json"]), &opts).unwrap();
        assert_eq!(result.options["verbose"], Value::Bool(true));
        assert_eq!(result.options["output"], Value::String("json".into()));
    }

    #[test]
    fn test_stacked_non_boolean_not_last_errors() {
        let opts = ParseOptions {
            args_fields: vec![],
            options_fields: vec![
                {
                    let mut f = field("output", FieldType::String);
                    f.alias = Some('o');
                    f
                },
                {
                    let mut f = field("verbose", FieldType::Boolean);
                    f.alias = Some('v');
                    f
                },
            ],
            aliases: HashMap::new(),
            defaults: None,
        };
        let err = parse(&argv(&["-ov", "json"]), &opts).unwrap_err();
        assert!(err.message.contains("Non-boolean flag"));
    }

    #[test]
    fn test_array_option_collects() {
        let opts = ParseOptions {
            args_fields: vec![],
            options_fields: vec![field("tag", FieldType::Array(Box::new(FieldType::String)))],
            aliases: HashMap::new(),
            defaults: None,
        };
        let result = parse(&argv(&["--tag", "a", "--tag", "b"]), &opts).unwrap();
        assert_eq!(
            result.options["tag"],
            Value::Array(vec![Value::String("a".into()), Value::String("b".into())])
        );
    }

    #[test]
    fn test_positional_args() {
        let opts = ParseOptions {
            args_fields: vec![
                field("source", FieldType::String),
                field("dest", FieldType::String),
            ],
            options_fields: vec![],
            aliases: HashMap::new(),
            defaults: None,
        };
        let result = parse(&argv(&["foo", "bar"]), &opts).unwrap();
        assert_eq!(result.args["source"], Value::String("foo".into()));
        assert_eq!(result.args["dest"], Value::String("bar".into()));
    }

    #[test]
    fn test_number_coercion() {
        let opts = ParseOptions {
            args_fields: vec![],
            options_fields: vec![field("port", FieldType::Number)],
            aliases: HashMap::new(),
            defaults: None,
        };
        let result = parse(&argv(&["--port", "8080"]), &opts).unwrap();
        assert_eq!(result.options["port"], Value::from(8080));
    }

    #[test]
    fn test_boolean_coercion() {
        let opts = ParseOptions {
            args_fields: vec![],
            options_fields: vec![field("dry_run", FieldType::Boolean)],
            aliases: HashMap::new(),
            defaults: None,
        };
        let result = parse(&argv(&["--dry-run=true"]), &opts).unwrap();
        assert_eq!(result.options["dry_run"], Value::Bool(true));
    }

    #[test]
    fn test_defaults_merged() {
        let mut defaults = BTreeMap::new();
        defaults.insert("output".to_string(), Value::String("toon".into()));
        defaults.insert("verbose".to_string(), Value::Bool(false));

        let opts = ParseOptions {
            args_fields: vec![],
            options_fields: vec![
                field("output", FieldType::String),
                field("verbose", FieldType::Boolean),
            ],
            aliases: HashMap::new(),
            defaults: Some(defaults),
        };
        // argv overrides output but not verbose
        let result = parse(&argv(&["--output", "json"]), &opts).unwrap();
        assert_eq!(result.options["output"], Value::String("json".into()));
        assert_eq!(result.options["verbose"], Value::Bool(false));
    }

    #[test]
    fn test_unknown_flag_errors() {
        let opts = ParseOptions {
            args_fields: vec![],
            options_fields: vec![],
            aliases: HashMap::new(),
            defaults: None,
        };
        let err = parse(&argv(&["--unknown"]), &opts).unwrap_err();
        assert!(err.message.contains("Unknown flag"));
    }

    #[test]
    fn test_missing_value_errors() {
        let opts = ParseOptions {
            args_fields: vec![],
            options_fields: vec![field("output", FieldType::String)],
            aliases: HashMap::new(),
            defaults: None,
        };
        let err = parse(&argv(&["--output"]), &opts).unwrap_err();
        assert!(err.message.contains("Missing value"));
    }

    #[test]
    fn test_kebab_to_snake_normalization() {
        let opts = ParseOptions {
            args_fields: vec![],
            options_fields: vec![field("dry_run", FieldType::Boolean)],
            aliases: HashMap::new(),
            defaults: None,
        };
        let result = parse(&argv(&["--dry-run"]), &opts).unwrap();
        assert_eq!(result.options["dry_run"], Value::Bool(true));
    }

    #[test]
    fn test_parse_env_basic() {
        let fields = vec![
            {
                let mut f = field("api_key", FieldType::String);
                f.env_name = Some("API_KEY");
                f
            },
            {
                let mut f = field("port", FieldType::Number);
                f.env_name = Some("PORT");
                f
            },
            {
                let mut f = field("debug", FieldType::Boolean);
                f.env_name = Some("DEBUG");
                f
            },
        ];

        let mut source = HashMap::new();
        source.insert("API_KEY".to_string(), "secret".to_string());
        source.insert("PORT".to_string(), "3000".to_string());
        source.insert("DEBUG".to_string(), "true".to_string());

        let result = parse_env(&fields, &source);
        assert_eq!(result["api_key"], Value::String("secret".into()));
        assert_eq!(
            result["port"],
            Value::Number(serde_json::Number::from_f64(3000.0).unwrap())
        );
        assert_eq!(result["debug"], Value::Bool(true));
    }

    #[test]
    fn test_parse_env_missing_vars() {
        let fields = vec![{
            let mut f = field("api_key", FieldType::String);
            f.env_name = Some("API_KEY");
            f
        }];

        let source = HashMap::new();
        let result = parse_env(&fields, &source);
        assert!(result.is_empty());
    }

    #[test]
    fn test_mixed_positional_and_options() {
        let opts = ParseOptions {
            args_fields: vec![field("command", FieldType::String)],
            options_fields: vec![
                field("verbose", FieldType::Boolean),
                field("output", FieldType::String),
            ],
            aliases: HashMap::new(),
            defaults: None,
        };
        let result = parse(&argv(&["deploy", "--verbose", "--output", "json"]), &opts).unwrap();
        assert_eq!(result.args["command"], Value::String("deploy".into()));
        assert_eq!(result.options["verbose"], Value::Bool(true));
        assert_eq!(result.options["output"], Value::String("json".into()));
    }

    #[test]
    fn test_positional_number_coercion() {
        let opts = ParseOptions {
            args_fields: vec![field("count", FieldType::Number)],
            options_fields: vec![],
            aliases: HashMap::new(),
            defaults: None,
        };
        let result = parse(&argv(&["42"]), &opts).unwrap();
        assert_eq!(result.args["count"], Value::from(42));
    }

    #[test]
    fn test_field_level_default() {
        let opts = ParseOptions {
            args_fields: vec![],
            options_fields: vec![{
                let mut f = field("format", FieldType::String);
                f.default = Some(Value::String("toon".into()));
                f
            }],
            aliases: HashMap::new(),
            defaults: None,
        };
        let result = parse(&argv(&[]), &opts).unwrap();
        assert_eq!(result.options["format"], Value::String("toon".into()));
    }
}
