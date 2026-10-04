//! Server selection for resolved operations, separate from explicit host overrides.
use crate::{
    ForgeError, ForgeResult, Operation, Server,
    runtime::{ForgeHttpRequest, ForgeHttpResponse, ForgeTransport, build_http_request},
};
use serde_json::Value;
use std::collections::BTreeMap;

/// Explicit selection of a declared server and its template variables.
#[derive(Clone, Debug, Default)]
pub struct ServerSelection {
    /// Zero-based index in the operation's effective server list.
    pub index: usize,
    /// Variable replacements; omitted variables use their declared defaults.
    pub variables: BTreeMap<String, String>,
    /// Absolute OpenAPI document URL for resolving a relative server URL.
    ///
    /// Hosts loading a local file supply the intended document URL explicitly.
    pub document_url: Option<String>,
}

/// Select and expand a server after operation/path/root inheritance was resolved.
pub fn select_server_url(
    operation: &Operation,
    selection: &ServerSelection,
) -> ForgeResult<String> {
    let fallback = Server::default();
    let server = if operation.servers.is_empty() && selection.index == 0 {
        &fallback
    } else {
        operation.servers.get(selection.index).ok_or_else(|| {
            ForgeError(format!(
                "server index {} is out of range for operation {}",
                selection.index, operation.id
            ))
        })?
    };
    for name in selection.variables.keys() {
        if !server.variables.contains_key(name) {
            return Err(ForgeError(format!("unknown server variable: {name}")));
        }
    }
    let mut expanded = String::new();
    let mut rest = server.url.as_str();
    while let Some(start) = rest.find('{') {
        expanded.push_str(&rest[..start]);
        let tail = &rest[start + 1..];
        let end = tail
            .find('}')
            .ok_or_else(|| ForgeError("unclosed server variable".into()))?;
        let name = &tail[..end];
        let variable = server
            .variables
            .get(name)
            .ok_or_else(|| ForgeError(format!("undeclared server variable: {name}")))?;
        let value = selection.variables.get(name).unwrap_or(&variable.default);
        if variable
            .values
            .as_ref()
            .is_some_and(|values| !values.contains(value))
        {
            return Err(ForgeError(format!(
                "server variable {name} is outside its enum"
            )));
        }
        if value.contains(['{', '}']) || value.chars().any(char::is_control) {
            return Err(ForgeError(format!(
                "server variable {name} contains invalid URL characters"
            )));
        }
        expanded.push_str(value);
        rest = &tail[end + 1..];
    }
    expanded.push_str(rest);
    if expanded.contains(['{', '}']) || expanded.chars().any(char::is_control) {
        return Err(ForgeError("invalid server URL template".into()));
    }
    let url = match url::Url::parse(&expanded) {
        Ok(url) => url,
        Err(url::ParseError::RelativeUrlWithoutBase) => {
            let document = selection
                .document_url
                .as_deref()
                .ok_or_else(|| ForgeError("relative server URL requires document_url".into()))?;
            let base = url::Url::parse(document)
                .map_err(|error| ForgeError(format!("invalid document URL: {error}")))?;
            base.join(&expanded)
                .map_err(|error| ForgeError(format!("invalid relative server URL: {error}")))?
        }
        Err(error) => return Err(ForgeError(format!("invalid server URL: {error}"))),
    };
    if !matches!(url.scheme(), "http" | "https")
        || url.host_str().is_none()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(ForgeError(
            "server must be an HTTP URL without query or fragment".into(),
        ));
    }
    Ok(url.to_string())
}

/// Bind an operation using its declared server instead of an explicit URL override.
pub fn build_operation_request(
    operation: &Operation,
    schemas: &std::collections::BTreeMap<String, Value>,
    selection: &ServerSelection,
    arguments: &Value,
) -> ForgeResult<ForgeHttpRequest> {
    build_http_request(
        operation,
        schemas,
        &select_server_url(operation, selection)?,
        arguments,
    )
}

/// Invoke an operation through its selected declared server and a host transport.
pub async fn invoke_operation(
    operation: &Operation,
    schemas: &std::collections::BTreeMap<String, Value>,
    selection: &ServerSelection,
    arguments: &Value,
    transport: &impl ForgeTransport,
) -> ForgeResult<ForgeHttpResponse> {
    transport
        .exchange(build_operation_request(
            operation, schemas, selection, arguments,
        )?)
        .await
}
