use incurs_openai_mcp_protocol::{
    OpenAIFormField, OpenAIResourceSelection, OpenAIUserResourceOptions, Resource,
    SUPPORTED_SCHEMA_NAMES, complete_field_submission, file_input, is_valid_value_with_options,
    prepare_field_submission, resource_input, validate_file_selections, validate_form_content,
    validate_form_selections, validate_schema,
};
use serde::Deserialize;
use serde_json::Value;

#[derive(Debug, Deserialize)]
struct Fixture {
    name: String,
    schema: String,
    value: Value,
    expected: bool,
}

#[test]
fn validates_pinned_literal_fixtures() {
    let fixtures: Vec<Fixture> =
        serde_json::from_str(include_str!("../../../parity/fixtures.json"))
            .expect("fixtures are valid JSON");
    let mut mismatches = Vec::new();
    for fixture in fixtures {
        let actual = validate_schema(&fixture.schema, &fixture.value).is_ok();
        if actual != fixture.expected {
            mismatches.push(format!(
                "{} {} expected {} got {}",
                fixture.name, fixture.schema, fixture.expected, actual
            ));
        }
    }
    assert!(mismatches.is_empty(), "{}", mismatches.join("\n"));
}

#[test]
fn registry_covers_all_named_schemas() {
    let mut names = SUPPORTED_SCHEMA_NAMES.to_vec();
    names.sort_unstable();
    names.dedup();
    assert_eq!(names.len(), SUPPORTED_SCHEMA_NAMES.len());
    for name in [
        "OpenAIFileEntrypointInputSchema",
        "OpenAIMentionSearchResultSchema",
        "OpenAIUiEntrypointSchema",
        "OpenAISettingsReadResultSchema",
        "OpenAIFormSchema",
        "OpenAIResourceWriteParamsSchema",
        "OpenAIMessageParamsSchema",
    ] {
        assert!(SUPPORTED_SCHEMA_NAMES.contains(&name));
    }
}

#[test]
fn settings_layout_rejects_duplicate_or_unknown_properties() {
    let value = serde_json::json!({
        "schema": {"type":"object", "properties": {"units": {"type":"string"}}},
        "layout": [{"kind":"group", "title":"General", "items":[
            {"kind":"property", "property":"units"},
            {"kind":"property", "property":"units"}
        ]}],
        "values": {"units":"mm"}
    });
    assert!(validate_schema("OpenAISettingsReadResultSchema", &value).is_err());
}

#[test]
fn resource_write_requires_exactly_one_body_slot() {
    let value = serde_json::json!({"uri":"host-resource://part", "blob":"YWJj", "text":"abc"});
    assert!(validate_schema("OpenAIResourceWriteParamsSchema", &value).is_err());

    let value = serde_json::json!({"uri":"host-resource://part", "blob":"YWJj"});
    assert!(validate_schema("OpenAIResourceWriteParamsSchema", &value).is_ok());

    let value = serde_json::json!({"uri":"host-resource://part", "blob":""});
    assert!(validate_schema("OpenAIResourceWriteParamsSchema", &value).is_ok());
}

#[test]
fn optional_object_metadata_schemas_reject_arrays() {
    for schema in [
        "OpenAIResourceToolCallMetadataSchema",
        "OpenAIUiResourceMetadataSchema",
        "OpenAIResourceContentMetadataSchema",
        "OpenAIResourceMetadataSchema",
        "OpenAIResourceReadMetadataSchema",
    ] {
        assert!(
            validate_schema(schema, &serde_json::json!([])).is_err(),
            "{schema}"
        );
    }
}

#[test]
fn optional_metadata_properties_reject_null_values() {
    for (schema, value) in [
        (
            "OpenAIResourceToolCallMetadataSchema",
            serde_json::json!({"openai/resource": null}),
        ),
        (
            "OpenAIResourceReadMetadataSchema",
            serde_json::json!({"openai/resource": {"representation": null}}),
        ),
        (
            "OpenAIResourceMetadataSchema",
            serde_json::json!({"etag": null}),
        ),
        (
            "OpenAIResourceContentMetadataSchema",
            serde_json::json!({"openai/resource": null}),
        ),
        (
            "OpenAIUiResourceMetadataSchema",
            serde_json::json!({"availableDisplayModes": null}),
        ),
        (
            "OpenAIUiToolMetadataSchema",
            serde_json::json!({"entrypoints": null}),
        ),
        (
            "OpenAIMessageOptionsSchema",
            serde_json::json!({"target": null}),
        ),
        (
            "OpenAIMessageParamsSchema",
            serde_json::json!({"role":"user", "content":[], "_meta": null}),
        ),
    ] {
        assert!(
            validate_schema(schema, &value).is_err(),
            "{schema} accepted {value}"
        );
    }
}

