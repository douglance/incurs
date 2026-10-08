//! Response-specific read-model generation for generated SDKs.
use crate::schema_validation::{normalize, normalize_schema};
use crate::{OpenApiError, OpenApiResult, ResolvedOpenApi};
use serde_json::{Map, Value, json};
use std::borrow::Cow;
use std::collections::{BTreeMap, BTreeSet};

/// Compiled response read-model source and validation metadata.
pub(super) struct ResponseModels {
    /// Rust source for generated response read models.
    pub(super) source: String,
    /// One offline response schema graph consumed by generated response validators.
    pub(super) graph: Value,
    /// Response body model metadata keyed by operation id, status key, and media type.
    pub(super) media: BTreeMap<(String, String, String), ResponseModel>,
    /// Reserved input names plus every generated public type name.
    pub(super) used_names: BTreeSet<String>,
}

/// Metadata for one declared response media schema.
#[derive(Clone, Debug)]
pub(super) struct ResponseModel {
    /// Pointer key accepted by the generated response JSON validator.
    pub(super) schema_pointer: String,
    /// Rust type expression for the decoded response body.
    pub(super) rust_type: String,
    /// Source-version-normalized response-context root schema.
    pub(super) schema: Value,
    /// Whether the source schema explicitly carries a binary annotation.
    pub(super) binary: bool,
}

/// Compile response-specific read models and one offline validation graph.
pub(super) fn compile(
    contract: &ResolvedOpenApi,
    reserved: BTreeSet<String>,
) -> OpenApiResult<ResponseModels> {
    let normalized = normalize(contract)?;
    let legacy = contract.openapi_version.starts_with("3.0.");
    let mut compiler = Compiler::new(&normalized.schemas, reserved);
    let mut media = BTreeMap::new();

    for operation in &normalized.operations {
        for (status, response) in &operation.responses {
            for (media_type, schema) in &response.content {
                let root = normalize_schema(schema, legacy, false)?;
                let root = response_context_schema(&root, &normalized.schemas)?;
                let binary = !is_json_response_media(media_type)
                    && has_binary_annotation(&root, &normalized.schemas)?;
                let label = format!(
                    "{}{}{}Response",
                    pascal(&operation.name),
                    pascal(status),
                    pascal(media_type)
                );
                let (rust_type, schema_pointer) = if binary {
                    let pointer = compiler.ensure_root_schema(&label, &root)?;
                    ("JsonValue".to_string(), pointer)
                } else {
                    compiler.root_type(&root, &label)?
                };
                media.insert(
                    (operation.id.clone(), status.clone(), media_type.clone()),
                    ResponseModel {
                        schema_pointer,
                        rust_type,
                        schema: root,
                        binary,
                    },
                );
            }
        }
    }

    compiler.finish(media)
}

struct Compiler<'a> {
    schemas: &'a BTreeMap<String, Value>,
    used_names: BTreeSet<String>,
    models: BTreeMap<String, Model>,
    order: Vec<String>,
    shared_names: BTreeMap<String, String>,
    root_names: BTreeMap<String, String>,
    anonymous_names: BTreeMap<String, String>,
    rendering_model: Option<String>,
}

#[derive(Clone)]
struct Model {
    name: String,
    pointer: String,
    schema: Value,
}

impl<'a> Compiler<'a> {
    fn new(schemas: &'a BTreeMap<String, Value>, reserved: BTreeSet<String>) -> Self {
        Self {
            schemas,
            used_names: reserved,
            models: BTreeMap::new(),
            order: Vec::new(),
            shared_names: BTreeMap::new(),
            root_names: BTreeMap::new(),
            anonymous_names: BTreeMap::new(),
            rendering_model: None,
        }
    }

    fn finish(
        mut self,
        media: BTreeMap<(String, String, String), ResponseModel>,
    ) -> OpenApiResult<ResponseModels> {
        let mut source = String::new();
        let mut index = 0;
        while index < self.order.len() {
            let name = self.order[index].clone();
            let Some(model) = self.models.get(&name).cloned() else {
                return Err(OpenApiError(format!(
                    "response model {name} was not registered"
                )));
            };
            self.rendering_model = Some(name);
            source.push_str(&self.render_model(&model)?);
            self.rendering_model = None;
            index += 1;
        }
        let graph = self.graph(&media)?;
        Ok(ResponseModels {
            source,
            graph,
            media,
            used_names: self.used_names,
        })
    }

