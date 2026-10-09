import asyncio
import json
import time
from collections.abc import AsyncIterator, Callable
from contextlib import asynccontextmanager

import moq

# Cloud Run stops an idle djev after about ten minutes, so a request after a longer pause waits for
# a cold start. A warm djev answers within a second, so a request still pending after
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


class DjevStatus:
    def __init__(self, track_name: str, clock: Callable[[], float] = time.monotonic):
        self._track_name = track_name
        self._clock = clock
        self._last_answer_at: float | None = None
        self._pending = 0
        self._state = READY
        self._since_ms = epoch_ms()
        self._track: moq.TrackProducer | None = None
        # The relay isolates a track whose publisher repeats a location, so group ids are seeded
        # from the wall clock.
        self._next_group_id = time.time_ns() // 1_000

    def publish_on(self, broadcast: moq.BroadcastProducer) -> None:
        self._track = broadcast.publish_track(self._track_name)
        self._write()

    @asynccontextmanager
    async def request(self) -> AsyncIterator[None]:
        self._pending += 1
        started_ms = epoch_ms()
        if self._last_answer_at is None or self._clock() - self._last_answer_at > IDLE_STOP_SECONDS:
            self._raise_to(STARTING, started_ms)
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
                self._set(READY, epoch_ms())
            elif self._pending == 0:
                self._set(READY, epoch_ms())

    async def _escalate(self, started_ms: int) -> None:
        await asyncio.sleep(SLOW_SECONDS)
        self._raise_to(SLOW, started_ms)
        await asyncio.sleep(COLD_START_SECONDS - SLOW_SECONDS)
        self._raise_to(STARTING, started_ms)

    def _raise_to(self, state: str, since_ms: int) -> None:
        if SEVERITY[state] > SEVERITY[self._state]:
            self._set(state, since_ms)

    def _set(self, state: str, since_ms: int) -> None:
        if state == self._state:
            return
        self._state = state
        self._since_ms = since_ms
        self._write()

    def _write(self) -> None:
        if self._track is None:
            return
        group = self._track.create_group(self._next_group_id)
        self._next_group_id += 1
        group.write_frame(
            json.dumps({"djev": self._state, "since": self._since_ms}, separators=(",", ":")).encode()
        )
        group.finish()
