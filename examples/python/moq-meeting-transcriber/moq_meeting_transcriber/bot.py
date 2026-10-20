import argparse
import asyncio
import re
from collections.abc import Mapping
from functools import partial

import aiohttp
from faster_whisper import WhisperModel
from loguru import logger
from moq_bot_rooms.identity_token import IdentityToken, gcloud_identity_token, metadata_identity_token
from moq_bot_rooms.server import add_serve_arguments, serve_rooms

from moq_meeting_transcriber.asr import HttpRecognizer
from moq_meeting_transcriber.room import Room
from moq_meeting_transcriber.stt import Recognizer, SharedWhisper

DEFAULT_RELAY_URL = "https://127.0.0.1:4433"
DEFAULT_PORT = 8100
# Whisper runs on the CPU and every room shares it.
DEFAULT_MAX_ROOMS = 2
DEFAULT_MODEL = "medium"
DEFAULT_ASR = "whisper"
# A Meeting room name is any single namespace element.
ROOM_ID_PATTERN = re.compile(r"[^/]{1,64}")


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description="Transcribe every participant of a MoQ Meeting room")
    add_serve_arguments(parser, default_port=DEFAULT_PORT, default_max_rooms=DEFAULT_MAX_ROOMS)
    parser.add_argument("--relay-url", default=DEFAULT_RELAY_URL)
    parser.add_argument(
        "--insecure",
        action="store_true",
        help="skip TLS verification, for a local relay with a self-signed certificate",
    )
    parser.add_argument("--model", default=DEFAULT_MODEL, help="the faster-whisper model of `whisper`, such as small")
    parser.add_argument(
        "--asr",
        action="append",
        default=[],
        metavar="NAME=URL",
        help="a speech recognition service with the OpenAI-compatible transcription API that serves the model NAME, "
        "which a page picks with ?asr=NAME; repeatable",
    )
    auth = parser.add_mutually_exclusive_group()
    auth.add_argument(
        "--gcloud-auth",
        action="store_true",
        help="send a gcloud identity token to the --asr services, for a Cloud Run service that requires IAM",
    )
    auth.add_argument(
        "--metadata-auth",
        action="store_true",
        help="send the metadata server's identity token to the --asr services, for a bot on Cloud Run or GCE",
    )
    return parser.parse_args()


def http_recognizers(args: argparse.Namespace, session: aiohttp.ClientSession) -> dict[str, Recognizer]:
    recognizers: dict[str, Recognizer] = {}
    for entry in args.asr:
        name, separator, url = entry.partition("=")
        if not separator or not name or not url:
            raise SystemExit(f"--asr wants NAME=URL, got {entry!r}")
        token: IdentityToken | None = (
            gcloud_identity_token()
            if args.gcloud_auth
            else metadata_identity_token(session, url)
            if args.metadata_auth
            else None
        )
        recognizers[name] = HttpRecognizer(name, url, session, token)
    return recognizers


def bot_from_query(args: argparse.Namespace, recognizers: Mapping[str, Recognizer], query: Mapping[str, str]):
    name = query.get("asr", DEFAULT_ASR)
    recognizer = recognizers.get(name)
    if recognizer is None:
        raise ValueError(f"unknown asr {name!r}; this transcriber offers {sorted(recognizers)}")
    return partial(transcribe_room, args, recognizer)


async def transcribe_room(args: argparse.Namespace, recognizer: Recognizer, room_id: str):
    await Room(args.relay_url, not args.insecure, recognizer, room_id).run()


async def main():
    args = parse_args()
    logger.info(f"loading Whisper {args.model}")
    model = await asyncio.to_thread(WhisperModel, args.model, device="cpu", compute_type="int8")
    async with aiohttp.ClientSession() as session:
        recognizers = {DEFAULT_ASR: SharedWhisper(model), **http_recognizers(args, session)}
        await serve_rooms(
            args,
            partial(transcribe_room, args, recognizers[DEFAULT_ASR]),
            room_id_pattern=ROOM_ID_PATTERN,
            bot_from_query=partial(bot_from_query, args, recognizers),
        )


if __name__ == "__main__":
    asyncio.run(main())
