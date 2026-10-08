//! Typed composition preparation and validation code generation.
use super::{pascal, symbols::SdkSymbols, type_for_schema};
use crate::{OpenApiError, OpenApiResult, ResolvedOpenApi};
use serde_json::{Map, Value, json};
use std::collections::{BTreeMap, BTreeSet};

pub(super) fn used(contract: &ResolvedOpenApi) -> bool {
    fn constrained(schema: &Value) -> bool {
        if ["anyOf", "oneOf", "allOf"]
            .iter()
            .any(|key| schema.get(*key).is_some())
            || needs_untyped_value(schema)
        {
            return true;
        }
        for key in [
            "properties",
            "patternProperties",
            "$defs",
            "definitions",
            "dependentSchemas",
        ] {
            if schema
                .get(key)
                .and_then(Value::as_object)
                .is_some_and(|children| children.values().any(constrained))
            {
                return true;
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
            if schema.get(key).is_some_and(constrained) {
                return true;
            }
        }
        schema
            .get("prefixItems")
            .and_then(Value::as_array)
            .is_some_and(|children| children.iter().any(constrained))
    }
    contract.schemas.values().any(constrained)
        || contract.operations.iter().any(|op| {
            op.parameters.iter().any(|p| constrained(&p.schema))
                || op
                    .request_body
                    .as_ref()
                    .is_some_and(|b| b.content.values().any(constrained))
        })
}

fn needs_untyped_value(schema: &Value) -> bool {
    schema.is_object()
        && schema.get("type").is_none()
        && schema.get("$ref").is_none()
        && [
            "enum",
            "const",
            "multipleOf",
            "maximum",
            "minimum",
            "exclusiveMaximum",
            "exclusiveMinimum",
            "maxLength",
            "minLength",
            "pattern",
            "items",
            "prefixItems",
            "contains",
            "minContains",
            "maxContains",
            "maxItems",
            "minItems",
            "uniqueItems",
            "properties",
            "patternProperties",
            "additionalProperties",
            "unevaluatedProperties",
            "unevaluatedItems",
            "required",
            "dependentRequired",
            "dependentSchemas",
            "maxProperties",
            "minProperties",
            "propertyNames",
            "not",
            "if",
            "then",
            "else",
        ]
        .iter()
        .any(|key| schema.get(*key).is_some())
}

pub(super) fn reference_name(reference: &str) -> Option<String> {
    reference
        .strip_prefix("#/components/schemas/")
        .map(|name| name.replace("~1", "/").replace("~0", "~"))
}
fn reference(name: &str) -> Value {
    json!({"$ref":format!("#/components/schemas/{}",name.replace('~',"~0").replace('/',"~1"))})
}

pub(super) fn prepare(contract: &ResolvedOpenApi) -> OpenApiResult<ResolvedOpenApi> {
    if !used(contract) {
        return Ok(contract.clone());
    }
    let normalized = super::validation::normalize(contract)?;
    let contract = &normalized;
    let mut out = contract.clone();
    let mut compiler = Compiler {
        original: &contract.schemas,
        generated: BTreeMap::new(),
        names: contract.schemas.keys().map(|n| pascal(n)).collect(),
    };
    for (name, schema) in &contract.schemas {
        let prepared = compiler.walk(schema, &pascal(name), true, false)?;
        compiler.generated.insert(name.clone(), prepared);
    }
    for operation in &mut out.operations {
        for parameter in &mut operation.parameters {
            parameter.schema = compiler.walk(
                &parameter.schema,
                &format!("{}{}", pascal(&operation.name), pascal(&parameter.name)),
                false,
                false,
            )?;
        }
        if let Some(body) = &mut operation.request_body {
            for (media, schema) in &mut body.content {
                *schema = compiler.walk(
                    schema,
                    &format!("{}Body{}", pascal(&operation.name), pascal(media)),
                    false,
                    false,
                )?;
            }
        }
    }
    out.schemas = compiler.generated;
    Ok(out)
}

struct Compiler<'a> {
    original: &'a BTreeMap<String, Value>,
    generated: BTreeMap<String, Value>,
    names: BTreeSet<String>,
}
impl Compiler<'_> {
    fn walk(
        &mut self,
        schema: &Value,
        label: &str,
        root: bool,
        branch: bool,
    ) -> OpenApiResult<Value> {
        if schema.get("$ref").is_some() {
            return Ok(schema.clone());
        }
        if !root {
            if let Some((name, _)) = self.original.iter().find(|(_, candidate)| {
                *candidate == schema
                    && (schema.get("anyOf").is_some()
                        || schema.get("oneOf").is_some()
                        || schema.get("allOf").is_some()
                        || schema.get("type").and_then(Value::as_str) == Some("object")
                        || (!branch && needs_untyped_value(schema)))
            }) {
                return Ok(reference(name));
            }
            if schema.get("anyOf").is_some()
                || schema.get("oneOf").is_some()
                || schema.get("allOf").is_some()
                || (branch && schema.get("type").and_then(Value::as_str) == Some("object"))
                || (!branch && needs_untyped_value(schema))
            {
                let mut name = pascal(label);
                let mut suffix = 2;
                while !self.names.insert(name.clone()) {
                    name = format!("{}{suffix}", pascal(label));
                    suffix += 1;
                }
                let prepared = self.walk(schema, &name, true, false)?;
                self.generated.insert(name.clone(), prepared);
                return Ok(reference(&name));
            }
        }
        let Some(fields) = schema.as_object() else {
            return Ok(schema.clone());
        };
        let mut out = fields.clone();
        if let Some(properties) = fields.get("properties").and_then(Value::as_object) {
            let mut mapped = Map::new();
            for (name, property) in properties {
                mapped.insert(
                    name.clone(),
                    self.walk(property, &format!("{label}{}", pascal(name)), false, false)?,
                );
            }
            out.insert("properties".into(), Value::Object(mapped));
        }
        for key in ["items", "additionalProperties", "not"] {
            if let Some(child) = fields.get(key) {
                out.insert(
                    key.into(),
                    self.walk(child, &format!("{label}{}", pascal(key)), false, false)?,
                );
            }
        }
        for key in ["oneOf", "allOf", "anyOf"] {
            if let Some(children) = fields.get(key).and_then(Value::as_array) {
                let children = children
                    .iter()
                    .enumerate()
                    .map(|(index, child)| {
                        self.walk(
                            child,
                            &format!("{label}Variant{}", index + 1),
                            false,
                            key == "oneOf" || key == "anyOf",
                        )
                    })
                    .collect::<OpenApiResult<Vec<_>>>()?;
                out.insert(key.into(), Value::Array(children));
            }
        }
        Ok(Value::Object(out))
    }
}

