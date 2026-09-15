//! Presentation model for a structured tool result.
//!
//! A [`ToolCallOutcome`] carries a JSON value shaped by the command's output
//! schema. Showing that value verbatim asks a person to read JSON. This module
//! flattens it into labelled rows the workbench can lay out directly, while
//! keeping the original value available for anyone who wants it.
//!
//! Result content is data, never instruction. Nothing here interprets a string
//! as a directive; it only decides how to display one.
//!
//! [`ToolCallOutcome`]: incurs::tool::ToolCallOutcome

use serde_json::Value;

use crate::text::humanize;

/// One line of a rendered result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DisplayRow {
    /// Nesting depth, used for indentation.
    pub depth: usize,
    /// The label shown at the start of the line.
    pub label: String,
    /// The formatted value, or `None` when the row only introduces children.
    pub value: Option<String>,
}

impl DisplayRow {
    /// Returns whether this row introduces nested rows rather than a value.
    pub fn is_heading(&self) -> bool {
        self.value.is_none()
    }
}

/// Flattens a structured result into labelled display rows.
///
/// A top-level object becomes one row per property. Any other value becomes a
/// single unlabelled row, so a command that returns a bare string or list still
/// renders as something readable.
pub fn rows_for(value: &Value) -> Vec<DisplayRow> {
    let mut rows = Vec::new();
    match value {
        Value::Object(fields) if !fields.is_empty() => {
            for (key, child) in fields {
                push_rows(&humanize(key), child, 0, &mut rows);
            }
        }
        Value::Null => {}
        other => push_rows("Result", other, 0, &mut rows),
    }
    rows
}

/// Appends the rows for one labelled value at the given depth.
fn push_rows(label: &str, value: &Value, depth: usize, rows: &mut Vec<DisplayRow>) {
    match value {
        Value::Object(fields) if fields.is_empty() => rows.push(DisplayRow {
            depth,
            label: label.to_string(),
            value: Some("Nothing".to_string()),
        }),
        Value::Object(fields) => {
            rows.push(DisplayRow {
                depth,
                label: label.to_string(),
                value: None,
            });
            for (key, child) in fields {
                push_rows(&humanize(key), child, depth + 1, rows);
            }
        }
        Value::Array(items) if items.is_empty() => rows.push(DisplayRow {
            depth,
            label: label.to_string(),
            value: Some("None".to_string()),
        }),
        Value::Array(items) if items.iter().all(is_scalar) => rows.push(DisplayRow {
            depth,
            label: label.to_string(),
            value: Some(
                items
                    .iter()
                    .map(format_scalar)
                    .collect::<Vec<_>>()
                    .join(", "),
            ),
        }),
        Value::Array(items) => {
            rows.push(DisplayRow {
                depth,
                label: label.to_string(),
                value: None,
            });
            for (index, item) in items.iter().enumerate() {
                push_rows(&format!("{}", index + 1), item, depth + 1, rows);
            }
        }
        scalar => rows.push(DisplayRow {
            depth,
            label: label.to_string(),
            value: Some(format_scalar(scalar)),
        }),
    }
}

/// Returns whether a value renders as a single word or phrase.
fn is_scalar(value: &Value) -> bool {
    !matches!(value, Value::Object(_) | Value::Array(_))
}

/// Formats a scalar for a reader rather than a parser.
fn format_scalar(value: &Value) -> String {
    match value {
        Value::Null => "—".to_string(),
        Value::Bool(true) => "Yes".to_string(),
        Value::Bool(false) => "No".to_string(),
        Value::String(text) => text.clone(),
        Value::Number(number) => number.to_string(),
        other => other.to_string(),
    }
}

/// Renders a value as indented JSON for the raw view.
pub fn raw_json(value: &Value) -> String {
    serde_json::to_string_pretty(value).unwrap_or_else(|_| value.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn object_properties_become_labelled_rows() {
        let rows = rows_for(&json!({"id": 42, "title": "Buy milk"}));

        assert_eq!(
            rows,
            vec![
                DisplayRow {
                    depth: 0,
                    label: "Id".into(),
                    value: Some("42".into())
                },
                DisplayRow {
                    depth: 0,
                    label: "Title".into(),
                    value: Some("Buy milk".into())
                },
            ]
        );
    }

    /// Returns the value of the row carrying `label`.
    fn value_of<'a>(rows: &'a [DisplayRow], label: &str) -> Option<&'a str> {
        rows.iter()
            .find(|row| row.label == label)
            .and_then(|row| row.value.as_deref())
    }

    #[test]
    fn booleans_read_as_words() {
        let rows = rows_for(&json!({"done": true, "archived": false}));

        assert_eq!(value_of(&rows, "Done"), Some("Yes"));
        assert_eq!(value_of(&rows, "Archived"), Some("No"));
    }

    #[test]
    fn nested_objects_indent_under_a_heading() {
        let rows = rows_for(&json!({"owner": {"name": "Ada"}}));

        assert!(rows[0].is_heading());
        assert_eq!(rows[0].label, "Owner");
        assert_eq!(rows[1].depth, 1);
        assert_eq!(rows[1].label, "Name");
        assert_eq!(rows[1].value.as_deref(), Some("Ada"));
    }

    #[test]
    fn a_list_of_scalars_stays_on_one_line() {
        let rows = rows_for(&json!({"tags": ["a", "b"]}));

        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].value.as_deref(), Some("a, b"));
    }

    #[test]
    fn a_list_of_objects_becomes_numbered_groups() {
        let rows = rows_for(&json!({"todos": [{"id": 1}, {"id": 2}]}));

        assert_eq!(rows[0].label, "Todos");
        assert!(rows[0].is_heading());
        assert_eq!(rows[1].label, "1");
        assert!(rows[1].is_heading());
        assert_eq!(rows[2].label, "Id");
        assert_eq!(rows[2].depth, 2);
        assert_eq!(rows[2].value.as_deref(), Some("1"));
    }

    #[test]
    fn empty_collections_say_so_rather_than_showing_brackets() {
        let rows = rows_for(&json!({"todos": [], "meta": {}}));

        assert_eq!(value_of(&rows, "Meta"), Some("Nothing"));
        assert_eq!(value_of(&rows, "Todos"), Some("None"));
    }

    #[test]
    fn a_bare_value_still_renders() {
        let rows = rows_for(&json!("hello"));

        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].value.as_deref(), Some("hello"));
    }

    #[test]
    fn a_null_result_renders_nothing() {
        assert!(rows_for(&Value::Null).is_empty());
    }

    #[test]
    fn null_properties_render_as_a_dash() {
        let rows = rows_for(&json!({"due": null}));

        assert_eq!(rows[0].value.as_deref(), Some("—"));
    }

    #[test]
    fn raw_json_is_indented() {
        assert_eq!(raw_json(&json!({"a": 1})), "{\n  \"a\": 1\n}");
    }
}
