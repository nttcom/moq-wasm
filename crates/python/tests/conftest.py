import json
import os
import socket
import subprocess
import time
import urllib.request
from pathlib import Path

import pytest

REPO_ROOT = Path(__file__).resolve().parents[3]
TARGET = REPO_ROOT / "target" / "debug"
APPS_FILE = "crates/vts/apps.example.json"
RELAY_APP_ID = "11111111-2222-3333-4444-555555555555"
READY_TIMEOUT_SECONDS = 120


def free_port(kind: int, count: int = 1) -> int:
    """A free port followed by `count - 1` more free ports; the relay binds its port and the next one."""
    while True:
        with socket.socket(socket.AF_INET, kind) as probe:
            probe.bind(("127.0.0.1", 0))
            port = probe.getsockname()[1]
        try:
            for offset in range(1, count):
                with socket.socket(socket.AF_INET, kind) as next_port:
                    next_port.bind(("127.0.0.1", port + offset))
            return port
        except OSError:
            continue


def wait_until(ready, what: str) -> None:
    deadline = time.monotonic() + READY_TIMEOUT_SECONDS
    while time.monotonic() < deadline:
        if ready():
            return
        time.sleep(0.2)
    raise TimeoutError(f"{what} did not become ready")


def vts_answers(port: int) -> bool:
    try:
        return urllib.request.urlopen(f"http://127.0.0.1:{port}/healthz", timeout=1).status == 200
    except OSError:
        return False


@pytest.fixture(scope="session")
def relay_url(tmp_path_factory) -> str:
    """A relay and the VTS it verifies clients against, both built from this workspace."""
    subprocess.run(["cargo", "build", "-q", "-p", "relay", "-p", "vts"], cwd=REPO_ROOT, check=True)
    subprocess.run(["cargo", "build", "-q", "-p", "auth-token", "--features", "cli"], cwd=REPO_ROOT, check=True)
    logs = tmp_path_factory.mktemp("relay")
    vts_port = free_port(socket.SOCK_STREAM)
    vts = subprocess.Popen(
        [TARGET / "vts"],
        cwd=REPO_ROOT,
        env={**os.environ, "VTS_PORT": str(vts_port), "VTS_APPS_FILE": APPS_FILE},
        stdout=open(logs / "vts.log", "w"),
        stderr=subprocess.STDOUT,
    )
    relay = None
    try:
        wait_until(lambda: vts_answers(vts_port), "VTS")
        apps = json.loads((REPO_ROOT / APPS_FILE).read_text())
        relay_secret = next(app["secret"] for app in apps if app["appId"] == RELAY_APP_ID)
        token = subprocess.run(
            [TARGET / "auth-token", "--app-id", RELAY_APP_ID, "--publish", "", "--subscribe", "", "--ttl", "1h"],
            cwd=REPO_ROOT,
            env={**os.environ, "AUTH_TOKEN_SECRET": relay_secret},
            check=True,
            capture_output=True,
            text=True,
        ).stdout.strip()
        relay_port = free_port(socket.SOCK_DGRAM, count=2)
        relay_log = logs / "relay.log"
        relay = subprocess.Popen(
            [TARGET / "relay"],
            cwd=REPO_ROOT,
            env={
                **os.environ,
                "RELAY_PORT": str(relay_port),
                "AUTH_VTS_URL": f"http://127.0.0.1:{vts_port}/verify",
                "AUTH_RELAY_TOKEN": token,
                "OTEL_SDK_DISABLED": "true",
            },
            stdout=open(relay_log, "w"),
            stderr=subprocess.STDOUT,
        )
        wait_until(lambda: "Relay server started" in relay_log.read_text(errors="replace"), "relay")
        yield f"moqt://127.0.0.1:{relay_port}"
    finally:
        for process in (relay, vts):
            if process is not None:
                process.terminate()
                process.wait(timeout=30)