pub(super) fn render(
    schema_name: &str,
    schema: &Value,
    schemas: &BTreeMap<String, Value>,
    symbols: &SdkSymbols,
) -> OpenApiResult<Option<String>> {
    let name = symbols.schema_type(schema_name).to_string();
    let pointer = super::validation::key(schema_name);
    let keyword = if schema.get("oneOf").is_some() {
        "oneOf"
    } else {
        "anyOf"
    };
    if let Some(branches) = schema.get(keyword).and_then(Value::as_array) {
        if branches.is_empty() {
            return Err(fail(format!("{keyword} must contain at least one schema")));
        }

        let mut variants = BTreeSet::new();
        let mut out = format!(
            "/// A value selected from schema {name}.\n#[derive(Clone, Debug, PartialEq)]\npub enum {name} {{\n"
        );
        let mut arms = String::new();
        for (index, branch) in branches.iter().enumerate() {
            let ty = match type_for_schema(branch, Some(&name), schemas, symbols) {
                Ok(ty) => ty,
                Err(_)
                    if branch.is_object()
                        && ["type", "$ref", "oneOf", "anyOf", "allOf"]
                            .iter()
                            .all(|key| branch.get(*key).is_none()) =>
                {
                    "JsonValue".to_owned()
                }
                Err(error) => return Err(error),
            };
            let base = branch
                .get("$ref")
                .and_then(Value::as_str)
                .and_then(reference_name)
                .map(|n| symbols.schema_type(&n).to_string())
                .unwrap_or_else(|| {
                    pascal(
                        branch
                            .get("type")
                            .and_then(Value::as_str)
                            .unwrap_or("Value"),
                    )
                });
            let mut variant = base.clone();
            let mut suffix = 2;
            while !variants.insert(variant.clone()) {
                variant = format!("{base}{suffix}");
                suffix += 1;
            }
            out.push_str(&format!(
                "    /// Alternative {} in the source schema.\n    {variant}({ty}),\n",
                index + 1
            ));
            arms.push_str(&format!(
                "            Self::{variant}(value) => (value.into_json(), {index}),\n"
            ));
        }
        out.push_str(&format!("}}\n\nimpl IntoJson for {name} {{\n    fn into_json(self) -> JsonValue {{\n        let (value, selected) = match self {{\n{arms}        }};\n        let pointer = {pointer:?};\n        let branch = format!(\"{{pointer}}/{keyword}/{{selected}}\");\n        if json_matches(&value, &branch) && json_matches(&value, pointer) {{ value }} else {{ JsonValue::Invalid }}\n    }}\n}}\n\n"));
        Ok(Some(out))
    } else if schema.get("allOf").is_some() {
        match merge_object(schema, schemas, &mut BTreeSet::new()) {
            Ok(merged) => {
                let guard = format!("json_matches(&value, {pointer:?})");
                Ok(Some(super::render_object_schema(
                    &name,
                    &merged,
                    Some(&guard),
                    schemas,
                    symbols,
                )?))
            }
            Err(_) => Ok(Some(render_validated_value(&name, &pointer))),
        }
    } else if needs_untyped_value(schema) {
        Ok(Some(render_validated_value(&name, &pointer)))
    } else {
        Ok(None)
    }
}

