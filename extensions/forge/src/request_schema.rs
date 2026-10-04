//! Request schemas shared by discovery and compiled HTTP validation.
use crate::{ForgeError, ForgeResult, Operation, ResolvedOpenApi};
use serde_json::{Map, Value, json};
use std::collections::{BTreeMap, BTreeSet};

pub(crate) struct RequestSchemas {
    pub(crate) inputs: BTreeMap<String, Value>,
    pub(crate) graph: Value,
}

pub(crate) fn build(contract: &ResolvedOpenApi) -> ForgeResult<RequestSchemas> {
    let mut request_contract = contract.clone();
    let mut pending = BTreeSet::new();
    for operation in &contract.operations {
        for parameter in &operation.parameters {
            crate::schema::references(&parameter.schema, &mut pending)?;
        }
        if let Some(body) = &operation.request_body {
            for schema in body.content.values() {
                crate::schema::references(schema, &mut pending)?;
            }
        }
    }
    let mut reachable = BTreeSet::new();
    while let Some(name) = pending.pop_first() {
        if !reachable.insert(name.clone()) {
            continue;
        }
        let schema = contract
            .schemas
            .get(&name)
            .ok_or_else(|| ForgeError(format!("missing input schema definition {name}")))?;
        crate::schema::references(schema, &mut pending)?;
    }
    request_contract
        .schemas
        .retain(|name, _| reachable.contains(name));
    let mut contract = crate::schema_validation::normalize(&request_contract)?;
    if contract.openapi_version.starts_with("3.0.") {
        let definitions = contract.schemas.clone();
        for schema in contract.schemas.values_mut() {
            request_required(schema, &definitions)?;
        }
        for operation in &mut contract.operations {
            for parameter in &mut operation.parameters {
                request_required(&mut parameter.schema, &definitions)?;
            }
            if let Some(body) = &mut operation.request_body {
                for schema in body.content.values_mut() {
                    request_required(schema, &definitions)?;
                }
            }
        }
    }
    let mut inputs = BTreeMap::new();
    let mut properties = Map::new();
    let mut definitions = Map::new();
    for operation in &contract.operations {
        let input = operation_input_schema(operation, &contract)?;
        let mut graph_input = input.clone();
        if let Some(Value::Object(local)) = graph_input.as_object_mut().unwrap().remove("$defs") {
            for (name, schema) in local {
                if let Some(previous) = definitions.insert(name.clone(), schema.clone())
                    && previous != schema
                {
                    return Err(ForgeError(format!(
                        "conflicting request schema definition {name}"
                    )));
                }
            }
        }
        if inputs.insert(operation.id.clone(), input).is_some() {
            return Err(ForgeError(format!(
                "duplicate operation identity: {}",
                operation.id
            )));
        }
        properties.insert(operation.id.clone(), graph_input);
    }
    Ok(RequestSchemas {
        inputs,
        graph: json!({"type":"object","properties":properties,"additionalProperties":false,"$defs":definitions}),
    })
}

fn request_required(schema: &mut Value, definitions: &BTreeMap<String, Value>) -> ForgeResult<()> {
    let Some(object) = schema.as_object_mut() else {
        return Ok(());
    };
    let mut readonly = BTreeSet::new();
    if let Some(properties) = object.get("properties").and_then(Value::as_object) {
        for (name, property) in properties {
            if crate::schema::binding_view(property, definitions)?.get("readOnly")
                == Some(&Value::Bool(true))
            {
                readonly.insert(name.clone());
            }
        }
    }
    if let Some(Value::Array(required)) = object.get_mut("required") {
        required.retain(|name| name.as_str().is_none_or(|name| !readonly.contains(name)));
    }
    for key in [
        "properties",
        "patternProperties",
        "$defs",
        "definitions",
        "dependentSchemas",
    ] {
        if let Some(Value::Object(children)) = object.get_mut(key) {
            for child in children.values_mut() {
                request_required(child, definitions)?;
            }
        }
    }
    for key in [
        "items",
        "additionalProperties",
        "unevaluatedProperties",
        "unevaluatedItems",
        "contains",
        "propertyNames",
        "not",
        "if",
        "then",
        "else",
        "contentSchema",
    ] {
        if let Some(child) = object.get_mut(key) {
            request_required(child, definitions)?;
        }
    }
    for key in ["allOf", "anyOf", "oneOf", "prefixItems"] {
        if let Some(Value::Array(children)) = object.get_mut(key) {
            for child in children {
                request_required(child, definitions)?;
            }
        }
    }
    Ok(())
}

