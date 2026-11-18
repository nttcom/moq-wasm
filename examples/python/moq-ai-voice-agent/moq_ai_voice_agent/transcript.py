import time
from collections import deque
from collections.abc import Awaitable, Callable

from moq_wasm.pipecat import MoqWasmEventRecord
from pipecat.frames.frames import (
    BotStartedSpeakingFrame,
    Frame,
    InterruptionFrame,
    LLMFullResponseEndFrame,
    OutputTransportMessageUrgentFrame,
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


def _record_frame(role: str, text: str, turn: int | None = None) -> OutputTransportMessageUrgentFrame:
    """The output transport appends the record to its `transcript` event timeline. Urgent, so it does
    not wait behind the speech queued there, and is written from either direction."""
    return OutputTransportMessageUrgentFrame(
        message=MoqWasmEventRecord(TRACK_NAME, transcript_record(role, text, _now_ms(), turn))
    )


def write_user_turns(aggregator: LLMUserAggregator) -> None:
    @aggregator.event_handler("on_user_turn_stopped")
    async def on_user_turn_stopped(aggregator: LLMUserAggregator, _strategy, message: UserTurnStoppedMessage):
        if message.content:
            await aggregator.push_frame(_record_frame(USER, message.content))


Send = Callable[[Frame], Awaitable[None]]


class _Tap(FrameProcessor):
    """Hands each downstream frame to `on_frame` with a way to send a frame toward the output
    transport: downstream from before it, upstream from after it."""

    def __init__(self, on_frame: Callable[[Frame, Send], Awaitable[None]], toward_output: FrameDirection):
        super().__init__()
        self._on_frame = on_frame
        self._toward_output = toward_output

    async def process_frame(self, frame: Frame, direction: FrameDirection):
        await super().process_frame(frame, direction)
        if direction == FrameDirection.DOWNSTREAM:
            await self._on_frame(frame, lambda sent: self.push_frame(sent, self._toward_output))
        await self.push_frame(frame, direction)


class AnswerTranscript:
    """Appends the bot's answer sentence by sentence as each starts to play.

    The TTS hands over a sentence's text right after synthesizing its audio, faster than it plays,
    and the output transport passes that text on once the audio has played. So `synthesized`, put
    before the output transport, learns the sentences ahead; `spoken`, put after it, shows each
    sentence as the one before it finishes playing, or as soon as it is synthesized if that one has
    already played."""

    def __init__(self):
        # The sentences of the answer now playing, then of each answer synthesized after it.
        self._answers: deque[list[TextPartForConcatenation]] = deque([[]])
        self._shown = 0
        self._played = 0
        self._turn = 0
        self.synthesized = _Tap(self._on_synthesized, FrameDirection.DOWNSTREAM)
        self.spoken = _Tap(self._on_spoken, FrameDirection.UPSTREAM)

    async def _on_synthesized(self, frame: Frame, send: Send) -> None:
        if isinstance(frame, TTSTextFrame) and frame.text.strip():
            self._answers[-1].append(TextPartForConcatenation(frame.text, frame.includes_inter_frame_spaces))
            # Everything shown has played, so this sentence plays as soon as its audio arrives.
            if len(self._answers) == 1 and 0 < self._shown == self._played:
                await self._show_through(self._shown + 1, send)
        elif isinstance(frame, LLMFullResponseEndFrame):
            self._answers.append([])
        elif isinstance(frame, InterruptionFrame):
            self._forget()

    async def _on_spoken(self, frame: Frame, send: Send) -> None:
        if isinstance(frame, BotStartedSpeakingFrame):
            await self._show_through(self._played + 1, send)
        elif isinstance(frame, TTSTextFrame) and frame.text.strip():
            self._played += 1
            await self._show_through(self._played + 1, send)
        elif isinstance(frame, LLMFullResponseEndFrame):
            if len(self._answers) > 1:
                self._answers.popleft()
            self._shown = self._played = 0
        elif isinstance(frame, InterruptionFrame):
            self._forget()

    async def _show_through(self, count: int, send: Send) -> None:
        sentences = self._answers[0]
        count = min(count, len(sentences))
        if count <= self._shown:
            return
        if self._shown == 0:
            # Wall-clock milliseconds, so a page that stays across a bot reconnect still tells answers apart.
            self._turn = max(_now_ms(), self._turn + 1)
        self._shown = count
        await send(_record_frame(ASSISTANT, concatenate_aggregated_text(sentences[:count]), self._turn))

    def _forget(self) -> None:
        self._answers = deque([[]])
        self._shown = self._played = 0
