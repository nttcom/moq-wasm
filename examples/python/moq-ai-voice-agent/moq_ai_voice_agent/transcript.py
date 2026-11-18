import time
from collections import deque
from collections.abc import Awaitable, Callable

from moq_wasm.media import EventTimeline
from pipecat.frames.frames import (
    BotStartedSpeakingFrame,
    Frame,
    InterruptionFrame,
    LLMFullResponseEndFrame,
    TTSTextFrame,
)
from pipecat.processors.aggregators.llm_response_universal import LLMUserAggregator, UserTurnStoppedMessage
from pipecat.processors.frame_processor import FrameDirection, FrameProcessor
from pipecat.utils.string import TextPartForConcatenation, concatenate_aggregated_text

TRACK_NAME = "transcript"
USER = "user"
ASSISTANT = "assistant"


def transcript_record(role: str, text: str, at_ms: int, turn: int | None = None) -> dict:
    """A draft-ietf-moq-msf-02 §8 record of the conversation. The records of one answer of the bot
    share `turn`, the time of its first record, each with the answer so far."""
    data: dict = {"role": role, "text": text}
    if turn is not None:
        data["turn"] = turn
    return {"T": at_ms, "data": data}


def _now_ms() -> int:
    return int(time.time() * 1000)


async def _append(timeline: EventTimeline, role: str, text: str, turn: int | None = None) -> None:
    await timeline.append(transcript_record(role, text, _now_ms(), turn))


def write_user_turns(aggregator: LLMUserAggregator, timeline: EventTimeline) -> None:
    @aggregator.event_handler("on_user_turn_stopped")
    async def on_user_turn_stopped(_aggregator, _strategy, message: UserTurnStoppedMessage):
        if message.content:
            await _append(timeline, USER, message.content)


class _Tap(FrameProcessor):
    def __init__(self, on_frame: Callable[[Frame], Awaitable[None]]):
        super().__init__()
        self._on_frame = on_frame

    async def process_frame(self, frame: Frame, direction: FrameDirection):
        await super().process_frame(frame, direction)
        if direction == FrameDirection.DOWNSTREAM:
            await self._on_frame(frame)
        await self.push_frame(frame, direction)


class AnswerTranscript:
    """Appends the bot's answer sentence by sentence as each starts to play.

    The TTS hands over a sentence's text right after synthesizing its audio, faster than it plays,
    and the output transport passes that text on once the audio has played. So `synthesized`, put
    before the output transport, learns the sentences ahead; `spoken`, put after it, shows each
    sentence as the one before it finishes playing, or as soon as it is synthesized if that one has
    already played."""

    def __init__(self, timeline: EventTimeline):
        self._timeline = timeline
        # The sentences of the answer now playing, then of each answer synthesized after it.
        self._answers: deque[list[TextPartForConcatenation]] = deque([[]])
        self._shown = 0
        self._played = 0
        self._turn = 0
        self.synthesized = _Tap(self._on_synthesized)
        self.spoken = _Tap(self._on_spoken)

    async def _on_synthesized(self, frame: Frame) -> None:
        if isinstance(frame, TTSTextFrame) and frame.text.strip():
            self._answers[-1].append(TextPartForConcatenation(frame.text, frame.includes_inter_frame_spaces))
            # Everything shown has played, so this sentence plays as soon as its audio arrives.
            if len(self._answers) == 1 and 0 < self._shown == self._played:
                await self._show_through(self._shown + 1)
        elif isinstance(frame, LLMFullResponseEndFrame):
            self._answers.append([])
        elif isinstance(frame, InterruptionFrame):
            self._forget()

    async def _on_spoken(self, frame: Frame) -> None:
        if isinstance(frame, BotStartedSpeakingFrame):
            await self._show_through(self._played + 1)
        elif isinstance(frame, TTSTextFrame) and frame.text.strip():
            self._played += 1
            await self._show_through(self._played + 1)
        elif isinstance(frame, LLMFullResponseEndFrame):
            if len(self._answers) > 1:
                self._answers.popleft()
            self._shown = self._played = 0
        elif isinstance(frame, InterruptionFrame):
            self._forget()

    async def _show_through(self, count: int) -> None:
        sentences = self._answers[0]
        count = min(count, len(sentences))
        if count <= self._shown:
            return
        if self._shown == 0:
            # Wall-clock milliseconds, so a page that stays across a bot reconnect still tells answers apart.
            self._turn = max(_now_ms(), self._turn + 1)
        self._shown = count
        await _append(self._timeline, ASSISTANT, concatenate_aggregated_text(sentences[:count]), self._turn)

    def _forget(self) -> None:
        self._answers = deque([[]])
        self._shown = self._played = 0
