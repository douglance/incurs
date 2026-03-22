//! Output formatting for the incur framework.
//!
//! Ported from `src/Formatter.ts`. Serializes a [`serde_json::Value`] to a
//! string in the requested [`Format`].

use serde_json::Value;

use crate::output::Format;

/// Serializes a value to the specified format. Defaults to TOON.
pub fn format(value: &Value, fmt: Format) -> String {
    match fmt {
        Format::Json => format_json(value),
        Format::Jsonl => format_jsonl(value),
        Format::Yaml => format_yaml(value),
        Format::Markdown => format_markdown(value, &[]),
        Format::Toon => format_toon(value),
        Format::Table => format_table(value),
        Format::Csv => format_csv(value),
    }
}

// ---------------------------------------------------------------------------
// JSON
// ---------------------------------------------------------------------------

fn format_json(value: &Value) -> String {
    serde_json::to_string_pretty(value).unwrap_or_default()
}

// ---------------------------------------------------------------------------
// JSONL
// ---------------------------------------------------------------------------

fn format_jsonl(value: &Value) -> String {
    match value {
        Value::Array(arr) => arr
            .iter()
            .map(|v| serde_json::to_string(v).unwrap_or_default())
            .collect::<Vec<_>>()
            .join("\n"),
        _ => serde_json::to_string(value).unwrap_or_default(),
    }
}

// ---------------------------------------------------------------------------
// YAML (behind `yaml` feature)
// ---------------------------------------------------------------------------

#[cfg(feature = "yaml")]
fn format_yaml(value: &Value) -> String {
    serde_yaml_ng::to_string(value).unwrap_or_default()
}

#[cfg(not(feature = "yaml"))]
fn format_yaml(value: &Value) -> String {
    // Fallback to JSON when yaml feature is not enabled.
    format_json(value)
}

// ---------------------------------------------------------------------------
// TOON (behind `toon` feature)
// ---------------------------------------------------------------------------

#[cfg(feature = "toon")]
fn format_toon(value: &Value) -> String {
    if is_scalar(value) {
        scalar_to_string(value)
    } else {
        let options = toon_format::EncodeOptions::default();
        toon_format::encode(value, &options).unwrap_or_else(|_| format_json(value))
    }
}

#[cfg(not(feature = "toon"))]
fn format_toon(value: &Value) -> String {
    if is_scalar(value) {
        scalar_to_string(value)
    } else {
        format_json(value)
    }
}

// ---------------------------------------------------------------------------
// Markdown
// ---------------------------------------------------------------------------

/// Whether a JSON value is a scalar (string, number, bool, null).
fn is_scalar(value: &Value) -> bool {
    matches!(
        value,
        Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_)
    )
}

/// Converts a scalar value to its display string.
fn scalar_to_string(value: &Value) -> String {
    match value {
        Value::Null => "null".to_string(),
        Value::Bool(b) => b.to_string(),
        Value::Number(n) => n.to_string(),
        Value::String(s) => s.clone(),
        _ => String::new(),
    }
}

/// Whether all values in an object are scalars.
fn is_flat(obj: &serde_json::Map<String, Value>) -> bool {
    obj.values().all(is_scalar)
}

/// Whether a value is a non-empty array of plain objects.
fn is_array_of_objects(value: &Value) -> bool {
    match value {
        Value::Array(arr) => {
            !arr.is_empty() && arr.iter().all(|v| matches!(v, Value::Object(_)))
        }
        _ => false,
    }
}

