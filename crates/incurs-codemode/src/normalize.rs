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
    let prefix = prefix.strip_prefix("async ").map_or(prefix, str::trim_start);
    // The prefix has to be the arrow's own parameter list and nothing else.
    //
    // Testing only that it ends with `)` accepted any program whose first
    // top-level `=>` happened to follow a parenthesised group, so
    // `const f = (a) => a; return f(1);` was mistaken for a single arrow
    // expression, left unwrapped, and failed to parse — while the same program
    // written `const f = a => a;` worked. A parameter list is either a bare
    // identifier or one balanced group spanning the whole prefix.
    is_identifier(prefix) || is_parameter_list(prefix)
}

/// Returns whether the text is exactly one parenthesised group.
///
/// The group must open at the first character and close at the last, so
/// `(a, b)` qualifies while `const f = (a)` and `(a) + (b)` do not.
fn is_parameter_list(prefix: &str) -> bool {
    if !prefix.starts_with('(') || !prefix.ends_with(')') {
        return false;
    }
    let mut depth = 0usize;
    for (index, ch) in prefix.char_indices() {
        match ch {
            '(' | '[' | '{' => depth += 1,
            ')' | ']' | '}' => {
                depth = depth.saturating_sub(1);
                if depth == 0 && index + ch.len_utf8() != prefix.len() {
                    return false;
                }
            }
            _ => {}
        }
    }
    depth == 0
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

    /// A program that names an arrow with a parenthesised parameter list was
    /// mistaken for a single arrow expression and left unwrapped, so it reached
    /// the sandbox as a statement list in expression position and failed with an
    /// opaque "Exception generated by QuickJS". Writing the same program with a
    /// bare parameter worked, which is what made it look like a sandbox fault
    /// rather than a normalisation one.
    #[test]
    fn a_named_arrow_with_a_parenthesised_parameter_is_still_a_program() {
        for source in [
            "const f = (a) => a + 1; return f(1);",
            "const f = (a, b) => a + b; return f(1, 2);",
            "const run = (dir, t) => dir + t;\nreturn run(\"a\", \"b\");",
            "const f = async (a) => a; return await f(1);",
        ] {
            let normalized = normalize_code(source);
            assert!(
                normalized.starts_with("async () => {"),
                "must be wrapped as a program, got: {normalized}"
            );
        }
    }

    /// The other half of the same boundary: a source that really is one arrow
    /// must still be passed through untouched.
    #[test]
    fn a_bare_arrow_expression_is_left_alone() {
        for source in [
            "(a) => a + 1",
            "(a, b) => a + b",
            "async (a) => a",
            "a => a + 1",
            "async () => { return 1; }",
        ] {
            assert_eq!(
                normalize_code(source),
                source,
                "a genuine arrow must not be wrapped"
            );
        }
    }

    /// A parenthesised group that is not a parameter list must not be read as one.
    #[test]
    fn a_parenthesised_expression_before_an_arrow_is_not_a_parameter_list() {
        assert!(!is_parameter_list("const f = (a)"));
        assert!(!is_parameter_list("(a) + (b)"));
        assert!(is_parameter_list("(a)"));
        assert!(is_parameter_list("(a, b = (1))"));
    }
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