    fn graph(
        &mut self,
        media: &BTreeMap<(String, String, String), ResponseModel>,
    ) -> OpenApiResult<Value> {
        let mut definitions = Map::new();
        let mut properties = Map::new();
        let mut needed_shared = BTreeSet::new();
        for model in self.models.values() {
            collect_schema_references(&model.schema, &mut needed_shared)?;
            if model.pointer != graph_pointer(&model.name)
                && let Some(name) = graph_pointer_name(&model.pointer)
            {
                needed_shared.insert(name);
            }
        }
        for response in media.values() {
            collect_schema_references(&response.schema, &mut needed_shared)?;
        }
        let mut pending = needed_shared.iter().cloned().collect::<Vec<_>>();
        while let Some(name) = pending.pop() {
            let schema = self.schemas.get(&name).ok_or_else(|| {
                OpenApiError(format!(
                    "response schema reference target not found: {name}"
                ))
            })?;
            let normalized = response_context_schema(schema, self.schemas)?;
            let before = needed_shared.len();
            collect_schema_references(&normalized, &mut needed_shared)?;
            if needed_shared.len() != before {
                for found in needed_shared.iter().cloned() {
                    if !definitions.contains_key(&found) && !pending.contains(&found) {
                        pending.push(found);
                    }
                }
            }
        }
        for name in needed_shared {
            let schema = self.schemas.get(&name).ok_or_else(|| {
                OpenApiError(format!(
                    "response schema reference target not found: {name}"
                ))
            })?;
            let schema = response_context_schema(schema, self.schemas)?;
            definitions.insert(name.clone(), normalize_schema(&schema, false, true)?);
        }
        for model in self.models.values() {
            if model.pointer == graph_pointer(&model.name) {
                definitions.insert(
                    model.name.clone(),
                    normalize_schema(&model.schema, false, true)?,
                );
            }
        }
        for (name, schema) in definitions.iter() {
            let pointer = graph_pointer(name);
            properties.insert(pointer.clone(), json!({"$ref": pointer}));
            add_branch_properties(&mut properties, &pointer, schema);
        }
        for response in media.values() {
            properties.insert(
                response.schema_pointer.clone(),
                json!({"$ref": response.schema_pointer}),
            );
        }
        let graph = json!({
            "type": "object",
            "properties": properties,
            "additionalProperties": false,
            "$defs": definitions,
        });
        jsonschema::draft202012::options()
            .offline()
            .should_validate_formats(false)
            .build(&graph)
            .map_err(|error| {
                OpenApiError(format!(
                    "invalid generated response validation graph: {error}"
                ))
            })?;
        Ok(graph)
    }

    fn root_type(&mut self, schema: &Value, label: &str) -> OpenApiResult<(String, String)> {
        let ty = self.type_for_schema(schema, Some(label), TypeContext::Root)?;
        let pointer = if let Some(reference) = schema.get("$ref").and_then(Value::as_str) {
            if schema.as_object().is_some_and(|fields| fields.len() == 1) {
                graph_pointer(&reference_name(reference)?)
            } else {
                self.ensure_root_schema(label, schema)?
            }
        } else if self
            .models
            .get(&ty)
            .is_some_and(|model| model.schema == *schema)
        {
            graph_pointer(&ty)
        } else {
            self.ensure_root_schema(label, schema)?
        };
        Ok((ty, pointer))
    }

    fn ensure_root_schema(&mut self, label: &str, schema: &Value) -> OpenApiResult<String> {
        let key = format!("{}\n{}", label, schema);
        if let Some(name) = self.root_names.get(&key) {
            return Ok(graph_pointer(name));
        }
        let name = self.allocate_type_name(&format!("Read{label}"));
        let pointer = graph_pointer(&name);
        self.root_names.insert(key, name.clone());
        self.models.insert(
            name.clone(),
            Model {
                name: name.clone(),
                pointer: pointer.clone(),
                schema: schema.clone(),
            },
        );
        self.order.push(name);
        Ok(pointer)
    }

    fn ensure_model(&mut self, label: &str, schema: &Value) -> OpenApiResult<String> {
        if let Some(reference) = schema.get("$ref").and_then(Value::as_str) {
            return self.ensure_shared_model(&reference_name(reference)?);
        }
        let key = serde_json::to_string(schema).map_err(|error| OpenApiError(error.to_string()))?;
        if let Some(name) = self.anonymous_names.get(&key) {
            return Ok(name.clone());
        }
        let name = self.allocate_type_name(&format!("Read{label}"));
        self.anonymous_names.insert(key, name.clone());
        let pointer = graph_pointer(&name);
        self.models.insert(
            name.clone(),
            Model {
                name: name.clone(),
                pointer,
                schema: schema.clone(),
            },
        );
        self.order.push(name.clone());
        Ok(name)
    }

    fn ensure_shared_model(&mut self, schema_name: &str) -> OpenApiResult<String> {
        if let Some(name) = self.shared_names.get(schema_name) {
            return Ok(name.clone());
        }
        let schema = self.schemas.get(schema_name).ok_or_else(|| {
            OpenApiError(format!(
                "response schema reference target not found: {schema_name}"
            ))
        })?;
        let schema = response_context_schema(schema, self.schemas)?;
        let name = self.allocate_type_name(&format!("Read{}", pascal(schema_name)));
        let pointer = graph_pointer(schema_name);
        self.shared_names
            .insert(schema_name.to_string(), name.clone());
        self.models.insert(
            name.clone(),
            Model {
                name: name.clone(),
                pointer,
                schema,
            },
        );
        self.order.push(name.clone());
        Ok(name)
    }

    fn allocate_type_name(&mut self, base: &str) -> String {
        let base = pascal(base);
        if self.used_names.insert(base.clone()) {
            return base;
        }
        for suffix in 2_u64.. {
            let candidate = format!("{base}{suffix}");
            if self.used_names.insert(candidate.clone()) {
                return candidate;
            }
        }
        unreachable!()
    }

    fn type_for_schema(
        &mut self,
        schema: &Value,
        label: Option<&str>,
        context: TypeContext,
    ) -> OpenApiResult<String> {
        let nullable_reference_item = matches!(context, TypeContext::ArrayItem)
            && schema.get("$ref").is_some()
            && is_nullable(binding_view(schema, self.schemas)?.as_ref());
        let ty = self.type_for_unboxed_schema(schema, label, context)?;
        let ty = if self.models.contains_key(&ty) && self.reaches_rendering_model(schema)? {
            format!("Box<{ty}>")
        } else {
            ty
        };
        Ok(if nullable_reference_item {
            format!("Option<{ty}>")
        } else {
            ty
        })
    }

