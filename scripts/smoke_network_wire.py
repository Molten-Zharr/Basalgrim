#!/usr/bin/env python3
"""Actual GIO worker against four independent loopback protocol servers."""
import json
from pathlib import Path
import subprocess
import sys
import tempfile
sys.dont_write_bytecode = True
from network_fixture import Fixture

build = subprocess.run(
    ["python3", "scripts/cargo_local.py", "test", "--all-features", "--test", "network_wire",
     "--no-run", "--message-format=json"], stdout=subprocess.PIPE, text=True, check=True)
artifacts = [json.loads(line) for line in build.stdout.splitlines()]
binary = next(item["executable"] for item in artifacts
              if item.get("executable") and item.get("target", {}).get("name") == "network_wire")
for protocol in ("ftp", "dav", "sftp", "smb"):
    with tempfile.TemporaryDirectory(prefix="bgwire-") as tmp, Fixture(Path(tmp), protocol) as fixture:
        env = dict(fixture.env, BASALGRIM_TEST_NETWORK_URI=fixture.uri, BASALGRIM_TEST_NETWORK_ROOT=tmp)
        result = subprocess.run([binary, "--ignored", "--nocapture"], env=env, timeout=60)
        if result.returncode:
            print((Path(tmp)/"gvfs.log").read_text())
            raise SystemExit(result.returncode)
        print(f"GVfs wire {protocol}: passed", flush=True)
