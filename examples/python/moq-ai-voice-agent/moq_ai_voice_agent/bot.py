import argparse
import asyncio
import os
from functools import partial

import moq_wasm
from moq_bot_rooms.relay import add_relay_arguments
from moq_bot_rooms.rooms import GRACE_SECONDS, BotRoom
from moq_bot_rooms.server import add_serve_arguments, serve_rooms
from moq_system_one_bot.relay import stay_connected
from moq_wasm.pipecat import MoqWasmParams, MoqWasmTransport
from pipecat.audio.vad.silero import SileroVADAnalyzer
from pipecat.pipeline.pipeline import Pipeline
from pipecat.pipeline.worker import PipelineWorker
from pipecat.processors.aggregators.llm_context import LLMContext
from pipecat.processors.aggregators.llm_response_universal import LLMContextAggregatorPair, LLMUserAggregatorParams
from pipecat.processors.filters.function_filter import FunctionFilter
from pipecat.transcriptions.language import Language
from pipecat.workers.runner import WorkerRunner

from moq_ai_voice_agent import transcript
from moq_ai_voice_agent.gemini import GeminiServices, VertexProject
from moq_ai_voice_agent.listener import Listener

DEFAULT_PORT = 8101
# Every room holds its own Gemini sessions; the bot itself does little but VAD.
DEFAULT_MAX_ROOMS = 4
# The page publishes its microphone one level under the room, and the bot its voice and transcript.
PARTICIPANT_ID = "gemini"
# The transcript is kept for FETCH as long as a room runs by default (--max-room-seconds).
TRANSCRIPT_RETENTION_SECONDS = 60 * 60
LANGUAGE = Language.JA_JP
VOICE = "Kore"
# Gemini Live closes a session that gets no audio for a minute, and pipecat's Gemini STT gives up after
# three such closes in a row. Nobody may speak for longer than that: before the first participant
# joins, or while the one the bot listens to is muted. Silence sent after this many seconds without
# audio keeps the session open.
STT_KEEPALIVE_SECONDS = 10.0
SYSTEM_INSTRUCTION = (
    "あなたはユーザーと音声で会話するアシスタントです。"
    "あなたの返答は音声合成で読み上げられるので、記号、箇条書き、Markdown、絵文字を使わず、"
    "話し言葉の短い文で、1 回に 2、3 文までで答えてください。"
)


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description="Talk with the MoQ AI Voice Agent page through Gemini")
    add_serve_arguments(parser, bot="moq-ai-voice-agent", default_port=DEFAULT_PORT, default_max_rooms=DEFAULT_MAX_ROOMS)
    add_relay_arguments(parser)
    parser.add_argument(
        "--vertex-project",
        help="call Gemini on this Google Cloud project's Vertex AI with the application default credentials "
        "instead of the Gemini API with GOOGLE_API_KEY",
    )
    return parser.parse_args()


def voice_pipeline(gemini: GeminiServices, transport: MoqWasmTransport, listener: Listener) -> Pipeline:
    stt = gemini.stt(keepalive_timeout=STT_KEEPALIVE_SECONDS)
    llm = gemini.llm(SYSTEM_INSTRUCTION)
    tts = gemini.tts(VOICE)
    aggregators = LLMContextAggregatorPair(
        LLMContext(), user_params=LLMUserAggregatorParams(vad_analyzer=SileroVADAnalyzer())
    )
    transcript.write_user_turns(aggregators.user())
    answer = transcript.AnswerTranscript()
    return Pipeline(
        [
            transport.input(),
            FunctionFilter(listener.accepts, filter_system_frames=True),
            stt,
            aggregators.user(),
            llm,
            tts,
            answer.synthesized,
            transport.output(),
            answer.spoken,
            aggregators.assistant(),
        ]
    )


async def talk_in_session(gemini: GeminiServices, room: BotRoom, session: moq_wasm.Session) -> None:
    # The namespace examples/browser/examples/moq-ai-voice-agent/main.ts uses.
    namespace = [room.namespace_root, "moq-ai-voice-agent", room.room_id]
    transport = MoqWasmTransport(
        session,
        MoqWasmParams(
            room=namespace,
            participant_id=PARTICIPANT_ID,
            audio_in_enabled=True,
            audio_out_enabled=True,
            event_timelines_out={transcript.TRACK_NAME: TRANSCRIPT_RETENTION_SECONDS},
            empty_timeout=GRACE_SECONDS,
        ),
    )
    listener = Listener()
    transport.add_event_handler("on_client_connected", lambda _, participant_id: listener.joined(participant_id))
    transport.add_event_handler("on_client_disconnected", lambda _, participant_id: listener.left(participant_id))
    worker = PipelineWorker(voice_pipeline(gemini, transport, listener), enable_rtvi=False, idle_timeout_secs=None)
    # The room server owns SIGINT and SIGTERM; it stops every room before it exits.
    runner = WorkerRunner(handle_sigint=False)

    @transport.event_handler("on_room_empty")
    async def on_room_empty(_):
        await runner.cancel()

    await runner.add_workers(worker)
    await runner.run()


async def talk_in_room(args: argparse.Namespace, gemini: GeminiServices, room: BotRoom) -> None:
    await stay_connected(args, room, partial(talk_in_session, gemini, room))


async def main():
    args = parse_args()
    if args.vertex_project:
        gemini = GeminiServices(LANGUAGE, vertex=VertexProject(args.vertex_project))
    elif api_key := os.environ.get("GOOGLE_API_KEY"):
        gemini = GeminiServices(LANGUAGE, api_key=api_key)
    else:
        raise SystemExit("set GOOGLE_API_KEY to a Gemini API key, or pass --vertex-project")
    await serve_rooms(args, partial(talk_in_room, args, gemini))


if __name__ == "__main__":
    asyncio.run(main())
