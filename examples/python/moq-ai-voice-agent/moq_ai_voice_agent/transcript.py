import time

from moq_wasm.media import EventTimeline
from pipecat.processors.aggregators.llm_response_universal import (
    AssistantTurnStoppedMessage,
    LLMContextAggregatorPair,
    UserTurnStoppedMessage,
)

TRACK_NAME = "transcript"
USER = "user"
ASSISTANT = "assistant"


def transcript_record(role: str, text: str, at_ms: int) -> dict:
    """A draft-ietf-moq-msf-02 §8 record of one turn of the conversation."""
    return {"T": at_ms, "data": {"role": role, "text": text}}


def write_transcript(aggregators: LLMContextAggregatorPair, timeline: EventTimeline) -> None:
    """Appends what the user said and what the bot answered, each as its turn ends."""

    async def append(role: str, text: str | None) -> None:
        if text:
            await timeline.append(transcript_record(role, text, int(time.time() * 1000)))

    @aggregators.user().event_handler("on_user_turn_stopped")
    async def on_user_turn_stopped(_aggregator, _strategy, message: UserTurnStoppedMessage):
        await append(USER, message.content)

    @aggregators.assistant().event_handler("on_assistant_turn_stopped")
    async def on_assistant_turn_stopped(_aggregator, message: AssistantTurnStoppedMessage):
        await append(ASSISTANT, message.content)
