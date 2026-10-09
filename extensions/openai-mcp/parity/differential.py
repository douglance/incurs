#!/usr/bin/env python3
"""Compare Rust normalized schema results with the independently pinned SDK."""
import json
import copy
from pathlib import Path
import subprocess
import sys
ROOT = Path(__file__).resolve().parent
REPO = ROOT.parents[2]

def exchange(argv, requests):
    text = "".join(json.dumps(request, ensure_ascii=False) + "\n" for request in requests)
    result = subprocess.run(argv, input=text, text=True, capture_output=True, cwd=ROOT, check=False)
    if result.returncode:
        raise RuntimeError(json.dumps({"command": argv, "exit": result.returncode, "stderr": result.stderr[-12000:]}))
    observations = [json.loads(line) for line in result.stdout.splitlines() if line.strip()]
    if len(observations) != len(requests):
        raise RuntimeError(json.dumps({"command": argv, "expected_responses": len(requests), "actual": len(observations)}))
    return observations

def variants(value):
    """Change supplied data without deriving expected answers from Rust."""
    if not isinstance(value, dict):
        return []
    output = []
    for key, child in value.items():
        for replacement in [None, [], {}, "", 0, True]:
            changed = copy.deepcopy(value)
            changed[key] = replacement
            output.append(changed)
        changed = copy.deepcopy(value)
        del changed[key]
        output.append(changed)
        if isinstance(child, dict):
            for mutation in variants(child):
                changed = copy.deepcopy(value)
                changed[key] = mutation
                output.append(changed)
    return output

