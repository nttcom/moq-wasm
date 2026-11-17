import argparse
import asyncio
import re
from collections.abc import Mapping
from functools import partial

import aiohttp
from faster_whisper import WhisperModel
from loguru import logger
from moq_bot_rooms.identity_token import add_identity_token_arguments, identity_token_from_arguments
from moq_bot_rooms.relay import add_relay_arguments
from moq_bot_rooms.rooms import BotRoom
from moq_bot_rooms.server import add_serve_arguments, serve_rooms

from moq_meeting_transcriber.asr import GRANITE_SPEECH, GraniteSpeechRecognizer, HttpRecognizer
from moq_meeting_transcriber.granite import SharedGranite
from moq_meeting_transcriber.room import Room
from moq_meeting_transcriber.stt import Recognizer, SharedWhisper

DEFAULT_PORT = 8100
# Whisper runs on the CPU and every room shares it.
DEFAULT_MAX_ROOMS = 2
DEFAULT_MODEL = "small"
DEFAULT_ASR = "whisper"
# A Meeting room name is any single namespace element.
ROOM_ID_PATTERN = re.compile(r"[^/]{1,64}")


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description="Transcribe every participant of a MoQ Meeting room")
    add_serve_arguments(
        parser, bot="moq-meeting-transcriber", default_port=DEFAULT_PORT, default_max_rooms=DEFAULT_MAX_ROOMS
    )
    add_relay_arguments(parser)
    parser.add_argument("--model", default=DEFAULT_MODEL, help="the faster-whisper model of `whisper`, such as small")
    parser.add_argument(
        "--asr",
        action="append",
        default=[],
        metavar="NAME=URL",
        help="a speech recognition service with the OpenAI-compatible API that serves the model NAME, "
        f"which a page picks with ?asr=NAME; {GRANITE_SPEECH} is called through the chat API, any other through the "
        "transcription API; repeatable",
    )
    add_identity_token_arguments(parser)
    return parser.parse_args()


def http_recognizers(args: argparse.Namespace, session: aiohttp.ClientSession) -> dict[str, Recognizer]:
    recognizers: dict[str, Recognizer] = {}
    for entry in args.asr:
        name, separator, url = entry.partition("=")
        if not separator or not name or not url:
            raise SystemExit(f"--asr wants NAME=URL, got {entry!r}")
        token = identity_token_from_arguments(args, session, url)
        recognizer = GraniteSpeechRecognizer if name == GRANITE_SPEECH else HttpRecognizer
        recognizers[name] = recognizer(name, url, session, token)
    return recognizers


def bot_from_query(args: argparse.Namespace, recognizers: Mapping[str, Recognizer], query: Mapping[str, str]):
    name = query.get("asr", DEFAULT_ASR)
    recognizer = recognizers.get(name)
    if recognizer is None:
        raise ValueError(f"unknown asr {name!r}; this transcriber offers {sorted(recognizers)}")
    return partial(transcribe_room, args, recognizer)


async def transcribe_room(args: argparse.Namespace, recognizer: Recognizer, room: BotRoom):
    await Room(args.relay_url, not args.insecure, recognizer, room).run()


async def load_whisper(name: str) -> WhisperModel:
    logger.info(f"loading Whisper {name}")
    model = await asyncio.to_thread(WhisperModel, name, device="cpu", compute_type="int8")
    logger.info(f"Whisper {name} loaded")
    return model


async def main():
    args = parse_args()
    model = asyncio.ensure_future(load_whisper(args.model))
    async with aiohttp.ClientSession() as session:
        granite = SharedGranite()
        recognizers = {DEFAULT_ASR: SharedWhisper(model), granite.name: granite, **http_recognizers(args, session)}
        await serve_rooms(
            args,
            partial(transcribe_room, args, recognizers[DEFAULT_ASR]),
            room_id_pattern=ROOM_ID_PATTERN,
            bot_from_query=partial(bot_from_query, args, recognizers),
        )


if __name__ == "__main__":
    asyncio.run(main())
