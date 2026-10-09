//! Parity between the native `rmcp` Streamable HTTP service and
//! [`super::portable_server::McpHttpServer`].
//!
//! Every case is sent to both servers. The expected observation always comes
//! from the native `rmcp` service; the portable server must reproduce its
//! status, content type, `Allow` header, plain-text body, and JSON-RPC message
//! sequence. The only permitted differences are listed in
//! [`PERMITTED_DIFFERENCES`].

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use axum::body::Body;
use futures::StreamExt;
use serde_json::{Value, json};
use tower::ServiceExt;

use super::portable_server::{McpHttpBody, McpHttpConfig, McpHttpRequest, McpHttpServer};
use super::{McpDiscovery, McpServeOptions, McpToolFilter};
use crate::cli::Cli;
use crate::command::{
    CommandContext, CommandDef, CommandHandler, McpAnnotations, McpCommandOptions, McpResultContent,
};
use crate::output::{CommandResult, CtaBlock, CtaEntry};
use crate::schema::{FieldMeta, FieldType};
use crate::tool::EnvironmentSource;

/// Environment variable read by the `whoami` fixture command.
const ENV_NAME: &str = "INCURS_PARITY_TOKEN";
/// Value the native side reads from the process and the portable side from config.
const ENV_VALUE: &str = "parity-secret";

/// Cases where the portable server is required to differ from native rmcp.
///
/// Native rmcp, as configured by `http_service`, never inspects `Origin`.
/// The portable server rejects a present `Origin` that is not allowed.
const PERMITTED_DIFFERENCES: &[&str] = &["origin-disallowed", "origin-malformed"];

// ---------------------------------------------------------------------------
// Fixture CLI
// ---------------------------------------------------------------------------

struct Echo;

#[async_trait::async_trait]
impl CommandHandler for Echo {
    async fn run(&self, ctx: CommandContext) -> CommandResult {
        // Sorted so the rendered text is deterministic.
        let request = ctx.request.map(|request| {
            let headers = request
                .headers
                .into_iter()
                .collect::<std::collections::BTreeMap<_, _>>();
            json!({ "method": request.method, "path": request.path, "headers": headers })
        });
        CommandResult::Ok {
            data: json!({ "args": ctx.args, "options": ctx.options, "request": request }),
            cta: None,
            exit_code: None,
        }
    }
}

struct Profile;

#[async_trait::async_trait]
impl CommandHandler for Profile {
    async fn run(&self, _ctx: CommandContext) -> CommandResult {
        CommandResult::Ok {
            data: json!({ "name": "Ada", "active": true }),
            cta: None,
            exit_code: None,
        }
    }
}

struct Snapshot;

#[async_trait::async_trait]
impl CommandHandler for Snapshot {
    async fn run(&self, _ctx: CommandContext) -> CommandResult {
        CommandResult::Ok {
            data: json!({
                "preview": { "data": "aW1hZ2U=", "mimeType": "image/png" },
                "url": "https://example.test/ui.png",
            }),
            cta: None,
            exit_code: None,
        }
    }
}

struct Suggest;

#[async_trait::async_trait]
impl CommandHandler for Suggest {
    async fn run(&self, _ctx: CommandContext) -> CommandResult {
        CommandResult::Ok {
            data: json!({ "status": "ready" }),
            cta: Some(CtaBlock {
                commands: vec![
                    CtaEntry::Simple("status".to_string()),
                    CtaEntry::Detailed {
                        command: "parity deploy --force".to_string(),
                        description: Some("Deploy now".to_string()),
                    },
                ],
                description: None,
            }),
            exit_code: None,
        }
    }
}

struct Fail;

#[async_trait::async_trait]
impl CommandHandler for Fail {
    async fn run(&self, _ctx: CommandContext) -> CommandResult {
        CommandResult::Error {
            code: "DEPLOY_FAILED".to_string(),
            message: "deploy failed".to_string(),
            retryable: true,
            exit_code: Some(3),
            cta: Some(CtaBlock {
                commands: vec![CtaEntry::Simple("logs".to_string())],
                description: Some("Next:".to_string()),
            }),
        }
    }
}

struct Watch;

#[async_trait::async_trait]
impl CommandHandler for Watch {
    async fn run(&self, _ctx: CommandContext) -> CommandResult {
        CommandResult::Stream(Box::pin(futures::stream::iter(vec![
            json!({ "tick": 1 }),
            json!({ "tick": 2 }),
            json!("done"),
        ])))
    }
}

struct WhoAmI;

#[async_trait::async_trait]
impl CommandHandler for WhoAmI {
    async fn run(&self, ctx: CommandContext) -> CommandResult {
        CommandResult::Ok {
            data: json!({ "env": ctx.env }),
            cta: None,
            exit_code: None,
        }
    }
}

fn field(name: &'static str, field_type: FieldType, required: bool) -> FieldMeta {
    FieldMeta {
        name,
        cli_name: name.to_string(),
        description: None,
        field_type,
        required,
        default: None,
        alias: None,
        deprecated: false,
        env_name: None,
    }
}

fn fixture_cli(discovery: McpDiscovery) -> Cli {
    let mut echo = CommandDef::build("echo", Echo)
        .description("Echo validated arguments and options")
        .mcp(McpCommandOptions {
            annotations: Some(McpAnnotations {
                read_only_hint: Some(true),
                ..Default::default()
            }),
            instructions: Some("Pass a message".to_string()),
            ..Default::default()
        })
        .done();
    echo.options_fields = vec![field("message", FieldType::String, false)];

    let mut profile = CommandDef::build("profile", Profile)
        .description("Show the profile")
        .done();
    profile.output_schema = Some(json!({
        "type": "object",
        "properties": { "name": { "type": "string" }, "active": { "type": "boolean" } },
    }));

    let mut snapshot = CommandDef::build("snapshot", Snapshot)
        .description("Capture a UI snapshot")
        .mcp(McpCommandOptions {
            result_content: vec![McpResultContent::Image {
                data_pointer: "/preview/data".to_string(),
                mime_type_pointer: "/preview/mimeType".to_string(),
            }],
            ..Default::default()
        })
        .done();
    snapshot.output_schema = Some(json!({ "type": "object" }));

    let mut count = CommandDef::build("count", Echo)
        .description("Count things")
        .done();
    count.options_fields = vec![field("count", FieldType::Number, true)];

    let mut whoami = CommandDef::build("whoami", WhoAmI)
        .description("Show the configured token")
        .done();
    whoami.env_fields = vec![FieldMeta {
        env_name: Some(ENV_NAME),
        ..field("token", FieldType::String, false)
    }];

    Cli::create("parity")
        .version("1.2.3")
        .mcp(McpServeOptions {
            instructions: Some("Use the parity tools".to_string()),
            tools: McpToolFilter {
                discovery,
                ..Default::default()
            },
            ..Default::default()
        })
        .command("echo", echo)
        .command("profile", profile)
        .command("snapshot", snapshot)
        .command(
            "suggest",
            CommandDef::build("suggest", Suggest)
                .description("Suggest a next step")
                .done(),
        )
        .command(
            "fail",
            CommandDef::build("fail", Fail)
                .description("Always fails")
                .done(),
        )
        .command("count", count)
        .command(
            "watch",
            CommandDef::build("watch", Watch)
                .description("Stream ticks")
                .done(),
        )
        .command("whoami", whoami)
}