/// Renders an aligned markdown table from headers and rows.
fn table(headers: &[String], rows: &[Vec<String>]) -> String {
    // Compute column widths.
    let widths: Vec<usize> = headers
        .iter()
        .enumerate()
        .map(|(i, h)| {
            let max_row = rows.iter().map(|r| r.get(i).map_or(0, |c| c.len())).max().unwrap_or(0);
            h.len().max(max_row)
        })
        .collect();

    let pad = |s: &str, i: usize| -> String {
        let w = widths[i];
        format!("{:<width$}", s, width = w)
    };

    let header_row = format!(
        "| {} |",
        headers
            .iter()
            .enumerate()
            .map(|(i, h)| pad(h, i))
            .collect::<Vec<_>>()
            .join(" | ")
    );

    let sep = format!(
        "|{}|",
        widths
            .iter()
            .map(|w| format!("{:-<width$}", "", width = w + 2))
            .collect::<Vec<_>>()
            .join("|")
    );

    let body: Vec<String> = rows
        .iter()
        .map(|r| {
            let cells: Vec<String> = headers
                .iter()
                .enumerate()
                .map(|(i, _)| pad(r.get(i).map_or("", |s| s.as_str()), i))
                .collect();
            format!("| {} |", cells.join(" | "))
        })
        .collect();

    format!("{}\n{}\n{}", header_row, sep, body.join("\n"))
}

/// Renders a key-value table from a flat object.
fn kv_table(obj: &serde_json::Map<String, Value>) -> String {
    let headers = vec!["Key".to_string(), "Value".to_string()];
    let rows: Vec<Vec<String>> = obj
        .iter()
        .map(|(k, v)| vec![k.clone(), scalar_to_string(v)])
        .collect();
    table(&headers, &rows)
}

/// Renders a columnar table from an array of objects.
fn columnar_table(items: &[Value]) -> String {
    // Collect all unique keys preserving insertion order.
    let mut keys: Vec<String> = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for item in items {
        if let Value::Object(map) = item {
            for key in map.keys() {
                if seen.insert(key.clone()) {
                    keys.push(key.clone());
                }
            }
        }
    }

    let rows: Vec<Vec<String>> = items
        .iter()
        .map(|item| {
            keys.iter()
                .map(|k| {
                    item.as_object()
                        .and_then(|m| m.get(k))
                        .map(scalar_to_string)
                        .unwrap_or_default()
                })
                .collect()
        })
        .collect();

    table(&keys, &rows)
}

/// Formats a value as Markdown, recursing into nested objects.
fn format_markdown(value: &Value, path: &[String]) -> String {
    if is_scalar(value) {
        if path.is_empty() {
            return scalar_to_string(value);
        }
        return format!("## {}\n\n{}", path.join("."), scalar_to_string(value));
    }

    if let Value::Array(arr) = value {
        if is_array_of_objects(value) {
            let tbl = columnar_table(arr);
            if path.is_empty() {
                return tbl;
            }
            return format!("## {}\n\n{}", path.join("."), tbl);
        }
        // Fallback: stringify the array.
        let s = arr.iter().map(scalar_to_string).collect::<Vec<_>>().join(", ");
        return format_markdown(&Value::String(s), path);
    }

    if let Value::Object(obj) = value {
        // Single flat object at root — no headings needed.
        if path.is_empty() && is_flat(obj) {
            return kv_table(obj);
        }

        let entries: Vec<(&String, &Value)> = obj.iter().collect();

        let needs_headings =
            !path.is_empty() || entries.len() > 1 || entries.iter().any(|(_, v)| !is_scalar(v));

        if needs_headings {
            let sections: Vec<String> = entries
                .iter()
                .map(|(key, val)| {
                    let mut child_path = path.to_vec();
                    child_path.push((*key).clone());

                    if is_scalar(val) {
                        format!("## {}\n\n{}", child_path.join("."), scalar_to_string(val))
                    } else if is_array_of_objects(val) {
                        let arr = val.as_array().unwrap();
                        format!("## {}\n\n{}", child_path.join("."), columnar_table(arr))
                    } else if let Value::Object(nested) = val {
                        if is_flat(nested) {
                            format!("## {}\n\n{}", child_path.join("."), kv_table(nested))
                        } else {
                            format_markdown(val, &child_path)
                        }
                    } else {
                        format!("## {}\n\n{}", child_path.join("."), scalar_to_string(val))
                    }
                })
                .collect();
            return sections.join("\n\n");
        }

        return kv_table(obj);
    }

    String::new()
}

// ---------------------------------------------------------------------------
// Table (aligned ASCII table)
// ---------------------------------------------------------------------------