def main():
    inventory = json.loads((ROOT / "exports.json").read_text())["typescript"]
    literals = json.loads((ROOT / "fixtures.json").read_text())
    samples = [None, True, False, 0, -1, 1.5, "", " ", " x ", "text", "blob", [], {}, {"unexpected": True}]
    samples.extend(fixture["value"] for fixture in literals)
    samples.extend([
        {"type": "global"}, {"type": "thread"}, {"type": "settings", "searchTerms": [" units "]},
        {"type": "file", "extensions": [" .stl "]}, {"type": "global", "unknown": True},
        {"query": "bolt", "unknown": True}, {"type": "resource", "resourceUri": " cad://part ", "title": " Part "},
        {"path": ""}, {"path": "/workspace/a"}, {"url": "/parts"}, {"url": "relative"},
        {"updateId": "id", "content": []}, {"target": "new"}, {"target": "active", "send": False},
        {"role": "user", "content": []}, {"role": "user", "content": [], "_meta": {"openai/message": {}}},
        {"etag": "v1", "writable": True}, {"etag": 2}, {"outcome": "too-large", "maxBytes": 100.0},
        {"uri": "u", "blob": ""}, {"uri": "u", "text": "", "unknown": True},
        {"openai/resource": {"representation": "text"}}, {"openai/resource": {"path": "/workspace/a"}},
        {"openai/modelContext": {"updateId": "id"}},
        {"kind": "property", "property": ""}, {"kind": "tool", "tool": "go", "title": " Go "},
        {"kind": "group", "title": "Group", "items": []},
        {"title": " Grid ", "description": "D"},
        {"schema": {"type": "object", "properties": {}}, "values": {}},
        {"type": "object", "properties": {"name": {"type": "string", "pattern": "^a+$"}}, "required": ["name"]},
        {"type": "string", "enum": ["a", "b"]},
        {"type": "string", "oneOf": [{"const": "a", "title": "A", "description": "D"}]},
        {"type": "array", "items": {"type": "string", "enum": ["a", "b"]}, "minItems": 1},
        {"type": "string", "format": "uri", "x-openai-input": {"type": "resource", "options": []}},
    ])
    samples.extend([
        {"entrypoints": [{"type": "global"}, {"type": "file", "extensions": [".stl"]}], "preferredModelDisplayMode": "inline"},
        {"availableDisplayModes": ["inline", "fullscreen", "pip"], "preferredDisplayMode": "pip"},
        {"type": "boolean", "default": False},
        {"type": "integer", "minimum": -1, "maximum": 2, "default": 0},
        {"type": "number", "minimum": 1.25, "maximum": 2.5},
        {"type": "string", "minLength": 1, "maxLength": 4, "format": "email", "pattern": "^a+$", "default": "a"},
        {"type": "string", "enum": ["a", "b"], "enumNames": ["A", "B"]},
        {"type": "array", "items": {"anyOf": [{"const": "a", "title": "A", "x-openai-thumbnail": {"src": "https://example.test/a"}}]}, "minItems": 0, "maxItems": 2},
        {"type": "array", "items": {"type": "string", "minLength": 2}, "uniqueItems": True},
        {"type": "string", "x-openai-input": {"type": "file", "accept": [".stl"]}},
        {"type": "object", "properties": {}},
        {"query": "", "items": []},
        {"items": [{"type": "resource_link", "uri": "cad://part", "name": "Part", "mimeType": "text/plain"}]},
        {"readTool": "settings.read", "updateTool": "settings.update"},
        {"kind": "property", "property": "units"},
        {"kind": "tool", "tool": "preferences.open"},
        {"kind": "group", "title": "Units", "items": [{"kind": "property", "property": "units"}]},
        {"schema": {"type": "object", "properties": {"units": {"type": "string", "enum": ["mm", "in"]}}, "required": ["units"]}, "values": {"units": "mm"}},
        {"values": {"units": "mm"}},
        {"action": "accept", "content": {"value": "a", "enabled": True, "amount": 1.5, "tags": ["a"]}},
        {"content": [{"type": "text", "text": "Context"}], "structuredContent": {"part": 1}, "updateId": "u1"},
        {"openai/modelContext": {"updateId": "u1"}},
        {"uri": "host-resource://file", "blob": "AA==", "ifMatch": "v1"},
        {"uri": "host-resource://file", "text": "", "ifMatch": "v1"},
        {"type": "resource", "resourceUri": "cad://part", "title": "Part", "subtitle": "Bolt"},
        {"path": "/projects/part", "state": {"part": 1}},
    ])
    samples += [mutation for value in list(samples) for mutation in variants(value)]
    # Preserve a deterministic, unique corpus.
    samples = list({json.dumps(value, sort_keys=True): value for value in samples}.values())
    requests = []
    for module, items in inventory.items():
        for item in items:
            if item["name"].endswith("Schema") and not item["name"].startswith("create"):
                requests.extend({"module": module, "schema": item["name"], "value": value} for value in samples)
    expected = exchange(["node", "--import", "tsx", "oracle.mjs"], requests)
    rust_command = ["cargo", "run", "--quiet", "--locked", "--manifest-path",
                    str(REPO / "extensions/openai-mcp/Cargo.toml"), "-p",
                    "incurs-openai-mcp-protocol", "--example", "schema_oracle"]
    actual = exchange(rust_command, requests)
    failures = []
    positive = {module + ':' + item['name']: 0 for module, items in inventory.items() for item in items if item['name'].endswith('Schema') and not item['name'].startswith('create')}
    for index, (request, wanted, observed) in enumerate(zip(requests, expected, actual)):
        if wanted.get("ok"):
            positive[request["module"] + ":" + request["schema"]] += 1
        if wanted.get("ok") != observed.get("ok") or (wanted.get("ok") and wanted.get("value") != observed.get("value")):
            failures.append({"index": index, "request": request, "expected": wanted, "actual": observed})
    missing_positive = [name for name, count in positive.items() if count == 0]
    print(json.dumps({"cases": len(requests), "positive_controls": positive, "missing_positive": missing_positive, "failures": failures}))
    return bool(failures or missing_positive)

if __name__ == "__main__":
    try:
        sys.exit(main())
    except Exception as error:
        print(json.dumps({"error": str(error)}))
        sys.exit(1)