// ---------------------------------------------------------------------------
// Corpus
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Fixture {
    Direct,
    Progressive,
}

struct Case {
    name: &'static str,
    fixture: Fixture,
    method: &'static str,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}

const MODERN: &str = "2026-07-28";
const STANDARDS: [&str; 5] = [
    "2024-11-05",
    "2025-03-26",
    "2025-06-18",
    "2025-11-25",
    "2026-07-28",
];

fn base_headers() -> Vec<(String, String)> {
    vec![
        ("host".to_string(), "localhost".to_string()),
        (
            "accept".to_string(),
            "application/json, text/event-stream".to_string(),
        ),
        ("content-type".to_string(), "application/json".to_string()),
    ]
}

fn with(mut headers: Vec<(String, String)>, extra: &[(&str, &str)]) -> Vec<(String, String)> {
    for (name, value) in extra {
        headers.retain(|(existing, _)| !existing.eq_ignore_ascii_case(name));
        headers.push((name.to_string(), value.to_string()));
    }
    headers
}

fn without(mut headers: Vec<(String, String)>, name: &str) -> Vec<(String, String)> {
    headers.retain(|(existing, _)| !existing.eq_ignore_ascii_case(name));
    headers
}

fn rpc(id: Value, method: &str, params: Option<Value>) -> Vec<u8> {
    let mut message = json!({ "jsonrpc": "2.0", "id": id, "method": method });
    if let Some(params) = params {
        message["params"] = params;
    }
    serde_json::to_vec(&message).unwrap()
}

fn modern_meta() -> Value {
    json!({
        "io.modelcontextprotocol/protocolVersion": MODERN,
        "io.modelcontextprotocol/clientCapabilities": {},
        "io.modelcontextprotocol/clientInfo": { "name": "parity", "version": "1.0.0" },
    })
}

fn modern_headers(method: &str, name: Option<&str>) -> Vec<(String, String)> {
    let mut headers = with(
        base_headers(),
        &[("mcp-protocol-version", MODERN), ("mcp-method", method)],
    );
    if let Some(name) = name {
        headers.push(("mcp-name".to_string(), name.to_string()));
    }
    headers
}

fn initialize(version: &str) -> Vec<u8> {
    rpc(
        json!(1),
        "initialize",
        Some(json!({
            "protocolVersion": version,
            "capabilities": {},
            "clientInfo": { "name": "parity", "version": "1.0.0" },
        })),
    )
}

fn call(id: Value, name: &str, arguments: Value) -> Vec<u8> {
    rpc(
        id,
        "tools/call",
        Some(json!({ "name": name, "arguments": arguments })),
    )
}

fn case(name: &'static str, headers: Vec<(String, String)>, body: Vec<u8>) -> Case {
    Case {
        name,
        fixture: Fixture::Direct,
        method: "POST",
        headers,
        body,
    }
}

fn progressive(mut case: Case) -> Case {
    case.fixture = Fixture::Progressive;
    case
}

fn corpus() -> Vec<Case> {
    let legacy = |version: &str| with(base_headers(), &[("mcp-protocol-version", version)]);
    let mut cases = vec![
        // HTTP method, Accept, Content-Type, body parsing, Host.
        Case {
            method: "GET",
            ..case("get-405", base_headers(), Vec::new())
        },
        Case {
            method: "DELETE",
            ..case("delete-405", base_headers(), Vec::new())
        },
        Case {
            method: "PUT",
            ..case("put-405", base_headers(), Vec::new())
        },
        case(
            "accept-json-only-406",
            with(base_headers(), &[("accept", "application/json")]),
            rpc(json!(1), "ping", None),
        ),
        case(
            "accept-missing-406",
            without(base_headers(), "accept"),
            rpc(json!(1), "ping", None),
        ),
        case(
            "content-type-415",
            with(base_headers(), &[("content-type", "text/plain")]),
            rpc(json!(1), "ping", None),
        ),
        case(
            "content-type-charset-ok",
            with(
                base_headers(),
                &[("content-type", "application/json; charset=utf-8")],
            ),
            rpc(json!(1), "ping", None),
        ),
        case("bad-json-415", base_headers(), b"{not json".to_vec()),
        case("batch-415", base_headers(), b"[]".to_vec()),
        case(
            "batch-nonempty",
            base_headers(),
            br#"[{"jsonrpc":"2.0","id":1,"method":"ping"},{"jsonrpc":"2.0","id":2,"method":"ping"}]"#.to_vec(),
        ),
        case(
            "wrong-jsonrpc-version-415",
            base_headers(),
            br#"{"jsonrpc":"1.0","id":1,"method":"ping"}"#.to_vec(),
        ),
        case(
            "array-params-415",
            base_headers(),
            br#"{"jsonrpc":"2.0","id":1,"method":"tools/list","params":[1]}"#.to_vec(),
        ),
        case(
            "oversized-413",
            base_headers(),
            vec![b' '; 4 * 1024 * 1024 + 1],
        ),
        case(
            "host-disallowed-403",
            with(base_headers(), &[("host", "evil.example")]),
            rpc(json!(1), "ping", None),
        ),
        case(
            "host-missing-400",
            without(base_headers(), "host"),
            rpc(json!(1), "ping", None),
        ),
        case(
            "host-malformed-400",
            with(base_headers(), &[("host", "local host")]),
            rpc(json!(1), "ping", None),
        ),
        case(
            "host-with-port",
            with(base_headers(), &[("host", "127.0.0.1:8080")]),
            rpc(json!(1), "ping", None),
        ),
        case(
            "host-ipv6",
            with(base_headers(), &[("host", "[::1]:3000")]),
            rpc(json!(1), "ping", None),
        ),
        case(
            "origin-allowed",
            with(base_headers(), &[("origin", "http://localhost:5173")]),
            rpc(json!(1), "ping", None),
        ),
        case(
            "origin-disallowed",
            with(base_headers(), &[("origin", "https://evil.example")]),
            rpc(json!(1), "ping", None),
        ),
        case(
            "origin-malformed",
            with(base_headers(), &[("origin", "not an origin")]),
            rpc(json!(1), "ping", None),
        ),
        // Lifecycle.
        case(
            "initialize-unknown-version",
            base_headers(),
            initialize("2099-01-01"),
        ),
        case(
            "initialize-header-mismatch-400",
            legacy("2025-06-18"),
            initialize("2025-03-26"),
        ),
        case(
            "initialize-malformed-params",
            base_headers(),
            rpc(
                json!(1),
                "initialize",
                Some(json!({ "protocolVersion": 1 })),
            ),
        ),
        case(
            "initialized-notification-202",
            base_headers(),
            br#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#.to_vec(),
        ),
        case(
            "cancelled-notification-202",
            legacy("2025-11-25"),
            br#"{"jsonrpc":"2.0","method":"notifications/cancelled","params":{"requestId":7}}"#
                .to_vec(),
        ),
        case(
            "client-response-202",
            base_headers(),
            br#"{"jsonrpc":"2.0","id":9,"result":{}}"#.to_vec(),
        ),
        case(
            "client-error-202",
            base_headers(),
            br#"{"jsonrpc":"2.0","id":9,"error":{"code":-32601,"message":"nope"}}"#.to_vec(),
        ),
        case(
            "fractional-id-is-notification-202",
            base_headers(),
            br#"{"jsonrpc":"2.0","id":1.5,"method":"ping"}"#.to_vec(),
        ),
        case("ping", base_headers(), rpc(json!("abc"), "ping", None)),
        case(
            "unsupported-version-header-400",
            legacy("1999-01-01"),
            rpc(json!(1), "tools/list", None),
        ),
        case(
            "unknown-method",
            legacy("2025-11-25"),
            rpc(json!(2), "foo/bar", Some(json!({}))),
        ),
        case(
            "completion-complete",
            legacy("2025-11-25"),
            rpc(
                json!(3),
                "completion/complete",
                Some(json!({
                    "ref": { "type": "ref/prompt", "name": "x" },
                    "argument": { "name": "a", "value": "b" },
                })),
            ),
        ),
        case(
            "prompts-get-not-found",
            legacy("2025-11-25"),
            rpc(json!(4), "prompts/get", Some(json!({ "name": "x" }))),
        ),
        case(
            "resources-read-not-found",
            legacy("2025-11-25"),
            rpc(
                json!(5),
                "resources/read",
                Some(json!({ "uri": "file:///x" })),
            ),
        ),
        case(
            "logging-set-level-not-found",
            legacy("2025-11-25"),
            rpc(
                json!(6),
                "logging/setLevel",
                Some(json!({ "level": "info" })),
            ),
        ),
        case(
            "resources-subscribe-not-found",
            legacy("2025-11-25"),
            rpc(
                json!(7),
                "resources/subscribe",
                Some(json!({ "uri": "file:///x" })),
            ),
        ),
        case(
            "tasks-get-not-found",
            legacy("2025-11-25"),
            rpc(json!(8), "tasks/get", Some(json!({ "taskId": "t" }))),
        ),
        case(
            "subscriptions-listen-legacy",
            legacy("2025-11-25"),
            rpc(
                json!(8),
                "subscriptions/listen",
                Some(json!({ "notifications": {} })),
            ),
        ),
        case(
            "prompts-list",
            legacy("2025-11-25"),
            rpc(json!(9), "prompts/list", None),
        ),
        case(
            "resources-list",
            legacy("2025-06-18"),
            rpc(json!(10), "resources/list", Some(json!({ "cursor": "c" }))),
        ),
        case(
            "resource-templates-list",
            legacy("2025-03-26"),
            rpc(json!(11), "resources/templates/list", None),
        ),
        case(
            "list-with-bad-cursor-is-custom",
            legacy("2025-11-25"),
            rpc(json!(12), "tools/list", Some(json!({ "cursor": 5 }))),
        ),
        // Tool calls (direct discovery).
        case(
            "call-echo",
            legacy("2025-11-25"),
            call(json!(20), "echo", json!({ "message": "hi" })),
        ),
        case(
            "call-profile-structured",
            legacy("2025-06-18"),
            call(json!(21), "profile", json!({})),
        ),
        case(
            "call-snapshot-image",
            legacy("2025-11-25"),
            call(json!(22), "snapshot", json!({})),
        ),
        case(
            "call-suggest-cta",
            legacy("2025-11-25"),
            call(json!(23), "suggest", Value::Null),
        ),
        case(
            "call-fail-exit-code",
            legacy("2025-11-25"),
            call(json!(24), "fail", json!({})),
        ),
        case(
            "call-count-field-errors",
            legacy("2025-11-25"),
            call(json!(25), "count", json!({ "count": "many" })),
        ),
        case(
            "call-unknown-option",
            legacy("2025-11-25"),
            call(json!(26), "echo", json!({ "nope": 1 })),
        ),
        case(
            "call-unknown-tool",
            legacy("2025-11-25"),
            call(json!(27), "missing", json!({})),
        ),
        case(
            "call-watch-progress",
            legacy("2025-11-25"),
            rpc(
                json!(28),
                "tools/call",
                Some(
                    json!({ "name": "watch", "arguments": {}, "_meta": { "progressToken": "p1" } }),
                ),
            ),
        ),
        case(
            "call-watch-no-progress-token",
            legacy("2025-11-25"),
            call(json!(29), "watch", json!({})),
        ),
        case(
            "call-whoami-env",
            legacy("2025-11-25"),
            call(json!(30), "whoami", json!({})),
        ),
        case(
            "call-arguments-not-object-is-custom",
            legacy("2025-11-25"),
            rpc(
                json!(31),
                "tools/call",
                Some(json!({ "name": "echo", "arguments": "x" })),
            ),
        ),
        // 2026-07-28 per-request negotiation and SEP-2243 headers.
        case(
            "modern-tools-list",
            modern_headers("tools/list", None),
            rpc(
                json!(40),
                "tools/list",
                Some(json!({ "_meta": modern_meta() })),
            ),
        ),
        case(
            "modern-missing-mcp-method-400",
            without(modern_headers("tools/list", None), "mcp-method"),
            rpc(
                json!(41),
                "tools/list",
                Some(json!({ "_meta": modern_meta() })),
            ),
        ),
        case(
            "modern-mcp-method-mismatch-400",
            modern_headers("prompts/list", None),
            rpc(
                json!(42),
                "tools/list",
                Some(json!({ "_meta": modern_meta() })),
            ),
        ),
        case(
            "modern-call-missing-mcp-name-400",
            modern_headers("tools/call", None),
            rpc(
                json!(43),
                "tools/call",
                Some(json!({ "name": "echo", "arguments": {}, "_meta": modern_meta() })),
            ),
        ),
        case(
            "modern-call-bad-base64-name-400",
            modern_headers("tools/call", Some("=?base64?***?=")),
            rpc(
                json!(44),
                "tools/call",
                Some(json!({ "name": "echo", "arguments": {}, "_meta": modern_meta() })),
            ),
        ),
        case(
            "modern-call-base64-name",
            modern_headers("tools/call", Some("=?base64?cHJvZmlsZQ==?=")),
            rpc(
                json!(45),
                "tools/call",
                Some(json!({ "name": "profile", "arguments": {}, "_meta": modern_meta() })),
            ),
        ),
        case(
            "modern-call-watch-progress",
            modern_headers("tools/call", Some("watch")),
            rpc(
                json!(46),
                "tools/call",
                Some(json!({
                    "name": "watch",
                    "arguments": {},
                    "_meta": { "progressToken": 7, "io.modelcontextprotocol/protocolVersion": MODERN, "io.modelcontextprotocol/clientCapabilities": {} },
                })),
            ),
        ),
        case(
            "modern-call-fail",
            modern_headers("tools/call", Some("fail")),
            rpc(
                json!(47),
                "tools/call",
                Some(json!({ "name": "fail", "_meta": modern_meta() })),
            ),
        ),
        case(
            "modern-call-unknown-tool-400",
            modern_headers("tools/call", Some("missing")),
            rpc(
                json!(48),
                "tools/call",
                Some(json!({ "name": "missing", "_meta": modern_meta() })),
            ),
        ),
        case(
            "meta-version-without-header-400",
            base_headers(),
            rpc(
                json!(49),
                "tools/list",
                Some(json!({ "_meta": modern_meta() })),
            ),
        ),
        case(
            "meta-version-header-mismatch-400",
            with(base_headers(), &[("mcp-protocol-version", "2025-11-25")]),
            rpc(
                json!(50),
                "tools/list",
                Some(json!({ "_meta": modern_meta() })),
            ),
        ),
        case(
            "meta-unsupported-version-400",
            with(
                base_headers(),
                &[
                    ("mcp-protocol-version", "2099-01-01"),
                    ("mcp-method", "tools/list"),
                ],
            ),
            rpc(
                json!(51),
                "tools/list",
                Some(json!({ "_meta": {
                    "io.modelcontextprotocol/protocolVersion": "2099-01-01",
                    "io.modelcontextprotocol/clientCapabilities": {},
                } })),
            ),
        ),
        case(
            "meta-missing-capabilities-400",
            modern_headers("tools/list", None),
            rpc(
                json!(52),
                "tools/list",
                Some(json!({ "_meta": { "io.modelcontextprotocol/protocolVersion": MODERN } })),
            ),
        ),
        case(
            "meta-older-version-per-request",
            with(base_headers(), &[("mcp-protocol-version", "2025-11-25")]),
            rpc(
                json!(53),
                "tools/list",
                Some(json!({ "_meta": {
                    "io.modelcontextprotocol/protocolVersion": "2025-11-25",
                } })),
            ),
        ),
        case(
            "discover",
            modern_headers("server/discover", None),
            rpc(
                json!(54),
                "server/discover",
                Some(json!({ "_meta": modern_meta() })),
            ),
        ),
        case(
            "discover-without-meta-400",
            modern_headers("server/discover", None),
            rpc(json!(55), "server/discover", Some(json!({}))),
        ),
        case(
            "discover-legacy-header",
            legacy("2025-11-25"),
            rpc(
                json!(56),
                "server/discover",
                Some(json!({ "_meta": {
                "io.modelcontextprotocol/protocolVersion": "2025-11-25",
            } })),
            ),
        ),
        case(
            "modern-ping-404",
            modern_headers("ping", None),
            rpc(json!(57), "ping", Some(json!({ "_meta": modern_meta() }))),
        ),
        case(
            "modern-unknown-method-404",
            modern_headers("foo/bar", None),
            rpc(
                json!(58),
                "foo/bar",
                Some(json!({ "_meta": modern_meta() })),
            ),
        ),
        case(
            "modern-completion",
            modern_headers("completion/complete", None),
            rpc(
                json!(59),
                "completion/complete",
                Some(json!({
                    "_meta": modern_meta(),
                    "ref": { "type": "ref/prompt", "name": "x" },
                    "argument": { "name": "a", "value": "b" },
                })),
            ),
        ),
        case(
            "modern-prompts-list",
            modern_headers("prompts/list", None),
            rpc(
                json!(60),
                "prompts/list",
                Some(json!({ "_meta": modern_meta() })),
            ),
        ),
        case(
            "modern-resources-list",
            modern_headers("resources/list", None),
            rpc(
                json!(61),
                "resources/list",
                Some(json!({ "_meta": modern_meta() })),
            ),
        ),
        case(
            "modern-resource-templates-list",
            modern_headers("resources/templates/list", None),
            rpc(
                json!(62),
                "resources/templates/list",
                Some(json!({ "_meta": modern_meta() })),
            ),
        ),
        case(
            "modern-resources-read-404",
            modern_headers("resources/read", Some("file:///x")),
            rpc(
                json!(63),
                "resources/read",
                Some(json!({ "uri": "file:///x", "_meta": modern_meta() })),
            ),
        ),
        case(
            "modern-header-legacy-body",
            modern_headers("tools/list", None),
            rpc(json!(64), "tools/list", None),
        ),
        case(
            "modern-header-notification-missing-method-400",
            with(base_headers(), &[("mcp-protocol-version", MODERN)]),
            br#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#.to_vec(),
        ),
        // Progressive discovery.
        progressive(case(
            "progressive-tools-list",
            legacy("2025-11-25"),
            rpc(json!(70), "tools/list", None),
        )),
        progressive(case(
            "progressive-search",
            legacy("2025-11-25"),
            call(
                json!(71),
                "search_tools",
                json!({ "query": "", "limit": 3 }),
            ),
        )),
        progressive(case(
            "progressive-search-offset",
            legacy("2025-11-25"),
            call(
                json!(72),
                "search_tools",
                json!({ "query": "s", "offset": 1 }),
            ),
        )),
        progressive(case(
            "progressive-details",
            legacy("2025-11-25"),
            call(json!(73), "get_tool_details", json!({ "name": "profile" })),
        )),
        progressive(case(
            "progressive-details-missing-name",
            legacy("2025-11-25"),
            call(json!(74), "get_tool_details", json!({})),
        )),
        progressive(case(
            "progressive-call-read",
            legacy("2025-11-25"),
            call(
                json!(75),
                "call_read_tool",
                json!({ "name": "echo", "arguments": { "message": "x" } }),
            ),
        )),
        progressive(case(
            "progressive-call-read-on-writable",
            legacy("2025-11-25"),
            call(json!(76), "call_read_tool", json!({ "name": "profile" })),
        )),
        progressive(case(
            "progressive-call-write-on-read-only",
            legacy("2025-11-25"),
            call(json!(77), "call_write_tool", json!({ "name": "echo" })),
        )),
        progressive(case(
            "progressive-call-write",
            legacy("2025-11-25"),
            call(json!(78), "call_write_tool", json!({ "name": "suggest" })),
        )),
        progressive(case(
            "progressive-call-unknown",
            legacy("2025-11-25"),
            call(json!(79), "call_write_tool", json!({ "name": "missing" })),
        )),
        progressive(case(
            "progressive-direct-name-rejected",
            legacy("2025-11-25"),
            call(json!(80), "profile", json!({})),
        )),
        progressive(case(
            "progressive-modern-tools-list",
            modern_headers("tools/list", None),
            rpc(
                json!(81),
                "tools/list",
                Some(json!({ "_meta": modern_meta() })),
            ),
        )),
        progressive(case(
            "progressive-modern-call-missing-name-400",
            modern_headers("tools/call", Some("call_write_tool")),
            rpc(
                json!(82),
                "tools/call",
                Some(json!({ "name": "call_write_tool", "arguments": {}, "_meta": modern_meta() })),
            ),
        )),
    ];
    for (index, version) in STANDARDS.iter().enumerate() {
        let names = [
            "initialize-2024-11-05",
            "initialize-2025-03-26",
            "initialize-2025-06-18",
            "initialize-2025-11-25",
            "initialize-2026-07-28",
        ];
        let lists = [
            "tools-list-2024-11-05",
            "tools-list-2025-03-26",
            "tools-list-2025-06-18",
            "tools-list-2025-11-25",
            "tools-list-header-2026-07-28",
        ];
        cases.push(case(names[index], legacy(version), initialize(version)));
        cases.push(case(
            lists[index],
            legacy(version),
            rpc(json!(100 + index), "tools/list", None),
        ));
    }
    cases
}

// ---------------------------------------------------------------------------
// Observation
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, serde::Serialize)]
struct Observation {
    status: u16,
    content_type: Option<String>,
    allow: Option<String>,
    text: Option<String>,
    messages: Vec<Value>,
}

