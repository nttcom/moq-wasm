import asyncio
import contextlib
import json
import math
import time
from dataclasses import dataclass
from typing import Any

import moq

from moq_ptz_tracking.vision import Position

MOVE_VELOCITY = 0.5
# A sweep stops by itself when the tracker stops renewing it, e.g. once the target is cleared.
SWEEP_LEASE_SECONDS = 2.0
# A Tapo camera takes up to about this long to return to (0, 0) from a pan limit at full speed.
CENTER_SECONDS = 4.0


@dataclass(frozen=True)
class PtzMove:
    """Seconds to turn at MOVE_VELOCITY; as in ONVIF, positive values pan right and tilt up."""

    pan_seconds: float
    tilt_seconds: float


@dataclass(frozen=True)
class PtzCalibration:
    """`pan_per_second` and `tilt_per_second` are the fractions of the picture width and height the
    view shifts per second of a positive ContinuousMove at MOVE_VELOCITY; negative for a camera that
    pans left or tilts down."""

    pan_per_second: float
    tilt_per_second: float
    dead_zone: float

    def toward(self, position: Position) -> PtzMove:
        return PtzMove(
            self._seconds(position.x - 0.5, self.pan_per_second),
            self._seconds(0.5 - position.y, self.tilt_per_second),
        )

    def _seconds(self, offset: float, per_second: float) -> float:
        if abs(offset) < self.dead_zone:
            return 0.0
        return round(offset / per_second, 2)


class PtzCommands:
    """Writes onvif-ingest's command track: one group per move, holding a ContinuousMove and a Stop
    JSON object for each axis it turns, or one sweep. Groups travel on separate streams, so only objects in one
    group reach onvif-ingest in order and a Stop never overtakes the next ContinuousMove."""

    def __init__(self, broadcast: moq.BroadcastProducer, track_name: str):
        # The broadcast is unannounced once its producer is garbage collected; its tracks do not keep it.
        self._broadcast = broadcast
        self._track = broadcast.publish_track(track_name)
        # The relay isolates a track whose publisher repeats a location, and the browser's ONVIF
        # page writes the same track, so group ids are seeded from the wall clock.
        self._next_group_id = time.time_ns() // 1_000

    def sweep(self, heading: "Heading") -> "Sweep":
        return Sweep(self._open_group(), heading)

    def center(self) -> None:
        """AbsoluteMove to pan 0, tilt 0: the known origin the search starts from."""
        group = self._open_group()
        write_command(group, {"type": "center", "speed": 1.0})
        group.finish()

    async def move(self, move: PtzMove) -> None:
        group = self._open_group()
        try:
            if move.pan_seconds:
                await turn(group, math.copysign(MOVE_VELOCITY, move.pan_seconds), 0.0, abs(move.pan_seconds))
            if move.tilt_seconds:
                await turn(group, 0.0, math.copysign(MOVE_VELOCITY, move.tilt_seconds), abs(move.tilt_seconds))
        finally:
            group.finish()

    def _open_group(self) -> moq.GroupProducer:
        group = self._track.create_group(self._next_group_id)
        self._next_group_id += 1
        return group


@dataclass(frozen=True)
class Heading:
    """Shares of MOVE_VELOCITY to pan and tilt at, each from -1 to 1; positive pans right and tilts up."""

    pan: float
    tilt: float


class Sweep:
    """One command group turning at MOVE_VELOCITY along `heading` until `stop()`, or until
    SWEEP_LEASE_SECONDS pass without `renew()`."""

    def __init__(self, group: moq.GroupProducer, heading: Heading):
        self.heading = heading
        self._group = group
        self._renewed_at = time.monotonic()
        self._stop_requested = asyncio.Event()
        command = {"pan": heading.pan * MOVE_VELOCITY, "tilt": heading.tilt * MOVE_VELOCITY}
        write_command(group, {"type": "continuous", **command, "zoom": 0.0, "speed": 1.0})
        self._task = asyncio.create_task(self._stop_when_released())

    @property
    def turning(self) -> bool:
        return not self._task.done()

    def renew(self) -> None:
        self._renewed_at = time.monotonic()

    async def stop(self) -> None:
        self._stop_requested.set()
        await self._task

    async def _stop_when_released(self) -> None:
        try:
            while not self._stop_requested.is_set() and (
                remaining := self._renewed_at + SWEEP_LEASE_SECONDS - time.monotonic()
            ) > 0:
                with contextlib.suppress(TimeoutError):
                    await asyncio.wait_for(self._stop_requested.wait(), remaining)
        finally:
            write_command(self._group, {"type": "stop"})
            self._group.finish()


async def turn(group: moq.GroupProducer, pan: float, tilt: float, seconds: float) -> None:
    write_command(group, {"type": "continuous", "pan": pan, "tilt": tilt, "zoom": 0.0, "speed": 1.0})
    try:
        await asyncio.sleep(seconds)
    finally:
        write_command(group, {"type": "stop"})


def write_command(group: moq.GroupProducer, command: dict[str, Any]) -> None:
    group.write_frame(json.dumps(command, separators=(",", ":")).encode())
