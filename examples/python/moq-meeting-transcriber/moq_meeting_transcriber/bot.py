import argparse
import asyncio
from functools import partial

import aiohttp
import moq_wasm
from faster_whisper import WhisperModel
from loguru import logger
from moq_bot_job.bot_job import BotJob, Bots, add_bot_job_arguments, serve_bot_job
from moq_bot_job.identity_token import add_identity_token_arguments, identity_token_from_arguments

from moq_meeting_transcriber.asr import GRANITE_SPEECH, GraniteSpeechRecognizer, HttpRecognizer
from moq_meeting_transcriber.granite import SharedGranite
from moq_meeting_transcriber.room import Room
from moq_meeting_transcriber.stt import Recognizer, SharedWhisper

DEFAULT_MODEL = "small"
DEFAULT_ASR = "whisper"


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        description="Transcribe every participant of the MoQ Meeting room the bot service launched the bot for"
    )
    parser.set_defaults(bot="moq-meeting-transcriber")
    add_bot_job_arguments(parser)
    parser.add_argument("--model", default=DEFAULT_MODEL, help="the faster-whisper model of `whisper`, such as small")
    parser.add_argument(
        "--asr",
        action="append",
        default=[],
        metavar="NAME=URL",
        help="a speech recognition service with the OpenAI-compatible API that serves the model NAME, "
        f"which a page picks with the option asr=NAME; {GRANITE_SPEECH} is called through the chat API, any other "
        "through the transcription API; repeatable",
    )
    add_identity_token_arguments(parser)
    return parser.parse_args()


def http_services(args: argparse.Namespace) -> dict[str, str]:
    services = {}
    for entry in args.asr:
        name, separator, url = entry.partition("=")
        if not separator or not name or not url:
            raise SystemExit(f"--asr wants NAME=URL, got {entry!r}")
        services[name] = url
    return services


def recognizer_named(args: argparse.Namespace, http: aiohttp.ClientSession, name: str) -> Recognizer:
    """The recognizer the room's option `asr` names. Raises ValueError for one the bot was not started
    with."""
    if name == DEFAULT_ASR:
        return SharedWhisper(asyncio.ensure_future(load_whisper(args.model)))
    granite = SharedGranite()
    if name == granite.name:
        return granite
    services = http_services(args)
    url = services.get(name)
    if url is None:
        offered = sorted({DEFAULT_ASR, granite.name, *services})
        raise ValueError(f"unknown asr {name!r}; this transcriber offers {offered}")
    token = identity_token_from_arguments(args, http, url)
    recognizer = GraniteSpeechRecognizer if name == GRANITE_SPEECH else HttpRecognizer
    return recognizer(name, url, http, token)


async def transcribe_room(args: argparse.Namespace, job: BotJob, bots: Bots) -> None:
    async with aiohttp.ClientSession() as http:
        recognizer = recognizer_named(args, http, job.options.get("asr", DEFAULT_ASR))
        token = await bots.token()
        connect = partial(
            moq_wasm.connect,
            args.relay_url,
            verify_certificate=not args.insecure,
            token=token,
            refresh_token=bots.refresh if token else None,
        )
        await Room(connect, job.namespace_prefix, recognizer, job.grace_seconds).run()


async def load_whisper(name: str) -> WhisperModel:
    logger.info(f"loading Whisper {name}")
    model = await asyncio.to_thread(WhisperModel, name, device="cpu", compute_type="int8")
    logger.info(f"Whisper {name} loaded")
    return model


async def main():
    args = parse_args()
    await serve_bot_job(args, partial(transcribe_room, args))


if __name__ == "__main__":
    asyncio.run(main())
