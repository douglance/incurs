use incurs_codemode::{ConnectorDescription, ProgramSourceOptions, build_program_source};

/// Dynamic Worker sandbox settings.
#[derive(Debug, Clone)]
pub struct DynamicWorkerOptions {
    /// Maximum execution time in milliseconds.
    pub timeout_ms: u64,
    /// Compatibility date for the child Worker.
    pub compatibility_date: String,
    /// Additional JavaScript modules.
    pub modules: Vec<(String, String)>,
}

impl Default for DynamicWorkerOptions {
    fn default() -> Self {
        Self {
            timeout_ms: 60_000,
            compatibility_date: "2025-06-01".to_string(),
            modules: Vec::new(),
        }
    }
}

/// Builds the internal Dynamic Worker module for one sandbox program.
pub fn build_executor_module(
    code: &str,
    connectors: &[ConnectorDescription],
    timeout_ms: u64,
) -> Result<String, String> {
    let program = build_program_source(
        code,
        connectors,
        &ProgramSourceOptions {
            dispatch: r#"async (payload) => {
      const response = await fetch(
        "https://incurs-codemode.internal/__incurs_codemode_dispatch",
        {
          method: "POST",
          headers: { "content-type": "application/json" },
          body: JSON.stringify({
            ...payload,
            execution_id: this.env.__INCURS_CODEMODE_EXECUTION_ID
          })
        }
      );
      if (!response.ok) throw new Error(await response.text());
      return response.json();
    }"#
            .to_string(),
            execution_id: "this.env.__INCURS_CODEMODE_EXECUTION_ID".to_string(),
            timeout_ms: Some(timeout_ms),
        },
    )?;
    Ok(format!(
        r#"import {{ WorkerEntrypoint }} from "cloudflare:workers";
export default class CodeExecutor extends WorkerEntrypoint {{
  async evaluate() {{
    {program}
  }}
}}
"#
    ))
}

#[cfg(test)]
mod tests {
    use incurs_codemode::{ConnectorTool, ToolAnnotations};
    use serde_json::json;

    use super::*;

    #[test]
    fn emits_isolated_fetch_harness() {
        let source = build_executor_module(
            "state.read({ id: 1 })",
            &[ConnectorDescription {
                name: "state".to_string(),
                instructions: None,
                tools: vec![ConnectorTool {
                    name: "read".to_string(),
                    description: None,
                    input_schema: json!({"type": "object"}),
                    output_schema: None,
                    instructions: None,
                    examples: Vec::new(),
                    annotations: ToolAnnotations::default(),
                    policy: incurs_codemode::ToolPolicy::default(),
                }],
            }],
            60_000,
        )
        .unwrap();
        assert!(source.contains("const state"));
        assert!(source.contains("const response = await fetch("));
        assert!(source.contains("kind: 'call'"));
        assert!(source.contains("Code execution timed out"));
    }
}