    fn reaches_rendering_model(&self, schema: &Value) -> OpenApiResult<bool> {
        let Some(model) = self
            .rendering_model
            .as_ref()
            .and_then(|name| self.models.get(name))
        else {
            return Ok(false);
        };
        if contains_schema(schema, &model.schema) {
            return Ok(true);
        }
        let mut pending = BTreeSet::new();
        collect_schema_references(schema, &mut pending)?;
        let mut visited = BTreeSet::new();
        while let Some(name) = pending.pop_first() {
            if !visited.insert(name.clone()) {
                continue;
            }
            let target = self.schemas.get(&name).ok_or_else(|| {
                OpenApiError(format!(
                    "response schema reference target not found: {name}"
                ))
            })?;
            let target = response_context_schema(target, self.schemas)?;
            if contains_schema(&target, &model.schema) {
                return Ok(true);
            }
            collect_schema_references(&target, &mut pending)?;
        }
        Ok(false)
    }

    fn type_for_unboxed_schema(
        &mut self,
        schema: &Value,
        label: Option<&str>,
        context: TypeContext,
    ) -> OpenApiResult<String> {
        if is_nullable(schema) {
            let inner = without_null_type(schema)?;
            let ty = self.type_for_schema(&inner, label, TypeContext::Nested)?;
            return match context {
                TypeContext::ArrayItem => Ok(format!("Option<{ty}>")),
                TypeContext::Field | TypeContext::Root | TypeContext::Nested => Ok(ty),
            };
        }
        self.type_for_nonnullable_schema(schema, label)
    }

    fn type_for_nonnullable_schema(
        &mut self,
        schema: &Value,
        label: Option<&str>,
    ) -> OpenApiResult<String> {
        if schema == &Value::Bool(true) || schema.as_object().is_some_and(Map::is_empty) {
            return Ok("JsonValue".to_string());
        }
        if schema == &Value::Bool(false) {
            return Ok("JsonValue".to_string());
        }
        if let Some(reference) = schema.get("$ref").and_then(Value::as_str) {
            let name = reference_name(reference)?;
            let target = self.schemas.get(&name).ok_or_else(|| {
                OpenApiError(format!(
                    "response schema reference target not found: {name}"
                ))
            })?;
            let target = response_context_schema(target, self.schemas)?;
            let nonnullable_target = if is_nullable(&target) {
                without_null_type(&target)?
            } else {
                target
            };
            if can_inline_ref_target(&nonnullable_target) {
                return self.type_for_schema(&nonnullable_target, label, TypeContext::Nested);
            }
            let ty = self.ensure_shared_model(&name)?;
            if let Some(current) = label
                && self.schema_reaches_type(&name, current, &mut BTreeSet::new())?
            {
                return Ok(format!("Box<{ty}>"));
            }
            return Ok(ty);
        }
        if string_enum(schema).is_some() {
            return self.ensure_model(label.unwrap_or("Enum"), schema);
        }
        if schema.get("oneOf").is_some()
            || schema.get("anyOf").is_some()
            || schema.get("allOf").is_some()
        {
            return self.ensure_model(label.unwrap_or("Value"), schema);
        }
        match schema.get("type") {
            Some(Value::String(kind)) => match kind.as_str() {
                "string" => Ok("String".to_string()),
                "integer" => Ok("ResponseInteger".to_string()),
                "number" => Ok("ResponseNumber".to_string()),
                "boolean" => Ok("bool".to_string()),
                "null" => Ok("()".to_string()),
                "array" => {
                    let items = schema.get("items").ok_or_else(|| {
                        OpenApiError("response array schema requires items".to_string())
                    })?;
                    let item_label = format!("{}Item", label.unwrap_or("Array"));
                    let item =
                        self.type_for_schema(items, Some(&item_label), TypeContext::ArrayItem)?;
                    Ok(format!("Vec<{item}>"))
                }
                "object" => self.ensure_model(label.unwrap_or("Object"), schema),
                other => Err(OpenApiError(format!(
                    "unsupported response schema type {other}"
                ))),
            },
            Some(Value::Array(_)) => self.ensure_model(label.unwrap_or("Value"), schema),
            None => {
                if schema.as_object().is_some_and(|object| {
                    object.contains_key("properties")
                        || object.contains_key("required")
                        || object.contains_key("additionalProperties")
                        || object.contains_key("unevaluatedProperties")
                }) {
                    self.ensure_model(label.unwrap_or("Object"), schema)
                } else {
                    Ok("JsonValue".to_string())
                }
            }
            _ => Err(OpenApiError(format!(
                "unsupported response schema shape {schema}"
            ))),
        }
    }

    fn schema_reaches_type(
        &self,
        schema_name: &str,
        current_type: &str,
        visited: &mut BTreeSet<String>,
    ) -> OpenApiResult<bool> {
        if !visited.insert(schema_name.to_string()) {
            return Ok(false);
        }
        if self
            .shared_names
            .get(schema_name)
            .is_some_and(|name| name == current_type)
        {
            return Ok(true);
        }
        let schema = self.schemas.get(schema_name).ok_or_else(|| {
            OpenApiError(format!(
                "response schema reference target not found: {schema_name}"
            ))
        })?;
        let mut references = BTreeSet::new();
        collect_schema_references(schema, &mut references)?;
        for reference in references {
            if self.schema_reaches_type(&reference, current_type, visited)? {
                return Ok(true);
            }
        }
        Ok(false)
    }