fn operation_input_schema(operation: &Operation, contract: &ResolvedOpenApi) -> ForgeResult<Value> {
    for parameter in &operation.parameters {
        if !["path", "query", "querystring", "header", "cookie"]
            .contains(&parameter.location.as_str())
        {
            return Err(ForgeError(format!(
                "unsupported parameter location: {}",
                parameter.location
            )));
        }
    }
    let mut properties = Map::new();
    let mut required = Vec::new();
    for location in ["path", "query", "querystring", "header", "cookie"] {
        let mut fields = Map::new();
        let mut needed = Vec::new();
        for parameter in operation
            .parameters
            .iter()
            .filter(|p| p.location == location)
        {
            if fields
                .insert(
                    parameter.name.clone(),
                    schema_for_request(&parameter.schema),
                )
                .is_some()
            {
                return Err(ForgeError(format!(
                    "duplicate {location} parameter: {}",
                    parameter.name
                )));
            }
            if parameter.required {
                needed.push(parameter.name.clone());
            }
        }
        if !fields.is_empty() {
            if !needed.is_empty() {
                required.push(location);
            }
            properties.insert(location.into(), json!({
                "type":"object","properties":fields,"required":needed,"additionalProperties":false
            }));
        }
    }
    let mut constraints = Vec::new();
    let mut body_input_schemas = Vec::new();
    if let Some(body) = &operation.request_body {
        if body.content.is_empty() {
            return Err(ForgeError(
                "request body has no declared media types".into(),
            ));
        }
        let mut alternatives = BTreeMap::new();
        let mut slot_schemas: BTreeMap<String, Vec<Value>> = BTreeMap::new();
        for (media, schema) in &body.content {
            let slots = body_slots(media, schema, &contract.schemas)?;
            if let Some(schema) = slots.get("body") {
                body_input_schemas.push(schema.clone());
            }
            for (slot, schema) in &slots {
                slot_schemas
                    .entry(slot.clone())
                    .or_default()
                    .push(schema_for_request(schema));
            }
            alternatives.insert(media, slots);
        }
        for (slot, schemas) in &slot_schemas {
            properties.insert(
                slot.clone(),
                match schemas.as_slice() {
                    [schema] => schema.clone(),
                    _ => json!({"anyOf":schemas}),
                },
            );
        }
        properties.insert(
            "media_type".into(),
            json!({"type":"string","enum":body.content.keys().collect::<Vec<_>>()}),
        );
        let present: Vec<_> = slot_schemas
            .keys()
            .map(|slot| json!({"required":[slot]}))
            .collect();
        let presence = if present.is_empty() {
            json!(false)
        } else {
            json!({"anyOf":present})
        };
        for (a, b) in [
            ("body", "body_json"),
            ("body", "body_base64"),
            ("body_json", "body_base64"),
        ] {
            constraints.push(json!({"not":{"required":[a,b]}}));
        }
        constraints.push(json!({"if":{"required":["media_type"]},"then":presence}));
        if body.required {
            constraints.push(presence);
        }
        if body.content.len() > 1 {
            let dependencies: Map<_, _> = slot_schemas
                .keys()
                .map(|slot| (slot.clone(), json!(["media_type"])))
                .collect();
            constraints.push(json!({"dependentRequired":dependencies}));
            for (media, slots) in alternatives {
                let selected: Map<_, _> = ["body", "body_json", "body_base64"]
                    .into_iter()
                    .map(|slot| {
                        (
                            slot.to_owned(),
                            slots
                                .get(slot)
                                .map(schema_for_request)
                                .unwrap_or(json!(false)),
                        )
                    })
                    .collect();
                constraints.push(json!({
                    "if":{"required":["media_type"],"properties":{"media_type":{"const":media}}},
                    "then":{"properties":selected}
                }));
            }
        }
    }
    let mut pending = BTreeSet::new();
    for parameter in &operation.parameters {
        crate::schema::references(&parameter.schema, &mut pending)?;
    }
    for schema in &body_input_schemas {
        crate::schema::references(schema, &mut pending)?;
    }
    let mut definitions = Map::new();
    while let Some(name) = pending.pop_first() {
        if definitions.contains_key(&name) {
            continue;
        }
        let schema = contract
            .schemas
            .get(&name)
            .ok_or_else(|| ForgeError(format!("missing input schema definition {name}")))?;
        crate::schema::references(schema, &mut pending)?;
        definitions.insert(name, schema_for_request(schema));
    }
    let mut schema = json!({
        "type":"object","properties":properties,"required":required,
        "additionalProperties":false,"$defs":definitions,
        "x-forge-operation-id":operation.id
    });
    if !constraints.is_empty() {
        schema["allOf"] = json!(constraints);
    }
    Ok(schema)
}

