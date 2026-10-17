#!/usr/bin/env python3
"""Exercise the build/publish scripts in disposable directories, without nodes."""

import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tarfile
import tempfile
import tomllib

REPO = Path(__file__).resolve().parents[2]
STUB = r'''
import json, os, shutil, sys
from pathlib import Path
name = Path(sys.argv[1]).name
args = sys.argv[2:]
root = Path(os.environ["TEST_ROOT"])
out = root / "target/local-webapp"
def arg(key): return args[args.index(key) + 1]
if name in ("python3", "curl", "sed"): sys.exit(0)
if name == "freenet": raise RuntimeError("A test must not start nodes")
if name == "git":
    if args[0] == "rev-parse": print("test")
elif name == "dx":
    dist = root / "target/dx/chess-ui/release/web/public"
    dist.mkdir(parents=True)
    (dist / "index.html").write_text(os.environ["TEST_BUILD"])
    shutil.copytree(root / "ui/assets/stockfish", dist / "assets/stockfish")
    if os.environ.get("TEST_PROCESS_ASSETS"):
        worker = dist / "assets/stockfish/stockfish-19-lite-single.js"
        worker.write_bytes(worker.read_bytes() + b"\n// processed by the asset pipeline\n")
elif name == "cargo":
    if args[0] == "build": sys.exit(0)
    if arg("-p") == "webapp-packager":
        if "--params-only" in args: (out / "webapp.params").write_text("params")
        else: (out / "webapp.state").write_text(json.dumps({"version": int(arg("--version")), "build": os.environ["TEST_BUILD"]}))
    else:
        (out / "lobby.params").write_text("params")
        (out / "lobby.state").write_text("state")
elif name == "fdev":
    if args[0] == "get-contract-id": print("TestKey"); sys.exit(0)
    if "lobby_contract.wasm" in arg("--code"): sys.exit(0)
    port = arg("--port")
    if port == os.environ.get("TEST_FAIL_PORT"): sys.exit(1)
    candidate = json.loads((out / "webapp.state").read_text())
    remote = root / ("remote-" + port + ".json")
    current = json.loads(remote.read_text()) if remote.exists() else {"version": 0}
    # The real web-container contract retains its state for equal/older versions.
    if candidate["version"] > current["version"]: remote.write_text(json.dumps(candidate))
else: raise RuntimeError((name, args))
'''


def workspace(root):
    shutil.copytree(REPO / "scripts", root / "scripts", ignore=shutil.ignore_patterns("tests"))
    shutil.copytree(REPO / "ui/assets/stockfish", root / "ui/assets/stockfish")
    shutil.copyfile(REPO / "ui/Dioxus.toml", root / "ui/Dioxus.toml")
    (root / "target/local-webapp").mkdir(parents=True)
    (root / "target/local-webapp/version").write_text("5\n")
    (root / "bin").mkdir()
    (root / "stub.py").write_text(STUB)
    for name in ("cargo", "dx", "freenet", "fdev", "python3", "curl", "git", "sed"):
        tool = root / "bin" / name
        tool.write_text('#!/bin/sh\nexec "$TEST_PYTHON" "$TEST_ROOT/stub.py" "$0" "$@"\n')
        tool.chmod(0o755)
    return os.environ | {
        "PATH": str(root / "bin") + os.pathsep + os.environ["PATH"],
        "TEST_ROOT": str(root),
        "TEST_PYTHON": sys.executable,
        "TEST_BUILD": "first-build",
    }


def local(root, env):
    return subprocess.run(["bash", "scripts/local.sh"], cwd=root, env=env, capture_output=True, text=True)


def partial_publish_retry():
    with tempfile.TemporaryDirectory(prefix="freechess-build-test-") as name:
        root = Path(name)
        env = workspace(root)
        first = local(root, env | {"TEST_FAIL_PORT": "7512"})
        assert first.returncode == 1, first.stdout + first.stderr
        retry = local(root, env | {"TEST_BUILD": "edited-build"})
        assert retry.returncode == 0, retry.stdout + retry.stderr
        states = [json.loads((root / f"remote-{port}.json").read_text()) for port in (7511, 7512, 7514)]
        assert states == [{"version": 7, "build": "edited-build"}] * 3, states
    print("PASS: edited retry after partial publication reaches every node with a fresh version")


def pinned_assets():
    build_ui = tomllib.loads((REPO / "Makefile.toml").read_text())["tasks"]["build-ui"]["script"]
    for pipeline in ("local", "build-ui"):
        with tempfile.TemporaryDirectory(prefix="freechess-assets-test-") as name:
            root = Path(name)
            env = workspace(root) | {"TEST_PROCESS_ASSETS": "1", "PUBLISHED": "published-contract"}
            result = local(root, env) if pipeline == "local" else subprocess.run(
                ["bash", "-c", build_ui], cwd=root, env=env, capture_output=True, text=True
            )
            assert result.returncode == 0, result.stdout + result.stderr
            source = root / "ui/assets/stockfish"
            shipped = root / "target/dx/chess-ui/release/web/public/assets/stockfish"
            for line in (shipped / "SHA256SUMS").read_text().splitlines():
                expected, filename = line.split("  ", 1)
                assert hashlib.sha256((shipped / filename).read_bytes()).hexdigest() == expected, f"{pipeline}: shipped {filename} fails its pinned checksum"
            for asset in source.iterdir():
                assert (shipped / asset.name).read_bytes() == asset.read_bytes(), (pipeline, asset.name)
            if pipeline == "local":
                with tarfile.open(root / "target/local-webapp/webapp.tar.xz") as archive:
                    for asset in source.iterdir():
                        assert archive.extractfile("./assets/stockfish/" + asset.name).read() == asset.read_bytes(), asset.name
        print(f"PASS: {pipeline} preserves every pinned Stockfish asset and its shipped checksums")


if __name__ == "__main__":
    partial_publish_retry()
    pinned_assets()
