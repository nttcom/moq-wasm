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

from moq_wasm import _session
from moq_wasm.media import _remote_broadcast

REPO_ROOT = Path(__file__).resolve().parents[3]
TARGET = REPO_ROOT / "target" / "debug"
APPS_FILE = "services/vts/apps.example.json"
RELAY_APP_ID = "11111111-2222-3333-4444-555555555555"
# The VTS verifies every `temp-` app with this secret, as it does the token mint's apps.
TEMPORARY_APP_SECRET = "python-binding-temporary-app-secret"
READY_TIMEOUT_SECONDS = 120


# The relay binds on every address: QUIC and WebTransport on UDP `RELAY_PORT`, WebSocket on TCP
# `RELAY_PORT`, and relay-to-relay QUIC on UDP `RELAY_PORT + 1`.
RELAY_SOCKETS = ((socket.SOCK_DGRAM, 0), (socket.SOCK_STREAM, 0), (socket.SOCK_DGRAM, 1))


def free_port(kind: int) -> int:
    with socket.socket(socket.AF_INET, kind) as probe:
        probe.bind(("127.0.0.1", 0))
        return probe.getsockname()[1]


def free_relay_port() -> int:
    while True:
        port = free_port(socket.SOCK_DGRAM)
        try:
            for kind, offset in RELAY_SOCKETS:
                with socket.socket(socket.AF_INET6, kind) as probe:
                    probe.setsockopt(socket.IPPROTO_IPV6, socket.IPV6_V6ONLY, 0)
                    probe.bind(("::", port + offset))
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
    return text in log.read_text(errors="replace")


def stop(process: subprocess.Popen) -> None:
    process.terminate()
    process.wait(timeout=30)


def signed_token(app_id: str, secret: str, ttl: str = "1h") -> str:
    """A token of `app_id` that may publish and subscribe anywhere under it."""
    return subprocess.run(
        [TARGET / "auth-token", "--app-id", app_id, "--publish", "", "--subscribe", "", "--ttl", ttl],
        cwd=REPO_ROOT,
        env={**os.environ, "AUTH_TOKEN_SECRET": secret},
        check=True,
        capture_output=True,
        text=True,
    ).stdout.strip()


@pytest.fixture
def no_reconnect_delay(monkeypatch):
    """A session redials and resubscribes at once, for the tests on fake native sessions."""
    monkeypatch.setattr(_session, "FIRST_RECONNECT_DELAY_SECONDS", 0.0)
    monkeypatch.setattr(_remote_broadcast, "RESUBSCRIBE_DELAY_SECONDS", 0.0)


@pytest.fixture
def temporary_app_token(relay_auth):
    """Signs a token of the `temp-` app it is given, which the VTS accepts."""
    return lambda app_id: signed_token(app_id, TEMPORARY_APP_SECRET)


@dataclass
class RelayAuth:
    env: dict[str, str]
    logs: Path


@dataclass
class Relay:
    url: str
    port: int
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
        env={
            **os.environ,
            "VTS_PORT": str(vts_port),
            "VTS_APPS_FILE": APPS_FILE,
            "VTS_TEMPORARY_APP_SECRET": TEMPORARY_APP_SECRET,
        },
        stdout=open(logs / "vts.log", "w"),
        stderr=subprocess.STDOUT,
    )
    try:
        wait_until(lambda: vts_answers(vts_port), "VTS")
        apps = json.loads((REPO_ROOT / APPS_FILE).read_text())
        relay_secret = next(app["secret"] for app in apps if app["appId"] == RELAY_APP_ID)
        token = signed_token(RELAY_APP_ID, relay_secret)
        yield RelayAuth(
            env={"AUTH_VTS_URL": f"http://127.0.0.1:{vts_port}/verify", "AUTH_RELAY_TOKEN": token},
            logs=logs,
        )
    finally:
        stop(vts)


@contextmanager
def started_relay(auth: RelayAuth, relay_id: str, env: dict[str, str] | None = None, port: int | None = None):
    """`port` restarts a relay on the port a stopped one had."""
    port = port or free_relay_port()
    log = auth.logs / f"{relay_id}-{port}-{time.monotonic_ns()}.log"
    process = subprocess.Popen(
        [TARGET / "relay"],
        cwd=REPO_ROOT,
        env={
            **os.environ,
            **auth.env,
            "RELAY_ID": relay_id,
            "RELAY_PORT": str(port),
            "RELAY_ADVERTISE_HOST": "127.0.0.1",
            # Every relay serves this certificate, so each trusts it to dial another.
            "RELAY_INNER_CA_FILE": str(REPO_ROOT / "services" / "relay" / "keys" / "cert.pem"),
            "RELAY_PUBLIC_HOST": f"127.0.0.1:{port}",
            "OTEL_SDK_DISABLED": "true",
            **(env or {}),
        },
        stdout=open(log, "w"),
        stderr=subprocess.STDOUT,
    )
    try:
        def started() -> bool:
            if process.poll() is not None:
                raise RuntimeError(f"{relay_id} exited with code {process.returncode}; see {log}")
            return logged(log, "Relay server started")

        wait_until(started, relay_id)
        yield Relay(f"moqt://127.0.0.1:{port}", port, process)
    finally:
        stop(process)


@pytest.fixture(scope="session")
def relay_url(relay_auth) -> str:
    with started_relay(relay_auth, "relay") as relay:
        yield relay.url


def redis_image() -> str:
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


@pytest.fixture(scope="session")
def draft18_relay_url(relay_auth) -> str:
    """A relay that accepts draft-18 only."""
    with started_relay(relay_auth, "relay-draft18", {"RELAY_MOQT_VERSIONS": "draft-18"}) as relay:
        yield relay.url