fn observe(status: u16, headers: &[(String, String)], body: &[u8]) -> Observation {
    let header = |name: &str| {
        headers
            .iter()
            .find(|(existing, _)| existing.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.clone())
    };
    let content_type = header("content-type");
    let text = String::from_utf8_lossy(body).to_string();
    let mut messages = Vec::new();
    let mut plain = None;
    match content_type.as_deref() {
        Some("text/event-stream") => {
            for event in text.split("\n\n") {
                let data = event
                    .lines()
                    .filter_map(|line| line.strip_prefix("data:"))
                    .map(|line| line.strip_prefix(' ').unwrap_or(line))
                    .collect::<Vec<_>>()
                    .join("\n");
                if !data.is_empty() {
                    messages
                        .push(serde_json::from_str(&data).unwrap_or_else(|error| {
                            panic!("invalid SSE data ({error}): {data:?}")
                        }));
                }
            }
        }
        Some("application/json") => {
            messages.push(serde_json::from_slice(body).expect("JSON body"));
        }
        _ => {
            // Normalize the serde error detail: the message family is pinned,
            // the parser's column-level wording is not.
            let prefix = "fail to deserialize request body";
            plain = Some(if text.starts_with(prefix) {
                prefix.to_string()
            } else {
                text
            });
        }
    }
    Observation {
        status,
        content_type,
        allow: header("allow"),
        text: plain,
        messages,
    }
}