/// Renders an aligned ASCII table with box-drawing-style separators.
fn format_table(value: &Value) -> String {
    if is_scalar(value) {
        return scalar_to_string(value);
    }

    if let Value::Array(arr) = value {
        if arr.is_empty() {
            return "(empty)".to_string();
        }
        if is_array_of_objects(value) {
            return ascii_table_from_array(arr);
        }
        // Array of scalars — one per line
        return arr.iter().map(scalar_to_string).collect::<Vec<_>>().join("\n");
    }

    if let Value::Object(obj) = value {
        return ascii_kv_table(obj);
    }

    String::new()
}

/// Renders an array of objects as an aligned ASCII table.
fn ascii_table_from_array(items: &[Value]) -> String {
    // Collect all unique keys preserving insertion order.
    let mut keys: Vec<String> = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for item in items {
        if let Value::Object(map) = item {
            for key in map.keys() {
                if seen.insert(key.clone()) {
                    keys.push(key.clone());
                }
            }
        }
    }

    let rows: Vec<Vec<String>> = items
        .iter()
        .map(|item| {
            keys.iter()
                .map(|k| {
                    item.as_object()
                        .and_then(|m| m.get(k))
                        .map(|v| value_to_cell(v))
                        .unwrap_or_default()
                })
                .collect()
        })
        .collect();

    ascii_table(&keys, &rows)
}

/// Renders a key-value ASCII table from an object.
fn ascii_kv_table(obj: &serde_json::Map<String, Value>) -> String {
    let headers = vec!["Key".to_string(), "Value".to_string()];
    let rows: Vec<Vec<String>> = obj
        .iter()
        .map(|(k, v)| vec![k.clone(), value_to_cell(v)])
        .collect();
    ascii_table(&headers, &rows)
}

/// Renders an aligned ASCII table with borders.
fn ascii_table(headers: &[String], rows: &[Vec<String>]) -> String {
    let widths: Vec<usize> = headers
        .iter()
        .enumerate()
        .map(|(i, h)| {
            let max_row = rows.iter().map(|r| r.get(i).map_or(0, |c| c.len())).max().unwrap_or(0);
            h.len().max(max_row)
        })
        .collect();

    let sep_line = format!(
        "+-{}-+",
        widths.iter().map(|w| "-".repeat(*w)).collect::<Vec<_>>().join("-+-")
    );

    let header_row = format!(
        "| {} |",
        headers
            .iter()
            .enumerate()
            .map(|(i, h)| format!("{:<width$}", h, width = widths[i]))
            .collect::<Vec<_>>()
            .join(" | ")
    );

    let data_rows: Vec<String> = rows
        .iter()
        .map(|r| {
            let cells: Vec<String> = headers
                .iter()
                .enumerate()
                .map(|(i, _)| format!("{:<width$}", r.get(i).map_or("", |s| s.as_str()), width = widths[i]))
                .collect();
            format!("| {} |", cells.join(" | "))
        })
        .collect();

    let mut lines = Vec::new();
    lines.push(sep_line.clone());
    lines.push(header_row);
    lines.push(sep_line.clone());
    for row in &data_rows {
        lines.push(row.clone());
    }
    lines.push(sep_line);

    lines.join("\n")
}

/// Converts a Value to a display string for table cells.
fn value_to_cell(value: &Value) -> String {
    match value {
        Value::Null => "".to_string(),
        Value::Bool(b) => b.to_string(),
        Value::Number(n) => n.to_string(),
        Value::String(s) => s.clone(),
        Value::Array(arr) => {
            let items: Vec<String> = arr.iter().map(|v| value_to_cell(v)).collect();
            items.join(", ")
        }
        Value::Object(_) => serde_json::to_string(value).unwrap_or_default(),
    }
}

// ---------------------------------------------------------------------------
// CSV
// ---------------------------------------------------------------------------

