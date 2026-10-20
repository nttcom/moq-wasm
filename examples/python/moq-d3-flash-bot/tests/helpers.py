import asyncio
import json
from dataclasses import dataclass, field
from types import SimpleNamespace
from typing import Any

from aiohttp import web

from moq_d3_flash_bot.d3_flash_status import D3FlashStatus

READ_TIMEOUT_SECONDS = 2
SETTLE_SECONDS = 0.05


class RecordingPublication:
    def __init__(self):
        self._writes: asyncio.Queue[tuple[bool, Any]] = asyncio.Queue()

    async def write(self, payload: bytes, *, new_group: bool = False) -> None:
        self._writes.put_nowait((new_group, json.loads(payload)))

    async def next_write(self) -> tuple[bool, Any]:
        return await asyncio.wait_for(self._writes.get(), READ_TIMEOUT_SECONDS)


async def published_status(status: D3FlashStatus) -> RecordingPublication:
    publication = RecordingPublication()
    await status.publish_on(publication)
    return publication


async def next_status(publication: RecordingPublication) -> str:
    _, status = await publication.next_write()
    return status["d3_flash"]


class FakeNamespaceSession:
    def __init__(self):
        self.prefixes: list[list[str]] = []
        self._events: asyncio.Queue[SimpleNamespace] = asyncio.Queue()

    def publish(self, namespace: list[str], *, active: bool = True) -> None:
        self._events.put_nowait(SimpleNamespace(namespace=namespace, active=active))

    async def subscribe_namespace(self, prefix: list[str]):
        self.prefixes.append(prefix)
        return self._iterate()

    async def _iterate(self):
        while True:
            yield await self._events.get()


class FakeRun:
    def __init__(self):
        self.starts = 0
        self.running = 0

    async def __call__(self) -> None:
        self.starts += 1
        self.running += 1
        try:
            await asyncio.Event().wait()
        finally:
            self.running -= 1


async def settle() -> None:
    await asyncio.sleep(SETTLE_SECONDS)
