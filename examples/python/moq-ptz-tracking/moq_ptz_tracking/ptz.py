import json
import time
from dataclasses import dataclass

import moq

from moq_ptz_tracking.vision import POSITIONS

GRID_COLUMNS = 3
MOVE_SPEED = 0.5


@dataclass(frozen=True)
class PanTilt:
    pan: float
    tilt: float


@dataclass(frozen=True)
class PtzSteps:
    pan: float
    tilt: float

    def toward(self, position: str) -> PanTilt:
        """ONVIF pans right and tilts up for positive values."""
        if position not in POSITIONS:
            return PanTilt(0.0, 0.0)
        row, column = divmod(POSITIONS.index(position), GRID_COLUMNS)
        return PanTilt((column - 1) * self.pan, (1 - row) * self.tilt)


class PtzCommands:
    """Writes onvif-ingest's command track: one RelativeMove JSON object per group."""

    def __init__(self, broadcast: moq.BroadcastProducer, track_name: str):
        # The broadcast is unannounced once its producer is garbage collected; its tracks do not keep it.
        self._broadcast = broadcast
        self._track = broadcast.publish_track(track_name)
        # The relay isolates a track whose publisher repeats a location, and the browser's ONVIF
        # page writes the same track, so group ids are seeded from the wall clock.
        self._next_group_id = time.time_ns() // 1_000

    def relative_move(self, move: PanTilt) -> None:
        group = self._track.create_group(self._next_group_id)
        self._next_group_id += 1
        command = {"type": "relative", "pan": move.pan, "tilt": move.tilt, "zoom": 0.0, "speed": MOVE_SPEED}
        group.write_frame(json.dumps(command, separators=(",", ":")).encode())
        group.finish()
