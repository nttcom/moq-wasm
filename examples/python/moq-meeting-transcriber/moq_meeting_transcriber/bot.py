import argparse
import asyncio
import re
from functools import partial

from faster_whisper import WhisperModel
from loguru import logger
from moq_bot_rooms.server import add_serve_arguments, serve_rooms

from moq_meeting_transcriber.room import Room
from moq_meeting_transcriber.stt import SharedWhisper

DEFAULT_RELAY_URL = "https://127.0.0.1:4433"
DEFAULT_PORT = 8100
# Whisper runs on the CPU and every room shares it.
DEFAULT_MAX_ROOMS = 2
DEFAULT_MODEL = "small"
# A Meeting room name is any single namespace element.
ROOM_ID_PATTERN = re.compile(r"[^/]{1,64}")


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description="Transcribe every participant of a MoQ Meeting room with Whisper")
    add_serve_arguments(parser, default_port=DEFAULT_PORT, default_max_rooms=DEFAULT_MAX_ROOMS)
    parser.add_argument("--relay-url", default=DEFAULT_RELAY_URL)
    parser.add_argument(
        "--insecure",
        action="store_true",
        help="skip TLS verification, for a local relay with a self-signed certificate",
    )
    parser.add_argument("--model", default=DEFAULT_MODEL, help="a faster-whisper model, such as small or large-v3")
    return parser.parse_args()


async def transcribe_room(args: argparse.Namespace, whisper: SharedWhisper, room_id: str):
    await Room(args.relay_url, not args.insecure, whisper, room_id).run()


async def main():
    args = parse_args()
    logger.info(f"loading Whisper {args.model}")
    model = await asyncio.to_thread(WhisperModel, args.model, device="cpu", compute_type="int8")
    whisper = SharedWhisper(model, args.model)
    await serve_rooms(args, partial(transcribe_room, args, whisper), room_id_pattern=ROOM_ID_PATTERN)


if __name__ == "__main__":
    asyncio.run(main())