    fn render_model(&mut self, model: &Model) -> OpenApiResult<String> {
        if let Some(values) = string_enum(&model.schema) {
            return Ok(render_string_enum(&model.name, &model.pointer, &values));
        }
        if let Some(keyword) = composition_keyword(&model.schema) {
            if keyword == "allOf" {
                if let Ok(merged) =
                    merge_all_of_object(&model.schema, self.schemas, &mut BTreeSet::new())
                {
                    return self.render_object(&model.name, &model.pointer, &merged);
                }
                return Ok(render_validated_wrapper(&model.name, &model.pointer));
            }
            return self.render_union(&model.name, &model.pointer, keyword, &model.schema);
        }
        if model.schema.get("type").and_then(Value::as_str) == Some("object") {
            return self.render_object(&model.name, &model.pointer, &model.schema);
        }
        Ok(render_validated_wrapper(&model.name, &model.pointer))
    }

    fn render_union(
        &mut self,
        name: &str,
        pointer: &str,
        keyword: &str,
        schema: &Value,
    ) -> OpenApiResult<String> {
        let branches = schema
            .get(keyword)
            .and_then(Value::as_array)
            .filter(|branches| !branches.is_empty())
            .ok_or_else(|| {
                OpenApiError(format!(
                    "response {keyword} must contain at least one schema"
                ))
            })?;
        let mut variants = BTreeSet::new();
        let mut rendered = format!(
            "/// Response value selected from schema `{name}`.\n#[derive(Clone, Debug, PartialEq)]\npub enum {name} {{\n"
        );
        let mut arms = String::new();
        for (index, branch) in branches.iter().enumerate() {
            let branch_label = format!("{name}Variant{}", index + 1);
            let ty = self.type_for_schema(branch, Some(&branch_label), TypeContext::Nested)?;
            let base = branch_variant_base(branch, &ty);
            let variant = unique_variant(&mut variants, &base);
            rendered.push_str(&format!(
                "    /// Alternative {} in the source response schema.\n    {variant}({ty}),\n",
                index + 1
            ));
            let branch_pointer = format!("{pointer}/{keyword}/{index}");
            arms.push_str(&format!(
                "        if response_json_matches(value, {branch_pointer:?}) {{ return <{ty} as DecodeResponseValue>::decode_response_value(value).map(Self::{variant}); }}\n"
            ));
        }
        rendered.push_str("}\n\n");
        rendered.push_str(&format!(
            "impl DecodeResponseValue for {name} {{\n    fn decode_response_value(value: &JsonValue) -> Result<Self, String> {{\n        if !response_json_matches(value, {pointer:?}) {{ return Err(format!(\"response value does not match schema {pointer}\")); }}\n{arms}        Err(\"response value matched the parent union but no declared branch\".to_string())\n    }}\n}}\n\n"
        ));
        Ok(rendered)
    }

    fn render_object(
        &mut self,
        name: &str,
        pointer: &str,
        schema: &Value,
    ) -> OpenApiResult<String> {
        let properties = schema
            .get("properties")
            .and_then(Value::as_object)
            .cloned()
            .unwrap_or_default();
        let required = required_response_set(schema, self.schemas)?;
        let mut wire_names = properties.keys().cloned().collect::<Vec<_>>();
        wire_names.push("additional_properties".to_string());
        let fields = field_names(wire_names.iter().map(String::as_str), &[]);
        let extra_field = fields
            .last()
            .cloned()
            .unwrap_or_else(|| "additional_properties".to_string());
        let field_names = fields[..fields.len().saturating_sub(1)].to_vec();
        let known = properties.keys().cloned().collect::<BTreeSet<_>>();
        let mut out = format!(
            "/// Response object decoded from schema `{name}`.\n#[derive(Clone, Debug, PartialEq)]\npub struct {name} {{\n"
        );
        let mut initializers = String::new();
        for ((wire, property), field) in properties.iter().zip(&field_names) {
            let doc_wire = wire.escape_debug();
            let label = format!("{name}{}", pascal(wire));
            let current = if property.get("$ref").is_some() {
                name
            } else {
                &label
            };
            let ty = self.type_for_schema(property, Some(current), TypeContext::Field)?;
            if required.contains(wire)
                && !is_nullable(binding_view(property, self.schemas)?.as_ref())
            {
                out.push_str(&format!(
                    "    /// Wire field `{doc_wire}`.\n    pub {field}: {ty},\n"
                ));
                initializers.push_str(&format!(
                    "            {field}: response_required::<{ty}>(object, {})?,\n",
                    rust_string(wire)
                ));
            } else {
                out.push_str(&format!(
                    "    /// Wire field `{doc_wire}`.\n    pub {field}: Field<{ty}>,\n"
                ));
                initializers.push_str(&format!(
                    "            {field}: response_field::<{ty}>(object, {})?,\n",
                    rust_string(wire)
                ));
            }
        }
        out.push_str(&format!(
            "    /// Unknown response properties accepted by the schema.\n    pub {extra_field}: Vec<(String, JsonValue)>,\n"
        ));
        out.push_str("}\n\n");
        let known_expr = known
            .iter()
            .map(|name| rust_string(name))
            .collect::<Vec<_>>()
            .join(", ");
        out.push_str(&format!(
            "impl DecodeResponseValue for {name} {{\n    fn decode_response_value(value: &JsonValue) -> Result<Self, String> {{\n        if !response_json_matches(value, {pointer:?}) {{ return Err(format!(\"response value does not match schema {pointer}\")); }}\n        let object = response_object(value)?;\n        let known: &[&str] = &[{known_expr}];\n        let {extra_field} = object.iter().filter(|(key, _)| !known.iter().any(|known| known == key)).cloned().collect();\n        Ok(Self {{\n{initializers}            {extra_field},\n        }})\n    }}\n}}\n\n"
        ));
        Ok(out)
    }
}

