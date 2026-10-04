"""Build the isolated Worker with exact toolchain revisions."""
import argparse
import os
import pathlib
import subprocess

ROOT = pathlib.Path(__file__).resolve().parent
TOOLCHAIN = "beta-2026-09-29"
WORKERS_REV = "b57ba6ef8198c65499c2f92b1845cc2412dd6e8c"


def run(*args, env=None):
    subprocess.run(args, cwd=ROOT, env=env, check=True)


def rust_environment(toolchain, output):
    cargo = subprocess.check_output(
        ["rustup", "which", "--toolchain", toolchain, "cargo"], text=True
    ).strip()
    binary_dir = pathlib.Path(cargo).parent
    env = os.environ.copy()
    env["PATH"] = str(binary_dir) + os.pathsep + env.get("PATH", "")
    env["RUSTC"] = str(binary_dir / "rustc")
    env["RUSTDOC"] = str(binary_dir / "rustdoc")
    env["RUSTC_WRAPPER"] = ""
    env["RUSTC_WORKSPACE_WRAPPER"] = ""
    # The pinned worker-build stages snippets from Cargo's legacy deps directory.
    # Cargo tracks this temporary switch at rust-lang/cargo#17182.
    env["__CARGO_TEMPORARY_BUILD_DIR_NEW_LAYOUT_OPT_OUT"] = "1"
    env["CARGO_TARGET_DIR"] = str(ROOT / "target" / output)
    env.setdefault("CARGO_BUILD_JOBS", "2")
    return cargo, env


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--skip-install", action="store_true",
                        help="Use the previously installed pinned toolchain.")
    args = parser.parse_args()
    if not args.skip_install:
        run("rustup", "toolchain", "install", TOOLCHAIN, "--profile", "minimal",
            "--target", "wasm32-unknown-emscripten")
        cargo, env = rust_environment("stable", "builder-build")
        run(cargo, "install", "worker-build", "--git",
            "https://github.com/cloudflare/workers-rs", "--rev", WORKERS_REV,
            "--locked", "--root", str(ROOT / "target/toolchain"),
            "--bin", "worker-build", env=env)
    _, env = rust_environment(TOOLCHAIN, "emscripten-direct")
    run(str(ROOT / "target/toolchain/bin/worker-build"),
        "--emscripten", "--release", ".", "--locked", env=env)


if __name__ == "__main__":
    main()
