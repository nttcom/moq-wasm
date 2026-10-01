import time

from pipecat.tests.utils import run_test

from moq_camera_detection.detector import STALE_KEYFRAME_SECONDS, CameraKeyframe, PersonDetector
from tests.helpers import StubVision, next_records, published_timeline


async def test_fresh_keyframe_verdict_is_recorded_at_its_group():
    # Arrange
    timeline, group = await published_timeline()
    vision = StubVision(person=True)

    # Act
    await run_test(
        PersonDetector(vision, timeline),
        frames_to_send=[CameraKeyframe(jpeg=b"jpeg", group_id=42, decoded_at=time.monotonic())],
        expected_down_frames=[],
    )

    # Assert
    assert await next_records(group) == [{"l": [42, 0], "data": {"person": True}}]


async def test_stale_keyframe_is_not_judged():
    # Arrange
    timeline, _ = await published_timeline()
    vision = StubVision(person=True)
    decoded_at = time.monotonic() - STALE_KEYFRAME_SECONDS - 1

    # Act
    await run_test(
        PersonDetector(vision, timeline),
        frames_to_send=[CameraKeyframe(jpeg=b"jpeg", group_id=43, decoded_at=decoded_at)],
        expected_down_frames=[],
    )

    # Assert
    assert vision.images == []