fn render_validated_value(name: &str, pointer: &str) -> String {
    format!(
        "/// A value satisfying every constraint in schema {name}.\n#[derive(Clone, Debug, PartialEq)]\npub struct {name}(JsonValue);\n\nimpl {name} {{\n    /// Validate a JSON value against this schema.\n    pub fn try_new(value: JsonValue) -> Result<Self, JsonEncodeError> {{\n        if json_matches(&value, {pointer:?}) {{ Ok(Self(value)) }} else {{ Err(JsonEncodeError) }}\n    }}\n    /// Borrow the validated JSON value.\n    pub fn as_json(&self) -> &JsonValue {{ &self.0 }}\n}}\n\nimpl IntoJson for {name} {{\n    fn into_json(self) -> JsonValue {{ self.0 }}\n}}\n\n"
    )
}

fn merge_object(
    schema: &Value,
    schemas: &BTreeMap<String, Value>,
    stack: &mut BTreeSet<String>,
) -> OpenApiResult<Value> {
    if let Some(reference) = schema.get("$ref").and_then(Value::as_str) {
        let name = reference_name(reference)
            .ok_or_else(|| fail("external allOf reference is unsupported"))?;
        if !stack.insert(name.clone()) {
            return Err(fail("recursive allOf flattening is unsupported"));
        }
        let target = schemas
            .get(&name)
            .ok_or_else(|| fail(format!("unknown schema {name}")))?;
        let result = merge_object(target, schemas, stack);
        stack.remove(&name);
        return result;
    }
    let mut properties = Map::new();
    let mut required = BTreeSet::new();
    let mut object = schema.get("type").and_then(Value::as_str) == Some("object");
    if let Some(kind) = schema.get("type")
        && kind != &Value::String("object".into())
    {
        return Err(fail(
            "allOf typed generation currently requires object intersections",
        ));
    }
    if let Some(branches) = schema.get("allOf") {
        let branches = branches
            .as_array()
            .filter(|b| !b.is_empty())
            .ok_or_else(|| fail("allOf must be a nonempty array"))?;
        for branch in branches {
            let merged = merge_object(branch, schemas, stack)?;
            object = true;
            for (key, value) in merged["properties"].as_object().unwrap() {
                if properties.get(key).is_some_and(|old| old != value) {
                    return Err(fail(format!(
                        "allOf property {key} has incompatible typed declarations"
                    )));
                }
                properties.insert(key.clone(), value.clone());
            }
            required.extend(super::required_set(&merged));
        }
    }
    if let Some(fields) = schema.get("properties").and_then(Value::as_object) {
        for (key, value) in fields {
            if properties.get(key).is_some_and(|old| old != value) {
                return Err(fail(format!(
                    "allOf property {key} has incompatible typed declarations"
                )));
            }
            properties.insert(key.clone(), value.clone());
        }
    }
    required.extend(super::required_set(schema));
    if !object {
        return Err(fail(
            "allOf typed generation requires explicit object types",
        ));
    }
    if required.iter().any(|key| !properties.contains_key(key)) {
        return Err(fail(
            "allOf requires a property without a typed declaration",
        ));
    }
    Ok(json!({"type":"object","properties":properties,"required":required}))
}

fn fail(message: impl Into<String>) -> OpenApiError {
    OpenApiError(message.into())
}