#[test]
fn form_field_defaults_are_type_checked_without_constraint_checking() {
    for value in [
        serde_json::json!({"type":"string", "default": 0}),
        serde_json::json!({"type":"boolean", "default": ""}),
        serde_json::json!({"type":"integer", "default": true}),
    ] {
        assert!(validate_schema("OpenAIFormFieldSchema", &value).is_err());
    }

    let value = serde_json::json!({"type":"string", "format":"email", "default":"a"});
    assert!(validate_schema("OpenAIFormFieldSchema", &value).is_ok());
}

#[test]
fn form_file_defaults_must_name_supplied_resources() {
    let value = serde_json::json!({
        "type":"string",
        "format":"uri",
        "default":"host-resource://missing",
        "x-openai-input": {
            "type":"resource",
            "options":[{"uri":"host-resource://known", "name":"Known"}]
        }
    });
    assert!(validate_schema("OpenAIFileFormFieldSchema", &value).is_err());
}

#[test]
fn ui_entrypoints_return_trimmed_zod_values() {
    let value = serde_json::json!({"type":"file", "extensions":[" .stl "]});
    let normalized = validate_schema("OpenAIUiEntrypointSchema", &value).unwrap();
    assert_eq!(
        normalized,
        serde_json::json!({"type":"file", "extensions":[".stl"]})
    );

    let value = serde_json::json!({"type":"settings", "searchTerms":[" units "]});
    let normalized = validate_schema("OpenAIUiEntrypointSchema", &value).unwrap();
    assert_eq!(
        normalized,
        serde_json::json!({"type":"settings", "searchTerms":["units"]})
    );
}

#[test]
fn form_content_checks_field_constraints_without_defaults() {
    let form = serde_json::json!({
        "type":"object",
        "required":["name", "qty", "tags"],
        "properties":{
            "name":{"type":"string", "minLength":2, "maxLength":4, "oneOf":[{"const":"bolt", "title":"Bolt"}]},
            "qty":{"type":"integer", "minimum":1, "maximum":9},
            "tags":{"type":"array", "minItems":1, "maxItems":2, "uniqueItems":true, "items":{"type":"string", "enum":["a", "b"]}}
        }
    });
    assert!(
        validate_form_content(
            &form,
            &serde_json::json!({"name":"bolt", "qty":2, "tags":["a"]})
        )
        .is_ok()
    );
    assert!(
        validate_form_content(
            &form,
            &serde_json::json!({"name":"x", "qty":2, "tags":["a"]})
        )
        .is_err()
    );
    assert!(
        validate_form_content(
            &form,
            &serde_json::json!({"name":"bolt", "qty":10, "tags":["a"]})
        )
        .is_err()
    );
    assert!(
        validate_form_content(
            &form,
            &serde_json::json!({"name":"bolt", "qty":2, "tags":["a", "a"]})
        )
        .is_err()
    );
}

#[test]
fn form_content_validates_supported_patterns() {
    let form = serde_json::json!({
        "type":"object",
        "properties":{"name":{"type":"string", "pattern":"^a+$"}}
    });
    assert!(validate_form_content(&form, &serde_json::json!({"name":"aaa"})).is_ok());
    assert!(validate_form_content(&form, &serde_json::json!({"name":"bbb"})).is_err());
}

#[test]
fn form_content_counts_string_length_by_unicode_scalar_values() {
    let form = serde_json::json!({
        "type":"object",
        "properties":{"name":{"type":"string", "minLength":2, "maxLength":4}}
    });
    assert!(validate_form_content(&form, &serde_json::json!({"name":"😀"})).is_err());

    let form = serde_json::json!({
        "type":"object",
        "properties":{"name":{"type":"string", "minLength":1, "maxLength":1}}
    });
    assert!(validate_form_content(&form, &serde_json::json!({"name":"𐐀"})).is_ok());
}

