#!/usr/bin/env python3
"""Compare accepted form answers with the pinned TypeScript SDK validator."""
import ast
import json
from pathlib import Path
import subprocess
import sys
ROOT = Path(__file__).resolve().parent
REPO = ROOT.parents[2]
def literal_assignment(name):
    for node in ast.walk(ast.parse((ROOT / "helper-differential.py").read_text())):
        if isinstance(node, ast.Assign) and any(isinstance(t, ast.Name) and t.id == name for t in node.targets):
            return ast.literal_eval(node.value)
    raise RuntimeError("Missing literal corpus " + name)
def exchange(argv, requests):
    result = subprocess.run(argv, cwd=ROOT, input="".join(json.dumps(r) + "\n" for r in requests),
        text=True, capture_output=True, check=False)
    if result.returncode:
        raise RuntimeError(json.dumps({"command": argv, "exit": result.returncode, "stderr": result.stderr[-8000:]}))
    observations = [json.loads(line) for line in result.stdout.splitlines() if line.strip()]
    if len(observations) != len(requests):
        raise RuntimeError("Oracle response count mismatch")
    return observations
def main():
    requests = []
    fields = literal_assignment("fields")
    fields.extend([{"type": "string", "pattern": "(?<=a)b"}, {"type": "string", "pattern": r"^(a)\\1$"},
        {"type": "string", "minLength": 1, "maxLength": 1}, {"type": "string", "default": "x"}])
    values = literal_assignment("values") + ["ab", "aa", "😀", "𐐀"]
    for field in fields:
        for required in [[], ["x"]]:
            form = {"type": "object", "properties": {"x": field}, "required": required}
            content_values = [{}, {"unexpected": "extra"}]
            content_values.extend({"x": value} for value in values)
            for content in content_values:
                requests.append({"schema": "OpenAIFormContentSchema", "form": form, "value": content})
    expected = exchange(["node", "--import", "tsx", "oracle.mjs"], requests)
    actual = exchange(["cargo", "run", "--quiet", "--locked", "--manifest-path",
        str(REPO / "extensions/openai-mcp/Cargo.toml"), "-p", "incurs-openai-mcp-protocol",
        "--example", "schema_oracle"], requests)
    failures = [{"index": i, "request": r, "expected": e, "actual": a}
        for i, (r, e, a) in enumerate(zip(requests, expected, actual))
        if e.get("ok") != a.get("ok") or (e.get("ok") and e.get("value") != a.get("value"))]
    print(json.dumps({"cases": len(requests), "failures": failures}))
    return bool(failures)
if __name__ == "__main__":
    try:
        sys.exit(main())
    except Exception as error:
        print(json.dumps({"error": str(error)}))
        sys.exit(1)
