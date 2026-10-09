#!/usr/bin/env python3
"""Compare public Rust form helpers with pinned Python SDK observations."""
import json
from pathlib import Path
import subprocess
import sys
from rust_oracle import cargo_example, exchange_jsonl
ROOT = Path(__file__).resolve().parent
REPO = ROOT.parents[2]
def exchange(argv, requests):
    return exchange_jsonl(argv, requests, ROOT)

def main():
    fields = [
        {"type": "string"}, {"type": "string", "minLength": 2, "maxLength": 4},
        {"type": "string", "pattern": "^a+$"}, {"type": "string", "format": "uri"},
        {"type": "string", "format": "email"}, {"type": "string", "format": "date"},
        {"type": "string", "format": "date-time"},
        {"type": "string", "enum": ["a", "b"]},
        {"type": "string", "oneOf": [{"const": "a", "title": "A"}, {"const": "b", "title": "B"}]},
        {"type": "integer", "minimum": -1, "maximum": 2},
        {"type": "number", "minimum": -1, "maximum": 2}, {"type": "boolean"},
        {"type": "array", "items": {"type": "string"}, "minItems": 1, "maxItems": 3},
        {"type": "array", "items": {"type": "string"}, "uniqueItems": True},
        {"type": "array", "items": {"type": "string", "enum": ["a", "b"]}},
        {"type": "array", "items": {"anyOf": [{"const": "a", "title": "A"}]}},
        {"type": "string", "format": "uri", "x-openai-input": {"type": "resource", "options": [{"uri": "cad://part", "name": "Part"}]}},
        {"type": "string", "format": "uri", "x-openai-input": {"type": "file", "options": [], "userOptions": {"kind": "file", "accept": [".stl"]}}},
        {"type": "array", "items": {"type": "string", "format": "uri"}, "minItems": 1, "maxItems": 2,
         "x-openai-input": {"type": "file", "options": [{"uri": "cad://part", "name": "Part"}], "userOptions": {"kind": "file"}}},
    ]
    values = [None, True, False, 0, 1, 1.0, 1.5, -2, 3, "", "a", "ab", "aaaaa", "😀",
        "a@example.test", "not email", "cad://part", "host-resource://uploaded", "relative", "https://example.test",
        "2024-02-29", "2024-02-30", "2024-01-01T01:02:03Z", "2024-01-01T01:02:03",
        [], ["a"], ["a", "a"], ["a", "b"], ["cad://part"], ["host-resource://uploaded"], {}]
    requests = []
    for field in fields:
        for value in values:
            requests.append({"operation": "is_valid_value", "field": field, "value": value})
            requests.append({"operation": "prepare_field_submission", "field": field, "name": "x", "content": {"x": value}})
        for content in [{}, {"x": []}, {"x": ["cad://part"]}, {"x": "cad://part"}]:
            for pending in [0, 1, 2, 3]:
                requests.append({"operation": "prepare_field_submission", "field": field, "name": "x", "content": content, "pending_uploads": pending})
            for uploaded in [[], ["host-resource://uploaded"], ["host-resource://one", "host-resource://two"]]:
                requests.append({"operation": "complete_field_submission", "field": field, "name": "x", "content": content, "uploaded_uris": uploaded})
        for value in values:
            form = {"type": "object", "properties": {"x": field}, "required": ["x"]}
            for operation in ["validate_file_selections", "validate_form_selections"]:
                requests.append({"operation": operation, "form": form, "content": {"x": value}})
    expected = exchange(["uv", "run", "--no-project", "--with-requirements", "python-requirements.txt", "python", "python-oracle.py"], requests)
    actual = exchange(cargo_example(REPO, "helper_oracle"), requests)
    failures = [{"index": index, "request": request, "expected": wanted, "actual": observed}
        for index, (request, wanted, observed) in enumerate(zip(requests, expected, actual)) if wanted != observed]
    print(json.dumps({"cases": len(requests), "failures": failures}))
    return bool(failures)
if __name__ == "__main__":
    try:
        sys.exit(main())
    except Exception as error:
        print(json.dumps({"error": str(error)}))
        sys.exit(1)
