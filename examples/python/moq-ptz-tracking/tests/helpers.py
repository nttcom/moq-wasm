from dataclasses import dataclass, field
from typing import Any

from moq_system_one_bot import testing
from moq_system_one_bot.testing import READ_TIMEOUT_SECONDS
from moq_ptz_tracking.ptz import PtzCommands
from moq_ptz_tracking.target import LatestTarget, Target
from moq_ptz_tracking.vision import Answer, Box, Sighting, TargetBox

# next_group ends a group after this long without a write, so a test's turns must be shorter.
MOVE_END_WAIT_SECONDS = 0.3

MUG_TARGET = Target(("a red mug",), "video/profile_1", (7, 0))


class RecordingPublication(testing.RecordingPublication):
    def __init__(self):
        super().__init__()
        self._held: tuple[bool, Any] | None = None

    async def next_write(self, timeout: float = READ_TIMEOUT_SECONDS) -> tuple[bool, Any]:
        if self._held is not None:
            held, self._held = self._held, None
            return held
        return await super().next_write(timeout)

    async def next_group(self, timeout: float = READ_TIMEOUT_SECONDS) -> list[Any]:
        """The objects of the next group: from a write that opens one until the next such write, or
        until MOVE_END_WAIT_SECONDS pass without a write."""
        new_group, first = await self.next_write(timeout)
        assert new_group, f"{first} does not open a group"
        objects = [first]
        while True:
            try:
                new_group, written = await self.next_write(MOVE_END_WAIT_SECONDS)
            except TimeoutError:
                return objects
            if new_group:
                self._held = (new_group, written)
                return objects
            objects.append(written)


async def published_commands() -> tuple[PtzCommands, RecordingPublication]:
    commands = PtzCommands()
    publication = RecordingPublication()
    await commands.publish_on(publication)
    return commands, publication


def tracking(target: Target) -> LatestTarget:
    latest_target = LatestTarget()
    latest_target.set(target)
    return latest_target


def sighting(*boxes: Box) -> Sighting:
    return Sighting(tuple(TargetBox(0, box) for box in boxes))


@dataclass
class StubVision:
    answer: Sighting | Answer
    images: list[bytes] = field(default_factory=list)

    async def locate(self, jpeg: bytes, descriptions: tuple[str, ...]) -> Sighting | Answer:
        self.images.append(jpeg)
        return self.answer
