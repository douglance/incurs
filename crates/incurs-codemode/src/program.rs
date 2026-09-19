use crate::{ConnectorDescription, normalize_code};

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
    // One lazy binding per namespace rather than a materialized object of methods.
    // Writing the methods out required every connector's tool list, which meant
    // describing — and so connecting to — every configured server before any
    // program could run, including the ones the program never mentions. The proxy
    // needs only the name; the connection opens on the first call.
    let bindings = connectors
        .iter()
        .map(|connector| {
            format!(
                "const {} = __namespace({});",
                connector.name,
                serde_json::to_string(&connector.name).unwrap(),
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
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
        r#"const __logs = [];
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
const __namespace = (name) => new Proxy({{}}, {{
  get: (_target, method) => {{
    // Only `then` is withheld, and only because a namespace must not look
    // thenable: awaiting one, or returning it from an async function, probes
    // `then` and would otherwise receive a dispatch function and hang. Every
    // other name is forwarded, because anything withheld here silently deletes a
    // tool that is legitimately called that -- `inspect` and `constructor` are
    // both real tool names in this workspace.
    if (typeof method !== "string" || method === "then") return undefined;
    return async (args = {{}}) => __unwrap(await __dispatch({{
      kind: "call",
      seq: __seq++,
      connector: name,
      method,
      arguments: __encode(args)
    }}));
  }},
  has: () => true
}});
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

/// Names the generated program already binds, or that JavaScript refuses as a
/// binding.
///
/// A connector taking one of these produces a `const` redeclaration or a parse
/// error inside the sandbox, which surfaces far from its cause, so it is
/// rejected here where the name is still attributable to a connector.
pub(crate) const RESERVED_CONNECTOR_NAMES: &[&str] = &[
    // Bindings emitted by `build_program_source`, in declaration order.
    "__incursDispatch",
    "__dispatch",
    "__encode",
    "__decode",
    "__unwrap",
    "__seq",
    "__logs",
    "__program",
    "__result",
    "__base64Encode",
    "__base64Decode",
    "__callBuiltin",
    "codemode",
    "console",
    // Globals the harness itself calls.
    "Promise",
    "setTimeout",
    "Error",
    "fetch",
    "JSON",
    "Object",
    "Array",
    "String",
    "Number",
    "Boolean",
    "BigInt",
    "Uint8Array",
    "Math",
    "Symbol",
    "globalThis",
    // Reserved words. `valid_identifier` accepts these, but `const <word> = …`
    // is a syntax error.
    "await",
    "break",
    "case",
    "catch",
    "class",
    "const",
    "continue",
    "debugger",
    "default",
    "delete",
    "do",
    "else",
    "enum",
    "export",
    "extends",
    "false",
    "finally",
    "for",
    "function",
    "if",
    "implements",
    "import",
    "in",
    "instanceof",
    "interface",
    "let",
    "new",
    "null",
    "package",
    "private",
    "protected",
    "public",
    "return",
    "static",
    "super",
    "switch",
    "this",
    "throw",
    "true",
    "try",
    "typeof",
    "var",
    "void",
    "while",
    "with",
    "yield",
];

fn validate_names(connectors: &[ConnectorDescription]) -> Result<(), String> {
    let mut seen = std::collections::BTreeSet::new();
    for connector in connectors {
        if RESERVED_CONNECTOR_NAMES.contains(&connector.name.as_str()) {
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

    fn named(name: &str) -> ConnectorDescription {
        ConnectorDescription {
            name: name.to_string(),
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
        }
    }

    fn build(name: &str) -> Result<String, String> {
        build_program_source(
            "return 1",
            &[named(name)],
            &ProgramSourceOptions {
                dispatch: "async (payload) => payload".to_string(),
                execution_id: "\"test\"".to_string(),
                timeout_ms: None,
            },
        )
    }

    #[test]
    fn rejects_names_the_harness_already_binds() {
        // These are `const`-declared by `build_program_source` itself, so a
        // connector taking one produces a redeclaration inside the sandbox.
        for name in ["__callBuiltin", "__base64Encode", "__base64Decode"] {
            assert!(
                build(name).is_err(),
                "{name} is bound by the harness but was accepted"
            );
        }
    }

    #[test]
    fn rejects_reserved_words_that_cannot_be_bound() {
        // `valid_identifier` accepts each of these, but `const class = {…}` is
        // a syntax error that would surface far from the offending connector.
        for name in ["class", "default", "delete", "new", "import", "return"] {
            assert!(
                build(name).is_err(),
                "reserved word {name} was accepted as a connector name"
            );
        }
    }

    #[test]
    fn accepts_an_ordinary_name_that_merely_resembles_a_reserved_one() {
        // The guard is exact-match, so renaming around a collision works.
        for name in ["mcp_fetch", "classroom", "defaults", "newRelic"] {
            assert!(build(name).is_ok(), "{name} should be a usable namespace");
        }
    }
}
