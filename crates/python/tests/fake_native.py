"""Stand-ins for `moq_wasm._native`, so the SDK's session logic is tested without a relay."""

import asyncio
import base64
import json
from types import SimpleNamespace


def unsigned_token(app_id: str, expires_at: float | None) -> str:
    """A JWT carrying `app_id` and `exp`; the SDK reads its claims without verifying it."""

    def encode(part: dict) -> str:
        return base64.urlsafe_b64encode(json.dumps(part).encode()).rstrip(b"=").decode()

    claims = {"app_id": app_id} if expires_at is None else {"app_id": app_id, "exp": expires_at}
    return f"{encode({'alg': 'none'})}.{encode(claims)}.signature"


class FakeWriter:
    def __init__(self):
        self.objects: list[tuple[bytes, bool]] = []

    async def write(self, payload: bytes, *, new_group: bool = False) -> None:
        self.objects.append((payload, new_group))


class FakeNativeSession:
    """One connection. `calls` lists its requests in order; `drop()` ends it as the relay would."""

    def __init__(self, token: str | None):
        self.token = token
        self.calls: list[tuple] = []
        self.writers: dict[str, FakeWriter] = {}
        self.refreshed: list[str] = []
        self._closed = asyncio.get_running_loop().create_future()
        self._namespace_events: asyncio.Queue = asyncio.Queue()

    def publish(self, namespace: list[str], *, active: bool = True) -> None:
        self._namespace_events.put_nowait(SimpleNamespace(namespace=namespace, active=active))

    def drop(self) -> None:
        if not self._closed.done():
            self._closed.set_result(None)

    async def publish_track(self, namespace: list[str], name: str, *, retention_seconds: float) -> FakeWriter:
        self.calls.append(("publish_track", name))
        return self.writers.setdefault(name, FakeWriter())

    async def announce(self, namespace: list[str]) -> None:
        self.calls.append(("announce", namespace))

    async def watch_namespaces(self, prefix: list[str]):
        self.calls.append(("watch_namespaces", prefix))
        return self._events()

    async def _events(self):
        while True:
            yield await self._namespace_events.get()

    async def subscribe(self, namespace: list[str], name: str):
        self.calls.append(("subscribe", name))
        return SimpleNamespace(namespace=namespace, name=name)

    async def refresh_token(self, token: str, *, app_id: str) -> None:
        self.refreshed.append(token)

    async def closed(self) -> None:
        await self._closed

    async def close(self) -> None:
        self.drop()


class FakeRelay:
    """Opens a `FakeNativeSession` for each dial; `refuse_next` makes the next dials fail."""

    def __init__(self):
        self.sessions: list[FakeNativeSession] = []
        self.refuse_next = 0
        self._dialed = asyncio.Event()

    async def dial(self, token: str | None) -> FakeNativeSession:
        if self.refuse_next > 0:
            self.refuse_next -= 1
            raise RuntimeError("connection refused")
        session = FakeNativeSession(token)
        self.sessions.append(session)
        self._dialed.set()
        return session

    async def next_session(self, count: int) -> FakeNativeSession:
        """Waits until `count` sessions have been opened, and returns the `count`-th."""
        while len(self.sessions) < count:
            self._dialed.clear()
            await self._dialed.wait()
        return self.sessions[count - 1]
