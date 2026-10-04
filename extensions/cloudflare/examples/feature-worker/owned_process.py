#!/usr/bin/env python3
"""Run a fixture command in an owned process group with bounded cleanup."""

from __future__ import annotations

import os
import signal
import subprocess
import sys
import time


def _terminate_group(child: subprocess.Popen[bytes]) -> None:
    """Terminate this child's dedicated group and reap the child."""
    try:
        os.killpg(child.pid, signal.SIGTERM)
    except ProcessLookupError:
        child.wait(timeout=5)
        return
    deadline = time.monotonic() + 5
    while time.monotonic() < deadline:
        child.poll()
        try:
            os.killpg(child.pid, 0)
        except ProcessLookupError:
            child.wait(timeout=5)
            return
        time.sleep(0.05)
    try:
        os.killpg(child.pid, signal.SIGKILL)
    except ProcessLookupError:
        pass
    child.wait(timeout=5)


def main() -> int:
    """Forward output and clean up the dedicated command group."""
    if len(sys.argv) < 4 or sys.argv[2] != "--":
        raise SystemExit("usage: owned_process.py LOG -- COMMAND [ARG ...]")
    with open(sys.argv[1], "w", encoding="utf-8") as log:
        child = subprocess.Popen(
            sys.argv[3:], stdout=log, stderr=subprocess.STDOUT,
            start_new_session=True,
        )
        stopping = False

        def stop(signum: int, _frame: object) -> None:
            nonlocal stopping
            if stopping:
                return
            stopping = True
            _terminate_group(child)
            raise SystemExit(128 + signum)

        signal.signal(signal.SIGTERM, stop)
        signal.signal(signal.SIGINT, stop)
        try:
            return child.wait()
        finally:
            if not stopping:
                stopping = True
                _terminate_group(child)


if __name__ == "__main__":
    raise SystemExit(main())
