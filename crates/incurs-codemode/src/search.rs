use serde::{Deserialize, Serialize};

use crate::{ConnectorDescription, Snippet, generate_types};

const LIMIT: usize = 50;

/// One ranked connector method or snippet match.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SearchResult {
    /// Address used with describe or run.
    pub path: String,
    /// Connector namespace, or `snippet`.
    pub connector: String,
    /// Connector method or snippet name.
    pub method: String,
    /// Human-readable description.
    pub description: Option<String>,
    /// TypeScript declarations or snippet source needed to use the match.
    pub types: String,
    /// Whether invocation pauses for approval.
    pub requires_approval: bool,
    /// Match kind.
    pub kind: String,
    /// Weighted relevance score.
    pub score: u32,
}

/// Bounded ranked search response.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SearchOutput {
    /// Ranked matches.
    pub results: Vec<SearchResult>,
    /// Total matches before limiting.
    pub total: usize,
    /// Whether matches were omitted.
    pub truncated: bool,
}

/// Documentation for one connector, method, or snippet.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DescribeOutput {
    /// Resolved path.
    pub path: String,
    /// Human-readable description.
    pub description: Option<String>,
    /// Whether invocation pauses for approval.
    pub requires_approval: bool,
    /// TypeScript declarations or snippet source.
    pub types: String,
    /// Target kind.
    pub kind: String,
}

/// Searches connector methods and saved snippets with weighted token matching.
pub fn search(
    query: &str,
    connectors: &[ConnectorDescription],
    snippets: &[Snippet],
) -> SearchOutput {
    let normalized = normalize(query);
    let tokens = tokenize(query);
    let mut results = Vec::new();
    for connector in connectors {
        for tool in &connector.tools {
            let path = format!("{}.{}", connector.name, tool.name);
            if let Some(score) = score(
                &normalized,
                &tokens,
                [
                    (&path, 12),
                    (&connector.name, 8),
                    (&tool.name, 10),
                    (tool.description.as_deref().unwrap_or_default(), 5),
                ],
            ) {
                results.push(SearchResult {
                    path,
                    connector: connector.name.clone(),
                    method: tool.name.clone(),
                    description: tool.description.clone(),
                    types: {
                        let mut single = connector.clone();
                        single.instructions = None;
                        single.tools = vec![tool.clone()];
                        generate_types(&single)
                    },
                    requires_approval: tool.policy.requires_approval,
                    kind: "method".to_string(),
                    score,
                });
            }
        }
    }
    for snippet in snippets {
        if let Some(score) = score(
            &normalized,
            &tokens,
            [
                (&snippet.name, 12),
                ("snippet", 8),
                (&snippet.name, 10),
                (&snippet.description, 5),
            ],
        ) {
            results.push(SearchResult {
                path: snippet.name.clone(),
                connector: "snippet".to_string(),
                method: snippet.name.clone(),
                description: Some(snippet.description.clone()),
                types: snippet.code.clone(),
                requires_approval: false,
                kind: "snippet".to_string(),
                score,
            });
        }
    }
    results.sort_by(|left, right| {
        right
            .score
            .cmp(&left.score)
            .then_with(|| left.path.cmp(&right.path))
    });
    let total = results.len();
    results.truncate(LIMIT);
    SearchOutput {
        results,
        total,
        truncated: total > LIMIT,
    }
}

