import asyncio
import base64
import json
import logging

import websockets

from .base import PcmFormat, Transcript, TranscriptCallback

log = logging.getLogger("stt.openai")


class OpenAiRealtimeBackend:
    """OpenAI Realtime API in transcription mode: pcm16 at 24 kHz appended
    to the input buffer, transcripts delivered as delta / completed events."""

    pcm_format = PcmFormat(24000)

    def __init__(self, api_key: str, language: str = "ja", model: str = "gpt-4o-transcribe") -> None:
        self.api_key = api_key
        self.language = language
        self.model = model
        self._socket: websockets.ClientConnection | None = None
        self._receiver: asyncio.Task | None = None

    async def start(self, on_transcript: TranscriptCallback) -> None:
        self._socket = await websockets.connect(
            "wss://api.openai.com/v1/realtime?intent=transcription",
            additional_headers={
                "Authorization": f"Bearer {self.api_key}",
                "OpenAI-Beta": "realtime=v1",
            },
        )
        await self._socket.send(
            json.dumps(
                {
                    "type": "transcription_session.update",
                    "session": {
                        "input_audio_format": "pcm16",
                        "input_audio_transcription": {"model": self.model, "language": self.language},
                        "turn_detection": {"type": "server_vad"},
                    },
                }
            )
        )
        self._receiver = asyncio.ensure_future(self._receive(on_transcript))

    async def send_pcm(self, pcm: bytes) -> None:
        if self._socket is None:
            raise RuntimeError("start() must be called before send_pcm()")
        await self._socket.send(
            json.dumps({"type": "input_audio_buffer.append", "audio": base64.b64encode(pcm).decode()})
        )

    async def close(self) -> None:
        if self._socket is None:
            return
        await self._socket.send(json.dumps({"type": "input_audio_buffer.commit"}))
        if self._receiver is not None:
            self._receiver.cancel()
        await self._socket.close()
        self._socket = None

    async def _receive(self, on_transcript: TranscriptCallback) -> None:
        assert self._socket is not None
        async for message in self._socket:
            event = json.loads(message)
            match event.get("type"):
                case "conversation.item.input_audio_transcription.delta":
                    on_transcript(Transcript(text=event.get("delta", ""), is_final=False))
                case "conversation.item.input_audio_transcription.completed":
                    on_transcript(Transcript(text=event.get("transcript", ""), is_final=True))
                case "error":
                    log.error("openai realtime error: %s", event.get("error"))
