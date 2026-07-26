use crate::{ConnectorDescription, generate_types, normalize_code};

/// Adapter-specific settings for the shared Code Mode JavaScript harness.
#[derive(Debug, Clone)]
pub struct ProgramSourceOptions {
    /// JavaScript expression that evaluates to the asynchronous dispatch function.
    pub dispatch: String,
    /// JavaScript expression exposed as `codemode.executionId`.
    pub execution_id: String,
    /// Optional JavaScript timer timeout for hosts that provide `setTimeout`.
    pub timeout_ms: Option<u64>,
}

/// Builds the shared body used by local and remote Code Mode executors.
pub fn build_program_source(
    code: &str,
    connectors: &[ConnectorDescription],
    options: &ProgramSourceOptions,
) -> Result<String, String> {
    validate_names(connectors)?;
    let code = normalize_code(code);
    let bindings = connectors
        .iter()
        .map(|connector| {
            let methods = connector
                .tools
                .iter()
                .map(|tool| {
                    format!(
                        "{}: async (args = {{}}) => __unwrap(await __dispatch({{ kind: 'call', seq: __seq++, connector: {}, method: {}, arguments: __encode(args) }}))",
                        serde_json::to_string(&tool.name).unwrap(),
                        serde_json::to_string(&connector.name).unwrap(),
                        serde_json::to_string(&tool.name).unwrap(),
                    )
                })
                .collect::<Vec<_>>()
                .join(",\n");
            format!("const {} = {{\n{methods}\n}};", connector.name)
        })
        .collect::<Vec<_>>()
        .join("\n");
    let type_comment = connectors
        .iter()
        .map(generate_types)
        .collect::<Vec<_>>()
        .join("\n\n")
        .replace("*/", "*\\/");
    let run = if let Some(timeout_ms) = options.timeout_ms {
        format!(
            r#"await Promise.race([
      __program(),
      new Promise((_, reject) => setTimeout(() => reject(new Error("Code execution timed out after {timeout_ms}ms")), {timeout_ms}))
    ])"#
        )
    } else {
        "await __program()".to_string()
    };
    Ok(format!(
        r#"/* Model-facing declarations:
{type_comment}
*/
const __logs = [];
let __seq = 0;
const console = {{
  log: (...values) => __logs.push(values.map(String).join(" ")),
  info: (...values) => __logs.push(values.map(String).join(" ")),
  warn: (...values) => __logs.push(values.map(String).join(" ")),
  error: (...values) => __logs.push(values.map(String).join(" "))
}};
const __base64Encode = (value) => {{
  const alphabet = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
  let result = "";
  for (let index = 0; index < value.length; index += 3) {{
    const first = value[index];
    const second = value[index + 1];
    const third = value[index + 2];
    result += alphabet[first >> 2];
    result += alphabet[((first & 3) << 4) | ((second ?? 0) >> 4)];
    result += second === undefined ? "=" : alphabet[((second & 15) << 2) | ((third ?? 0) >> 6)];
    result += third === undefined ? "=" : alphabet[third & 63];
  }}
  return result;
}};
const __base64Decode = (value) => {{
  const alphabet = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
  const clean = value.replace(/=+$/, "");
  const bytes = [];
  let buffer = 0;
  let bits = 0;
  for (const char of clean) {{
    buffer = (buffer << 6) | alphabet.indexOf(char);
    bits += 6;
    if (bits >= 8) {{
      bits -= 8;
      bytes.push((buffer >> bits) & 255);
    }}
  }}
  return new Uint8Array(bytes);
}};
const __encode = (value) => {{
  if (value === undefined) return {{ __codemode_type: "undefined" }};
  if (typeof value === "bigint") return {{ __codemode_type: "bigint", value: value.toString() }};
  if (value instanceof Uint8Array) return {{ __codemode_type: "binary", value: __base64Encode(value) }};
  if (Array.isArray(value)) return value.map(__encode);
  if (value && typeof value === "object") {{
    return Object.fromEntries(Object.entries(value).map(([key, child]) => [key, __encode(child)]));
  }}
  return value;
}};
const __decode = (value) => {{
  if (Array.isArray(value)) return value.map(__decode);
  if (value && typeof value === "object") {{
    if (value.__codemode_type === "undefined") return undefined;
    if (value.__codemode_type === "bigint") return BigInt(value.value);
    if (value.__codemode_type === "binary") return __base64Decode(value.value);
    return Object.fromEntries(Object.entries(value).map(([key, child]) => [key, __decode(child)]));
  }}
  return value;
}};
const __dispatch = {dispatch};
const __unwrap = (response) => {{
  if (response.__codemode_control__ === "pause") throw new Error("__CODEMODE_PAUSE__");
  if (response.__codemode_control__ === "error") throw new Error(response.message);
  return __decode(response.result);
}};
{bindings}
const __callBuiltin = async (method, args) => __unwrap(await __dispatch({{
  kind: "call",
  seq: __seq++,
  connector: "codemode",
  method,
  arguments: __encode(args)
}}));
const codemode = {{
  executionId: {execution_id},
  search: (query) => __callBuiltin("search", {{ query }}),
  describe: (target) => __callBuiltin("describe", {{ target }}),
  step: async (name, fn) => {{
    const seq = __seq++;
    const decision = await __dispatch({{ kind: "begin_step", seq, name }});
    if (decision.kind === "pause") throw new Error("__CODEMODE_PAUSE__");
    if (decision.kind === "replay") return __decode(decision.result);
    const result = await fn();
    await __dispatch({{ kind: "record_step", seq, result: __encode(result) }});
    return result;
  }}
}};
try {{
  const __program = ({code});
  const __result = {run};
  return {{ result: __encode(__result), error: null, logs: __logs }};
}} catch (error) {{
  return {{
    result: null,
    error: error instanceof Error ? error.message : String(error),
    logs: __logs
  }};
}}"#,
        dispatch = options.dispatch,
        execution_id = options.execution_id,
    ))
}

