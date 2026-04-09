//! Filter expressions for selecting and slicing output data.
//!
//! Ported from `src/Filter.ts`. Parses dot-separated key paths with optional
//! array slices and applies them to [`serde_json::Value`] trees.

use serde_json::Value;

/// A single segment in a filter path.
#[derive(Debug, Clone)]
pub enum Segment {
    /// A named key to descend into an object.
    Key(String),
    /// An array slice `[start, end)`.
    Slice { start: usize, end: usize },
}

/// A filter path is an ordered list of segments to traverse.
pub type FilterPath = Vec<Segment>;

/// Parses a filter expression string into structured filter paths.
///
/// Tokens are split on commas at the top level (commas inside `[...]` are part
/// of slice syntax). Each token is then split on `.` for key segments.
///
/// # Examples
///
/// ```
/// use incurs::filter::{parse, Segment};
///
/// let paths = parse("foo,bar.baz,items[0,3]");
/// assert_eq!(paths.len(), 3);
/// ```
pub fn parse(expression: &str) -> Vec<FilterPath> {
    let tokens = split_top_level_commas(expression);
    tokens.iter().map(|t| parse_token(t)).collect()
}

/// Applies parsed filter paths to data, returning a filtered copy.
///
/// Behavior:
/// - Single key selecting a scalar returns the scalar directly.
/// - Array inputs are mapped element-wise.
/// - Object inputs merge results from each path.
/// - Key segments descend into objects.
/// - Slice segments slice arrays.
pub fn apply(data: &Value, paths: &[FilterPath]) -> Value {
    if paths.is_empty() {
        return data.clone();
    }

    // Special case: single key selecting a scalar → return scalar directly.
    if paths.len() == 1 && paths[0].len() == 1 {
        if let Segment::Key(key) = &paths[0][0] {
            if let Value::Array(arr) = data {
                return Value::Array(arr.iter().map(|item| apply(item, paths)).collect());
            }
            if let Value::Object(obj) = data {
                if let Some(val) = obj.get(key) {
                    if is_scalar(val) {
                        return val.clone();
                    }
                    let mut result = serde_json::Map::new();
                    result.insert(key.clone(), val.clone());
                    return Value::Object(result);
                }
            }
            return Value::Null;
        }
    }

    if let Value::Array(arr) = data {
        return Value::Array(arr.iter().map(|item| apply(item, paths)).collect());
    }

    let mut result = serde_json::Map::new();
    for path in paths {
        merge(&mut result, data, path, 0);
    }
    Value::Object(result)
}

// ---------------------------------------------------------------------------
// Internals
// ---------------------------------------------------------------------------

/// Whether a value is a scalar (not object or array).
fn is_scalar(value: &Value) -> bool {
    matches!(
        value,
        Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_)
    )
}

/// Splits a string on commas, but ignores commas inside `[...]`.
fn split_top_level_commas(expression: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    let mut depth: i32 = 0;

    for ch in expression.chars() {
        match ch {
            '[' => {
                depth += 1;
                current.push(ch);
            }
            ']' => {
                depth -= 1;
                current.push(ch);
            }
            ',' if depth == 0 => {
                if !current.is_empty() {
                    tokens.push(current);
                }
                current = String::new();
            }
            _ => {
                current.push(ch);
            }
        }
    }
    if !current.is_empty() {
        tokens.push(current);
    }
    tokens
}

/// Parses a single filter token into a path of segments.
fn parse_token(token: &str) -> FilterPath {
    let mut path = Vec::new();
    let mut remaining = token;

    while !remaining.is_empty() {
        if let Some(bracket_idx) = remaining.find('[') {
            // Parse dot-separated keys before the bracket.
            let before = &remaining[..bracket_idx];
            for part in before.split('.') {
                if !part.is_empty() {
                    path.push(Segment::Key(part.to_string()));
                }
            }

            // Parse the slice [start,end].
            let close_bracket = remaining[bracket_idx..]
                .find(']')
                .map(|i| bracket_idx + i)
                .unwrap_or(remaining.len());
            let inner = &remaining[bracket_idx + 1..close_bracket];
            let parts: Vec<&str> = inner.split(',').collect();
            if parts.len() == 2 {
                let start = parts[0].parse::<usize>().unwrap_or(0);
                let end = parts[1].parse::<usize>().unwrap_or(0);
                path.push(Segment::Slice { start, end });
            } else if parts.len() == 1 {
                // Single index — treat as [n, n+1].
                let idx = parts[0].parse::<usize>().unwrap_or(0);
                path.push(Segment::Slice {
                    start: idx,
                    end: idx + 1,
                });
            }

            remaining = if close_bracket + 1 < remaining.len() {
                let rest = &remaining[close_bracket + 1..];
                if rest.starts_with('.') { &rest[1..] } else { rest }
            } else {
                ""
            };
        } else {
            // No more slices — split remaining by dots.
            for part in remaining.split('.') {
                if !part.is_empty() {
                    path.push(Segment::Key(part.to_string()));
                }
            }
            break;
        }
    }

    path
}

