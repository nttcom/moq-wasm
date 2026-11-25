import asyncio
import json
from dataclasses import dataclass, field

import pytest
from moq_wasm import Location
from moq_wasm.pipecat import MoqWasmTrackMessage
from pipecat.frames.frames import (
    EndFrame,
    InputTransportMessageFrame,
    OutputTransportMessageUrgentFrame,
    UserImageRawFrame,
    UserImageRequestFrame,
)
from pipecat.tests.utils import SleepFrame, run_test

from moq_ptz_tracking.pan_planner import PanPlanner
from moq_ptz_tracking.ptz import PtzCalibration, PtzCommands
from moq_ptz_tracking.tracker import PtzTracker
from moq_ptz_tracking.vision import Answer, Box, Sighting, TargetBox
from moq_system_one_bot.testing import records
from tests.helpers import MUG_TARGET, StubVision, published_commands, sighting

CALIBRATION = PtzCalibration(pan_per_second=10.0, tilt_per_second=10.0, margin=0.1, dead_zone=0.05)
OFF_CENTER = sighting(Box(0.7, 0.4, 0.95, 0.6))
NOTHING_MORE_WAIT_SECONDS = 0.2
SIZE = (16, 16)


def prompt(targets: list[str], video: str = MUG_TARGET.video_track, location: tuple[int, int] = (7, 0)):
    payload = json.dumps({"targets": targets, "video": video}).encode()
    return InputTransportMessageFrame(message=MoqWasmTrackMessage("viewer", "prompt", payload, Location(*location)))


MUG_PROMPT = prompt(["a red mug"])


def picture(location: tuple[int, int], video: str = MUG_TARGET.video_track) -> UserImageRawFrame:
    frame = UserImageRawFrame(
        image=bytes(SIZE[0] * SIZE[1] * 3),
        size=SIZE,
        format="RGB",
        user_id="client",
        request=UserImageRequestFrame(user_id="client", video_source=video),
    )
    frame.metadata["location"] = Location(*location)
    return frame


def requests(frames) -> list[UserImageRequestFrame]:
    return [frame for frame in frames if isinstance(frame, UserImageRequestFrame)]


@dataclass
class Released:
    tracks: list[str] = field(default_factory=list)

    async def __call__(self, track: str) -> None:
        self.tracks.append(track)


async def tracker_with(vision, released: Released | None = None):
    commands, command_publication = await published_commands()
    return PtzTracker(vision, commands, CALIBRATION, released or Released()), command_publication


async def test_a_box_leaving_the_margin_moves_the_camera_and_is_recorded():
    # Arrange
    tracker, command_track = await tracker_with(StubVision(OFF_CENTER))

    # Act
    down, _ = await run_test(tracker, frames_to_send=[MUG_PROMPT, picture((42, 3)), SleepFrame()])

    # Assert
    assert [command["type"] for command in await command_track.next_group()] == ["continuous", "stop"]
    assert records(down) == [
        {
            "L": [42, 3],
            "data": {
                "prompt": (7, 0),
                "boxes": [[0, [0.7, 0.4, 0.95, 0.6]]],
                "pan_seconds": 0.03,
                "tilt_seconds": 0.0,
                "sweep": None,
            },
        }
    ]


async def test_boxes_of_every_target_inside_the_margin_leave_the_camera_still():
    # Arrange
    boxes = Sighting((TargetBox(0, Box(0.15, 0.2, 0.3, 0.8)), TargetBox(1, Box(0.7, 0.5, 0.85, 0.85))))
    tracker, command_track = await tracker_with(StubVision(boxes))

    # Act
    down, _ = await run_test(tracker, frames_to_send=[MUG_PROMPT, picture((42, 3)), SleepFrame()])

    # Assert
    assert records(down)[0]["data"]["boxes"] == [[0, [0.15, 0.2, 0.3, 0.8]], [1, [0.7, 0.5, 0.85, 0.85]]]
    with pytest.raises(TimeoutError):
        await command_track.next_write(NOTHING_MORE_WAIT_SECONDS)


