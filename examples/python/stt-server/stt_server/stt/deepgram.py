import asyncio
import json
import logging
from urllib.parse import urlencode

import websockets

from .base import PcmFormat, Transcript, TranscriptCallback

log = logging.getLogger("stt.deepgram")


class DeepgramBackend:
    """Deepgram live streaming API: raw linear16 frames over a websocket,
    JSON `Results` messages back."""

    pcm_format = PcmFormat(16000)

    def __init__(self, api_key: str, language: str = "ja", model: str = "nova-3") -> None:
        self.api_key = api_key
        self.language = language
        self.model = model
        self._socket: websockets.ClientConnection | None = None
        self._receiver: asyncio.Task | None = None

    async def start(self, on_transcript: TranscriptCallback) -> None:
        query = urlencode(
            {
                "encoding": "linear16",
                "sample_rate": self.pcm_format.sample_rate,
                "channels": self.pcm_format.channels,
                "model": self.model,
                "language": self.language,
                "interim_results": "true",
            }
        )
        self._socket = await websockets.connect(
            f"wss://api.deepgram.com/v1/listen?{query}",
            additional_headers={"Authorization": f"Token {self.api_key}"},
        )
        self._receiver = asyncio.ensure_future(self._receive(on_transcript))

    async def send_pcm(self, pcm: bytes) -> None:
        if self._socket is None:
            raise RuntimeError("start() must be called before send_pcm()")
        await self._socket.send(pcm)

    async def close(self) -> None:
        if self._socket is None:
            return
        await self._socket.send(json.dumps({"type": "CloseStream"}))
        if self._receiver is not None:
            await asyncio.wait_for(self._receiver, timeout=10)
        await self._socket.close()
        self._socket = None

    async def _receive(self, on_transcript: TranscriptCallback) -> None:
        assert self._socket is not None
        async for message in self._socket:
            event = json.loads(message)
            if event.get("type") != "Results":
                continue
            text = event["channel"]["alternatives"][0].get("transcript", "")
            if text:
                on_transcript(Transcript(text=text, is_final=bool(event.get("is_final"))))
            if event.get("from_finalize"):
                break
