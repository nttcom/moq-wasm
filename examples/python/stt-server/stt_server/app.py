"""MoQT voice pipeline server hosted inside a FastAPI application.

Audio tracks reach this process either as PUBLISH (publisher-initiated) or
after a PUBLISH_NAMESPACE, in which case the server subscribes to the
namespace's catalog and to every audio track it lists. Each audio track is
decoded to PCM16 and fed to one `VoicePipeline` (VAD → STT → LLM → TTS).
Results go back over MoQT: transcripts and replies as JSON objects on
`<namespace>/transcript`, synthesized speech as Opus on `<namespace>/reply`,
to every session that subscribes to those tracks.
"""

import asyncio
import collections
import contextlib
import json
import logging
import os
from typing import Callable

import moqt
from fastapi import FastAPI

from .audio import (
    CATALOG_TRACK_NAME,
    AudioCodec,
    AudioDecoder,
    AudioTrack,
    OpusEncoder,
    audio_tracks_from_catalog,
    parse_audio_object,
)
from .pipeline import build_pipeline, pipeline_summary
from .pipeline.base import PIPELINE_PCM, PipelineEvent, ReplyAudioEvent, TextEvent

MOQT_PORT = int(os.environ.get("MOQT_PORT", "4433"))
MOQT_CERT = os.environ.get("MOQT_CERT", "cert.pem")
MOQT_KEY = os.environ.get("MOQT_KEY", "key.pem")
FALLBACK_AUDIO_TRACK_NAME = os.environ.get("AUDIO_TRACK_NAME", "audio")
TRANSCRIPT_TRACK_NAME = os.environ.get("TRANSCRIPT_TRACK_NAME", "transcript")
REPLY_TRACK_NAME = os.environ.get("REPLY_TRACK_NAME", "reply")
RESULT_TRACK_NAMES = {TRANSCRIPT_TRACK_NAME, REPLY_TRACK_NAME}
FALLBACK_AUDIO_CODEC = AudioCodec(name="opus", sample_rate=48000, channels=1)
EVENT_HISTORY = 200

logging.basicConfig(level=logging.INFO, format="%(levelname)s:     %(message)s")
log = logging.getLogger("voice")

TrackKey = tuple[str, str]


class TrackFanout:
    """Writers of one track name in one namespace, one per subscribed session."""

    def __init__(self) -> None:
        self.writers: list[moqt.TrackWriter] = []

    async def write_group(self, objects: list[bytes]) -> None:
        for writer in list(self.writers):
            try:
                await writer.start_group()
                for payload in objects:
                    await writer.write(payload)
            except RuntimeError as error:
                log.info("dropping subscriber: %s", error)
                self.writers.remove(writer)


class Conversation:
    """Reads one audio track, decodes it and feeds the pipeline; pipeline
    events are published on the namespace's result tracks. The decoder is
    created on the first object, once the codec is known from the object
    itself, the catalog, or the fallback."""

    def __init__(self, track: TrackKey, codec: AudioCodec | None, fanout: "Callable[[str], TrackFanout]") -> None:
        self.track = track
        self.codec = codec
        self.fanout = fanout
        self.pipeline = build_pipeline("_".join(track).replace("/", "_"), self.publish_event)
        self.encoder = OpusEncoder()
        self.objects_received = 0
        self.pcm_bytes = 0
        self.events: collections.deque[dict] = collections.deque(maxlen=EVENT_HISTORY)
        self._decoder: AudioDecoder | None = None

    async def run(self, reader: moqt.TrackReader) -> None:
        await self.pipeline.start()
        try:
            async for obj in reader:
                self.objects_received += 1
                packet = parse_audio_object(obj.payload)
                pcm = self._decoder_for(packet.codec).decode(packet.data)
                if pcm:
                    self.pcm_bytes += len(pcm)
                    await self.pipeline.feed(pcm)
        except RuntimeError as error:
            log.info("track %s closed by the transport: %s", self.track, error)
        finally:
            await self.pipeline.close()
            log.info("track ended %s", self.track)

    async def publish_event(self, event: PipelineEvent) -> None:
        namespace, name = self.track
        match event:
            case TextEvent():
                record = {"type": event.kind, "track": name, "text": event.text, "at": event.at}
                log.info("%s/%s [%s] %s", namespace, name, event.kind, event.text)
                payload = json.dumps(record, ensure_ascii=False).encode()
                await self.fanout(TRANSCRIPT_TRACK_NAME).write_group([payload])
            case ReplyAudioEvent():
                packets = self.encoder.encode(event.speech.pcm, event.speech.pcm_format) + self.encoder.flush()
                record = {"type": "reply_audio", "track": name, "packets": len(packets), "at": event.at}
                await self.fanout(REPLY_TRACK_NAME).write_group(packets)
        self.events.append(record)

    def _decoder_for(self, codec_from_object: AudioCodec | None) -> AudioDecoder:
        if self._decoder is None:
            codec = codec_from_object or self.codec or FALLBACK_AUDIO_CODEC
            log.info("decoding %s as %s", self.track, codec)
            self._decoder = AudioDecoder(codec, PIPELINE_PCM)
        return self._decoder


class VoiceHub:
    def __init__(self) -> None:
        self.sessions = 0
        self.audio_tracks: dict[TrackKey, AudioTrack] = {}
        self.conversations: dict[TrackKey, Conversation] = {}
        self.fanouts: dict[TrackKey, TrackFanout] = {}

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
                    case moqt.SubscribeRequest() if event.name in RESULT_TRACK_NAMES:
                        writer = await event.accept()
                        self.fanouts.setdefault((event.namespace, event.name), TrackFanout()).writers.append(writer)
                        log.info("%s subscriber added for %s", event.name, event.namespace)
                    case moqt.SubscribeRequest():
                        await event.reject(0, f"only {sorted(RESULT_TRACK_NAMES)} can be subscribed")
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
        conversation = Conversation(
            key, codec, lambda name: self.fanouts.setdefault((key[0], name), TrackFanout())
        )
        self.conversations[key] = conversation
        await conversation.run(reader)

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
            "pipeline": pipeline_summary(),
            "sessions": self.sessions,
            "tracks": {
                f"{namespace}/{name}": {
                    "objects_received": conversation.objects_received,
                    "pcm_bytes": conversation.pcm_bytes,
                    "recent_events": len(conversation.events),
                }
                for (namespace, name), conversation in self.conversations.items()
            },
            "subscribers": {f"{ns}/{name}": len(fanout.writers) for (ns, name), fanout in self.fanouts.items()},
        }

    def events(self) -> dict[str, list[dict]]:
        return {
            f"{namespace}/{name}": list(conversation.events)
            for (namespace, name), conversation in self.conversations.items()
        }


hub = VoiceHub()


@contextlib.asynccontextmanager
async def lifespan(_: FastAPI):
    server = moqt.listen(MOQT_PORT, MOQT_CERT, MOQT_KEY)
    log.info("MoQT listening on udp/%d, pipeline %s", MOQT_PORT, pipeline_summary())
    accept_loop = asyncio.ensure_future(hub.serve(server))
    yield
    accept_loop.cancel()


app = FastAPI(title="MoQT voice pipeline server", lifespan=lifespan)


@app.get("/")
def stats() -> dict:
    return hub.stats()


@app.get("/transcripts")
def transcripts() -> dict:
    return hub.events()