#[derive(Clone, Copy)]
enum TypeContext {
    Root,
    Field,
    ArrayItem,
    Nested,
}

fn add_branch_properties(properties: &mut Map<String, Value>, pointer: &str, schema: &Value) {
    for keyword in ["oneOf", "anyOf"] {
        if let Some(branches) = schema.get(keyword).and_then(Value::as_array) {
            for index in 0..branches.len() {
                let branch = format!("{pointer}/{keyword}/{index}");
                properties.insert(branch.clone(), json!({"$ref": branch}));
            }
        }
    }
}

fn response_context_schema(
    schema: &Value,
    schemas: &BTreeMap<String, Value>,
) -> OpenApiResult<Value> {
    fn visit(schema: &Value, schemas: &BTreeMap<String, Value>) -> OpenApiResult<Value> {
        let Some(fields) = schema.as_object() else {
            return Ok(schema.clone());
        };
        let mut out = fields.clone();
        if let Some(properties) = fields.get("properties").and_then(Value::as_object) {
            let mut mapped = Map::new();
            for (name, property) in properties {
                mapped.insert(name.clone(), visit(property, schemas)?);
            }
            out.insert("properties".into(), Value::Object(mapped));
        }
        if let Some(required) = fields.get("required").and_then(Value::as_array) {
            let mut filtered = Vec::new();
            for name in required.iter().filter_map(Value::as_str) {
                let write_only = fields
                    .get("properties")
                    .and_then(Value::as_object)
                    .and_then(|properties| properties.get(name))
                    .map_or(Ok(false), |property| {
                        property_is_write_only(property, schemas)
                    })?;
                if !write_only {
                    filtered.push(Value::String(name.to_string()));
                }
            }
            out.insert("required".into(), Value::Array(filtered));
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
            if let Some(child) = fields.get(key) {
                out.insert(key.into(), visit(child, schemas)?);
            }
        }
        for key in ["allOf", "anyOf", "oneOf", "prefixItems"] {
            if let Some(children) = fields.get(key).and_then(Value::as_array) {
                out.insert(
                    key.into(),
                    Value::Array(
                        children
                            .iter()
                            .map(|child| visit(child, schemas))
                            .collect::<OpenApiResult<Vec<_>>>()?,
                    ),
                );
            }
        }
        for key in [
            "$defs",
            "definitions",
            "patternProperties",
            "dependentSchemas",
        ] {
            if let Some(children) = fields.get(key).and_then(Value::as_object) {
                let mut mapped = Map::new();
                for (name, child) in children {
                    mapped.insert(name.clone(), visit(child, schemas)?);
                }
                out.insert(key.into(), Value::Object(mapped));
            }
        }
        Ok(Value::Object(out))
    }
    visit(schema, schemas)
}

fn property_is_write_only(
    schema: &Value,
    schemas: &BTreeMap<String, Value>,
) -> OpenApiResult<bool> {
    let view = binding_view(schema, schemas)?;
    Ok(view
        .get("writeOnly")
        .and_then(Value::as_bool)
        .unwrap_or(false))
}

fn binding_view<'a>(
    schema: &'a Value,
    schemas: &'a BTreeMap<String, Value>,
) -> OpenApiResult<Cow<'a, Value>> {
    fn visit<'a>(
        schema: &'a Value,
        schemas: &'a BTreeMap<String, Value>,
        active: &mut BTreeSet<String>,
    ) -> OpenApiResult<Cow<'a, Value>> {
        let Some(reference) = schema.get("$ref").and_then(Value::as_str) else {
            return Ok(Cow::Borrowed(schema));
        };
        if !active.insert(reference.to_string()) {
            return Err(OpenApiError(format!(
                "cyclic response schema alias has no read metadata shape: {reference}"
            )));
        }
        let name = reference_name(reference)?;
        let target = schemas.get(&name).ok_or_else(|| {
            OpenApiError(format!(
                "response schema reference target not found: {name}"
            ))
        })?;
        let base = visit(target, schemas, active)?;
        active.remove(reference);
        let fields = schema.as_object().unwrap();
        if fields.len() == 1 || base.as_ref() == &Value::Bool(false) {
            return Ok(base);
        }
        let mut merged = base.as_object().cloned().unwrap_or_default();
        for (key, value) in fields {
            if key != "$ref" {
                merged.insert(key.clone(), value.clone());
            }
        }
        Ok(Cow::Owned(Value::Object(merged)))
    }
    visit(schema, schemas, &mut BTreeSet::new())
}