/// Describes one connector, method, or saved snippet.
pub fn describe(
    target: &str,
    connectors: &[ConnectorDescription],
    snippets: &[Snippet],
) -> DescribeOutput {
    if let Some(snippet) = snippets.iter().find(|snippet| snippet.name == target) {
        return DescribeOutput {
            path: target.to_string(),
            description: Some(snippet.description.clone()),
            requires_approval: false,
            types: format!("{}\n\n```ts\n{}\n```", snippet.description, snippet.code),
            kind: "snippet".to_string(),
        };
    }
    let (connector_name, method_name) = target
        .split_once('.')
        .map(|(connector, method)| (Some(connector), method))
        .unwrap_or((None, target));
    if connector_name.is_none()
        && let Some(connector) = connectors.iter().find(|connector| connector.name == target)
    {
        return DescribeOutput {
            path: target.to_string(),
            description: connector.instructions.clone(),
            requires_approval: false,
            types: generate_types(connector),
            kind: "connector".to_string(),
        };
    }
    for connector in connectors
        .iter()
        .filter(|connector| connector_name.is_none_or(|name| connector.name == name))
    {
        if let Some(tool) = connector.tools.iter().find(|tool| tool.name == method_name) {
            let mut single = connector.clone();
            single.instructions = None;
            single.tools = vec![tool.clone()];
            return DescribeOutput {
                path: format!("{}.{}", connector.name, tool.name),
                description: tool.description.clone(),
                requires_approval: tool.policy.requires_approval,
                types: generate_types(&single),
                kind: "method".to_string(),
            };
        }
    }
    DescribeOutput {
        path: target.to_string(),
        description: None,
        requires_approval: false,
        types: format!("\"{target}\" not found."),
        kind: "method".to_string(),
    }
}

fn score<const COUNT: usize>(
    query: &str,
    query_tokens: &[String],
    fields: [(&str, u32); COUNT],
) -> Option<u32> {
    if query.is_empty() || query_tokens.is_empty() {
        return None;
    }
    let mut score = 0;
    let mut matched = vec![false; query_tokens.len()];
    let mut phrase = false;
    for (value, weight) in fields {
        let value = normalize(value);
        let tokens = tokenize(&value);
        if value == query {
            score += weight * 14;
        } else if value.starts_with(query) {
            score += weight * 9;
        } else if value.contains(query) {
            score += weight * 6;
            phrase = true;
        }
        for (index, token) in query_tokens.iter().enumerate() {
            if tokens.contains(token) {
                score += weight * 4;
                matched[index] = true;
            } else if tokens
                .iter()
                .any(|candidate| candidate.starts_with(token) || token.starts_with(candidate))
            {
                score += weight * 2;
                matched[index] = true;
            } else if value.contains(token) {
                score += weight;
                matched[index] = true;
            }
        }
    }
    let matched_count = matched.iter().filter(|matched| **matched).count();
    if matched_count == 0 {
        return None;
    }
    let coverage = matched_count as f32 / query_tokens.len() as f32;
    if coverage < if query_tokens.len() <= 2 { 1.0 } else { 0.6 } && !phrase {
        return None;
    }
    Some(
        score
            + if matched_count == query_tokens.len() {
                25
            } else {
                10
            },
    )
}

fn normalize(value: &str) -> String {
    let mut result = String::new();
    let mut previous_lower = false;
    for ch in value.chars() {
        if ch.is_ascii_uppercase() && previous_lower {
            result.push(' ');
        }
        if ch.is_ascii_alphanumeric() {
            result.push(ch.to_ascii_lowercase());
            previous_lower = ch.is_ascii_lowercase() || ch.is_ascii_digit();
        } else {
            result.push(' ');
            previous_lower = false;
        }
    }
    result.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn tokenize(value: &str) -> Vec<String> {
    normalize(value)
        .split_whitespace()
        .map(str::to_string)
        .collect()
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use crate::{ConnectorTool, ToolAnnotations};

    use super::*;

    #[test]
    fn ranks_exact_method_matches_first() {
        let connectors = [ConnectorDescription {
            name: "github".to_string(),
            instructions: None,
            tools: vec![ConnectorTool {
                name: "listIssues".to_string(),
                description: Some("List repository issues".to_string()),
                input_schema: json!({"type": "object"}),
                output_schema: None,
                instructions: None,
                examples: Vec::new(),
                annotations: ToolAnnotations::default(),
                policy: crate::ToolPolicy::default(),
            }],
        }];
        let result = search("list issues", &connectors, &[]);
        assert_eq!(result.results[0].path, "github.listIssues");
        assert!(result.results[0].types.contains("listIssues"));
    }
}
