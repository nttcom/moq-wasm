"""MoQT speech-to-text server hosted inside a FastAPI application.

Audio tracks reach this process either as PUBLISH (publisher-initiated) or
after a PUBLISH_NAMESPACE, in which case the server subscribes to the
namespace's catalog and to every audio track it lists. Each audio track is
decoded to PCM16 and streamed into one speech-to-text session.
"""

import asyncio
import collections
import contextlib
import logging
import os
from dataclasses import dataclass, field

import moqt
from fastapi import FastAPI

from .audio import (
    CATALOG_TRACK_NAME,
    AudioCodec,
    AudioDecoder,
    AudioTrack,
    audio_tracks_from_catalog,
    parse_audio_object,
)
from .stt import SpeechToText, Transcript, create_backend

MOQT_PORT = int(os.environ.get("MOQT_PORT", "4433"))
MOQT_CERT = os.environ.get("MOQT_CERT", "cert.pem")
MOQT_KEY = os.environ.get("MOQT_KEY", "key.pem")
FALLBACK_AUDIO_TRACK_NAME = os.environ.get("AUDIO_TRACK_NAME", "audio")
FALLBACK_AUDIO_CODEC = AudioCodec(name="opus", sample_rate=48000, channels=1)
TRANSCRIPT_HISTORY = 200

logging.basicConfig(level=logging.INFO, format="%(levelname)s:     %(message)s")
log = logging.getLogger("stt")

TrackKey = tuple[str, str]


@dataclass
class TranscriptionState:
    objects_received: int = 0
    pcm_bytes: int = 0
    transcripts: collections.deque[Transcript] = field(
        default_factory=lambda: collections.deque(maxlen=TRANSCRIPT_HISTORY)
    )


class Transcription:
    """Reads one audio track and feeds a speech-to-text backend. The decoder
    is created on the first object, once the codec is known from the object
    itself, the catalog, or the fallback."""

    def __init__(self, track: TrackKey, codec: AudioCodec | None, backend: SpeechToText) -> None:
        self.track = track
        self.codec = codec
        self.backend = backend
        self.state = TranscriptionState()
        self._decoder: AudioDecoder | None = None

    async def run(self, reader: moqt.TrackReader) -> None:
        await self.backend.start(self._on_transcript)
        try:
            async for obj in reader:
                self.state.objects_received += 1
                packet = parse_audio_object(obj.payload)
                decoder = self._decoder_for(packet.codec)
                pcm = decoder.decode(packet.data)
                if pcm:
                    self.state.pcm_bytes += len(pcm)
                    await self.backend.send_pcm(pcm)
        except RuntimeError as error:
            log.info("track %s closed by the transport: %s", self.track, error)
        finally:
            await self.backend.close()
            log.info("track ended %s", self.track)

    def _decoder_for(self, codec_from_object: AudioCodec | None) -> AudioDecoder:
        if self._decoder is None:
            codec = codec_from_object or self.codec or FALLBACK_AUDIO_CODEC
            log.info("decoding %s as %s", self.track, codec)
            self._decoder = AudioDecoder(codec, self.backend.pcm_format)
        return self._decoder

    def _on_transcript(self, transcript: Transcript) -> None:
        self.state.transcripts.append(transcript)
        marker = "final" if transcript.is_final else "interim"
        log.info("%s/%s [%s] %s", self.track[0], self.track[1], marker, transcript.text)


class SttHub:
    def __init__(self) -> None:
        self.sessions = 0
        self.audio_tracks: dict[TrackKey, AudioTrack] = {}
        self.transcriptions: dict[TrackKey, Transcription] = {}

    async def serve(self, server: moqt.Server) -> None:
        async for session in server:
            self.sessions += 1
            asyncio.ensure_future(self.handle_session(session))

    async def handle_session(self, session: moqt.Session) -> None:
        try:
            async for event in session:
                match event:
                    case moqt.PublishNamespaceRequest():
                        await event.accept()
                        log.info("publish_namespace accepted %s", event.namespace)
                        asyncio.ensure_future(self.subscribe_namespace(session, event.namespace))
                    case moqt.PublishRequest():
                        key = (event.namespace, event.name)
                        reader = await event.accept()
                        log.info("publish accepted %s", key)
                        asyncio.ensure_future(self.consume_track(key, reader))
                    case moqt.SubscribeRequest():
                        await event.reject(0, "this server only consumes tracks")
                    case moqt.SubscribeNamespaceRequest():
                        await event.reject(0, "this server only consumes tracks")
                    case moqt.Disconnected() | moqt.ProtocolViolation():
                        break
        finally:
            self.sessions -= 1
            log.info("session ended")

    async def subscribe_namespace(self, session: moqt.Session, namespace: str) -> None:
        catalog_reader = await session.subscribe(namespace, CATALOG_TRACK_NAME)
        catalog_object = await catalog_reader.next_object()
        if catalog_object is None:
            return
        for track in self.register_catalog(namespace, catalog_object.payload):
            reader = await session.subscribe(track.namespace, track.name)
            asyncio.ensure_future(self.consume_track((track.namespace, track.name), reader))

    async def consume_track(self, key: TrackKey, reader: moqt.TrackReader) -> None:
        if key[1] == CATALOG_TRACK_NAME:
            async for obj in reader:
                self.register_catalog(key[0], obj.payload)
            return
        if not self.is_audio_track(key):
            log.info("ignoring non-audio track %s", key)
            return
        codec = self.audio_tracks[key].codec if key in self.audio_tracks else None
        transcription = Transcription(key, codec, create_backend(f"{key[0]}_{key[1]}".replace("/", "_")))
        self.transcriptions[key] = transcription
        await transcription.run(reader)

    def register_catalog(self, namespace: str, catalog_json: bytes) -> list[AudioTrack]:
        tracks = audio_tracks_from_catalog(namespace, catalog_json)
        for track in tracks:
            self.audio_tracks[(track.namespace, track.name)] = track
        log.info("catalog for %s lists audio tracks %s", namespace, [track.name for track in tracks])
        return tracks

    def is_audio_track(self, key: TrackKey) -> bool:
        return key in self.audio_tracks or key[1] == FALLBACK_AUDIO_TRACK_NAME

    def stats(self) -> dict:
        return {
            "sessions": self.sessions,
            "tracks": {
                f"{namespace}/{name}": {
                    "objects_received": transcription.state.objects_received,
                    "pcm_bytes": transcription.state.pcm_bytes,
                    "transcripts": len(transcription.state.transcripts),
                }
                for (namespace, name), transcription in self.transcriptions.items()
            },
        }

    def transcripts(self) -> dict[str, list[dict]]:
        return {
            f"{namespace}/{name}": [
                {"text": transcript.text, "final": transcript.is_final, "at": transcript.received_at}
                for transcript in transcription.state.transcripts
            ]
            for (namespace, name), transcription in self.transcriptions.items()
        }


hub = SttHub()


@contextlib.asynccontextmanager
async def lifespan(_: FastAPI):
    server = moqt.listen(MOQT_PORT, MOQT_CERT, MOQT_KEY)
    log.info("MoQT listening on udp/%d, STT backend %s", MOQT_PORT, os.environ.get("STT_BACKEND", "wav"))
    accept_loop = asyncio.ensure_future(hub.serve(server))
    yield
    accept_loop.cancel()


app = FastAPI(title="MoQT speech-to-text server", lifespan=lifespan)


@app.get("/")
def stats() -> dict:
    return hub.stats()


@app.get("/transcripts")
def transcripts() -> dict:
    return hub.transcripts()
