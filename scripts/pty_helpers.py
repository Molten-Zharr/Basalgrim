"""Shared process controls for real PTY scenarios."""
import subprocess
import time


def quit_when_ready(process, send, timeout=8):
    """Respect busy Quit rejection until the operation worker acknowledges completion.

    A published file can precede final journal writes and App consuming the
    worker result. Retry the real key within a deadline; a stuck worker or an
    abnormal process exit still fails the scenario.
    """
    deadline = time.monotonic() + timeout
    while process.poll() is None:
        send(b"\x11")
        try:
            process.wait(timeout=.15)
        except subprocess.TimeoutExpired:
            assert time.monotonic() < deadline, "Operation worker did not finish before Quit"
    assert process.returncode == 0, f"Unexpected application exit: {process.returncode}"