async def test_a_target_never_seen_returns_the_camera_to_home_before_searching():
    # Arrange
    tracker, command_track = await tracker_with(StubVision(Answer.NOT_VISIBLE))

    # Act
    down, _ = await run_test(tracker, frames_to_send=[MUG_PROMPT, picture((42, 3)), SleepFrame()])

    # Assert
    assert await command_track.next_group() == [{"type": "center", "speed": 1.0}]
    assert records(down)[0]["data"]["boxes"] == "not visible"


async def test_pictures_of_the_targets_track_are_asked_for_while_the_camera_is_still():
    # Arrange
    tracker, _ = await tracker_with(StubVision(sighting(Box(0.4, 0.4, 0.6, 0.6))))

    # Act
    _, up = await run_test(tracker, frames_to_send=[MUG_PROMPT, SleepFrame(0.3)])

    # Assert: one request waits for its picture before the next is made
    assert [(r.user_id, r.video_source) for r in requests(up)] == [("client", MUG_TARGET.video_track)]


async def test_after_a_move_no_picture_is_asked_for_until_the_camera_has_settled():
    # Arrange
    tracker, _ = await tracker_with(StubVision(OFF_CENTER))

    # Act
    _, up = await run_test(tracker, frames_to_send=[MUG_PROMPT, picture((42, 0)), SleepFrame(0.5)])

    # Assert
    assert requests(up) == []


async def test_a_picture_arriving_while_another_is_located_is_skipped():
    # Arrange
    vision = StubVision(sighting(Box(0.4, 0.4, 0.6, 0.6)))
    tracker, _ = await tracker_with(vision)

    # Act
    await run_test(tracker, frames_to_send=[MUG_PROMPT, picture((42, 0)), picture((42, 1)), SleepFrame()])

    # Assert
    assert len(vision.images) == 1


async def test_an_answer_for_a_target_stopped_meanwhile_neither_moves_the_camera_nor_is_recorded():
    # Arrange
    @dataclass
    class StoppingVision:
        tracker: PtzTracker | None = None

        async def locate(self, jpeg: bytes, descriptions: tuple[str, ...]) -> Sighting:
            await self.tracker.set_target(None)
            return OFF_CENTER

    vision = StoppingVision()
    tracker, command_track = await tracker_with(vision)
    vision.tracker = tracker

    # Act
    down, _ = await run_test(tracker, frames_to_send=[MUG_PROMPT, picture((44, 0)), SleepFrame()])

    # Assert
    assert records(down) == []
    with pytest.raises(TimeoutError):
        await command_track.next_write(NOTHING_MORE_WAIT_SECONDS)


@pytest.mark.parametrize(
    ("next_prompt", "released"),
    [
        (prompt(["a blue cap"], location=(8, 0)), []),
        (prompt(["a red mug"], video="video/profile_2", location=(8, 0)), [MUG_TARGET.video_track]),
        (prompt([], location=(8, 0)), [MUG_TARGET.video_track]),
    ],
)
async def test_the_camera_profile_is_released_when_the_target_leaves_it(next_prompt, released):
    # Arrange
    released_tracks = Released()
    tracker, _ = await tracker_with(StubVision(OFF_CENTER), released_tracks)

    # Act
    await run_test(tracker, frames_to_send=[MUG_PROMPT, next_prompt])

    # Assert
    assert released_tracks.tracks == released


async def test_commands_go_out_as_urgent_messages_and_a_turn_stops_when_the_pipeline_ends():
    # Arrange: a turn longer than the test, so only the end of the pipeline stops it
    tracker, _ = await tracker_with(StubVision(OFF_CENTER))
    commands = PtzCommands()
    commands.publish_on(tracker.command_writer())
    tracker._commands = commands
    long_turn = PtzCalibration(pan_per_second=0.01, tilt_per_second=0.01, margin=0.1, dead_zone=0.05)
    tracker._planner = PanPlanner(long_turn)

    # Act
    down, _ = await run_test(
        tracker, frames_to_send=[MUG_PROMPT, picture((42, 0)), SleepFrame(0.2), EndFrame()], send_end_frame=False
    )

    # Assert: the Stop reaches the output before the EndFrame closes it
    written = [
        json.loads(frame.message.payload)["type"] if isinstance(frame, OutputTransportMessageUrgentFrame) else "end"
        for frame in down
        if isinstance(frame, (OutputTransportMessageUrgentFrame, EndFrame))
    ]
    assert written == ["continuous", "stop", "end"]
