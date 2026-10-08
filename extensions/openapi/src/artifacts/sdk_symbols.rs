//! Render-only Rust symbol allocation for generated SDK artifacts.

use super::{CLIENT_METHOD_NAMES, pascal, snake, unique_names};
use crate::{OpenApiResult, Operation, ResolvedOpenApi};
use std::collections::{BTreeMap, BTreeSet};

/// Allocated symbols for one generated operation.
pub(super) struct OperationSymbols {
    pub(super) args_type: String,
    pub(super) response_type: String,
    pub(super) body_type: Option<String>,
    pub(super) method: String,
}

/// Render-only symbol table for generated public Rust names.
pub(super) struct SdkSymbols {
    schemas: BTreeMap<String, String>,
    operations: BTreeMap<String, OperationSymbols>,
}

impl SdkSymbols {
    /// Allocate every public SDK name without changing contract graph keys.
    pub(super) fn new(contract: &ResolvedOpenApi) -> OpenApiResult<Self> {
        let mut keys = Vec::new();
        let mut bases = Vec::new();
        for name in contract.schemas.keys() {
            keys.push(SymbolKey::Schema(name.clone()));
            bases.push(pascal(name));
        }
        for operation in &contract.operations {
            let base = pascal(&operation.name);
            keys.push(SymbolKey::OperationArgs(operation.id.clone()));
            bases.push(format!("{base}Args"));
            keys.push(SymbolKey::OperationResponse(operation.id.clone()));
            bases.push(format!("{base}Response"));
            if super::media::needs_enum(operation, &contract.schemas)? {
                keys.push(SymbolKey::OperationBody(operation.id.clone()));
                bases.push(format!("{base}Body"));
            }
        }

        let names = unique_names(bases, RESERVED_TYPE_NAMES, "");
        let mut schemas = BTreeMap::new();
        let mut operations = contract
            .operations
            .iter()
            .map(|operation| {
                (
                    operation.id.clone(),
                    OperationSymbols {
                        args_type: String::new(),
                        response_type: String::new(),
                        body_type: None,
                        method: String::new(),
                    },
                )
            })
            .collect::<BTreeMap<_, _>>();
        for (key, name) in keys.into_iter().zip(names) {
            match key {
                SymbolKey::Schema(key) => {
                    schemas.insert(key, name);
                }
                SymbolKey::OperationArgs(id) => operations.get_mut(&id).unwrap().args_type = name,
                SymbolKey::OperationResponse(id) => {
                    operations.get_mut(&id).unwrap().response_type = name;
                }
                SymbolKey::OperationBody(id) => {
                    operations.get_mut(&id).unwrap().body_type = Some(name)
                }
            }
        }

        let method_bases = contract
            .operations
            .iter()
            .map(|operation| operation_method_base(&operation.name))
            .collect();
        for (operation, method) in
            contract
                .operations
                .iter()
                .zip(unique_names(method_bases, CLIENT_METHOD_NAMES, "_"))
        {
            operations.get_mut(&operation.id).unwrap().method = method;
        }

        Ok(Self {
            schemas,
            operations,
        })
    }

    /// Return allocated and runtime names for additive generator modules.
    pub(super) fn reserved_names(&self) -> BTreeSet<String> {
        let mut names = RESERVED_TYPE_NAMES
            .iter()
            .map(|name| (*name).to_owned())
            .collect::<BTreeSet<_>>();
        names.extend(self.schemas.values().cloned());
        for operation in self.operations.values() {
            names.insert(operation.args_type.clone());
            names.insert(operation.response_type.clone());
            names.extend(operation.body_type.iter().cloned());
        }
        names
    }

    /// Return the Rust type name allocated to a schema key.
    pub(super) fn schema_type(&self, name: &str) -> &str {
        self.schemas
            .get(name)
            .map(String::as_str)
            .unwrap_or_else(|| panic!("missing SDK schema symbol for {name}"))
    }

    /// Return the Rust symbols allocated to an operation.
    pub(super) fn operation(&self, operation: &Operation) -> &OperationSymbols {
        self.operations
            .get(&operation.id)
            .unwrap_or_else(|| panic!("missing SDK operation symbol for {}", operation.id))
    }
}

enum SymbolKey {
    Schema(String),
    OperationArgs(String),
    OperationResponse(String),
    OperationBody(String),
}

fn operation_method_base(name: &str) -> String {
    let mut out = snake(name);
    if CLIENT_METHOD_NAMES.contains(&out.as_str()) {
        out.push_str("_operation");
    }
    out
}

const RESERVED_TYPE_NAMES: &[&str] = &[
    "Box",
    "Client",
    "ClientError",
    "DecodedResponse",
    "DecodeResponseValue",
    "ResponseDecodeError",
    "ResponseDecodeErrorKind",
    "ResponseInteger",
    "ResponseNumber",
    "Err",
    "Field",
    "IntoJson",
    "JsonEncodeError",
    "JsonValue",
    "Never",
    "None",
    "Ok",
    "OperationRequest",
    "OperationResponse",
    "Option",
    "Result",
    "Some",
    "String",
    "Transport",
    "Vec",
];