/// Renders CSV output from a JSON value.
fn format_csv(value: &Value) -> String {
    if is_scalar(value) {
        return csv_escape(&scalar_to_string(value));
    }

    if let Value::Array(arr) = value {
        if arr.is_empty() {
            return String::new();
        }
        if is_array_of_objects(value) {
            return csv_from_array(arr);
        }
        // Array of scalars — one per line
        return arr.iter().map(|v| csv_escape(&scalar_to_string(v))).collect::<Vec<_>>().join("\n");
    }

    if let Value::Object(obj) = value {
        // Single object — header + one data row
        let keys: Vec<&String> = obj.keys().collect();
        let header = keys.iter().map(|k| csv_escape(k)).collect::<Vec<_>>().join(",");
        let row = keys
            .iter()
            .map(|k| csv_escape(&value_to_cell(obj.get(*k).unwrap_or(&Value::Null))))
            .collect::<Vec<_>>()
            .join(",");
        return format!("{}\n{}", header, row);
    }

    String::new()
}

/// Renders an array of objects as CSV.
fn csv_from_array(items: &[Value]) -> String {
    // Collect all unique keys preserving insertion order.
    let mut keys: Vec<String> = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for item in items {
        if let Value::Object(map) = item {
            for key in map.keys() {
                if seen.insert(key.clone()) {
                    keys.push(key.clone());
                }
            }
        }
    }

    let header = keys.iter().map(|k| csv_escape(k)).collect::<Vec<_>>().join(",");

    let rows: Vec<String> = items
        .iter()
        .map(|item| {
            keys.iter()
                .map(|k| {
                    let val = item
                        .as_object()
                        .and_then(|m| m.get(k))
                        .map(|v| value_to_cell(v))
                        .unwrap_or_default();
                    csv_escape(&val)
                })
                .collect::<Vec<_>>()
                .join(",")
        })
        .collect();

    let mut lines = vec![header];
    lines.extend(rows);
    lines.join("\n")
}

