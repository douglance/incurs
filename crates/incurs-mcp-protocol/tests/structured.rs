use incurs_mcp_protocol::structured::{
    McpStructuredShape, project_output_schema, project_structured_content, projection_metadata,
    restore_output_schema, restore_structured_content,
};
use serde_json::{Map, Value, json};

fn metadata() -> Map<String, Value> {
    projection_metadata(McpStructuredShape::WrappedValue).unwrap()
}

#[test]
fn object_schemas_are_identity_projected() {
    let schema = json!({
        "type": "object",
        "properties": {
            "name": { "type": "string" }
        },
        "required": ["name"]
    });

    let projection = project_output_schema(&schema);

    assert_eq!(projection.shape, McpStructuredShape::Object);
    assert_eq!(projection.schema, schema);
    assert_eq!(projection_metadata(projection.shape), None);
    assert_eq!(
        project_structured_content(json!({ "name": "comsat" }), projection.shape).unwrap(),
        json!({ "name": "comsat" })
    );
}

#[test]
fn arrays_and_scalars_are_wrapped_and_restored() {
    for schema in [
        json!({ "type": "array", "items": { "type": "string" } }),
        json!({ "type": "string" }),
    ] {
        let projection = project_output_schema(&schema);

        assert_eq!(projection.shape, McpStructuredShape::WrappedValue);
        assert_eq!(
            projection.schema,
            json!({
                "type": "object",
                "properties": {
                    "data": schema
                },
                "required": ["data"],
                "additionalProperties": false
            })
        );
        assert_eq!(
            restore_output_schema(projection.schema.clone(), Some(&metadata())),
            schema
        );
    }

    let value = json!(["record"]);
    let projected = project_structured_content(value.clone(), McpStructuredShape::WrappedValue)
        .expect("wrapped values should project");
    assert_eq!(projected, json!({ "data": value }));
    assert_eq!(
        restore_structured_content(projected, Some(&metadata())),
        json!(["record"])
    );
}

#[test]
fn anonymous_root_dialect_keywords_are_promoted_to_wrapper_root() {
    let schema = json!({
        "$schema": "https://json-schema.org/draft/2020-12/schema",
        "$vocabulary": {
            "https://json-schema.org/draft/2020-12/vocab/core": true
        },
        "type": "array",
        "items": { "type": "string" }
    });

    let projected = project_output_schema(&schema).schema;

    assert_eq!(
        projected.pointer("/$schema"),
        Some(&json!("https://json-schema.org/draft/2020-12/schema"))
    );
    assert_eq!(
        projected.pointer("/$vocabulary/https:~1~1json-schema.org~1draft~12020-12~1vocab~1core"),
        Some(&json!(true))
    );
    assert_eq!(projected.pointer("/properties/data/$schema"), None);
    assert_eq!(projected.pointer("/properties/data/$vocabulary"), None);
    assert_eq!(restore_output_schema(projected, Some(&metadata())), schema);
}

#[test]
fn mixed_nullable_unions_and_references_are_conservatively_wrapped() {
    for schema in [
        json!({ "type": ["object", "null"] }),
        json!({ "anyOf": [{ "type": "object" }, { "type": "null" }] }),
        json!({ "$ref": "#/$defs/output", "$defs": { "output": { "type": "object" } } }),
    ] {
        let projection = project_output_schema(&schema);

        assert_eq!(projection.shape, McpStructuredShape::WrappedValue);
        assert_eq!(
            restore_output_schema(projection.schema, Some(&metadata())),
            schema
        );
    }
}

#[test]
fn local_json_pointers_rebase_inside_schema_keywords() {
    let schema = json!({
        "type": "array",
        "items": { "$ref": "#/$defs/item" },
        "contains": { "$dynamicRef": "#/$defs/item" },
        "$defs": {
            "item": {
                "type": "object",
                "properties": {
                    "child": { "$ref": "#/$defs/item" },
                    "anchored": { "$ref": "#node" },
                    "external": { "$ref": "https://example.com/schema.json#/$defs/item" }
                }
            }
        }
    });

    let projected = project_output_schema(&schema).schema;

    assert_eq!(
        projected.pointer("/properties/data/items/$ref"),
        Some(&json!("#/properties/data/$defs/item"))
    );
    assert_eq!(
        projected.pointer("/properties/data/contains/$dynamicRef"),
        Some(&json!("#/properties/data/$defs/item"))
    );
    assert_eq!(
        projected.pointer("/properties/data/$defs/item/properties/child/$ref"),
        Some(&json!("#/properties/data/$defs/item"))
    );
    assert_eq!(
        projected.pointer("/properties/data/$defs/item/properties/anchored/$ref"),
        Some(&json!("#node"))
    );
    assert_eq!(
        projected.pointer("/properties/data/$defs/item/properties/external/$ref"),
        Some(&json!("https://example.com/schema.json#/$defs/item"))
    );
    assert_eq!(restore_output_schema(projected, Some(&metadata())), schema);
}

