import asyncio
from types import SimpleNamespace

from moq_system_one_bot.system_one_status import SystemOneStatus
from moq_system_one_bot.testing import RecordingPublication

SETTLE_SECONDS = 0.05


async def published_status(status: SystemOneStatus) -> RecordingPublication:
    publication = RecordingPublication()
    await status.publish_on(publication)
    return publication


async def next_status(publication: RecordingPublication) -> str:
    _, status = await publication.next_write()
    return status["system_one"]


class FakeNamespaceSession:
    def __init__(self):
        self.prefixes: list[list[str]] = []
        self._events: asyncio.Queue[SimpleNamespace] = asyncio.Queue()
        self._closed = asyncio.get_running_loop().create_future()

    def close(self) -> None:
        self._closed.set_result(None)

    def closed(self) -> asyncio.Future:
        return self._closed

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
