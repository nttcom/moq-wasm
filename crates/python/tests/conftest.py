import json
import os
import re
import socket
import subprocess
import time
import urllib.request
import uuid
from contextlib import contextmanager
from dataclasses import dataclass
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


def logged(log: Path, text: str) -> bool:
    return log.exists() and text in log.read_text(errors="replace")


def stop(*processes: subprocess.Popen | None) -> None:
    running = [process for process in processes if process is not None]
    for process in running:
        process.terminate()
    for process in running:
        process.wait(timeout=30)


@dataclass
class RelayAuth:
    """What a relay needs to verify clients against the VTS."""

    env: dict[str, str]
    logs: Path


@dataclass
class Relay:
    url: str
    process: subprocess.Popen


@pytest.fixture(scope="session")
def relay_auth(tmp_path_factory):
    """A VTS built from this workspace and the relay token it accepts."""
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
        yield RelayAuth(
            env={"AUTH_VTS_URL": f"http://127.0.0.1:{vts_port}/verify", "AUTH_RELAY_TOKEN": token},
            logs=logs,
        )
    finally:
        stop(vts)


@contextmanager
def started_relay(auth: RelayAuth, relay_id: str, env: dict[str, str] | None = None):
    port = free_port(socket.SOCK_DGRAM, count=2)
    log = auth.logs / f"{relay_id}.log"
    process = subprocess.Popen(
        [TARGET / "relay"],
        cwd=REPO_ROOT,
        env={
            **os.environ,
            **auth.env,
            "RELAY_ID": relay_id,
            "RELAY_PORT": str(port),
            "RELAY_ADVERTISE_HOST": "127.0.0.1",
            "RELAY_PUBLIC_HOST": f"127.0.0.1:{port}",
            "OTEL_SDK_DISABLED": "true",
            **(env or {}),
        },
        stdout=open(log, "w"),
        stderr=subprocess.STDOUT,
    )
    try:
        wait_until(lambda: logged(log, "Relay server started"), relay_id)
        yield Relay(f"moqt://127.0.0.1:{port}", process)
    finally:
        stop(process)


@pytest.fixture(scope="session")
def relay_url(relay_auth) -> str:
    with started_relay(relay_auth, "relay") as relay:
        yield relay.url


def redis_image() -> str:
    """The digest-pinned Redis image of docker-compose.yml."""
    compose = (REPO_ROOT / "docker-compose.yml").read_text()
    return re.search(r"^ {2}redis:\n {4}image: (\S+)$", compose, re.MULTILINE).group(1)


@contextmanager
def started_redis(logs: Path):
    port = free_port(socket.SOCK_STREAM)
    name = f"python-binding-redis-{uuid.uuid4().hex[:8]}"
    log = logs / "redis.log"
    process = subprocess.Popen(
        ["docker", "run", "--rm", "--name", name, "-p", f"127.0.0.1:{port}:6379", redis_image()],
        stdout=open(log, "w"),
        stderr=subprocess.STDOUT,
    )
    try:
        wait_until(lambda: logged(log, "Ready to accept connections"), "redis")
        yield f"redis://127.0.0.1:{port}"
    finally:
        subprocess.run(["docker", "rm", "-f", name], capture_output=True)
        process.wait(timeout=30)


@pytest.fixture
def relays_sharing_redis(relay_auth):
    """Relay A and relay B registered in one Redis, so draining A sends its clients a GOAWAY naming B."""
    with started_redis(relay_auth.logs) as redis_url:
        env = {"REDIS_URL": redis_url}
        with started_relay(relay_auth, "relay-b", env) as relay_b:
            with started_relay(relay_auth, "relay-a", env) as relay_a:
                yield relay_a, relay_b
