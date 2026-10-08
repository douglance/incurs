//! Run an imported OpenAPI CLI or inspect its ToolCatalog definitions.
//!
//! Usage: openapi SPEC.json DOCUMENT_URL [--describe-tools | COMMAND OPTIONS...]
#[cfg(all(feature = "native-cli", not(target_arch = "wasm32")))]
#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<std::process::ExitCode, Box<dyn std::error::Error>> {
    use incurs::{cli::Runtime, outbound::ReqwestHttpClient};
    use incurs_openapi::{
        ResolveOptions, adapters::IncursHttpTransport, catalog::compile_cli, resolve_document,
        servers::ServerSelection,
    };
    use std::sync::Arc;
    let mut arguments = std::env::args().skip(1);
    let path = arguments
        .next()
        .ok_or("usage: openapi SPEC.json DOCUMENT_URL [COMMAND OPTIONS...]")?;
    let document_url = arguments
        .next()
        .ok_or("DOCUMENT_URL is required to bind relative servers")?;
    let document = serde_json::from_slice(&std::fs::read(path)?)?;
    let mut options = ResolveOptions::new("openapi");
    options.document_uri = document_url.clone();
    let contract = resolve_document(&document, options)?;
    let cli = compile_cli(
        &contract,
        IncursHttpTransport::new(Arc::new(ReqwestHttpClient::without_redirects()?)),
        &ServerSelection {
            document_url: Some(document_url),
            ..Default::default()
        },
    )?;
    let arguments: Vec<_> = arguments.collect();
    if arguments.as_slice() == ["--describe-tools"] {
        println!(
            "{}",
            serde_json::to_string_pretty(&cli.tool_catalog().definitions())?
        );
        return Ok(std::process::ExitCode::SUCCESS);
    }
    let code = cli
        .run_to(
            arguments,
            &mut std::io::stdout(),
            Runtime::new("openapi", Default::default(), false),
        )
        .await?;
    Ok(std::process::ExitCode::from(
        u8::try_from(code.unwrap_or(0)).unwrap_or(1),
    ))
}

#[cfg(any(not(feature = "native-cli"), target_arch = "wasm32"))]
fn main() -> Result<(), &'static str> {
    Err("this example requires a native target and the native-cli feature")
}
