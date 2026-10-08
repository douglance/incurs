//! Effective server declarations survive resolution before request binding.
use incurs_openapi::{ResolveOptions, resolve_document};
use serde_json::{Value, json};

fn document() -> Value {
    json!({
        "openapi":"3.1.1","info":{"title":"Servers","version":"1"},
        "servers":[{"url":"https://root.test/v1"}],
        "paths":{
            "/root":{"get":{"operationId":"root","responses":{"200":{"description":"OK"}}}},
            "/path":{
                "servers":[{"url":"https://path.test/v2"}],
                "get":{"operationId":"path","responses":{"200":{"description":"OK"}}},
                "post":{"operationId":"operation","servers":[
                    {"url":"https://{region}.operation.test/{version}",
                     "description":"Regional endpoint",
                     "variables":{
                        "region":{"default":"us","enum":["us","eu"]},
                        "version":{"default":"v3"}
                     }},
                    {"url":"https://backup.test/v4"}
                ],"responses":{"200":{"description":"OK"}}}
            }
        }
    })
}

#[test]
fn preserves_effective_servers_and_variables_with_operation_path_root_precedence() {
    let contract = resolve_document(&document(), ResolveOptions::new("proof")).unwrap();
    for (name, expected) in [
        ("root", "https://root.test/v1"),
        ("path", "https://path.test/v2"),
        ("operation", "https://{region}.operation.test/{version}"),
    ] {
        let operation = contract
            .operations
            .iter()
            .find(|op| op.name == name)
            .unwrap();
        let wire = serde_json::to_value(operation).unwrap();
        assert_eq!(wire["servers"][0]["url"], expected, "{name}");
        if name == "operation" {
            assert_eq!(wire["servers"][1]["url"], "https://backup.test/v4");
            assert_eq!(
                wire["servers"][0]["variables"]["region"]["enum"],
                json!(["us", "eu"])
            );
            assert_eq!(wire["servers"][0]["variables"]["version"]["default"], "v3");
        }
    }
}

#[test]
fn absent_or_empty_root_servers_preserve_the_spec_default() {
    for servers in [None, Some(json!([]))] {
        let mut doc = document();
        doc.as_object_mut().unwrap().remove("servers");
        if let Some(value) = servers {
            doc["servers"] = value;
        }
        let contract = resolve_document(&doc, ResolveOptions::new("proof")).unwrap();
        let operation = contract
            .operations
            .iter()
            .find(|op| op.name == "root")
            .unwrap();
        let wire = serde_json::to_value(operation).unwrap();
        assert_eq!(wire["servers"][0]["url"], "/");
    }
}

#[test]
fn expands_defaults_and_explicit_selection_without_changing_operation_identity() {
    use incurs_openapi::servers::{ServerSelection, build_operation_request, select_server_url};
    let contract = resolve_document(&document(), ResolveOptions::new("proof")).unwrap();
    let operation = contract
        .operations
        .iter()
        .find(|op| op.name == "operation")
        .unwrap();
    let defaults = ServerSelection::default();
    assert_eq!(
        select_server_url(operation, &defaults).unwrap(),
        "https://us.operation.test/v3"
    );
    let request =
        build_operation_request(operation, &contract.schemas, &defaults, &json!({})).unwrap();
    assert_eq!(request.method, "POST");
    assert_eq!(request.url, "https://us.operation.test/v3/path");
    let selection = ServerSelection {
        variables: [
            ("region".into(), "eu".into()),
            ("version".into(), "v9".into()),
        ]
        .into(),
        ..Default::default()
    };
    assert_eq!(
        select_server_url(operation, &selection).unwrap(),
        "https://eu.operation.test/v9"
    );
    let backup = ServerSelection {
        index: 1,
        ..Default::default()
    };
    assert_eq!(
        select_server_url(operation, &backup).unwrap(),
        "https://backup.test/v4"
    );
    assert_eq!(operation.id, "proof/operation");

    let mut changed = document();
    changed["paths"]["/path"]["post"]["servers"][0]["variables"]["version"]["default"] =
        json!("v4");
    let changed = resolve_document(&changed, ResolveOptions::new("proof")).unwrap();
    assert_ne!(
        incurs_openapi::contract_digest(&contract).unwrap(),
        incurs_openapi::contract_digest(&changed).unwrap()
    );
}

#[test]
fn relative_servers_require_an_explicit_document_base() {
    use incurs_openapi::servers::{ServerSelection, build_operation_request, select_server_url};
    for (server, expected) in [
        ("../api", "https://docs.test/api/root"),
        ("/v2", "https://docs.test/v2/root"),
        ("/", "https://docs.test/root"),
        ("//service.test/v3", "https://service.test/v3/root"),
    ] {
        let mut doc = document();
        doc["servers"] = json!([{"url":server}]);
        let contract = resolve_document(&doc, ResolveOptions::new("proof")).unwrap();
        let op = contract
            .operations
            .iter()
            .find(|op| op.name == "root")
            .unwrap();
        assert!(
            select_server_url(op, &ServerSelection::default())
                .unwrap_err()
                .to_string()
                .contains("requires document_url")
        );
        let selection = ServerSelection {
            document_url: Some("https://docs.test/spec/openapi.json".into()),
            ..Default::default()
        };
        assert_eq!(
            build_operation_request(op, &contract.schemas, &selection, &json!({}))
                .unwrap()
                .url,
            expected
        );
    }
}

#[test]
fn server_selection_rejects_invalid_indices_variables_and_urls() {
    use incurs_openapi::servers::{ServerSelection, select_server_url};
    let contract = resolve_document(&document(), ResolveOptions::new("proof")).unwrap();
    let operation = contract
        .operations
        .iter()
        .find(|op| op.name == "operation")
        .unwrap();
    for selection in [
        ServerSelection {
            index: 2,
            ..Default::default()
        },
        ServerSelection {
            variables: [("unknown".into(), "x".into())].into(),
            ..Default::default()
        },
        ServerSelection {
            variables: [("region".into(), "outside".into())].into(),
            ..Default::default()
        },
        ServerSelection {
            variables: [("version".into(), "{region}".into())].into(),
            ..Default::default()
        },
        ServerSelection {
            variables: [("version".into(), "v3\r\n".into())].into(),
            ..Default::default()
        },
    ] {
        assert!(select_server_url(operation, &selection).is_err());
    }
    for invalid in [
        "https://{missing}.test",
        "https://{unclosed.test",
        "https://bad}.test",
        "file:///tmp",
        "https://example.test?secret=x",
        "https://example.test/#fragment",
    ] {
        let mut operation = operation.clone();
        operation.servers[0].url = invalid.into();
        assert!(
            select_server_url(&operation, &ServerSelection::default()).is_err(),
            "{invalid}"
        );
    }
}

#[test]
fn resolver_rejects_malformed_server_contracts() {
    for servers in [
        json!({"url":"https://example.test"}),
        json!([{}]),
        json!([{"url":"https://{x}.test","variables":{"x":{}}}]),
        json!([{"url":"https://{x}.test","variables":{"x":{"default":"a","enum":[]}}}]),
        json!([{"url":"https://{x}.test","variables":{"x":{"default":"a","enum":["b"]}}}]),
    ] {
        let mut doc = document();
        doc["servers"] = servers;
        assert!(resolve_document(&doc, ResolveOptions::new("proof")).is_err());
    }
}