fn required_response_set(
    schema: &Value,
    schemas: &BTreeMap<String, Value>,
) -> OpenApiResult<BTreeSet<String>> {
    let mut required = BTreeSet::new();
    for name in schema
        .get("required")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
    {
        let write_only = schema
            .get("properties")
            .and_then(Value::as_object)
            .and_then(|properties| properties.get(name))
            .map_or(Ok(false), |property| {
                property_is_write_only(property, schemas)
            })?;
        if !write_only {
            required.insert(name.to_string());
        }
    }
    Ok(required)
}

fn contains_schema(schema: &Value, target: &Value) -> bool {
    if schema == target {
        return true;
    }
    let Some(fields) = schema.as_object() else {
        return false;
    };
    for key in [
        "properties",
        "patternProperties",
        "$defs",
        "definitions",
        "dependentSchemas",
    ] {
        if fields
            .get(key)
            .and_then(Value::as_object)
            .is_some_and(|children| {
                children
                    .values()
                    .any(|child| contains_schema(child, target))
            })
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
        if fields
            .get(key)
            .is_some_and(|child| contains_schema(child, target))
        {
            return true;
        }
    }
    for key in ["allOf", "anyOf", "oneOf", "prefixItems"] {
        if fields
            .get(key)
            .and_then(Value::as_array)
            .is_some_and(|children| children.iter().any(|child| contains_schema(child, target)))
        {
            return true;
        }
    }
    false
}

