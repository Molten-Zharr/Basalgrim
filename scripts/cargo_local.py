#!/usr/bin/env python3
"""Run Cargo with a bounded, repository-local build cache (Linux)."""

import fcntl
import os
from pathlib import Path
import signal
import subprocess
import sys

ROOT = Path(__file__).resolve().parents[1]
TARGET = ROOT / "target"
BUDGET = 8 * 1024**3
INTERVAL = 2


def target_bytes():
    """Count allocated bytes, including hard links only once; never follow links."""
    if TARGET.is_symlink():
        raise RuntimeError("target must be a real directory, not a symlink")
    if not TARGET.exists():
        return 0
    if not TARGET.is_dir():
        raise RuntimeError("target must be a directory")
    device = TARGET.stat().st_dev
    seen = set()
    total = 0
    for directory, dirs, files in os.walk(TARGET):
        for path in [Path(directory), *(Path(directory) / n for n in dirs + files)]:
            try:
                info = path.lstat()
            except FileNotFoundError:
                continue  # Cargo may replace an artifact during measurement.
            if info.st_dev != device:
                raise RuntimeError("target contains a mounted filesystem; refusing cleanup")
            identity = (info.st_dev, info.st_ino)
            if identity not in seen:
                seen.add(identity)
                total += info.st_blocks * 512
    return total


def report(size):
    print(f"target: {size / 1024**2:.1f} MiB / {BUDGET / 1024**2:.0f} MiB",
          file=sys.stderr, flush=True)


def clean(env):
    # Recheck before deleting; let Cargo remove its own generated output.
    target_bytes()
    subprocess.run(
        ["cargo", "clean", "--offline", "--target-dir", str(TARGET)],
        cwd=ROOT, env=env, check=True,
    )


def stop(child):
    """Stop our Cargo process group before any cleanup, including rustc children."""
    try:
        os.killpg(child.pid, signal.SIGTERM)
    except ProcessLookupError:
        return
    try:
        child.wait(timeout=10)
    except subprocess.TimeoutExpired:
        pass
    # Cargo may exit before a descendant which ignored SIGTERM. Stop the whole
    # remaining group before removing output, even when the leader already exited.
    try:
        os.killpg(child.pid, signal.SIGKILL)
    except ProcessLookupError:
        pass
    child.wait()


def main(args=None):
    args = list(sys.argv[1:] if args is None else args)
    if not args:
        print("Usage: python3 scripts/cargo_local.py <build|test|clippy|run|…> [arguments]\n"
              "       python3 scripts/cargo_local.py status", file=sys.stderr)
        return 2
    # Bound this repository's output, rather than silently measuring another path.
    cargo_args = args[:args.index("--")] if "--" in args else args
    forbidden = ("--target-dir", "--build-dir", "--manifest-path", "-m", "--config", "--artifact-dir", "-C")
    if any(a.split("=", 1)[0] in forbidden or a.startswith("-C") for a in cargo_args):
        raise RuntimeError("output/config overrides are unsupported by cargo_local.py")
    env = dict(os.environ, CARGO_TARGET_DIR=str(TARGET), CARGO_BUILD_BUILD_DIR=str(TARGET))
    # This lock survives cargo clean; concurrent local commands fail immediately.
    fd = os.open(ROOT / ".basalgrim-cargo.lock", os.O_CREAT | os.O_RDWR | os.O_NOFOLLOW, 0o600)
    with os.fdopen(fd, "w") as lock:
        try:
            fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
        except BlockingIOError:
            raise RuntimeError("another cargo_local.py command is running") from None
        size = target_bytes()
        report(size)
        if args == ["status"]:
            return 0 if size <= BUDGET else 75
        if size > BUDGET:
            print("Cache budget exceeded; cleaning target before Cargo.", file=sys.stderr, flush=True)
            clean(env)
        child = subprocess.Popen(["cargo", *args], cwd=ROOT, env=env, start_new_session=True)
        exceeded = False
        try:
            while True:
                try:
                    result = child.wait(timeout=INTERVAL)
                    break
                except subprocess.TimeoutExpired:
                    if target_bytes() > BUDGET:
                        exceeded = True
                        print("Cache budget exceeded; stopping Cargo and cleaning target.",
                              file=sys.stderr, flush=True)
                        stop(child)
                        result = 75
                        break
        except KeyboardInterrupt:
            stop(child)
            result = 130
        finally:
            if child.poll() is None:
                stop(child)
        size = target_bytes()
        if exceeded or size > BUDGET:
            clean(env)
            result = 75  # Never report a successful build after removing its output.
            size = target_bytes()
        report(size)
        return result


def interrupted(_signum, _frame):
    raise KeyboardInterrupt


if __name__ == "__main__":
    signal.signal(signal.SIGTERM, interrupted)
    try:
        sys.exit(main())
    except KeyboardInterrupt:
        sys.exit(130)
    except (OSError, RuntimeError, subprocess.CalledProcessError) as error:
        print(f"cargo_local.py: {error}", file=sys.stderr)
        sys.exit(1)
