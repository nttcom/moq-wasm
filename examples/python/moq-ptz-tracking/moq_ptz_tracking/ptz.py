import asyncio
import json
import math
import time
from dataclasses import dataclass
from typing import Any

import moq

from moq_ptz_tracking.vision import Position

MOVE_VELOCITY = 0.5


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
    JSON object for each axis it turns. Groups travel on separate streams, so only objects in one
    group reach onvif-ingest in order and a Stop never overtakes the next ContinuousMove."""

    def __init__(self, broadcast: moq.BroadcastProducer, track_name: str):
        # The broadcast is unannounced once its producer is garbage collected; its tracks do not keep it.
        self._broadcast = broadcast
        self._track = broadcast.publish_track(track_name)
        # The relay isolates a track whose publisher repeats a location, and the browser's ONVIF
        # page writes the same track, so group ids are seeded from the wall clock.
        self._next_group_id = time.time_ns() // 1_000

    async def move(self, move: PtzMove) -> None:
        group = self._track.create_group(self._next_group_id)
        self._next_group_id += 1
        try:
            if move.pan_seconds:
                await turn(group, math.copysign(MOVE_VELOCITY, move.pan_seconds), 0.0, abs(move.pan_seconds))
            if move.tilt_seconds:
                await turn(group, 0.0, math.copysign(MOVE_VELOCITY, move.tilt_seconds), abs(move.tilt_seconds))
        finally:
            group.finish()


async def turn(group: moq.GroupProducer, pan: float, tilt: float, seconds: float) -> None:
    write_command(group, {"type": "continuous", "pan": pan, "tilt": tilt, "zoom": 0.0, "speed": 1.0})
    try:
        await asyncio.sleep(seconds)
    finally:
        write_command(group, {"type": "stop"})


def write_command(group: moq.GroupProducer, command: dict[str, Any]) -> None:
    group.write_frame(json.dumps(command, separators=(",", ":")).encode())