fn collect_schema_references(schema: &Value, out: &mut BTreeSet<String>) -> OpenApiResult<()> {
    let Some(fields) = schema.as_object() else {
        return Ok(());
    };
    if let Some(reference) = fields.get("$ref").and_then(Value::as_str) {
        out.insert(reference_name(reference)?);
    }
    for key in [
        "properties",
        "patternProperties",
        "$defs",
        "definitions",
        "dependentSchemas",
    ] {
        if let Some(children) = fields.get(key).and_then(Value::as_object) {
            for child in children.values() {
                collect_schema_references(child, out)?;
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
        if let Some(child) = fields.get(key) {
            collect_schema_references(child, out)?;
        }
    }
    for key in ["allOf", "anyOf", "oneOf", "prefixItems"] {
        if let Some(children) = fields.get(key).and_then(Value::as_array) {
            for child in children {
                collect_schema_references(child, out)?;
            }
        }
    }
    Ok(())
}

fn is_json_response_media(media_type: &str) -> bool {
    let essence = media_type
        .split(';')
        .next()
        .unwrap_or(media_type)
        .trim()
        .to_ascii_lowercase();
    essence == "application/json" || essence.ends_with("+json")
}

fn has_binary_annotation(schema: &Value, schemas: &BTreeMap<String, Value>) -> OpenApiResult<bool> {
    fn visit(
        schema: &Value,
        schemas: &BTreeMap<String, Value>,
        visited: &mut BTreeSet<String>,
    ) -> OpenApiResult<bool> {
        let view = binding_view(schema, schemas)?;
        let string = view.get("type").is_some_and(|kind| {
            kind.as_str() == Some("string")
                || kind.as_array().is_some_and(|kinds| {
                    kinds.iter().any(|kind| kind.as_str() == Some("string"))
                        && kinds
                            .iter()
                            .all(|kind| matches!(kind.as_str(), Some("string" | "null")))
                })
        });
        if string
            && (view.get("format").and_then(Value::as_str) == Some("binary")
                || view.get("contentEncoding").and_then(Value::as_str) == Some("base64"))
        {
            return Ok(true);
        }
        let Some(fields) = schema.as_object() else {
            return Ok(false);
        };
        if let Some(reference) = fields.get("$ref").and_then(Value::as_str) {
            let name = reference_name(reference)?;
            if visited.insert(name.clone())
                && let Some(target) = schemas.get(&name)
                && visit(target, schemas, visited)?
            {
                return Ok(true);
            }
        }
        for key in ["allOf", "anyOf", "oneOf"] {
            if let Some(children) = fields.get(key).and_then(Value::as_array) {
                for child in children {
                    if visit(child, schemas, visited)? {
                        return Ok(true);
                    }
                }
            }
        }
        Ok(false)
    }
    visit(schema, schemas, &mut BTreeSet::new())
}

fn merge_all_of_object(
    schema: &Value,
    schemas: &BTreeMap<String, Value>,
    stack: &mut BTreeSet<String>,
) -> OpenApiResult<Value> {
    if let Some(reference) = schema.get("$ref").and_then(Value::as_str) {
        let name = reference_name(reference)?;
        if !stack.insert(name.clone()) {
            return Err(OpenApiError(
                "recursive allOf response flattening is unsupported".to_string(),
            ));
        }
        let target = schemas.get(&name).ok_or_else(|| {
            OpenApiError(format!(
                "response schema reference target not found: {name}"
            ))
        })?;
        let result = merge_all_of_object(target, schemas, stack);
        stack.remove(&name);
        return result;
    }
    let mut properties = Map::new();
    let mut required = BTreeSet::new();
    let mut object = schema.get("type").and_then(Value::as_str) == Some("object");
    if let Some(kind) = schema.get("type")
        && kind != &Value::String("object".to_string())
    {
        return Err(OpenApiError(
            "allOf response read model requires object branches".to_string(),
        ));
    }
    if let Some(branches) = schema.get("allOf").and_then(Value::as_array) {
        if branches.is_empty() {
            return Err(OpenApiError(
                "allOf response schema must contain at least one branch".to_string(),
            ));
        }
        for branch in branches {
            let merged = merge_all_of_object(branch, schemas, stack)?;
            object = true;
            if let Some(branch_properties) = merged.get("properties").and_then(Value::as_object) {
                for (key, value) in branch_properties {
                    if properties.get(key).is_some_and(|old| old != value) {
                        return Err(OpenApiError(format!(
                            "allOf response property {key} has incompatible read declarations"
                        )));
                    }
                    properties.insert(key.clone(), value.clone());
                }
            }
            required.extend(required_response_set(&merged, schemas)?);
        }
    }
    if let Some(fields) = schema.get("properties").and_then(Value::as_object) {
        for (key, value) in fields {
            if properties.get(key).is_some_and(|old| old != value) {
                return Err(OpenApiError(format!(
                    "allOf response property {key} has incompatible read declarations"
                )));
            }
            properties.insert(key.clone(), value.clone());
        }
    }
    required.extend(required_response_set(schema, schemas)?);
    if !object {
        return Err(OpenApiError(
            "allOf response read model requires object schemas".to_string(),
        ));
    }
    let mut out = Map::new();
    out.insert("type".into(), Value::String("object".into()));
    out.insert("properties".into(), Value::Object(properties));
    out.insert(
        "required".into(),
        Value::Array(required.into_iter().map(Value::String).collect()),
    );
    Ok(Value::Object(out))
}

fn render_string_enum(name: &str, pointer: &str, values: &[String]) -> String {
    let variants = unique_names(values.iter().map(|value| pascal(value)).collect(), &[], "");
    let mut out = format!(
        "/// Response string enum decoded from schema `{name}`.\n#[derive(Clone, Debug, Eq, PartialEq)]\npub enum {name} {{\n"
    );
    for (wire, variant) in values.iter().zip(&variants) {
        out.push_str(&format!(
            "    /// Wire value {}.\n    {variant},\n",
            rust_string(wire)
        ));
    }
    out.push_str("}\n\n");
    out.push_str(&format!(
        "impl DecodeResponseValue for {name} {{\n    fn decode_response_value(value: &JsonValue) -> Result<Self, String> {{\n        if !response_json_matches(value, {pointer:?}) {{ return Err(format!(\"response value does not match schema {pointer}\")); }}\n        let JsonValue::String(value) = value else {{ return Err(\"response value is not a string\".to_string()); }};\n        match value.as_str() {{\n"
    ));
    for (wire, variant) in values.iter().zip(&variants) {
        out.push_str(&format!(
            "            {} => Ok(Self::{variant}),\n",
            rust_string(wire)
        ));
    }
    out.push_str("            _ => Err(\"response string does not match a declared enum value\".to_string()),\n        }\n    }\n}\n\n");
    out
}

fn render_validated_wrapper(name: &str, pointer: &str) -> String {
    format!(
        "/// Response value validated against schema `{name}`.\n#[derive(Clone, Debug, PartialEq)]\npub struct {name}(JsonValue);\n\nimpl {name} {{\n    /// Borrow the complete validated response JSON value.\n    pub fn as_json(&self) -> &JsonValue {{ &self.0 }}\n}}\n\nimpl DecodeResponseValue for {name} {{\n    fn decode_response_value(value: &JsonValue) -> Result<Self, String> {{\n        if response_json_matches(value, {pointer:?}) {{ Ok(Self(value.clone())) }} else {{ Err(format!(\"response value does not match schema {pointer}\")) }}\n    }}\n}}\n\n"
    )
}

fn can_inline_ref_target(schema: &Value) -> bool {
    string_enum(schema).is_none()
        && schema.get("oneOf").is_none()
        && schema.get("anyOf").is_none()
        && schema.get("allOf").is_none()
        && matches!(
            schema.get("type").and_then(Value::as_str),
            Some("string" | "integer" | "number" | "boolean" | "null")
        )
}

fn is_nullable(schema: &Value) -> bool {
    schema
        .get("type")
        .and_then(Value::as_array)
        .is_some_and(|items| items.iter().any(|item| item.as_str() == Some("null")))
}

fn without_null_type(schema: &Value) -> OpenApiResult<Value> {
    let mut out = schema
        .as_object()
        .cloned()
        .ok_or_else(|| OpenApiError("nullable response schema must be an object".to_string()))?;
    let kinds = out.get("type").and_then(Value::as_array).ok_or_else(|| {
        OpenApiError("nullable response schema must have a type array".to_string())
    })?;
    let non_null = kinds
        .iter()
        .filter(|kind| kind.as_str() != Some("null"))
        .cloned()
        .collect::<Vec<_>>();
    match non_null.as_slice() {
        [kind] => {
            out.insert("type".into(), kind.clone());
            Ok(Value::Object(out))
        }
        [] => {
            out.insert("type".into(), Value::String("null".into()));
            Ok(Value::Object(out))
        }
        _ => {
            out.insert("type".into(), Value::Array(non_null));
            Ok(Value::Object(out))
        }
    }
}

fn string_enum(schema: &Value) -> Option<Vec<String>> {
    if schema.get("type").and_then(Value::as_str) != Some("string") {
        return None;
    }
    let values = schema
        .get("enum")?
        .as_array()?
        .iter()
        .map(|value| value.as_str().map(ToOwned::to_owned))
        .collect::<Option<Vec<_>>>()?;
    (!values.is_empty()).then_some(values)
}

fn composition_keyword(schema: &Value) -> Option<&'static str> {
    if schema.get("oneOf").is_some() {
        Some("oneOf")
    } else if schema.get("anyOf").is_some() {
        Some("anyOf")
    } else if schema.get("allOf").is_some() {
        Some("allOf")
    } else {
        None
    }
}

fn branch_variant_base(schema: &Value, ty: &str) -> String {
    schema
        .get("$ref")
        .and_then(Value::as_str)
        .and_then(|reference| reference_name(reference).ok())
        .map(|name| pascal(&name))
        .unwrap_or_else(|| {
            if ty == "JsonValue" {
                "Value".to_string()
            } else {
                ty.trim_start_matches("Option<")
                    .trim_end_matches('>')
                    .trim_start_matches("Box<")
                    .trim_end_matches('>')
                    .replace(['<', '>', ',', ' '], "")
            }
        })
}

fn unique_variant(seen: &mut BTreeSet<String>, base: &str) -> String {
    let base = pascal(base);
    if seen.insert(base.clone()) {
        return base;
    }
    for suffix in 2_u64.. {
        let candidate = format!("{base}{suffix}");
        if seen.insert(candidate.clone()) {
            return candidate;
        }
    }
    unreachable!()
}

fn field_names<'a>(
    wire_names: impl IntoIterator<Item = &'a str>,
    reserved: &[&str],
) -> Vec<String> {
    let bases = wire_names.into_iter().map(super::super::snake).collect();
    unique_names(bases, reserved, "_")
}

