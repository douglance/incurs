/// Normalizes model-generated JavaScript into an async zero-argument function.
pub fn normalize_code(code: &str) -> String {
    let source = strip_fence(code.trim()).trim();
    if source.is_empty() {
        return "async () => {}".to_string();
    }
    if is_arrow(source) {
        return source.to_string();
    }
    if let Some(inner) = source.strip_prefix("export default ") {
        return normalize_code(inner.trim_end_matches(';'));
    }
    if let Some(name) = single_function_name(source) {
        return format!("async () => {{\n{source}\nreturn {name}();\n}}");
    }
    if let Some((before, expression)) = split_last_expression(source) {
        return format!("async () => {{\n{before}return ({expression})\n}}");
    }
    format!("async () => {{\n{source}\n}}")
}

fn strip_fence(source: &str) -> &str {
    let Some(after) = source.strip_prefix("```") else {
        return source;
    };
    let Some(newline) = after.find('\n') else {
        return source;
    };
    let body = &after[newline + 1..];
    body.strip_suffix("```")
        .map(str::trim_end)
        .unwrap_or(source)
}

fn is_arrow(source: &str) -> bool {
    let Some(arrow) = top_level_arrow(source) else {
        return false;
    };
    let prefix = source[..arrow].trim();
    prefix.ends_with(')') || prefix.starts_with("async ") || is_identifier(prefix)
}

fn top_level_arrow(source: &str) -> Option<usize> {
    let mut depth = 0;
    let mut quote = None;
    let mut escaped = false;
    let mut chars = source.char_indices().peekable();
    while let Some((index, ch)) = chars.next() {
        if let Some(active) = quote {
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == active {
                quote = None;
            }
            continue;
        }
        match ch {
            '\'' | '"' | '`' => quote = Some(ch),
            '(' | '[' | '{' => depth += 1,
            ')' | ']' | '}' => depth -= 1,
            '=' if depth == 0 && chars.peek().is_some_and(|(_, next)| *next == '>') => {
                return Some(index);
            }
            _ => {}
        }
    }
    None
}

fn single_function_name(source: &str) -> Option<&str> {
    let rest = source
        .strip_prefix("async function ")
        .or_else(|| source.strip_prefix("function "))?;
    let end = rest.find('(')?;
    let name = rest[..end].trim();
    if is_identifier(name) && balanced(source) {
        Some(name)
    } else {
        None
    }
}

fn split_last_expression(source: &str) -> Option<(&str, &str)> {
    if !balanced(source) || source.ends_with('}') {
        return None;
    }
    let mut depth = 0;
    let mut quote = None;
    let mut escaped = false;
    let mut split = None;
    for (index, ch) in source.char_indices() {
        if let Some(active) = quote {
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == active {
                quote = None;
            }
            continue;
        }
        match ch {
            '\'' | '"' | '`' => quote = Some(ch),
            '(' | '[' | '{' => depth += 1,
            ')' | ']' | '}' => depth -= 1,
            ';' if depth == 0 => split = Some(index + 1),
            _ => {}
        }
    }
    let mut index = split.unwrap_or(0);
    while source[index..]
        .chars()
        .next()
        .is_some_and(char::is_whitespace)
    {
        index += source[index..].chars().next().unwrap().len_utf8();
    }
    let expression = source[index..].trim().trim_end_matches(';').trim();
    if expression.is_empty() || starts_statement(expression) {
        None
    } else {
        Some((&source[..index], expression))
    }
}

fn starts_statement(source: &str) -> bool {
    [
        "const ",
        "let ",
        "var ",
        "return ",
        "throw ",
        "if ",
        "for ",
        "while ",
        "class ",
        "function ",
        "import ",
        "export ",
        "try ",
        "switch ",
    ]
    .iter()
    .any(|prefix| source.starts_with(prefix))
}

fn balanced(source: &str) -> bool {
    let mut stack = Vec::new();
    let mut quote = None;
    let mut escaped = false;
    for ch in source.chars() {
        if let Some(active) = quote {
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == active {
                quote = None;
            }
            continue;
        }
        match ch {
            '\'' | '"' | '`' => quote = Some(ch),
            '(' | '[' | '{' => stack.push(ch),
            ')' if stack.pop() != Some('(') => return false,
            ']' if stack.pop() != Some('[') => return false,
            '}' if stack.pop() != Some('{') => return false,
            _ => {}
        }
    }
    stack.is_empty() && quote.is_none()
}

fn is_identifier(value: &str) -> bool {
    let mut chars = value.chars();
    chars
        .next()
        .is_some_and(|ch| ch == '_' || ch == '$' || ch.is_ascii_alphabetic())
        && chars.all(|ch| ch == '_' || ch == '$' || ch.is_ascii_alphanumeric())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_common_model_outputs() {
        assert_eq!(normalize_code(""), "async () => {}");
        assert_eq!(normalize_code("async () => 1"), "async () => 1");
        assert_eq!(
            normalize_code("const x = 1;\nx + 2"),
            "async () => {\nconst x = 1;\nreturn (x + 2)\n}"
        );
        assert_eq!(
            normalize_code("```js\nstate.read({ id: 1 })\n```"),
            "async () => {\nreturn (state.read({ id: 1 }))\n}"
        );
        assert_eq!(
            normalize_code(
                "const token = await codemode.step(\"token\", async () => \"stable\");\nreturn token;"
            ),
            "async () => {\nconst token = await codemode.step(\"token\", async () => \"stable\");\nreturn token;\n}"
        );
    }
}
