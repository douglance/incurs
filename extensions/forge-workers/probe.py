"""Exercise a running Emscripten Worker; exit nonzero on any behavioral failure."""
import argparse
import concurrent.futures
import json
import pathlib
import runpy
import subprocess
from build import rust_environment
import time
import urllib.error
import urllib.request


def request(base, path, body=None):
    data = None if body is None else json.dumps(body).encode()
    req = urllib.request.Request(
        base + path, data=data, headers={"content-type": "application/json"}
    )
    with urllib.request.urlopen(req, timeout=10) as response:
        return json.load(response)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("base", nargs="?", default="http://127.0.0.1:8797")
    args = parser.parse_args()
    health = request(args.base, "/health")
    assert health == {"ok": True, "target": "emscripten", "durable_files": False}, health

    fixture_path = pathlib.Path(__file__).resolve().parent / "fixtures/proof-openapi.json"
    fixture = json.loads(fixture_path.read_text())
    actual_contract = request(args.base, "/resolve", fixture)
    cargo, env = rust_environment("stable", "native-proof")
    native = subprocess.run(
        [cargo, "run", "--quiet", "--locked", "-j", "2", "--manifest-path",
         str(fixture_path.parents[2] / "forge/Cargo.toml"), "--example", "resolve",
         "--", str(fixture_path)],
        env=env, capture_output=True, text=True, timeout=180, check=True,
    )
    expected_contract = json.loads(native.stdout)
    assert actual_contract == expected_contract, {
        "worker": actual_contract, "native": expected_contract
    }
    operations = {item["name"]: item for item in actual_contract["contract"]["operations"]}
    assert set(operations) == {"listWidgets", "updateWidget"}, operations
    update = operations["updateWidget"]
    assert update["method"] == "POST" and update["path"] == "/widgets/{id}", update
    assert set(update["responses"]) == {"200", "204", "400"}, update
    assert update["responses"]["204"]["content"] == {}, update
    assert "application/problem+json" in update["responses"]["400"]["content"], update
    assert "X-Request-Id" in update["responses"]["200"]["headers"], update
    parameters = {(item["location"], item["name"]): item for item in update["parameters"]}
    assert parameters[("path", "id")]["required"] is True, parameters
    assert parameters[("query", "dry_run")]["schema"]["default"] is False, parameters
    assert ("header", "X-Trace") in parameters, parameters
    assert update["request_body"]["required"] is True, update
    assert len(actual_contract["digest"]) == 64, actual_contract["digest"]
    # Literal expectations also catch a shared native/Worker resolver defect.
    assert update["servers"][0]["url"] == "https://api.example.test/v1", update
    binding_document = json.loads(json.dumps(fixture))
    item = binding_document["paths"]["/widgets/{id}"]
    item["servers"] = [{"url": "https://path.example.test/v0"}]
    item["post"]["servers"] = [{
        "url": "https://{region}.operations.example.test/{version}",
        "variables": {
            "region": {"default": "us", "enum": ["us", "eu"]},
            "version": {"default": "v2"}
        }
    }]
    bind_input = {
        "document": binding_document, "operation": "updateWidget",
        "arguments": {
            "path": {"id": "a/b c"}, "query": {"dry_run": True},
            "header": {"X-Trace": "worker-binding"},
            "body": {"name": "Worker", "note": None}
        }
    }
    bound = request(args.base, "/bind", bind_input)
    assert bound["method"] == "POST", bound
    assert bound["url"] == "https://us.operations.example.test/v2/widgets/a%2Fb%20c?dry_run=true", bound
    assert bound["headers"] == [["X-Trace", "worker-binding"], ["Content-Type", "application/json"]], bound
    assert json.loads(bytes(bound["body"])) == {"name": "Worker", "note": None}, bound
    selected = request(args.base, "/bind", dict(bind_input, server_variables={"region": "eu", "version": "v3"}))
    assert selected["url"] == "https://eu.operations.example.test/v3/widgets/a%2Fb%20c?dry_run=true", selected
    for path_style, query_style, path_wire, query_wire in [
        ("matrix", "pipeDelimited", ";id=blue", "blue%7Cblack"),
        ("label", "pipeDelimited", ".blue", "blue%7Cblack"),
        ("matrix", "spaceDelimited", ";id=blue", "blue%20black"),
        ("label", "spaceDelimited", ".blue", "blue%20black"),
    ]:
        styled = json.loads(json.dumps(bind_input))
        styled_item = styled["document"]["paths"]["/widgets/{id}"]
        styled_item["parameters"][0]["style"] = path_style
        styled_item["post"]["parameters"].append({
            "name": "color", "in": "query", "style": query_style, "explode": False,
            "schema": {"type": "array", "items": {"type": "string"}}
        })
        styled["arguments"]["path"]["id"] = "blue"
        styled["arguments"]["query"]["color"] = ["blue", "black"]
        actual = request(args.base, "/bind", styled)
        assert actual["url"] == (
            "https://us.operations.example.test/v2/widgets/" + path_wire +
            "?color=" + query_wire + "&dry_run=true"
        ), actual
    media_probe = runpy.run_path(str(fixture_path.parents[2] / "forge/probe_cli.py"))
    media_document = json.loads((fixture_path.parents[2] / "forge/tests/fixtures/media_openapi.json").read_text())
    media_cases = media_probe["media_cases"]()
    for name, arguments, (path, content_type, expected) in media_cases:
        actual = request(args.base, "/bind", {"document": media_document, "operation": name, "arguments": arguments})
        assert actual["method"] == "POST" and actual["url"] == "https://example.test" + path, actual
        assert actual["headers"] == ([] if content_type is None else [["Content-Type", content_type]]), actual
        if content_type is None:
            assert actual["body"] is None, actual
        else:
            assert bytes(actual["body"]) == expected, actual
    invalid_media = media_probe["invalid_media_cases"]()
    for name, arguments in invalid_media:
        try:
            request(args.base, "/bind", {"document": media_document, "operation": name, "arguments": arguments})
            raise AssertionError("invalid media input was accepted")
        except urllib.error.HTTPError as error:
            assert error.code == 400, error.code

    text_path = fixture_path.parents[2] / "forge/tests/fixtures/text_openapi.json"
    text_document = json.loads(text_path.read_text())
    text_document["servers"] = [{"url": "https://text.example.test"}]
    for value in ['hello\n"雪"\0', ""]:
        text_result = request(args.base, "/bind", {
            "document": text_document, "operation": "sendText",
            "arguments": {"body": value},
        })
        assert text_result["method"] == "POST", text_result
        assert text_result["url"] == "https://text.example.test/text", text_result
        assert text_result["headers"] == [["Content-Type", "text/plain"]], text_result
        assert bytes(text_result["body"]) == value.encode("utf-8"), text_result
    omitted = request(args.base, "/bind", {
        "document": text_document, "operation": "sendText", "arguments": {},
    })
    assert omitted["body"] is None and omitted["headers"] == [], omitted
    form_document = json.loads((fixture_path.parents[2] / "forge/tests/fixtures/form_openapi.json").read_text())
    form_document["servers"] = [{"url": "https://form.example.test"}]
    form_bound = request(args.base, "/bind", {
        "document": form_document, "operation": "sendForm",
        "arguments": {"body": {"text": "a+b &雪", "count": 9007199254740993,
            "tags": ["red blue", "+&"], "address": {"city": "New York"},
            "quoted": "123", "packed": ["a,b", "c d"]}},
    })
    assert form_bound["method"] == "POST" and form_bound["url"] == "https://form.example.test/form", form_bound
    assert form_bound["headers"] == [["Content-Type", "application/x-www-form-urlencoded"]], form_bound
    assert bytes(form_bound["body"]) == b"address=%7B%22city%22%3A%22New+York%22%7D&count=9007199254740993&packed=a%2Cb,c%20d&quoted=%22123%22&tags=red%20blue&tags=%2B%26&text=a%2Bb+%26%E9%9B%AA", form_bound
    multipart_document = json.loads((fixture_path.parents[2] / "forge/tests/fixtures/multipart_openapi.json").read_text())
    multipart_document["servers"] = [{"url": "https://multipart.example.test"}]
    multipart_bound = request(args.base, "/bind", {
        "document": multipart_document, "operation": "sendMultipart",
        "arguments": {"body": {"text":"雪 --incurs-forge-0","count":9007199254740993,
            "encoded":"AP8B","tags":["red blue","+&"],"packed":["a","b"],"address":{"city":"New York"}}},
    })
    assert multipart_bound["method"] == "POST" and multipart_bound["url"] == "https://multipart.example.test/multipart", multipart_bound
    content_type = dict(multipart_bound["headers"])["Content-Type"]
    check_multipart = runpy.run_path(str(fixture_path.parents[2] / "forge/probe_cli.py"))["check_multipart_request"]
    check_multipart([("/multipart", bytes(multipart_bound["body"]), content_type)])
    graph_document = json.loads(json.dumps(multipart_document))
    graph_media = graph_document["paths"]["/multipart"]["post"]["requestBody"]["content"]["multipart/form-data"]
    graph_schemas = graph_document.setdefault("components", {}).setdefault("schemas", {})
    graph_schemas["GraphUpload"] = graph_media["schema"]
    graph_schemas["GraphEncoded"] = graph_schemas["GraphUpload"]["properties"]["encoded"]
    graph_schemas["GraphUpload"]["properties"]["encoded"] = {"$ref": "#/components/schemas/GraphEncoded"}
    graph_media["schema"] = {"$ref": "#/components/schemas/GraphUpload"}
    resolved_graph = request(args.base, "/resolve", graph_document)["contract"]
    graph_operation = next(op for op in resolved_graph["operations"] if op["name"] == "sendMultipart")
    assert graph_operation["request_body"]["content"]["multipart/form-data"] == {"$ref": "#/components/schemas/GraphUpload"}
    assert resolved_graph["schemas"]["GraphUpload"]["properties"]["encoded"] == {"$ref": "#/components/schemas/GraphEncoded"}
    assert resolved_graph["schemas"]["GraphEncoded"]["contentEncoding"] == "base64"
    graph_bound = request(args.base, "/bind", {
        "document": graph_document, "operation": "sendMultipart",
        "arguments": {"body": {"text":"雪 --incurs-forge-0","count":9007199254740993,
            "encoded":"AP8B","tags":["red blue","+&"],"packed":["a","b"],"address":{"city":"New York"}}},
    })
    check_multipart([("/multipart", bytes(graph_bound["body"]), dict(graph_bound["headers"])["Content-Type"])])
    v32_document = json.loads((fixture_path.parents[2] / "forge/tests/fixtures/openapi32.json").read_text())
    v32_document["servers"] = [{"url": "https://v32.example.test"}]
    found = request(args.base, "/bind", {
        "document": v32_document, "operation": "findItems",
        "arguments": {"querystring": {"filter": {"term": "a + b/é", "labels": ["red", "blue"]}},
                      "cookie": {"prefs": {"mode": "dark", "token": "a%2Fb"}}},
    })
    assert found["method"] == "QUERY", found
    assert found["url"] == "https://v32.example.test/find?labels=red&labels=blue&term=a+%2B+b%2F%C3%A9", found
    assert found["headers"] == [["Cookie", "mode=dark; token=a%2Fb"]] and found["body"] is None, found
    copied = request(args.base, "/bind", {
        "document": v32_document, "operation": "copyItem",
        "arguments": {"path": {"id": "a/b"}, "header": {"X-Meta": {"id": 9007199254740993}}},
    })
    assert copied["method"] == "x-Copy" and copied["url"] == "https://v32.example.test/copy/a%2Fb", copied
    assert copied["headers"] == [["X-Meta", '{"id":9007199254740993}']], copied
    for raw, expected in [({}, "https://v32.example.test/raw"), ({"querystring": {"raw": ""}}, "https://v32.example.test/raw?")]:
        result = request(args.base, "/bind", {"document": v32_document, "operation": "readRaw", "arguments": raw})
        assert result["url"] == expected, result
    try:
        request(args.base, "/bind", {"document": v32_document, "operation": "readRaw", "arguments": {"querystring": {"raw": "x=a#fragment"}}})
        raise AssertionError("query fragment injection was accepted")
    except urllib.error.HTTPError as error:
        assert error.code == 400, error.code
    schema_path = fixture_path.parents[2] / "forge/tests/fixtures/schema_openapi.json"
    schema_document = json.loads(schema_path.read_text())
    schema_document["servers"] = [{"url": "https://schema.example.test"}]
    schema_document["paths"]["/forbidden"]["post"]["requestBody"]["content"]["application/json"]["schema"] = {"$ref": "#/components/schemas/Nothing"}
    schema_values = {
        "id": 9223372036854775807, "minimum": -9223372036854775808,
        "ids": [9007199254740993], "label": "Worker", "enabled": True,
        "amount": 1.25, "anything": {"allowed": True},
    }
    schema_bound = request(args.base, "/bind", {
        "document": schema_document, "operation": "sendSchema",
        "arguments": {"body": schema_values},
    })
    assert schema_bound["url"] == "https://schema.example.test/schema", schema_bound
    assert json.loads(bytes(schema_bound["body"])) == schema_values, schema_bound
    for value in [None, False, 0, "", [], {}]:
        try:
            request(args.base, "/bind", {
                "document": schema_document, "operation": "sendImpossible",
                "arguments": {"body": value},
            })
            raise AssertionError("false body schema was accepted")
        except urllib.error.HTTPError as error:
            assert error.code == 400, error.code
    for invalid, status in [
        (dict(bind_input, server_variables={"region": "outside"}), 400),
        (dict(bind_input, server_index=1), 400),
        (dict(bind_input, operation="absent"), 400),
        ({"large": "x" * 131_072}, 413),
    ]:
        try:
            request(args.base, "/bind", invalid)
            raise AssertionError("invalid binding was accepted")
        except urllib.error.HTTPError as error:
            assert error.code == status, error.code
    for document, status in [({}, 400), ({"large": "x" * 131_072}, 413)]:
        try:
            request(args.base, "/resolve", document)
            raise AssertionError("invalid document was accepted")
        except urllib.error.HTTPError as error:
            assert error.code == status, error.code

    import sys
    sys.path.insert(0, str(fixture_path.parents[2] / "forge"))
    from probe_request_validation import VALID, OTHER, invalid_cases
    validation_document = json.loads((fixture_path.parents[2] / "forge/tests/fixtures/request_validation_openapi.json").read_text())
    validation_inputs = invalid_cases()
    for arguments in validation_inputs:
        try:
            request(args.base, "/bind", {"document": validation_document, "operation": "submit", "arguments": arguments})
            raise AssertionError("schema-invalid request was accepted")
        except urllib.error.HTTPError as error:
            assert error.code == 400, error.code
            assert "request validation failed" in error.read().decode()
    for arguments, url, expected_body in [
        (VALID, "https://example.test/submit?limit=3", VALID["body"]),
        (OTHER, "https://example.test/submit", {"other": True}),
    ]:
        actual = request(args.base, "/bind", {"document": validation_document, "operation": "submit", "arguments": arguments})
        assert actual["url"] == url, actual
        assert json.loads(bytes(actual["body"])) == expected_body, actual

    def probe(index):
        token = f"request-{index}-independent-content"
        actual = request(args.base, "/prove", {"token": token, "delay_ms": 100})
        assert actual["token"] == token, actual
        assert actual["file_roundtrip"] is True, actual
        assert actual["file_removed"] is True, actual
        assert actual["task_sum"] == 5, actual
        assert actual["elapsed_ms"] >= 80, actual
        assert actual["target"] == "emscripten", actual
        return actual

    start = time.monotonic()
    with concurrent.futures.ThreadPoolExecutor(max_workers=8) as pool:
        results = list(pool.map(probe, range(8)))
    elapsed_ms = (time.monotonic() - start) * 1000
    # A parked shared event loop would serialize eight 100ms waits.
    assert elapsed_ms < 700, {"elapsed_ms": elapsed_ms, "results": results}
    try:
        request(args.base, "/prove", {"token": "invalid", "delay_ms": 1001})
        raise AssertionError("oversized delay was accepted")
    except urllib.error.HTTPError as error:
        assert error.code == 400, error.code
    print(json.dumps({
        "passed": True,
        "requests": len(results),
        "request_schema_rejections": len(validation_inputs),
        "validated_request_cases": 2,
        "media": {"bindings": len(media_cases), "rejections": len(invalid_media)},
        "concurrent_elapsed_ms": elapsed_ms,
        "verified": ["emscripten", "temporary-files", "task-spawn", "timers", "concurrency", "limits", "native-worker-contract-parity", "declared-server-binding", "parameter-styles", "text-body-presence", "integer-body-preservation", "false-body-schema", "urlencoded-form-wire", "multipart-MIME-parser", "openapi32-QUERY", "custom-method-case", "querystring-content", "cookie-style", "parameter-content", "shared-schema-graph", "referenced-multipart-MIME-parser", "false-schema-alias"],
        "default_switch_eligible": False,
        "note": "Runtime proof only; production parity and comparative performance gate not run.",
    }, indent=2))


if __name__ == "__main__":
    main()
