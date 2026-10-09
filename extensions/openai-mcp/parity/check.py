#!/usr/bin/env python3
"""Verify pinned upstream source hashes, public exports, and independent controls."""
import ast
import hashlib
import json
import re
from pathlib import Path
import subprocess
import sys
ROOT = Path(__file__).resolve().parent

def run_oracle(script, *args):
    result = subprocess.run(["node", "--import", "tsx", script, *args],
                            cwd=ROOT, text=True, capture_output=True, check=False)
    return result.returncode, result.stdout, result.stderr

def python_exports():
    exports = {}
    for path in sorted((ROOT / "upstream/python/src").rglob("__init__.py")):
        for node in ast.parse(path.read_text()).body:
            if isinstance(node, ast.Assign) and any(isinstance(t, ast.Name) and t.id == "__all__" for t in node.targets):
                exports[str(path.relative_to(ROOT / "upstream"))] = sorted(ast.literal_eval(node.value))
    return exports

def main():
    baseline = json.loads((ROOT / "upstream.json").read_text())
    snapshot = json.loads((ROOT / "exports.json").read_text())
    problems = []
    for item in baseline["files"]:
        path = ROOT / "upstream" / item["path"]
        if not path.is_file() or hashlib.sha256(path.read_bytes()).hexdigest() != item["sha256"]:
            problems.append({"source_mismatch": item["path"]})
    stylesheet = ROOT.parent / "crates/incurs-openai-mcp-app/styles.css"
    reference_css = ROOT / "upstream/typescript/styles.css"
    if stylesheet.read_bytes() != reference_css.read_bytes():
        problems.append({"stylesheet_source_mismatch": True})
    code, stdout, stderr = run_oracle("oracle.mjs", "--inventory")
    inventory = json.loads(stdout) if code == 0 else {}
    if code:
        problems.append({"inventory_error": stderr})
    if inventory != snapshot["typescript"]:
        problems.append({"typescript_export_drift": True})
    python = python_exports()
    if python != snapshot["python"]:
        problems.append({"python_export_drift": True})
    for script, args in [("oracle.mjs", ["--fixtures"]), ("server-oracle.mjs", [])]:
        code, stdout, stderr = run_oracle(script, *args)
        if code:
            problems.append({"oracle_failed": script, "stdout": stdout, "stderr": stderr})
    if "--source-only" not in sys.argv:
        coverage_path = ROOT / "coverage.json"
        if not coverage_path.exists():
            problems.append({"coverage_missing": "coverage.json"})
        else:
            coverage = json.loads(coverage_path.read_text())
            expected = {(module, item["name"]) for module, items in inventory.items() for item in items}
            actual = {(item["module"], item["upstream"]) for item in coverage["typescript"]}
            if actual != expected:
                problems.append({"unmapped_exports": sorted(expected - actual), "unexpected_mappings": sorted(actual - expected)})
            expected_python = {(module, name) for module, names in python.items() for name in names}
            actual_python = {(item["module"], item["upstream"]) for item in coverage["python"]}
            if actual_python != expected_python:
                problems.append({"unmapped_python_exports": sorted(expected_python - actual_python)})
            for item in coverage["typescript"] + coverage["python"]:
                if not item.get("rust") or not item.get("evidence"):
                    problems.append({"incomplete_mapping": item["upstream"]})
                    continue
                source = ROOT.parents[2] / item["source"]
                symbol = re.escape(item["rust"])
                if not source.is_file() or not re.search(r"\bpub (?:async )?(?:fn|struct|enum|type|const|trait) " + symbol + r"\b", source.read_text()):
                    problems.append({"missing_rust_symbol": item["rust"], "source": item["source"]})
                for evidence in item["evidence"]:
                    if not (ROOT.parents[2] / evidence).is_file():
                        problems.append({"missing_evidence": evidence})
    print(json.dumps({"revision": baseline["revision"], "source_files": len(baseline["files"]),
                      "typescript_exports": sum(map(len, inventory.values())),
                      "python_exports": sum(map(len, python.values())), "problems": problems}))
    return bool(problems)

if __name__ == "__main__":
    sys.exit(main())