fn unique_names(bases: Vec<String>, reserved: &[&str], separator: &str) -> Vec<String> {
    let protected: BTreeSet<String> = bases
        .iter()
        .cloned()
        .chain(reserved.iter().map(|name| (*name).to_string()))
        .collect();
    let mut seen: BTreeSet<String> = reserved.iter().map(|name| (*name).to_string()).collect();
    bases
        .into_iter()
        .map(|base| {
            if seen.insert(base.clone()) {
                return base;
            }
            let name = (2_u64..)
                .map(|suffix| format!("{base}{separator}{suffix}"))
                .find(|candidate| !protected.contains(candidate) && !seen.contains(candidate))
                .unwrap();
            seen.insert(name.clone());
            name
        })
        .collect()
}

fn reference_name(reference: &str) -> OpenApiResult<String> {
    reference
        .strip_prefix("#/components/schemas/")
        .map(|name| name.replace("~1", "/").replace("~0", "~"))
        .ok_or_else(|| OpenApiError(format!("unsupported response schema reference {reference}")))
}

fn graph_pointer(name: &str) -> String {
    format!("#/$defs/{}", escape_pointer_token(name))
}

fn graph_pointer_name(pointer: &str) -> Option<String> {
    pointer
        .strip_prefix("#/$defs/")
        .map(|name| name.replace("~1", "/").replace("~0", "~"))
}

fn escape_pointer_token(value: &str) -> String {
    value.replace('~', "~0").replace('/', "~1")
}

fn pascal(value: &str) -> String {
    let mut out = String::new();
    let mut upper = true;
    for character in value.chars() {
        if character.is_ascii_alphanumeric() {
            if upper {
                out.push(character.to_ascii_uppercase());
                upper = false;
            } else {
                out.push(character);
            }
        } else {
            upper = true;
        }
    }
    if out.is_empty() {
        "Generated".to_string()
    } else if out.as_bytes()[0].is_ascii_digit() || out == "Self" {
        format!("Value{out}")
    } else {
        out
    }
}

fn rust_string(value: &str) -> String {
    format!("{value:?}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn binary_annotations_apply_to_the_response_value_not_its_children() {
        let binary = json!({"type":"string","format":"binary"});
        let schemas = BTreeMap::from([("Blob".to_string(), binary.clone())]);
        assert!(has_binary_annotation(&binary, &schemas).unwrap());
        assert!(
            has_binary_annotation(
                &json!({"allOf":[{"$ref":"#/components/schemas/Blob"},{"maxLength":3}]}),
                &schemas
            )
            .unwrap()
        );
        assert!(
            has_binary_annotation(
                &json!({"type":["string","null"],"format":"binary"}),
                &schemas
            )
            .unwrap()
        );
        for schema in [
            json!({"type":"object","properties":{"payload":binary}}),
            json!({"type":"array","items":{"$ref":"#/components/schemas/Blob"}}),
            json!({"type":"object","$defs":{"Unused":{"$ref":"#/components/schemas/Blob"}}}),
            json!({"not":{"$ref":"#/components/schemas/Blob"}}),
        ] {
            assert!(
                !has_binary_annotation(&schema, &schemas).unwrap(),
                "nested annotation changed the response representation: {schema}"
            );
        }
    }

    #[test]
    fn anonymous_read_models_reuse_schema_identity_across_labels() {
        let schemas = BTreeMap::new();
        let mut compiler = Compiler::new(&schemas, BTreeSet::new());
        let schema = json!({"type":"object","properties":{"value":{"type":"string"}}});
        let first = compiler.ensure_model("First", &schema).unwrap();
        let second = compiler.ensure_model("Second", &schema).unwrap();
        assert_eq!(
            first, second,
            "identical anonymous schemas must share a read model"
        );
        assert_eq!(compiler.models.len(), 1);
    }
}