async fn observe_native(cli: &Cli, case: &Case) -> Observation {
    let mut builder = axum::http::Request::builder()
        .method(case.method)
        .uri("/mcp");
    for (name, value) in &case.headers {
        builder = builder.header(name, value);
    }
    let request = builder.body(Body::from(case.body.clone())).unwrap();
    let response = super::http_service(cli)
        .unwrap()
        .oneshot(request)
        .await
        .unwrap();
    let status = response.status().as_u16();
    let headers = response
        .headers()
        .iter()
        .map(|(name, value)| {
            (
                name.as_str().to_string(),
                value.to_str().unwrap_or_default().to_string(),
            )
        })
        .collect::<Vec<_>>();
    let body = axum::body::to_bytes(Body::new(response.into_body()), usize::MAX)
        .await
        .unwrap();
    observe(status, &headers, &body)
}

async fn portable_response_bytes(body: McpHttpBody) -> Vec<u8> {
    match body {
        McpHttpBody::Empty => Vec::new(),
        McpHttpBody::Full(bytes) => bytes,
        McpHttpBody::EventStream(stream) => stream.collect::<Vec<_>>().await.concat().into_bytes(),
    }
}

async fn observe_portable(server: &McpHttpServer, case: &Case) -> Observation {
    let response = server
        .handle(McpHttpRequest {
            method: case.method.to_string(),
            path: "/mcp".to_string(),
            headers: case.headers.clone(),
            body: case.body.clone(),
        })
        .await;
    let body = portable_response_bytes(response.body).await;
    observe(response.status, &response.headers, &body)
}