fn validate_names(connectors: &[ConnectorDescription]) -> Result<(), String> {
    const RESERVED: &[&str] = &[
        "__incursDispatch",
        "__dispatch",
        "__encode",
        "__decode",
        "__unwrap",
        "__seq",
        "__logs",
        "__program",
        "__result",
        "Promise",
        "setTimeout",
        "Error",
        "console",
        "codemode",
        "fetch",
    ];
    let mut seen = std::collections::BTreeSet::new();
    for connector in connectors {
        if RESERVED.contains(&connector.name.as_str()) {
            return Err(format!("Connector name \"{}\" is reserved", connector.name));
        }
        if !valid_identifier(&connector.name) {
            return Err(format!(
                "Connector name \"{}\" is not a valid JavaScript identifier",
                connector.name
            ));
        }
        if !seen.insert(&connector.name) {
            return Err(format!("Duplicate connector name \"{}\"", connector.name));
        }
        let mut methods = std::collections::BTreeSet::new();
        for tool in &connector.tools {
            if !methods.insert(&tool.name) {
                return Err(format!(
                    "Duplicate tool name \"{}\" in connector \"{}\"",
                    tool.name, connector.name
                ));
            }
        }
    }
    Ok(())
}

fn valid_identifier(value: &str) -> bool {
    let mut chars = value.chars();
    chars
        .next()
        .is_some_and(|ch| ch == '_' || ch == '$' || ch.is_ascii_alphabetic())
        && chars.all(|ch| ch == '_' || ch == '$' || ch.is_ascii_alphanumeric())
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use crate::{ConnectorTool, ToolAnnotations};

    use super::*;

    #[test]
    fn emits_shared_connector_and_step_harness() {
        let source = build_program_source(
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
                    policy: crate::ToolPolicy::default(),
                }],
            }],
            &ProgramSourceOptions {
                dispatch: "async (payload) => payload".to_string(),
                execution_id: "\"test\"".to_string(),
                timeout_ms: None,
            },
        )
        .unwrap();

        assert!(source.contains("const state"));
        assert!(source.contains("const __dispatch = async (payload) => payload"));
        assert!(source.contains("step: async (name, fn)"));
    }
}
