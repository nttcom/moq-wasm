import io
from typing import Protocol

import numpy as np
import soundfile
from aiohttp import web


class Transcriber(Protocol):
    async def transcribe(self, audio: np.ndarray) -> str:
        """The text of `audio`, mono float32 at 16 kHz."""
        ...


SAMPLE_RATE = 16_000
TRANSCRIBER = web.AppKey("transcriber", Transcriber)


def app(transcriber: Transcriber) -> web.Application:
    """The subset of vLLM's API that moq-meeting-transcriber's `HttpRecognizer` calls, taking a 16 kHz
    mono WAV `file`. It serves one Japanese model, so it ignores the request's `model` and `language`."""
    application = web.Application()
    application[TRANSCRIBER] = transcriber
    application.router.add_get("/health", health)
    application.router.add_post("/v1/audio/transcriptions", transcriptions)
    return application


async def health(_: web.Request) -> web.Response:
    return web.Response()


async def transcriptions(request: web.Request) -> web.Response:
    form = await request.post()
    file = form.get("file")
    if not isinstance(file, web.FileField):
        raise web.HTTPBadRequest(text="the form has no audio file")
    try:
        audio, sample_rate = soundfile.read(io.BytesIO(file.file.read()), dtype="float32")
    except soundfile.LibsndfileError as error:
        raise web.HTTPBadRequest(text=f"unreadable audio: {error}") from error
    if sample_rate != SAMPLE_RATE or audio.ndim != 1:
        raise web.HTTPBadRequest(text=f"audio must be {SAMPLE_RATE} Hz mono")
    return web.json_response({"text": await request.app[TRANSCRIBER].transcribe(audio)})
