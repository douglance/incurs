//! Compiled schema enforcement at the operation HTTP boundary.
use super::{ForgeHttpRequest, ForgeHttpResponse, ForgeTransport, build_http_request};
use crate::{ForgeError, ForgeResult, ResolvedOpenApi};
use serde_json::{Value, json};
use std::collections::BTreeMap;

/// Failure before or during a validated HTTP exchange.
#[derive(Debug, thiserror::Error)]
pub enum HttpBindingError {
    /// Arguments cannot be serialized into the operation's HTTP binding.
    #[error("{0}")]
    Binding(ForgeError),
    /// Logical request arguments violate their selected schema.
    #[error("{0}")]
    Validation(ForgeError),
    /// The host transport could not complete the exchange.
    #[error("{0}")]
    Transport(ForgeError),
}

/// An immutable API contract and its request schemas compiled once for reuse.
///
/// JSON-compatible request values are checked against the operation and selected
/// media schema. Raw binary slots are decoded by the media codec and are not
/// misrepresented as logical JSON strings. Formats remain annotations.
pub struct HttpBinding {
    contract: ResolvedOpenApi,
    operations: BTreeMap<String, usize>,
    inputs: BTreeMap<String, Value>,
    validator: jsonschema::Validator,
}

impl HttpBinding {
    /// Compile offline validation using the source OpenAPI version and dialect.
    pub fn new(contract: &ResolvedOpenApi) -> ForgeResult<Self> {
        let schemas = crate::request_schema::build(contract)?;
        let validator = jsonschema::draft202012::options()
            .offline()
            .should_validate_formats(false)
            .build(&schemas.graph)
            .map_err(|error| ForgeError(format!("invalid request validation graph: {error}")))?;
        Ok(Self {
            contract: contract.clone(),
            operations: contract
                .operations
                .iter()
                .enumerate()
                .map(|(index, operation)| (operation.id.clone(), index))
                .collect(),
            inputs: schemas.inputs,
            validator,
        })
    }

    /// Return the same normalized input schema used by request validation.
    pub fn input_schema(&self, operation_id: &str) -> Option<&Value> {
        self.inputs.get(operation_id)
    }

    /// Bind and validate one request without performing network I/O.
    ///
    /// The server URL is an explicit host selection. Arguments use the binder's
    /// location keys and logical body value, after any CLI JSON decoding.
    /// Empty parameter groups emitted by SDKs are omitted from validation when
    /// the operation declares no parameters at that location.
    pub fn build_request(
        &self,
        operation_id: &str,
        server_url: &str,
        arguments: &Value,
    ) -> Result<ForgeHttpRequest, HttpBindingError> {
        let index = self.operations.get(operation_id).ok_or_else(|| {
            HttpBindingError::Binding(ForgeError(format!("unknown operation: {operation_id}")))
        })?;
        let operation = &self.contract.operations[*index];
        let request = build_http_request(operation, &self.contract.schemas, server_url, arguments)
            .map_err(HttpBindingError::Binding)?;
        let mut logical = arguments.clone();
        let properties = &self.inputs[operation_id]["properties"];
        for location in ["path", "query", "querystring", "header", "cookie"] {
            if properties.get(location).is_none()
                && logical
                    .get(location)
                    .and_then(Value::as_object)
                    .is_some_and(|values| values.is_empty())
            {
                logical.as_object_mut().unwrap().remove(location);
            }
        }
        let instance = json!({operation_id:logical});
        self.validator.validate(&instance).map_err(|error| {
            HttpBindingError::Validation(ForgeError(format!(
                "request validation failed for {operation_id} at {} (schema {})",
                error.instance_path(),
                error.schema_path()
            )))
        })?;
        Ok(request)
    }

    /// Validate and invoke an operation through the caller's transport.
    pub async fn invoke(
        &self,
        operation_id: &str,
        server_url: &str,
        arguments: &Value,
        transport: &impl ForgeTransport,
    ) -> Result<ForgeHttpResponse, HttpBindingError> {
        let request = self.build_request(operation_id, server_url, arguments)?;
        transport
            .exchange(request)
            .await
            .map_err(HttpBindingError::Transport)
    }
}