fn portable_config() -> McpHttpConfig {
    McpHttpConfig {
        environment: EnvironmentSource::Values(HashMap::from([(
            ENV_NAME.to_string(),
            ENV_VALUE.to_string(),
        )])),
        ..McpHttpConfig::default()
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn portable_server_matches_native_rmcp_service() {
    // SAFETY: this is the only test in the crate that reads this variable.
    unsafe { std::env::set_var(ENV_NAME, ENV_VALUE) };
    let direct = fixture_cli(McpDiscovery::Direct);
    let progressive = fixture_cli(McpDiscovery::Progressive);
    let portable_direct = McpHttpServer::from_cli(&direct, portable_config()).unwrap();
    let portable_progressive = McpHttpServer::from_cli(&progressive, portable_config()).unwrap();

    let corpus = corpus();
    assert!(corpus.len() >= 40, "corpus has {} cases", corpus.len());
    let mut failures = Vec::new();
    let mut permitted = Vec::new();
    let mut dump = serde_json::Map::new();
    for case in &corpus {
        let (cli, portable) = match case.fixture {
            Fixture::Direct => (&direct, &portable_direct),
            Fixture::Progressive => (&progressive, &portable_progressive),
        };
        let expected = observe_native(cli, case).await;
        let actual = observe_portable(portable, case).await;
        dump.insert(
            case.name.to_string(),
            json!({ "native": expected, "portable": actual }),
        );
        if PERMITTED_DIFFERENCES.contains(&case.name) {
            if expected != actual {
                permitted.push(case.name);
            }
            continue;
        }
        if expected != actual {
            failures.push(format!(
                "{}:\n  native:   {}\n  portable: {}",
                case.name,
                serde_json::to_string(&expected).unwrap(),
                serde_json::to_string(&actual).unwrap(),
            ));
        }
    }
    if let Ok(path) = std::env::var("INCURS_PARITY_DUMP") {
        std::fs::write(path, serde_json::to_string_pretty(&dump).unwrap()).unwrap();
    }
    assert!(
        failures.is_empty(),
        "{} of {} cases differ:\n{}",
        failures.len(),
        corpus.len(),
        failures.join("\n")
    );
    // The permitted differences must actually differ: the portable server
    // rejects them, native rmcp does not.
    assert_eq!(permitted, PERMITTED_DIFFERENCES.to_vec());
    for name in PERMITTED_DIFFERENCES {
        let observed = &dump[*name]["portable"];
        assert!(
            observed["status"] == 403 || observed["status"] == 400,
            "{name}: {observed}"
        );
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn modern_protocol_is_discovered_not_initialized() {
    let cli = fixture_cli(McpDiscovery::Direct);
    let portable = McpHttpServer::from_cli(&cli, portable_config()).unwrap();

    let initialize_modern = case(
        "initialize-modern",
        with(base_headers(), &[("mcp-protocol-version", MODERN)]),
        initialize(MODERN),
    );
    let native_initialize = observe_native(&cli, &initialize_modern).await;
    let portable_initialize = observe_portable(&portable, &initialize_modern).await;
    for observed in [&native_initialize, &portable_initialize] {
        let supported = observed.messages[0]["error"]["data"]["supported"]
            .as_array()
            .expect("unsupported protocol response lists initialize versions");
        assert!(
            supported.iter().all(|version| version != MODERN),
            "initialize advertised modern version: {observed:#?}"
        );
        assert!(
            supported.iter().any(|version| version == "2025-11-25"),
            "initialize did not advertise a legacy version: {observed:#?}"
        );
    }

    let discover_modern = case(
        "discover-modern",
        modern_headers("server/discover", None),
        rpc(
            json!(54),
            "server/discover",
            Some(json!({ "_meta": modern_meta() })),
        ),
    );
    let native_discover = observe_native(&cli, &discover_modern).await;
    let portable_discover = observe_portable(&portable, &discover_modern).await;
    for observed in [&native_discover, &portable_discover] {
        let supported = observed.messages[0]["result"]["supportedVersions"]
            .as_array()
            .expect("discover response lists supported versions");
        assert!(
            supported.iter().any(|version| version == MODERN),
            "discover did not advertise modern version: {observed:#?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Portable-only behaviour
// ---------------------------------------------------------------------------

struct Hang {
    dropped: Arc<AtomicBool>,
}

struct SetOnDrop(Arc<AtomicBool>);

impl Drop for SetOnDrop {
    fn drop(&mut self) {
        self.0.store(true, Ordering::SeqCst);
    }
}

#[async_trait::async_trait]
impl CommandHandler for Hang {
    async fn run(&self, _ctx: CommandContext) -> CommandResult {
        let _guard = SetOnDrop(Arc::clone(&self.dropped));
        std::future::pending::<()>().await;
        unreachable!("pending never resolves")
    }
}

#[tokio::test]
async fn dropping_the_event_stream_cancels_the_tool_call() {
    let dropped = Arc::new(AtomicBool::new(false));
    let cli = Cli::create("hang")
        .mcp(McpServeOptions {
            tools: McpToolFilter {
                discovery: McpDiscovery::Direct,
                ..Default::default()
            },
            ..Default::default()
        })
        .command(
            "hang",
            CommandDef::build(
                "hang",
                Hang {
                    dropped: Arc::clone(&dropped),
                },
            )
            .done(),
        );
    let server = McpHttpServer::from_cli(&cli, McpHttpConfig::default()).unwrap();
    let response = server
        .handle(McpHttpRequest {
            method: "POST".to_string(),
            path: "/mcp".to_string(),
            headers: base_headers(),
            body: call(json!(1), "hang", json!({})),
        })
        .await;
    let McpHttpBody::EventStream(mut stream) = response.body else {
        panic!("expected an event stream");
    };
    // Drive the call until the handler is running and parked.
    let next = futures::poll!(stream.next());
    assert!(next.is_pending());
    assert!(!dropped.load(Ordering::SeqCst));
    drop(stream);
    assert!(dropped.load(Ordering::SeqCst));
}

#[tokio::test]
async fn configured_environment_replaces_the_process_environment() {
    let cli = fixture_cli(McpDiscovery::Direct);
    let text = |config: McpHttpConfig| {
        let server = McpHttpServer::from_cli(&cli, config).unwrap();
        async move {
            let case = case(
                "whoami",
                base_headers(),
                call(json!(1), "whoami", json!({})),
            );
            let observed = observe_portable(&server, &case).await;
            observed.messages[0]["result"]["content"][0]["text"]
                .as_str()
                .unwrap()
                .to_string()
        }
    };
    let configured = text(McpHttpConfig {
        environment: EnvironmentSource::Values(HashMap::from([(
            ENV_NAME.to_string(),
            "from-config".to_string(),
        )])),
        ..McpHttpConfig::default()
    })
    .await;
    assert_eq!(configured, r#"{"env":{"token":"from-config"}}"#);
    let empty = text(McpHttpConfig {
        environment: EnvironmentSource::Empty,
        ..McpHttpConfig::default()
    })
    .await;
    assert_eq!(empty, r#"{"env":{}}"#);
}

#[test]
fn default_origins_follow_allowed_hosts() {
    let config = McpHttpConfig::default();
    assert_eq!(config.allowed_hosts, vec!["localhost", "127.0.0.1", "::1"]);
    assert!(config.allowed_origins.is_none());
}

struct OutputValue(Value);

#[async_trait::async_trait]
impl CommandHandler for OutputValue {
    async fn run(&self, _ctx: CommandContext) -> CommandResult {
        CommandResult::Ok {
            data: self.0.clone(),
            cta: None,
            exit_code: None,
        }
    }
}

async fn output_boundary(cli: &Cli, request: Vec<u8>) -> [Value; 2] {
    output_boundary_version(cli, request, "2025-06-18").await
}

async fn output_boundary_version(cli: &Cli, request: Vec<u8>, version: &str) -> [Value; 2] {
    let mut message: Value = serde_json::from_slice(&request).unwrap();
    let headers = if version == "2026-07-28" {
        message["params"]["_meta"] = modern_meta();
        modern_headers(
            message["method"].as_str().unwrap(),
            message["params"]["name"].as_str(),
        )
    } else {
        with(base_headers(), &[("mcp-protocol-version", version)])
    };
    let case = case(
        "output-boundary",
        headers,
        serde_json::to_vec(&message).unwrap(),
    );
    let portable = McpHttpServer::from_cli(cli, portable_config()).unwrap();
    let native = observe_native(cli, &case).await;
    let hosted = observe_portable(&portable, &case).await;
    assert_eq!(native.status, 200);
    assert_eq!(hosted.status, 200);
    [
        native.messages.last().expect("native response")["result"].clone(),
        hosted.messages.last().expect("portable response")["result"].clone(),
    ]
}

#[tokio::test]
async fn mcp_output_boundary_projects_media_with_literal_wire_expectations() {
    let data = json!({
        "audio": {"bytes": "YXVkaW8=", "mime": "audio/wav"},
        "link": {"uri": "https://example.test/clip.wav", "name": "clip.wav", "mime": "audio/wav"}
    });
    let command = || {
        let mut command = CommandDef::build("media", OutputValue(data.clone()))
            .mcp(McpCommandOptions {
                result_content: vec![
                    McpResultContent::Audio {
                        data_pointer: "/audio/bytes".into(),
                        mime_type_pointer: "/audio/mime".into(),
                    },
                    McpResultContent::ResourceLink {
                        uri_pointer: "/link/uri".into(),
                        name_pointer: "/link/name".into(),
                        mime_type_pointer: "/link/mime".into(),
                    },
                    McpResultContent::Audio {
                        data_pointer: "/missing".into(),
                        mime_type_pointer: "/audio/mime".into(),
                    },
                    McpResultContent::ResourceLink {
                        uri_pointer: "/link/uri".into(),
                        name_pointer: "/audio".into(),
                        mime_type_pointer: "/link/mime".into(),
                    },
                ],
                ..Default::default()
            })
            .done();
        command.output_schema = Some(json!({"type": "object"}));
        command
    };
    for discovery in [McpDiscovery::Direct, McpDiscovery::Progressive] {
        let cli = Cli::create("media")
            .mcp(McpServeOptions {
                tools: McpToolFilter {
                    discovery,
                    ..Default::default()
                },
                ..Default::default()
            })
            .command("media", command());
        let (name, args) = if discovery == McpDiscovery::Direct {
            ("media", json!({}))
        } else {
            ("call_write_tool", json!({"name": "media", "arguments": {}}))
        };
        for (version, block_count) in [
            ("2024-11-05", 1),
            ("2025-03-26", 2),
            ("2025-06-18", 3),
            ("2025-11-25", 3),
            ("2026-07-28", 3),
        ] {
            for result in
                output_boundary_version(&cli, call(json!(301), name, args.clone()), version).await
            {
                assert_eq!(result["isError"], false);
                assert_eq!(result["structuredContent"], data);
                assert_eq!(
                    result["content"].as_array().unwrap().len(),
                    block_count,
                    "{version}"
                );
                if block_count >= 2 {
                    assert_eq!(
                        result["content"][1],
                        json!({"type": "audio", "data": "YXVkaW8=", "mimeType": "audio/wav"})
                    );
                }
                if block_count >= 3 {
                    assert_eq!(
                        result["content"][2],
                        json!({
                            "type": "resource_link", "uri": "https://example.test/clip.wav",
                            "name": "clip.wav", "mimeType": "audio/wav"
                        })
                    );
                }
            }
        }
    }
}

#[tokio::test]
async fn mcp_output_boundary_wraps_json_roots_and_preserves_mapper_contracts() {
    let cases = [
        (
            json!({"type": "array", "items": {"type": "string"}}),
            json!(["Ada"]),
        ),
        (json!({"type": "string"}), json!("Ada")),
        (json!({"type": "integer"}), json!(42)),
        (json!({"type": "boolean"}), json!(false)),
        (json!({"type": "null"}), Value::Null),
        (json!({"type": ["object", "null"]}), Value::Null),
    ];
    let marker = json!({"io.incurs.outputProjection": {
        "version": 1, "shape": "value-wrapper", "field": "data", "schemaRefBase": "#/properties/data"
    }});
    for (schema, data) in cases {
        for discovery in [McpDiscovery::Direct, McpDiscovery::Progressive] {
            let command = || {
                let mut command = CommandDef::build("value", OutputValue(data.clone())).done();
                command.output_schema = Some(schema.clone());
                command
            };
            let base = McpServeOptions {
                tools: McpToolFilter {
                    discovery,
                    ..Default::default()
                },
                ..Default::default()
            };
            let unchanged = Cli::create("values")
                .mcp(base.clone())
                .command("value", command());
            let original_schema = schema.clone();
            let mapped = Cli::create("values")
                .mcp(McpServeOptions {
                    result_mapper: Some(super::McpResultMapper::new(move |context| {
                        assert_eq!(context.tool.name, "value");
                        assert_eq!(context.tool.output_schema.as_ref(), Some(&original_schema));
                        assert!(matches!(
                            context.outcome,
                            crate::tool::ToolCallOutcome::Ok { .. }
                        ));
                        super::McpResultMapping::error()
                    })),
                    ..base
                })
                .command("value", command());
            let discovery_request = if discovery == McpDiscovery::Direct {
                rpc(json!(302), "tools/list", None)
            } else {
                call(json!(302), "get_tool_details", json!({"name": "value"}))
            };
            let catalog = |cli: &Cli| {
                crate::tool::ToolCatalog::from_parts(
                    cli.name.clone(),
                    cli.version.clone(),
                    &cli.commands,
                    &cli.middleware,
                    &cli.env_fields,
                    &cli.globals_fields,
                    cli.config.as_ref(),
                )
                .unwrap()
            };
            let original_definitions = catalog(&unchanged).definitions();
            assert_eq!(
                original_definitions[0].output_schema.as_ref(),
                Some(&schema)
            );
            assert_eq!(
                serde_json::to_value(original_definitions).unwrap(),
                serde_json::to_value(catalog(&mapped).definitions()).unwrap(),
                "mapper must preserve all catalog identity, schema, annotation and instruction metadata"
            );
            let listed = output_boundary(&unchanged, discovery_request.clone()).await;
            let mapped_listed = output_boundary(&mapped, discovery_request).await;
            assert_eq!(
                listed, mapped_listed,
                "mapper must preserve tool identity and schemas"
            );
            for response in listed {
                let tool = if discovery == McpDiscovery::Direct {
                    response["tools"][0].clone()
                } else {
                    response["structuredContent"].clone()
                };
                assert_eq!(
                    tool["outputSchema"],
                    json!({
                        "type": "object", "properties": {"data": schema},
                        "required": ["data"], "additionalProperties": false
                    })
                );
                assert_eq!(tool["_meta"], marker);
            }
            let (name, args) = if discovery == McpDiscovery::Direct {
                ("value", json!({}))
            } else {
                ("call_write_tool", json!({"name": "value", "arguments": {}}))
            };
            let request = call(json!(303), name, args);
            let normal = output_boundary(&unchanged, request.clone()).await;
            let mapped = output_boundary(&mapped, request).await;
            for (normal, mut mapped) in normal.into_iter().zip(mapped) {
                assert_eq!(normal["structuredContent"], json!({"data": data}));
                assert_eq!(normal["_meta"], marker);
                assert_eq!(normal["isError"], false);
                assert_eq!(mapped["isError"], true);
                mapped["isError"] = json!(false);
                assert_eq!(normal, mapped, "mapper must preserve rendering");
            }
        }
    }
}

#[tokio::test]
async fn mcp_output_boundary_rejects_wrong_object_shape_with_coded_error() {
    let mut command = CommandDef::build("value", OutputValue(json!(["wrong"]))).done();
    command.output_schema = Some(json!({"type": "object"}));
    let cli = Cli::create("values")
        .mcp(McpServeOptions {
            tools: McpToolFilter {
                discovery: McpDiscovery::Direct,
                ..Default::default()
            },
            ..Default::default()
        })
        .command("value", command);
    for result in output_boundary(&cli, call(json!(304), "value", json!({}))).await {
        assert_eq!(result["isError"], true);
        let error: Value =
            serde_json::from_str(result["content"][0]["text"].as_str().unwrap()).unwrap();
        assert_eq!(error["code"], "MCP_OUTPUT_SHAPE_INVALID");
        assert_eq!(error["retryable"], false);
        assert_eq!(result.get("_meta"), None);
    }
}

#[tokio::test]
async fn mcp_output_boundary_rejects_unsupported_wrapping_dialects() {
    let schema = json!({
        "$schema": "https://json-schema.org/draft/2019-09/schema",
        "type": "array",
        "items": {"anyOf": [{"type": "integer"}, {"$recursiveRef": "#"}]}
    });
    let mut command = CommandDef::build("recursive", OutputValue(json!([1, [2]]))).done();
    command.output_schema = Some(schema.clone());
    let cli = Cli::create("recursive")
        .mcp(McpServeOptions {
            tools: McpToolFilter {
                discovery: McpDiscovery::Direct,
                ..Default::default()
            },
            ..Default::default()
        })
        .command("recursive", command);
    let portable = match McpHttpServer::from_cli(&cli, portable_config()) {
        Err(error) => error,
        Ok(_) => panic!("portable server must reject unsupported relocation"),
    };
    let native = match super::http_service(&cli) {
        Err(error) => error,
        Ok(_) => panic!("native server must reject unsupported relocation"),
    };
    for error in [portable, native] {
        let crate::errors::Error::Incur(error) = error else {
            panic!("dialect rejection must preserve a machine-readable error");
        };
        assert_eq!(error.code, "MCP_OUTPUT_SCHEMA_DIALECT_UNSUPPORTED");
        assert!(!error.retryable);
        assert!(
            error
                .message
                .contains("https://json-schema.org/draft/2019-09/schema")
        );
    }

    let mut hidden = CommandDef::build("recursive", OutputValue(json!([1, [2]]))).done();
    hidden.output_schema = Some(schema);
    let filtered = Cli::create("filtered")
        .mcp(McpServeOptions {
            tools: McpToolFilter {
                discovery: McpDiscovery::Direct,
                exclude: vec!["recursive".to_string()],
                ..Default::default()
            },
            ..Default::default()
        })
        .command("recursive", hidden);
    for listed in
        output_boundary_version(&filtered, rpc(json!(307), "tools/list", None), "2025-11-25").await
    {
        assert_eq!(
            listed["tools"],
            json!([]),
            "excluded schemas are never projected"
        );
    }

    // The policy applies only when wrapping relocates the schema.
    let schema = json!({
        "$schema": "https://json-schema.org/draft/2019-09/schema",
        "type": "object",
        "properties": {"children": {"type": "array", "items": {"$recursiveRef": "#"}}}
    });
    let data = json!({"children": [{"children": []}]});
    let mut command = CommandDef::build("object", OutputValue(data.clone())).done();
    command.output_schema = Some(schema.clone());
    let cli = Cli::create("object")
        .mcp(McpServeOptions {
            tools: McpToolFilter {
                discovery: McpDiscovery::Direct,
                ..Default::default()
            },
            ..Default::default()
        })
        .command("object", command);
    for listed in
        output_boundary_version(&cli, rpc(json!(305), "tools/list", None), "2025-11-25").await
    {
        assert_eq!(listed["tools"][0]["outputSchema"], schema);
        assert!(listed["tools"][0].get("_meta").is_none());
    }
    for called in
        output_boundary_version(&cli, call(json!(306), "object", json!({})), "2025-11-25").await
    {
        assert_eq!(called["structuredContent"], data);
        assert_eq!(called["isError"], false);
    }
}