#[test]
fn form_content_allows_user_resource_uris_when_declared() {
    let form = serde_json::json!({
        "type":"object",
        "properties":{"file":{"type":"string", "format":"uri", "x-openai-input":{"type":"file", "options":[], "userOptions":{"kind":"file"}}}}
    });
    assert!(
        validate_form_content(
            &form,
            &serde_json::json!({"file":"host-resource://uploaded"})
        )
        .is_ok()
    );
}

#[test]
fn form_content_normalizes_whole_float_integers() {
    let form = serde_json::json!({
        "type":"object",
        "properties":{"count":{"type":"integer", "minimum":-1, "maximum":2}}
    });
    let value: Value = serde_json::from_str("{\"count\":1.0}").unwrap();
    let normalized = validate_form_content(&form, &value).unwrap();
    assert_eq!(normalized, serde_json::json!({"count":1}));
}

#[test]
fn form_content_validates_ecmascript_patterns_against_javascript_controls() {
    struct Case {
        pattern: &'static str,
        value: &'static str,
        expected: bool,
    }

    let cases = [
        Case {
            pattern: r"(?=a)a",
            value: "a",
            expected: true,
        },
        Case {
            pattern: r"(?<=foo)bar",
            value: "foobar",
            expected: true,
        },
        Case {
            pattern: r"(?<=foo)bar",
            value: "bar",
            expected: false,
        },
        Case {
            pattern: r"(\w+) \1",
            value: "go go",
            expected: true,
        },
        Case {
            pattern: r"^(?!tmp-).+",
            value: "tmp-file",
            expected: false,
        },
        Case {
            pattern: r"^.$",
            value: "💙",
            expected: true,
        },
        Case {
            pattern: r"^(💙)\1$",
            value: "💙💙",
            expected: true,
        },
    ];

    for case in cases {
        assert_eq!(
            javascript_pattern_result(case.pattern, case.value),
            case.expected,
            "JavaScript control changed for {:?}",
            case.pattern
        );
        let form = serde_json::json!({
            "type":"object",
            "properties":{"name":{"type":"string", "pattern": case.pattern}}
        });
        assert_eq!(
            validate_form_content(&form, &serde_json::json!({"name": case.value})).is_ok(),
            case.expected,
            "Rust validation diverged from JavaScript for {:?}",
            case.pattern
        );
    }
}

fn javascript_pattern_result(pattern: &str, value: &str) -> bool {
    let script = format!(
        "const re = new RegExp({}, 'u'); process.exit(re.test({}) ? 0 : 1);",
        serde_json::to_string(pattern).unwrap(),
        serde_json::to_string(value).unwrap()
    );
    let status = std::process::Command::new("node")
        .arg("-e")
        .arg(script)
        .status()
        .expect("node is required for JavaScript regex controls");
    status.success()
}

#[test]
fn form_content_validates_array_item_patterns() {
    let form = serde_json::json!({
        "type":"object",
        "properties":{"names":{"type":"array", "items":{"type":"string", "pattern":"^item-[0-9]+$"}}}
    });
    assert!(
        validate_form_content(&form, &serde_json::json!({"names":["item-1", "item-20"]})).is_ok()
    );
    assert!(validate_form_content(&form, &serde_json::json!({"names":["item-a"]})).is_err());
}

#[test]
fn form_schema_normalization_strips_unknown_zod_object_keys() {
    let value = serde_json::json!({
        "type":"object",
        "unknown":"drop-me",
        "properties":{
            "name":{"type":"string", "unknown":"drop-me"}
        }
    });
    let normalized = validate_schema("OpenAIFormSchema", &value).unwrap();
    assert_eq!(
        normalized,
        serde_json::json!({"type":"object", "properties":{"name":{"type":"string"}}})
    );
}