#[test]
fn root_self_and_property_references_round_trip_exactly() {
    let schema = json!({
        "allOf": [{ "$ref": "#" }],
        "properties": {
            "nested": { "$ref": "#/properties/nested" }
        }
    });

    let projected = project_output_schema(&schema).schema;

    assert_eq!(
        projected.pointer("/properties/data/allOf/0/$ref"),
        Some(&json!("#/properties/data"))
    );
    assert_eq!(
        projected.pointer("/properties/data/properties/nested/$ref"),
        Some(&json!("#/properties/data/properties/nested"))
    );
    assert_eq!(restore_output_schema(projected, Some(&metadata())), schema);
}

#[test]
fn schema_resources_with_ids_are_not_rebased() {
    let root_id_schema = json!({
        "$schema": "https://json-schema.org/draft/2020-12/schema",
        "$id": "https://example.com/root.schema.json",
        "$ref": "#/$defs/item",
        "$defs": {
            "item": { "type": "string" }
        }
    });
    let root_projected = project_output_schema(&root_id_schema).schema;
    assert_eq!(root_projected.pointer("/$schema"), None);
    assert_eq!(
        root_projected.pointer("/properties/data/$schema"),
        Some(&json!("https://json-schema.org/draft/2020-12/schema"))
    );
    assert_eq!(
        root_projected.pointer("/properties/data/$ref"),
        Some(&json!("#/$defs/item"))
    );
    assert_eq!(
        restore_output_schema(root_projected, Some(&metadata())),
        root_id_schema
    );

    let nested_id_schema = json!({
        "type": "array",
        "items": {
            "$id": "nested.schema.json",
            "$ref": "#/$defs/item",
            "$defs": {
                "item": { "type": "string" }
            }
        }
    });
    let nested_projected = project_output_schema(&nested_id_schema).schema;
    assert_eq!(
        nested_projected.pointer("/properties/data/items/$ref"),
        Some(&json!("#/$defs/item"))
    );
    assert_eq!(
        restore_output_schema(nested_projected, Some(&metadata())),
        nested_id_schema
    );
}

#[test]
fn references_inside_arbitrary_data_are_untouched() {
    let schema = json!({
        "type": "string",
        "const": { "$ref": "#/$defs/item" },
        "default": { "$dynamicRef": "#/$defs/item" },
        "examples": [{ "$ref": "#/$defs/item" }],
        "enum": [{ "$ref": "#/$defs/item" }],
        "x-extension": { "$ref": "#/$defs/item" }
    });

    let projected = project_output_schema(&schema).schema;

    assert_eq!(
        projected.pointer("/properties/data/const/$ref"),
        Some(&json!("#/$defs/item"))
    );
    assert_eq!(
        projected.pointer("/properties/data/default/$dynamicRef"),
        Some(&json!("#/$defs/item"))
    );
    assert_eq!(
        projected.pointer("/properties/data/examples/0/$ref"),
        Some(&json!("#/$defs/item"))
    );
    assert_eq!(
        projected.pointer("/properties/data/enum/0/$ref"),
        Some(&json!("#/$defs/item"))
    );
    assert_eq!(
        projected.pointer("/properties/data/x-extension/$ref"),
        Some(&json!("#/$defs/item"))
    );
    assert_eq!(restore_output_schema(projected, Some(&metadata())), schema);
}

#[test]
fn restore_does_not_infer_natural_data_wrappers() {
    let natural = json!({ "data": ["natural"] });
    let schema = json!({
        "type": "object",
        "properties": {
            "data": { "type": "array" }
        }
    });
    let mut wrong = Map::new();
    wrong.insert(
        "io.incurs.outputProjection".to_string(),
        json!({ "version": 1, "shape": "other", "field": "data", "schemaRefBase": "#/properties/data" }),
    );

    assert_eq!(restore_structured_content(natural.clone(), None), natural);
    assert_eq!(
        restore_structured_content(natural.clone(), Some(&wrong)),
        natural
    );
    assert_eq!(restore_output_schema(schema.clone(), None), schema);
    assert_eq!(restore_output_schema(schema.clone(), Some(&wrong)), schema);
    assert_eq!(
        restore_structured_content(json!({ "value": ["not-data"] }), Some(&metadata())),
        json!({ "value": ["not-data"] })
    );
}

#[test]
fn wrong_wrappers_with_unrecognized_root_keywords_are_untouched() {
    let schema = json!({
        "$schema": "https://json-schema.org/draft/2020-12/schema",
        "type": "object",
        "properties": {
            "data": { "type": "array" }
        },
        "required": ["data"],
        "additionalProperties": false,
        "title": "not a projection wrapper"
    });

    assert_eq!(
        restore_output_schema(schema.clone(), Some(&metadata())),
        schema
    );
}

#[test]
fn object_projection_rejects_non_object_content() {
    let error = project_structured_content(json!(["not-object"]), McpStructuredShape::Object)
        .expect_err("object projection must reject non-object values");

    assert_eq!(
        error.to_string(),
        "object-shaped MCP structured output must be a JSON object"
    );
}
