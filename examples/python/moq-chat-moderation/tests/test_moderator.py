from pipecat.frames.frames import TextFrame
from pipecat.tests.utils import run_test

from moq_chat_moderation.chat import ChatMessage
from moq_chat_moderation.event_timeline import EventTimeline
from moq_chat_moderation.moderator import ChatModerator
from tests.helpers import StubD3Flash, published_timeline


async def moderate(d3_flash: StubD3Flash, chat: ChatMessage) -> list[dict]:
    timeline, publication = await published_timeline()
    await run_test(ChatModerator(d3_flash, timeline), frames_to_send=[chat], expected_down_frames=[])
    _, records = await publication.next_write()
    return records


async def test_abusive_chat_is_recorded_at_its_location():
    # Act
    records = await moderate(StubD3Flash(probability=0.93), ChatMessage(text="shut up", location=(7, 0)))

    # Assert
    assert records == [{"l": [7, 0], "data": {"abusive": True, "probability": 0.93}}]


async def test_ordinary_chat_is_recorded_as_not_abusive():
    # Act
    records = await moderate(StubD3Flash(probability=0.02), ChatMessage(text="hello", location=(8, 0)))

    # Assert
    assert records == [{"l": [8, 0], "data": {"abusive": False, "probability": 0.02}}]


async def test_other_frames_pass_through_unjudged():
    # Arrange
    d3_flash = StubD3Flash(probability=0.5)

    # Act
    await run_test(
        ChatModerator(d3_flash, EventTimeline()),
        frames_to_send=[TextFrame(text="hello")],
        expected_down_frames=[TextFrame],
    )

    # Assert
    assert d3_flash.texts == []
