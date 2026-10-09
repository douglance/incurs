"""Build a pinned Rust example, then run its compiler-reported executable directly."""
import json
import os
from pathlib import Path
import subprocess

def cargo_example(repo: Path, name: str) -> list[str]:
    build = subprocess.run(
        [os.environ.get("CARGO", "cargo"), "build", "--quiet", "--locked",
         "--message-format=json", "--manifest-path",
         str(repo / "extensions/openai-mcp/Cargo.toml"),
         "-p", "incurs-openai-mcp-protocol", "--example", name],
        cwd=repo, stdin=subprocess.DEVNULL, text=True, capture_output=True,
        check=False, timeout=180,
    )
    if build.returncode:
        raise RuntimeError(json.dumps({"example": name, "exit": build.returncode,
                                      "stderr": build.stderr[-8000:]}))
    for line in build.stdout.splitlines():
        try:
            artifact = json.loads(line)
        except json.JSONDecodeError:
            continue
        if (artifact.get("reason") == "compiler-artifact"
                and artifact.get("target", {}).get("name") == name
                and artifact.get("executable")):
            executable = Path(artifact["executable"])
            if not executable.is_file():
                raise RuntimeError("Missing compiler-reported executable: " + str(executable))
            return [str(executable)]
    raise RuntimeError("No compiler-reported executable for " + name)

def exchange_jsonl(argv, requests, cwd: Path):
    """Exchange a finite fixture corpus without relying on inherited pipe lifetimes."""
    import tempfile
    with tempfile.TemporaryDirectory(prefix="incurs-oracle-") as temporary:
        directory = Path(temporary)
        source = directory / "requests.jsonl"
        output = directory / "responses.jsonl"
        errors = directory / "stderr.txt"
        source.write_text("".join(json.dumps(item, ensure_ascii=False) + "\n"
                                  for item in requests), encoding="utf-8")
        with source.open("r", encoding="utf-8") as stdin, output.open("w", encoding="utf-8") as stdout, errors.open("w", encoding="utf-8") as stderr:
            result = subprocess.run(argv, cwd=cwd, stdin=stdin, stdout=stdout, stderr=stderr,
                                    text=True, check=False, timeout=120)
        if result.returncode:
            raise RuntimeError(json.dumps({"command": argv, "exit": result.returncode,
                                          "stderr": errors.read_text()[-12000:]}))
        observations = [json.loads(line) for line in output.read_text().splitlines() if line.strip()]
        if len(observations) != len(requests):
            raise RuntimeError(json.dumps({"command": argv, "expected_responses": len(requests),
                                          "actual_responses": len(observations)}))
        return observations
