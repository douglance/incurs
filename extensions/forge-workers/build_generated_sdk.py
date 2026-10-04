"""Build current SDK archives and their Emscripten consumer in a new directory."""
import argparse
import hashlib
import json
import pathlib
import subprocess

from build import ROOT, TOOLCHAIN, rust_environment

FORGE = ROOT.parent / "forge"


def run(args, *, cwd, env):
    subprocess.run([str(arg) for arg in args], cwd=cwd, env=env, check=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("output", type=pathlib.Path, help="New owned proof directory.")
    args = parser.parse_args()
    output = args.output.resolve()
    builder = ROOT / "target/toolchain/bin/worker-build"
    if not builder.is_file():
        parser.error("Run build.py first to install the pinned Worker builder.")
    output.mkdir(parents=True, exist_ok=False)

    cargo, native = rust_environment("stable", "generated-sdk-native")
    native["CARGO_TARGET_DIR"] = str(FORGE / "target")
    native["CARGO_INCREMENTAL"] = "0"
    run([cargo, "build", "--manifest-path", FORGE / "Cargo.toml",
         "--examples", "--all-features", "--locked", "--offline", "-j2"],
        cwd=FORGE, env=native)
    examples = FORGE / "target/debug/examples"
    run([examples / "compile",
         FORGE / "tests/fixtures/advanced_composition_openapi.json",
         output / "advanced"], cwd=FORGE, env=native)
    run([examples / "audit", FORGE / "tests/fixtures/media_openapi.json",
         "https://example.test/media-openapi.json", output / "media"],
        cwd=FORGE, env=native)

    run([examples / "compile", FORGE / "tests/fixtures/numeric_defaults_openapi.json",
         output / "numeric", "numeric-proof-sdk"], cwd=FORGE, env=native)

    run([examples / "compile", FORGE / "tests/fixtures/response_status_openapi.json",
         output / "response", "response-proof-sdk"], cwd=FORGE, env=native)

    run([examples / "compile", FORGE / "tests/fixtures/typed_responses_openapi.json",
         output / "typed", "typed-response-proof-sdk"], cwd=FORGE, env=native)

    native["CARGO_TARGET_DIR"] = str(output / "target/native")
    packages = {}
    for name, package in (("advanced", "worker-proof-sdk"),
                          ("media", "corpus-proof-sdk"),
                          ("numeric", "numeric-proof-sdk"),
                          ("response", "response-proof-sdk"),
                          ("typed", "typed-response-proof-sdk")):
        run([cargo, "package", "--all-features", "--manifest-path", output / name / "sdk/Cargo.toml",
             "--offline", "--locked", "-j2"], cwd=FORGE, env=native)
        extracted = output / "target/native/package" / (package + "-0.1.0")
        assert (extracted / "src/lib.rs").read_bytes() == (
            output / name / "sdk/src/lib.rs").read_bytes()
        packages[name] = extracted

    project = output / "worker"
    (project / "src").mkdir(parents=True)
    fixture = ROOT / "fixtures/generated_sdk_media_consumer.rs"
    (project / "src/main.rs").write_bytes(fixture.read_bytes())
    manifest = (ROOT / "Cargo.toml").read_text()
    manifest = manifest.replace('name = "incurs-forge-worker-proof"',
                                'name = "incurs-generated-sdk-worker-current-proof"')
    manifest = manifest.replace('path = "../forge"',
                                "path = " + json.dumps(str(FORGE)))
    marker = 'serde_json = "1"\n'
    assert manifest.count(marker) == 1
    dependencies = (
        'sdk = { package = "worker-proof-sdk", path = '
        + json.dumps(str(packages["advanced"])) + ' }\n'
        + 'media_sdk = { package = "corpus-proof-sdk", path = '
        + json.dumps(str(packages["media"])) + ' }\n'
        + 'numeric_sdk = { package = "numeric-proof-sdk", path = '
        + json.dumps(str(packages["numeric"])) + ' }\n'
        + 'response_sdk = { package = "response-proof-sdk", path = '
        + json.dumps(str(packages["response"])) + ' }\n'
        + 'typed_sdk = { package = "typed-response-proof-sdk", features = ["typed-responses"], path = '
        + json.dumps(str(packages["typed"])) + ' }\n'
    )
    (project / "Cargo.toml").write_text(manifest.replace(marker, marker + dependencies))
    config = (ROOT / "wrangler.toml").read_text().replace(
        "incurs-forge-worker-proof", "incurs-generated-sdk-worker-current-proof")
    (project / "wrangler.toml").write_text(config)
    (project / "rust-toolchain.toml").write_bytes(
        (ROOT / "rust-toolchain.toml").read_bytes())

    worker_cargo, worker_env = rust_environment(TOOLCHAIN, "emscripten-direct")
    worker_env["FORGE_MEDIA_CONTRACT"] = str(output / "media/contract.json")
    worker_env["FORGE_NUMERIC_CONTRACT"] = str(output / "numeric/contract.json")
    worker_env["FORGE_REQUEST_VALIDATION_DOCUMENT"] = str(FORGE / "tests/fixtures/request_validation_openapi.json")
    run([worker_cargo, "generate-lockfile", "--offline"], cwd=project, env=worker_env)
    run([builder, "--emscripten", "--release", ".", "--locked"],
        cwd=project, env=worker_env)
    assert (project / "src/main.rs").read_bytes() == fixture.read_bytes()
    paths = [project / "build/index_bg.wasm", project / "src/main.rs"]
    print(json.dumps({
        "passed": True,
        "project": str(project),
        "sdk_sources": "verified package archives",
        "sha256": {str(path): hashlib.sha256(path.read_bytes()).hexdigest()
                   for path in paths},
    }, indent=2))


if __name__ == "__main__":
    main()
