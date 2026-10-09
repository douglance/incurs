use incurs_openai_mcp_protocol::SUPPORTED_SCHEMA_NAMES;

#[test]
fn maps_every_upstream_schema_export_in_index_files() {
    let server = include_str!("../../../parity/upstream/typescript/src/server/index.ts");
    let app = include_str!("../../../parity/upstream/typescript/src/app/index.ts");
    let mut missing = Vec::new();
    for source in [server, app] {
        for token in source.split(|ch: char| !(ch.is_ascii_alphanumeric() || ch == '_')) {
            if token.starts_with("OpenAI")
                && token.ends_with("Schema")
                && !SUPPORTED_SCHEMA_NAMES.contains(&token)
            {
                missing.push(token.to_string());
            }
        }
    }
    missing.sort();
    missing.dedup();
    assert!(missing.is_empty(), "missing schema exports: {missing:?}");
}