/// Escapes a string for CSV output, quoting if necessary.
fn csv_escape(s: &str) -> String {
    if s.contains(',') || s.contains('"') || s.contains('\n') || s.contains('\r') {
        format!("\"{}\"", s.replace('"', "\"\""))
    } else {
        s.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_format_json() {
        let val = json!({"name": "alice", "age": 30});
        let result = format(&val, Format::Json);
        assert!(result.contains("\"name\": \"alice\""));
        assert!(result.contains("\"age\": 30"));
    }

    #[test]
    fn test_format_jsonl_array() {
        let val = json!([{"a":1},{"a":2}]);
        let result = format(&val, Format::Jsonl);
        let lines: Vec<&str> = result.lines().collect();
        assert_eq!(lines.len(), 2);
        assert!(lines[0].contains("\"a\":1"));
        assert!(lines[1].contains("\"a\":2"));
    }

    #[test]
    fn test_format_jsonl_scalar() {
        let val = json!(42);
        let result = format(&val, Format::Jsonl);
        assert_eq!(result, "42");
    }

    #[test]
    fn test_markdown_scalar_at_root() {
        let val = json!("hello");
        assert_eq!(format(&val, Format::Markdown), "hello");
    }

    #[test]
    fn test_markdown_flat_object() {
        let val = json!({"name": "alice", "age": 30});
        let result = format(&val, Format::Markdown);
        assert!(result.contains("| Key"));
        assert!(result.contains("| Value"));
        assert!(result.contains("name"));
        assert!(result.contains("alice"));
    }

    #[test]
    fn test_markdown_array_of_objects() {
        let val = json!([
            {"name": "alice", "age": 30},
            {"name": "bob", "age": 25}
        ]);
        let result = format(&val, Format::Markdown);
        assert!(result.contains("| name"));
        assert!(result.contains("| age"));
        assert!(result.contains("alice"));
        assert!(result.contains("bob"));
    }

    #[test]
    fn test_markdown_nested_objects() {
        let val = json!({
            "server": {
                "host": "localhost",
                "port": 8080
            }
        });
        let result = format(&val, Format::Markdown);
        assert!(result.contains("## server"));
    }

    #[test]
    fn test_markdown_table_alignment() {
        let val = json!([
            {"id": 1, "name": "alice"},
            {"id": 2, "name": "bob"}
        ]);
        let result = format(&val, Format::Markdown);
        // Check separator line exists.
        assert!(result.contains("|--"));
        // Check all rows present.
        let lines: Vec<&str> = result.lines().collect();
        assert!(lines.len() >= 4); // header, sep, 2 data rows
    }

    #[test]
    fn test_toon_scalar() {
        let val = json!(42);
        let result = format(&val, Format::Toon);
        assert_eq!(result, "42");
    }

    #[test]
    fn test_toon_string() {
        let val = json!("hello world");
        let result = format(&val, Format::Toon);
        assert_eq!(result, "hello world");
    }

    #[test]
    fn test_format_null() {
        let val = json!(null);
        assert_eq!(format(&val, Format::Toon), "null");
    }

    #[test]
    fn test_markdown_deeply_nested() {
        let val = json!({
            "a": {
                "b": {
                    "c": "deep"
                }
            }
        });
        let result = format(&val, Format::Markdown);
        // The inner {"c": "deep"} is flat, so it renders as a kv table under ## a.b
        assert!(result.contains("## a.b"));
        assert!(result.contains("deep"));
    }

    #[test]
    fn test_markdown_mixed_nested_and_flat() {
        let val = json!({
            "status": "ok",
            "data": {"x": 1, "y": 2}
        });
        let result = format(&val, Format::Markdown);
        assert!(result.contains("## status"));
        assert!(result.contains("## data"));
    }

    #[test]
    fn test_markdown_array_of_objects_nested_in_object() {
        let val = json!({
            "users": [
                {"name": "alice"},
                {"name": "bob"}
            ]
        });
        let result = format(&val, Format::Markdown);
        assert!(result.contains("## users"));
        assert!(result.contains("alice"));
        assert!(result.contains("bob"));
    }

    // Table format tests

    #[test]
    fn test_table_array_of_objects() {
        let val = json!([
            {"id": 1, "name": "alice"},
            {"id": 2, "name": "bob"}
        ]);
        let result = format(&val, Format::Table);
        assert!(result.contains("| id"));
        assert!(result.contains("| name"));
        assert!(result.contains("alice"));
        assert!(result.contains("bob"));
        assert!(result.contains("+--")); // box border
    }

    #[test]
    fn test_table_single_object() {
        let val = json!({"host": "localhost", "port": 8080});
        let result = format(&val, Format::Table);
        assert!(result.contains("Key"));
        assert!(result.contains("Value"));
        assert!(result.contains("host"));
        assert!(result.contains("localhost"));
    }

    #[test]
    fn test_table_scalar() {
        assert_eq!(format(&json!(42), Format::Table), "42");
        assert_eq!(format(&json!("hello"), Format::Table), "hello");
    }

    #[test]
    fn test_table_empty_array() {
        assert_eq!(format(&json!([]), Format::Table), "(empty)");
    }

    // CSV format tests

    #[test]
    fn test_csv_array_of_objects() {
        let val = json!([
            {"id": 1, "name": "alice"},
            {"id": 2, "name": "bob"}
        ]);
        let result = format(&val, Format::Csv);
        let lines: Vec<&str> = result.lines().collect();
        assert_eq!(lines[0], "id,name");
        assert_eq!(lines[1], "1,alice");
        assert_eq!(lines[2], "2,bob");
    }

    #[test]
    fn test_csv_single_object() {
        let val = json!({"name": "alice", "age": 30});
        let result = format(&val, Format::Csv);
        let lines: Vec<&str> = result.lines().collect();
        assert_eq!(lines.len(), 2); // header + 1 row
        assert!(lines[0].contains("name"));
        assert!(lines[1].contains("alice"));
    }

    #[test]
    fn test_csv_quoting() {
        let val = json!([{"msg": "hello, world"}, {"msg": "say \"hi\""}]);
        let result = format(&val, Format::Csv);
        let lines: Vec<&str> = result.lines().collect();
        assert_eq!(lines[0], "msg");
        assert_eq!(lines[1], "\"hello, world\"");
        assert_eq!(lines[2], "\"say \"\"hi\"\"\"");
    }

    #[test]
    fn test_csv_scalar() {
        assert_eq!(format(&json!(42), Format::Csv), "42");
    }
}
