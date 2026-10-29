from pipecat.frames.frames import TextFrame
from pipecat.tests.utils import run_test

from moq_chat_moderation.chat import ChatMessage, Label
from moq_system_one_bot.event_timeline import EventTimeline
from moq_chat_moderation.moderator import ChatModerator
from tests.helpers import StubSystemOne, published_timeline

LABELS = (Label("喜", "喜んでいる、うれしい"), Label("暴言", "相手を侮辱・攻撃・脅す"))


async def test_chat_is_recorded_with_each_label_probability_at_its_location():
    # Arrange
    timeline, publication = await published_timeline()
    chat = ChatMessage(text="shut up", labels=LABELS, location=(7, 0))

    # Act
    await run_test(ChatModerator(StubSystemOne([0.02, 0.93]), timeline), frames_to_send=[chat], expected_down_frames=[])

    # Assert
    _, records = await publication.next_write()
    assert records == [
        {"l": [7, 0], "data": {"labels": [{"name": "喜", "probability": 0.02}, {"name": "暴言", "probability": 0.93}]}}
    ]


async def test_chat_without_labels_is_not_judged():
    # Arrange
    system_one = StubSystemOne([])

    # Act
    await run_test(
        ChatModerator(system_one, EventTimeline(replay=True)),
        frames_to_send=[ChatMessage(text="hello", labels=(), location=(8, 0))],
        expected_down_frames=[],
    )

    # Assert
    assert system_one.texts == []


async def test_other_frames_pass_through_unjudged():
    # Arrange
    system_one = StubSystemOne([0.5])

    # Act
    await run_test(
        ChatModerator(system_one, EventTimeline(replay=True)),
        frames_to_send=[TextFrame(text="hello")],
        expected_down_frames=[TextFrame],
    )

    # Assert
    assert system_one.texts == []