fn body_slots(
    media: &str,
    schema: &Value,
    schemas: &BTreeMap<String, Value>,
) -> ForgeResult<Map<String, Value>> {
    use crate::media::BodyCodec;
    let mut slots = Map::new();
    let body = match crate::media::classify(media, schema, schemas)? {
        BodyCodec::Json | BodyCodec::UrlEncodedForm | BodyCodec::MultipartForm => {
            Some(schema.clone())
        }
        BodyCodec::Text | BodyCodec::JsonLines { binary: false, .. } => {
            Some(json!({"type":"string","allOf":[schema]}))
        }
        BodyCodec::Binary { text_alternative } => {
            slots.insert(
                "body_base64".into(),
                json!({"type":"string","contentEncoding":"base64"}),
            );
            if text_alternative {
                crate::media::text_alternative_schema(schema, schemas)?
            } else {
                None
            }
        }
        BodyCodec::JsonLines { binary: true, .. } => {
            slots.insert(
                "body_base64".into(),
                json!({"type":"string","contentEncoding":"base64"}),
            );
            None
        }
        BodyCodec::Unsupported(_) => None,
    };
    if let Some(body) = body {
        slots.insert("body".into(), body.clone());
        slots.insert(
            "body_json".into(),
            json!({
                "type":"string","contentMediaType":"application/json","contentSchema":body
            }),
        );
    }
    Ok(slots)
}

// Only visit schema positions. Example/default data may itself contain $ref keys.
fn schema_for_request(schema: &Value) -> Value {
    let Some(source) = schema.as_object() else {
        return schema.clone();
    };
    let mut object = source.clone();
    if let Some(reference) = object.get("$ref").and_then(Value::as_str)
        && let Some(name) = reference.strip_prefix("#/components/schemas/")
    {
        object.insert("$ref".into(), json!(format!("#/$defs/{name}")));
    }
    for key in [
        "properties",
        "$defs",
        "definitions",
        "patternProperties",
        "dependentSchemas",
    ] {
        if let Some(Value::Object(children)) = object.get_mut(key) {
            for child in children.values_mut() {
                *child = schema_for_request(child);
            }
        }
    }
    for key in ["allOf", "anyOf", "oneOf", "prefixItems"] {
        if let Some(Value::Array(children)) = object.get_mut(key) {
            for child in children {
                *child = schema_for_request(child);
            }
        }
    }
    for key in [
        "items",
        "additionalProperties",
        "unevaluatedProperties",
        "unevaluatedItems",
        "contains",
        "propertyNames",
        "not",
        "if",
        "then",
        "else",
        "contentSchema",
    ] {
        if let Some(child) = object.get_mut(key) {
            *child = schema_for_request(child);
        }
    }
    Value::Object(object)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compiled_graph_retains_shared_input_definitions_once() {
        let document: Value = serde_json::from_str(include_str!(
            "../tests/fixtures/request_validation_openapi.json"
        ))
        .unwrap();
        let mut contract =
            crate::resolve_document(&document, crate::ResolveOptions::new("graph")).unwrap();
        for index in 1..32 {
            let mut operation = contract.operations[0].clone();
            operation.id = format!("graph/submit{index}");
            operation.name = format!("submit{index}");
            contract.operations.push(operation);
        }
        let schemas = build(&contract).unwrap();
        let graph = schemas.graph;
        assert_eq!(graph["$defs"].as_object().unwrap().len(), 1);
        let operations = graph["properties"].as_object().unwrap();
        assert_eq!(operations.len(), 32);
        for operation in operations.values() {
            assert!(
                operation.get("$defs").is_none(),
                "operation duplicated shared definitions"
            );
            assert_eq!(
                operation["properties"]["body"]["anyOf"][0]["properties"]["profile"]["$ref"],
                "#/$defs/Profile"
            );
        }
        let size = serde_json::to_vec(&graph).unwrap().len();
        let definition_size = serde_json::to_vec(&graph["$defs"]).unwrap().len();
        assert!(
            size < definition_size + 32 * 3000,
            "request graph grew beyond envelopes: {size}"
        );
    }
}
