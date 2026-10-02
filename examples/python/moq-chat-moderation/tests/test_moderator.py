from pipecat.frames.frames import InputTransportMessageFrame
from pipecat.processors.frame_processor import FrameDirection
from pipecat.tests.utils import run_test

from moq_chat_moderation.event_timeline import EventTimeline
from moq_chat_moderation.moderator import ChatModerator
from tests.helpers import TRACK_NAME, StubJev, published_timeline


async def moderate(jev: StubJev, message: dict) -> list[dict]:
    timeline, reader = await published_timeline()
    await run_test(
        ChatModerator(jev, timeline),
        frames_to_send=[InputTransportMessageFrame(message=message)],
        frames_to_send_direction=FrameDirection.UPSTREAM,
        expected_up_frames=[],
    )
    return await reader.next_records()


async def test_abusive_chat_is_recorded_at_its_location():
    # Act
    records = await moderate(StubJev(probability=0.93), {"text": "shut up", "location": [7, 0]})

    # Assert
    assert records == [{"l": [7, 0], "data": {"abusive": True, "probability": 0.93}}]


async def test_ordinary_chat_is_recorded_as_not_abusive():
    # Act
    records = await moderate(StubJev(probability=0.02), {"text": "hello", "location": [8, 0]})

    # Assert
    assert records == [{"l": [8, 0], "data": {"abusive": False, "probability": 0.02}}]


async def test_other_transport_messages_pass_through_unjudged():
    # Arrange
    jev = StubJev(probability=0.5)
    timeline = EventTimeline(TRACK_NAME)

    # Act
    await run_test(
        ChatModerator(jev, timeline),
        frames_to_send=[InputTransportMessageFrame(message={"type": "client-ready"})],
        frames_to_send_direction=FrameDirection.UPSTREAM,
        expected_up_frames=[InputTransportMessageFrame],
    )

    # Assert
    assert jev.texts == []