#[test]
fn resource_input_builders_match_python_shape() {
    let options = vec![Resource {
        uri: "host-resource://known".to_string(),
        name: Some("Known".to_string()),
        mime_type: None,
        extra: Default::default(),
    }];
    let metadata = resource_input(
        options.clone(),
        Some(OpenAIResourceSelection::Implicit),
        Some(OpenAIUserResourceOptions {
            kind: None,
            accept: Some(vec![".stl".to_string()]),
        }),
    )
    .unwrap();
    assert_eq!(metadata["x-openai-input"]["type"], "resource");
    assert_eq!(metadata["x-openai-input"]["selection"], "implicit");
    assert_eq!(
        metadata["x-openai-input"]["userOptions"]["accept"][0],
        ".stl"
    );

    let metadata = file_input(options, None, None).unwrap();
    assert_eq!(metadata["x-openai-input"]["type"], "file");
}

#[test]
fn field_submission_helpers_validate_upload_flow() {
    let field: OpenAIFormField = serde_json::from_value(serde_json::json!({
        "type":"array",
        "items":{"type":"string", "format":"uri"},
        "minItems":1,
        "x-openai-input": {
            "type":"resource",
            "options":[{"uri":"host-resource://known"}],
            "userOptions":{"accept":[".txt"]}
        }
    }))
    .unwrap();
    let mut content = std::collections::BTreeMap::new();
    content.insert(
        "files".to_string(),
        serde_json::json!(["host-resource://known"]),
    );
    assert_eq!(
        prepare_field_submission(&field, "files", &content, 1).unwrap(),
        Some(vec![".txt".to_string()])
    );
    let completed = complete_field_submission(
        &field,
        "files",
        &content,
        vec!["host-resource://upload".to_string()],
    )
    .unwrap();
    assert_eq!(
        completed,
        serde_json::json!(["host-resource://known", "host-resource://upload"])
    );
}

#[test]
fn file_selection_helper_allows_user_files_when_declared() {
    let schema = serde_json::from_value(serde_json::json!({
        "type":"object",
        "properties":{
            "file":{
                "type":"string",
                "format":"uri",
                "x-openai-input": {"type":"resource", "options":[], "userOptions":{}}
            }
        }
    }))
    .unwrap();
    let mut content = std::collections::BTreeMap::new();
    content.insert(
        "file".to_string(),
        serde_json::json!("host-resource://user"),
    );
    validate_file_selections(&schema, &content).unwrap();
}

#[test]
fn python_helper_is_valid_value_rejects_pattern_fields() {
    let field: OpenAIFormField = serde_json::from_value(serde_json::json!({
        "type":"string",
        "pattern":"^a+$"
    }))
    .unwrap();
    assert!(is_valid_value_with_options(&field, &serde_json::json!("aaa"), 0, &[]).is_err());
    assert!(validate_form_content(
        &serde_json::json!({"type":"object", "properties":{"name":{"type":"string", "pattern":"^a+$"}}}),
        &serde_json::json!({"name":"aaa"})
    )
    .is_ok());
}

#[test]
fn python_helper_counts_string_lengths_as_python_code_points() {
    let field: OpenAIFormField = serde_json::from_value(serde_json::json!({
        "type":"string",
        "minLength":2,
        "maxLength":4
    }))
    .unwrap();
    assert!(!is_valid_value_with_options(&field, &serde_json::json!("😀"), 0, &[]).unwrap());
}

#[test]
fn form_selection_helper_only_validates_option_fields() {
    let schema = serde_json::from_value(serde_json::json!({
        "type":"object",
        "properties":{
            "free":{"type":"string", "minLength": 3},
            "choice":{"type":"string", "oneOf":[{"const":"a", "title":"A"}, {"const":"b", "title":"B"}]}
        }
    }))
    .unwrap();
    let mut content = std::collections::BTreeMap::new();
    content.insert("free".to_string(), serde_json::json!("x"));
    content.insert("choice".to_string(), serde_json::json!("a"));
    validate_form_selections(&schema, &content).unwrap();
    content.insert("choice".to_string(), serde_json::json!("z"));
    validate_form_selections(&schema, &content).unwrap_err();
}

#[test]
fn model_context_content_blocks_reject_nested_arrays() {
    let valid = serde_json::json!({"updateId":"u", "content":[{"type":"text", "text":"hello"}]});
    assert!(validate_schema("OpenAIModelContextHostStateSchema", &valid).is_ok());

    let invalid =
        serde_json::json!({"updateId":"u", "content":[[{"type":"text", "text":"hello"}]]});
    assert!(validate_schema("OpenAIModelContextHostStateSchema", &invalid).is_err());
}
