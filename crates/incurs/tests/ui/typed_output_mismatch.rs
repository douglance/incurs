use incurs::command::{CommandDef, TypedResult};
use schemars::JsonSchema;
use serde::Serialize;

#[derive(JsonSchema, Serialize)]
struct Expected {
    value: bool,
}

fn main() {
    let _ = CommandDef::typed::<(), (), (), Expected, _, _>("bad", |_ctx| async move {
        TypedResult::ok("wrong output".to_string())
    });
}
