"""Build and probe both isolated Forge Workers over real local HTTP."""
import argparse
import contextlib
import json
import os
import pathlib
import signal
import socket
import subprocess
import sys
import tempfile
import time
import urllib.error
import urllib.request

ROOT = pathlib.Path(__file__).resolve().parent
WRANGLER = "wrangler@4.143.0"


def stop(child):
    """Stop only the process group created for this child."""
    try:
        os.killpg(child.pid, signal.SIGTERM)
    except ProcessLookupError:
        pass
    try:
        child.wait(timeout=10)
    except subprocess.TimeoutExpired:
        try:
            os.killpg(child.pid, signal.SIGKILL)
        except ProcessLookupError:
            pass
        child.wait(timeout=10)


def run(*args, timeout, capture=False):
    """Run a bounded build or probe, retaining diagnostics on failure."""
    with subprocess.Popen(
        [str(arg) for arg in args], cwd=ROOT,
        stdout=subprocess.PIPE if capture else sys.stderr,
        text=True, start_new_session=True,
    ) as child:
        try:
            stdout, _ = child.communicate(timeout=timeout)
        except BaseException:
            stop(child)
            raise
        if child.returncode != 0:
            if capture:
                print(stdout, file=sys.stderr)
            raise subprocess.CalledProcessError(child.returncode, child.args)
    if capture:
        value = json.loads(stdout)
        if value.get("passed") is not True:
            raise RuntimeError(f"Probe did not pass: {value}")
        return value
    return None


@contextlib.contextmanager
def worker(project, startup_timeout):
    """Own one Wrangler process group and stop it on every exit path."""
    with socket.socket() as listener, socket.socket() as inspector:
        listener.bind(("127.0.0.1", 0))
        inspector.bind(("127.0.0.1", 0))
        port = listener.getsockname()[1]
        inspector_port = inspector.getsockname()[1]
    url = f"http://127.0.0.1:{port}"
    with tempfile.TemporaryFile(mode="w+") as log:
        child = subprocess.Popen(
            ["npm", "exec", "--yes", f"--package={WRANGLER}", "--",
             "wrangler", "dev", "--local", "--ip", "127.0.0.1",
             "--port", str(port), "--inspector-port", str(inspector_port)],
            cwd=project, stdin=subprocess.DEVNULL, stdout=log,
            stderr=subprocess.STDOUT, start_new_session=True,
            env={**os.environ, "CI": "true", "WRANGLER_SEND_METRICS": "false"},
        )
        try:
            deadline = time.monotonic() + startup_timeout
            while True:
                if child.poll() is not None:
                    raise RuntimeError(f"Wrangler exited with {child.returncode}")
                try:
                    with urllib.request.urlopen(url, timeout=1):
                        break
                except urllib.error.HTTPError:
                    # An HTTP error still proves that this owned listener is ready.
                    break
                except (urllib.error.URLError, TimeoutError, ConnectionError):
                    if time.monotonic() >= deadline:
                        raise TimeoutError("Wrangler did not become ready")
                    time.sleep(0.2)
            yield url
        except BaseException:
            log.seek(0)
            diagnostic = log.read()
            print(diagnostic[-12000:], file=sys.stderr)
            raise
        finally:
            stop(child)


def main():
    """Require both runtime probes and report their independently observed results."""
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--skip-install", action="store_true",
                        help="Reuse this workspace's installed pinned Worker builder.")
    parser.add_argument("--build-timeout", type=int, default=1800)
    parser.add_argument("--startup-timeout", type=int, default=120)
    parser.add_argument("--probe-timeout", type=int, default=300)
    args = parser.parse_args()
    for value in (args.build_timeout, args.startup_timeout, args.probe_timeout):
        if value <= 0:
            parser.error("Timeouts must be positive")

    build_args = [sys.executable, ROOT / "build.py"]
    if args.skip_install:
        build_args.append("--skip-install")
    run(*build_args, timeout=args.build_timeout)
    with worker(ROOT, args.startup_timeout) as url:
        base = run(sys.executable, ROOT / "probe.py", url,
                   timeout=args.probe_timeout, capture=True)

    with tempfile.TemporaryDirectory(prefix="incurs-forge-worker-smoke-") as temporary:
        output = pathlib.Path(temporary) / "sdk-proof"
        run(sys.executable, ROOT / "build_generated_sdk.py", output,
            timeout=args.build_timeout)
        with worker(output / "worker", args.startup_timeout) as url:
            sdk = run(sys.executable, ROOT / "probe_generated_sdk.py", url,
                      timeout=args.probe_timeout, capture=True)

    print(json.dumps({
        "passed": True,
        "base_worker": base,
        "generated_sdk_worker": sdk,
        "default_switch_eligible": False,
    }, indent=2))


if __name__ == "__main__":
    main()
