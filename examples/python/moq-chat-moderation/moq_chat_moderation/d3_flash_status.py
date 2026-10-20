import asyncio
import json
import time
from collections.abc import AsyncIterator, Callable
from contextlib import asynccontextmanager

from moq_chat_moderation.event_timeline import Publication

# Cloud Run stops an idle d3-flash after about ten minutes, so a request after a longer pause waits for
# a cold start. A warm d3-flash answers within a second, so a request still pending after
# COLD_START_SECONDS is a cold start too.
IDLE_STOP_SECONDS = 10 * 60
SLOW_SECONDS = 5.0
COLD_START_SECONDS = 15.0

READY = "ready"
SLOW = "slow"
STARTING = "starting"
SEVERITY = {READY: 0, SLOW: 1, STARTING: 2}


def epoch_ms() -> int:
    return time.time_ns() // 1_000_000


class D3FlashStatus:
    def __init__(self, clock: Callable[[], float] = time.monotonic):
        self._clock = clock
        self._last_answer_at: float | None = None
        self._pending = 0
        self._state = READY
        self._since_ms = epoch_ms()
        self._publication: Publication | None = None

    async def publish_on(self, publication: Publication) -> None:
        self._publication = publication
        await self._write()

    @asynccontextmanager
    async def request(self) -> AsyncIterator[None]:
        self._pending += 1
        started_ms = epoch_ms()
        if self._last_answer_at is None or self._clock() - self._last_answer_at > IDLE_STOP_SECONDS:
            await self._raise_to(STARTING, started_ms)
        escalation = asyncio.create_task(self._escalate(started_ms))
        answered = False
        try:
            yield
            answered = True
        finally:
            escalation.cancel()
            self._pending -= 1
            if answered:
                self._last_answer_at = self._clock()
                await self._set(READY, epoch_ms())
            elif self._pending == 0:
                await self._set(READY, epoch_ms())

    async def _escalate(self, started_ms: int) -> None:
        await asyncio.sleep(SLOW_SECONDS)
        await self._raise_to(SLOW, started_ms)
        await asyncio.sleep(COLD_START_SECONDS - SLOW_SECONDS)
        await self._raise_to(STARTING, started_ms)

    async def _raise_to(self, state: str, since_ms: int) -> None:
        if SEVERITY[state] > SEVERITY[self._state]:
            await self._set(state, since_ms)

    async def _set(self, state: str, since_ms: int) -> None:
        if state == self._state:
            return
        self._state = state
        self._since_ms = since_ms
        await self._write()

    async def _write(self) -> None:
        if self._publication is None:
            return
        await self._publication.write(
            json.dumps({"d3_flash": self._state, "since": self._since_ms}, separators=(",", ":")).encode(),
            new_group=True,
        )
