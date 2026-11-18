import json

from moq_wasm import Location
from moq_wasm.pipecat import MoqWasmEventRecord, MoqWasmTrackMessage
from pipecat.frames.frames import InputTransportMessageFrame, OutputTransportMessageFrame, TextFrame
from pipecat.tests.utils import SleepFrame, run_test

from moq_chat_moderation.moderator import EVENT_TIMELINE_TRACK, ChatModerator
from tests.helpers import StubSystemOne

LABELS = [{"name": "喜", "description": "喜んでいる、うれしい"}, {"name": "暴言", "description": "相手を侮辱・攻撃・脅す"}]


def chat(text: str, labels: list[dict], location: tuple[int, int], participant_id: str = "chat") -> InputTransportMessageFrame:
    payload = json.dumps({"text": text, "labels": labels}).encode()
    return InputTransportMessageFrame(message=MoqWasmTrackMessage(participant_id, "chat", payload, Location(*location)))


def records(frames) -> list[MoqWasmEventRecord]:
    return [frame.message for frame in frames if isinstance(frame, OutputTransportMessageFrame)]


async def test_a_chat_is_recorded_with_each_label_probability_at_its_location():
    # Act
    down, _ = await run_test(
        ChatModerator(StubSystemOne([0.02, 0.93])), frames_to_send=[chat("shut up", LABELS, (7, 0)), SleepFrame()]
    )

    # Assert
    assert records(down) == [
        MoqWasmEventRecord(
            EVENT_TIMELINE_TRACK,
            {"L": [7, 0], "data": {"labels": [{"name": "喜", "probability": 0.02}, {"name": "暴言", "probability": 0.93}]}},
        )
    ]


async def test_a_chat_without_labels_an_invalid_one_and_one_from_another_participant_are_not_judged():
    # Arrange
    system_one = StubSystemOne([0.5])
    invalid = InputTransportMessageFrame(message=MoqWasmTrackMessage("chat", "chat", b"\xff", Location(9, 0)))

    # Act
    down, _ = await run_test(
        ChatModerator(system_one),
        frames_to_send=[
            chat("hello", [], (8, 0)),
            invalid,
            chat("hello", LABELS[:1], (10, 0), participant_id="other"),
            SleepFrame(),
        ],
    )

    # Assert
    assert system_one.texts == []
    assert records(down) == []


async def test_other_frames_pass_through_unjudged():
    # Arrange
    system_one = StubSystemOne([0.5])

    # Act
    await run_test(ChatModerator(system_one), frames_to_send=[TextFrame(text="hello")], expected_down_frames=[TextFrame])

    # Assert
    assert system_one.texts == []
