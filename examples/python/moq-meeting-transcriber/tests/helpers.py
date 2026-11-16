import json
from collections.abc import Awaitable, Callable


class RecordingPublication:
    """`connect` stands for a connection registering the track."""

    def __init__(self):
        self.writes: list[tuple[bool, object]] = []
        self._on_connected: list[Callable[[], Awaitable[None]]] = []

    def on_connected(self, callback: Callable[[], Awaitable[None]]) -> None:
        self._on_connected.append(callback)

    async def connect(self) -> None:
        for callback in self._on_connected:
            await callback()

    async def write(self, payload: bytes, *, new_group: bool = False) -> None:
        self.writes.append((new_group, json.loads(payload)))


class NamedRecognizer:
    def __init__(self, name: str):
        self.name = name

    async def ready(self) -> None:
        pass


class SessionRefusingNamespaces:
    async def watch_namespaces(self, prefix: list[str]):
        raise RuntimeError("SUBSCRIBE_NAMESPACE refused")
