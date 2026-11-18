from pipecat.frames.frames import (
    BotStartedSpeakingFrame,
    InterruptionFrame,
    LLMFullResponseEndFrame,
    OutputTransportMessageUrgentFrame,
    TTSTextFrame,
)
from pipecat.tests.utils import SleepFrame, run_test

from moq_ai_voice_agent.transcript import ASSISTANT, AnswerTranscript, transcript_record


class RecordingTimeline:
    """The records the taps send toward the output transport, in order."""

    def __init__(self):
        self.records: list[dict] = []

    async def run(self, tap, frames) -> None:
        downstream, upstream = await run_test(tap, frames_to_send=frames)
        self.records += [
            frame.message.record
            for frame in [*downstream, *upstream]
            if isinstance(frame, OutputTransportMessageUrgentFrame)
        ]


def sentence(text: str) -> TTSTextFrame:
    return TTSTextFrame(text, text_type="sentence")


def answers(timeline: RecordingTimeline) -> list[tuple[int, str]]:
    """Each record's text with the index of its answer, in order of appearance."""
    turns: list[int] = []
    for record in timeline.records:
        if record["data"]["turn"] not in turns:
            turns.append(record["data"]["turn"])
    return [(turns.index(record["data"]["turn"]) + 1, record["data"]["text"]) for record in timeline.records]


def test_a_record_of_an_answer_carries_its_turn():
    # Act
    record = transcript_record(ASSISTANT, "こんにちは", 1_700_000_000_000, turn=2)

    # Assert: the shape examples/browser/examples/moq-ai-voice-agent/transcript.ts reads
    assert record == {"T": 1_700_000_000_000, "data": {"role": "assistant", "text": "こんにちは", "turn": 2}}


async def test_each_sentence_shows_as_it_starts_to_play():
    # Arrange: the TTS synthesized the whole answer before any of it played
    timeline = RecordingTimeline()
    answer = AnswerTranscript()
    await timeline.run(answer.synthesized, [sentence("こんにちは。"), sentence("いい天気ですね。"), LLMFullResponseEndFrame()])
    shown_before_playing = list(timeline.records)

    # Act: the output transport plays it; a sentence's text passes once its audio has played
    await timeline.run(
        answer.spoken,
        [BotStartedSpeakingFrame(), SleepFrame(), sentence("こんにちは。"), sentence("いい天気ですね。"), LLMFullResponseEndFrame()],
    )

    # Assert
    assert shown_before_playing == []
    assert answers(timeline) == [(1, "こんにちは。"), (1, "こんにちは。 いい天気ですね。")]


async def test_a_sentence_synthesized_after_the_one_before_played_shows_at_once():
    # Arrange: the first sentence played before the TTS finished the second
    timeline = RecordingTimeline()
    answer = AnswerTranscript()
    await timeline.run(answer.synthesized, [sentence("こんにちは。")])
    await timeline.run(answer.spoken, [BotStartedSpeakingFrame(), SleepFrame(), sentence("こんにちは。")])

    # Act
    await timeline.run(answer.synthesized, [sentence("長い二つ目の文です。")])

    # Assert
    assert answers(timeline) == [(1, "こんにちは。"), (1, "こんにちは。 長い二つ目の文です。")]


async def test_the_next_answer_starts_a_new_turn():
    # Arrange
    timeline = RecordingTimeline()
    answer = AnswerTranscript()
    await timeline.run(
        answer.synthesized, [sentence("一つ目。"), LLMFullResponseEndFrame(), sentence("二つ目。"), LLMFullResponseEndFrame()]
    )

    # Act
    await timeline.run(
        answer.spoken,
        [
            BotStartedSpeakingFrame(),
            SleepFrame(),
            sentence("一つ目。"),
            LLMFullResponseEndFrame(),
            BotStartedSpeakingFrame(),
            SleepFrame(),
            sentence("二つ目。"),
        ],
    )

    # Assert
    assert answers(timeline) == [(1, "一つ目。"), (2, "二つ目。")]


async def test_an_interruption_drops_the_sentences_not_yet_shown():
    # Arrange
    timeline = RecordingTimeline()
    answer = AnswerTranscript()
    await timeline.run(answer.synthesized, [sentence("途中まで。"), sentence("言わなかった文。")])

    # Act: the interruption is a system frame, which would overtake a sentence still queued
    await timeline.run(answer.spoken, [BotStartedSpeakingFrame(), SleepFrame(), InterruptionFrame(), sentence("途中まで。")])

    # Assert
    assert answers(timeline) == [(1, "途中まで。")]