/// Recursively merges a single filter path into a target object.
fn merge(
    target: &mut serde_json::Map<String, Value>,
    data: &Value,
    segments: &[Segment],
    index: usize,
) {
    if index >= segments.len() {
        return;
    }

    let obj = match data {
        Value::Object(obj) => obj,
        _ => return,
    };

    match &segments[index] {
        Segment::Key(key) => {
            let val = match obj.get(key) {
                Some(v) => v,
                None => return,
            };

            // Last segment — copy the value.
            if index + 1 >= segments.len() {
                target.insert(key.clone(), val.clone());
                return;
            }

            // Peek at next segment.
            let next = &segments[index + 1];
            if let Segment::Slice { start, end } = next {
                // Next segment is a slice — apply it.
                if let Value::Array(arr) = val {
                    let sliced: Vec<Value> = arr
                        .iter()
                        .skip(*start)
                        .take(end.saturating_sub(*start))
                        .cloned()
                        .collect();

                    if index + 2 >= segments.len() {
                        target.insert(key.clone(), Value::Array(sliced));
                    } else {
                        let mapped: Vec<Value> = sliced
                            .iter()
                            .map(|item| {
                                let mut sub = serde_json::Map::new();
                                merge(&mut sub, item, segments, index + 2);
                                Value::Object(sub)
                            })
                            .collect();
                        target.insert(key.clone(), Value::Array(mapped));
                    }
                }
                return;
            }

            // Next segment is a key — recurse into nested object.
            if let Value::Object(_) = val {
                let nested = target
                    .entry(key.clone())
                    .or_insert_with(|| Value::Object(serde_json::Map::new()));
                if let Value::Object(nested_map) = nested {
                    merge(nested_map, val, segments, index + 1);
                }
            }
        }
        Segment::Slice { .. } => {
            // Slice at root level in merge — shouldn't happen since merge
            // always starts from object keys.
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_parse_single_key() {
        let paths = parse("foo");
        assert_eq!(paths.len(), 1);
        assert_eq!(paths[0].len(), 1);
        assert!(matches!(&paths[0][0], Segment::Key(k) if k == "foo"));
    }

    #[test]
    fn test_parse_multiple_keys() {
        let paths = parse("foo,bar,baz");
        assert_eq!(paths.len(), 3);
    }

    #[test]
    fn test_parse_dotted_path() {
        let paths = parse("foo.bar.baz");
        assert_eq!(paths.len(), 1);
        assert_eq!(paths[0].len(), 3);
        assert!(matches!(&paths[0][0], Segment::Key(k) if k == "foo"));
        assert!(matches!(&paths[0][1], Segment::Key(k) if k == "bar"));
        assert!(matches!(&paths[0][2], Segment::Key(k) if k == "baz"));
    }

    #[test]
    fn test_parse_with_slice() {
        let paths = parse("items[0,3]");
        assert_eq!(paths.len(), 1);
        assert_eq!(paths[0].len(), 2);
        assert!(matches!(&paths[0][0], Segment::Key(k) if k == "items"));
        assert!(matches!(&paths[0][1], Segment::Slice { start: 0, end: 3 }));
    }

    #[test]
    fn test_parse_complex_expression() {
        let paths = parse("foo,bar.baz,a[0,3]");
        assert_eq!(paths.len(), 3);

        // foo
        assert_eq!(paths[0].len(), 1);
        assert!(matches!(&paths[0][0], Segment::Key(k) if k == "foo"));

        // bar.baz
        assert_eq!(paths[1].len(), 2);
        assert!(matches!(&paths[1][0], Segment::Key(k) if k == "bar"));
        assert!(matches!(&paths[1][1], Segment::Key(k) if k == "baz"));

        // a[0,3]
        assert_eq!(paths[2].len(), 2);
        assert!(matches!(&paths[2][0], Segment::Key(k) if k == "a"));
        assert!(matches!(&paths[2][1], Segment::Slice { start: 0, end: 3 }));
    }

    #[test]
    fn test_parse_slice_then_key() {
        let paths = parse("items[0,2].name");
        assert_eq!(paths.len(), 1);
        assert_eq!(paths[0].len(), 3);
        assert!(matches!(&paths[0][0], Segment::Key(k) if k == "items"));
        assert!(matches!(&paths[0][1], Segment::Slice { start: 0, end: 2 }));
        assert!(matches!(&paths[0][2], Segment::Key(k) if k == "name"));
    }

    #[test]
    fn test_apply_empty_paths() {
        let data = json!({"a": 1, "b": 2});
        let result = apply(&data, &[]);
        assert_eq!(result, data);
    }

    #[test]
    fn test_apply_single_scalar_key() {
        let data = json!({"name": "alice", "age": 30});
        let paths = parse("name");
        let result = apply(&data, &paths);
        assert_eq!(result, json!("alice"));
    }

    #[test]
    fn test_apply_single_object_key() {
        let data = json!({"user": {"name": "alice"}, "count": 1});
        let paths = parse("user");
        let result = apply(&data, &paths);
        assert_eq!(result, json!({"user": {"name": "alice"}}));
    }

    #[test]
    fn test_apply_multiple_keys() {
        let data = json!({"a": 1, "b": 2, "c": 3});
        let paths = parse("a,c");
        let result = apply(&data, &paths);
        assert_eq!(result, json!({"a": 1, "c": 3}));
    }

    #[test]
    fn test_apply_nested_key() {
        let data = json!({"user": {"name": "alice", "age": 30}});
        let paths = parse("user.name");
        let result = apply(&data, &paths);
        assert_eq!(result, json!({"user": {"name": "alice"}}));
    }

    #[test]
    fn test_apply_array_slice() {
        let data = json!({"items": [1, 2, 3, 4, 5]});
        let paths = parse("items[0,3]");
        let result = apply(&data, &paths);
        assert_eq!(result, json!({"items": [1, 2, 3]}));
    }

    #[test]
    fn test_apply_slice_then_key() {
        let data = json!({
            "users": [
                {"name": "alice", "age": 30},
                {"name": "bob", "age": 25},
                {"name": "charlie", "age": 35}
            ]
        });
        let paths = parse("users[0,2].name");
        let result = apply(&data, &paths);
        assert_eq!(
            result,
            json!({"users": [{"name": "alice"}, {"name": "bob"}]})
        );
    }

    #[test]
    fn test_apply_to_array_data() {
        let data = json!([
            {"name": "alice", "age": 30},
            {"name": "bob", "age": 25}
        ]);
        let paths = parse("name");
        let result = apply(&data, &paths);
        assert_eq!(result, json!(["alice", "bob"]));
    }

    #[test]
    fn test_apply_missing_key() {
        let data = json!({"a": 1});
        let paths = parse("b");
        let result = apply(&data, &paths);
        assert_eq!(result, Value::Null);
    }

    #[test]
    fn test_apply_multiple_paths_with_nesting() {
        let data = json!({
            "user": {"name": "alice", "email": "alice@example.com"},
            "count": 42
        });
        let paths = parse("user.name,count");
        let result = apply(&data, &paths);
        assert_eq!(result, json!({"user": {"name": "alice"}, "count": 42}));
    }

    #[test]
    fn test_parse_empty_string() {
        let paths = parse("");
        assert!(paths.is_empty());
    }

    #[test]
    fn test_apply_to_array_multiple_keys() {
        let data = json!([
            {"name": "alice", "age": 30, "city": "NYC"},
            {"name": "bob", "age": 25, "city": "LA"}
        ]);
        let paths = parse("name,age");
        let result = apply(&data, &paths);
        assert_eq!(
            result,
            json!([
                {"name": "alice", "age": 30},
                {"name": "bob", "age": 25}
            ])
        );
    }
}
