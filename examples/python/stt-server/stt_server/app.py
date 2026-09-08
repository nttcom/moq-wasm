"""MoQT voice pipeline server hosted inside a FastAPI application.

Audio tracks reach this process either as PUBLISH (publisher-initiated) or
after a PUBLISH_NAMESPACE, in which case the server subscribes to the
namespace's catalog and to every audio track it lists. Each audio track is
decoded to PCM16 and fed to one `VoicePipeline` (VAD → STT → LLM → TTS).

Two tracks report back to whoever subscribes to them:

- `<namespace>/pipeline` carries JSON. Group 0 describes the pipeline itself
  (nodes and edges) so a client can draw it; group N carries the events of
  conversation turn N, one object per stage transition.
- `<namespace>/reply` carries the synthesized speech as Opus, one group per
  turn, with the same group id as the turn's events.
"""

import asyncio
import contextlib
import json
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
    encode_opus,
    parse_audio_object,
)
from .pipeline import build_pipeline, pipeline_summary, stage_labels
from .pipeline.base import PipelineEvent, ReplyAudio, TurnEvent

MOQT_PORT = int(os.environ.get("MOQT_PORT", "4433"))
MOQT_CERT = os.environ.get("MOQT_CERT", "cert.pem")
MOQT_KEY = os.environ.get("MOQT_KEY", "key.pem")
FALLBACK_AUDIO_TRACK_NAME = "audio"
PIPELINE_TRACK_NAME = "pipeline"
REPLY_TRACK_NAME = "reply"
RESULT_TRACK_NAMES = {PIPELINE_TRACK_NAME, REPLY_TRACK_NAME}
FALLBACK_AUDIO_CODEC = AudioCodec(name="opus", sample_rate=48000, channels=1)
TOPOLOGY_GROUP_ID = 0

logging.basicConfig(level=logging.INFO, format="%(levelname)s:     %(message)s")
log = logging.getLogger("voice")

TrackKey = tuple[str, str]


@dataclass
class ResultWriter:
    """A subscriber of a result track. Groups are numbered by turn, so the
    open group has to be remembered: reopening the same id would be rejected."""

    writer: moqt.TrackWriter
    open_group: int | None = None

    async def append(self, group_id: int, objects: list[bytes]) -> None:
        if self.open_group != group_id:
            await self.writer.start_group(group_id)
            self.open_group = group_id
        for payload in objects:
            await self.writer.write(payload)


@dataclass
class Subscribers:
    tracks: dict[TrackKey, list[ResultWriter]] = field(default_factory=dict)

    def of(self, key: TrackKey) -> list[ResultWriter]:
        return self.tracks.setdefault(key, [])

    async def write_objects(self, key: TrackKey, group_id: int, objects: list[bytes]) -> None:
        """Appends to `group_id` on every subscriber of the track; a subscriber
        whose session ended is dropped."""
        writers = self.tracks.get(key, [])
        for writer in list(writers):
            try:
                await writer.append(group_id, objects)
            except RuntimeError as error:
                log.info("dropping subscriber of %s: %s", key, error)
                writers.remove(writer)


def pipeline_topology(audio_track_name: str) -> dict:
    """The graph a client draws: the two client endpoints, the two MoQT tracks
    carrying audio in and speech out, and the stages in between."""
    stages = stage_labels()
    nodes = [
        {"id": "mic", "kind": "client", "label": "Microphone"},
        {"id": "audio", "kind": "track", "label": f"{audio_track_name} track"},
        {"id": "vad", "kind": "stage", "label": "VAD", "impl": stages["vad"]},
        {"id": "stt", "kind": "stage", "label": "STT", "impl": stages["stt"]},
        {"id": "llm", "kind": "stage", "label": "LLM", "impl": stages["llm"]},
        {"id": "tts", "kind": "stage", "label": "TTS", "impl": stages["tts"]},
        {"id": "reply", "kind": "track", "label": f"{REPLY_TRACK_NAME} track"},
        {"id": "player", "kind": "client", "label": "Playback"},
    ]
    order = [node["id"] for node in nodes]
    return {
        "type": "topology",
        "nodes": nodes,
        "edges": [[source, target] for source, target in zip(order, order[1:])],
    }


class Conversation:
    """Reads one audio track, decodes it and feeds the pipeline; pipeline
    events are published on the namespace's result tracks. The decoder is
    created on the first object, once the codec is known from the object
    itself, the catalog, or the fallback."""

    def __init__(self, track: TrackKey, codec: AudioCodec | None, subscribers: Subscribers) -> None:
        self.track = track
        self.codec = codec
        self.subscribers = subscribers
        self.pipeline = build_pipeline(self.publish_event)
        self.objects_received = 0
        self.pcm_bytes = 0
        self._decoder: AudioDecoder | None = None

    async def run(self, reader: moqt.TrackReader) -> None:
        try:
            async for obj in reader:
                self.objects_received += 1
                packet = parse_audio_object(obj.payload)
                pcm = self._decoder_for(packet.codec).decode(packet.data)
                if pcm:
                    self.pcm_bytes += len(pcm)
                    await self.pipeline.feed(
                        pcm, {"group_id": obj.group_id, "object_id": obj.object_id}
                    )
        except RuntimeError as error:
            log.info("track %s closed by the transport: %s", self.track, error)
        finally:
            await self.pipeline.close()
            log.info("track ended %s", self.track)

    async def publish_event(self, event: PipelineEvent) -> None:
        namespace, name = self.track
        match event:
            case TurnEvent():
                record = {"type": "turn_event", "track": name} | {
                    key: value
                    for key, value in (
                        ("turn", event.turn),
                        ("stage", event.stage),
                        ("state", event.state),
                        ("elapsed_ms", round(event.elapsed_ms, 1) if event.elapsed_ms else None),
                        ("text", event.text),
                        ("detail", event.detail),
                        ("at", event.at),
                    )
                    if value is not None
                }
                if event.text:
                    log.info(
                        "%s/%s turn %d %s: %s", namespace, name, event.turn, event.stage, event.text
                    )
                payload = json.dumps(record, ensure_ascii=False).encode()
                await self.subscribers.write_objects(
                    (namespace, PIPELINE_TRACK_NAME), event.turn, [payload]
                )
            case ReplyAudio():
                if not self.subscribers.of((namespace, REPLY_TRACK_NAME)):
                    return
                packets = encode_opus(event.speech.pcm, event.speech.sample_rate)
                log.info(
                    "%s/%s turn %d reply audio: %d packets", namespace, name, event.turn, len(packets)
                )
                await self.subscribers.write_objects(
                    (namespace, REPLY_TRACK_NAME), event.turn, packets
                )
                # The reply group stays open until the next turn, so the count
                # is what tells a subscriber the turn's audio is complete.
                announcement = {
                    "type": "reply_audio",
                    "turn": event.turn,
                    "packets": len(packets),
                    "sec": round(len(event.speech.pcm) / (event.speech.sample_rate * 2), 3),
                }
                await self.subscribers.write_objects(
                    (namespace, PIPELINE_TRACK_NAME),
                    event.turn,
                    [json.dumps(announcement).encode()],
                )

    def _decoder_for(self, codec_from_object: AudioCodec | None) -> AudioDecoder:
        if self._decoder is None:
            codec = codec_from_object or self.codec or FALLBACK_AUDIO_CODEC
            log.info("decoding %s as %s", self.track, codec)
            self._decoder = AudioDecoder(codec)
        return self._decoder


class VoiceHub:
    def __init__(self) -> None:
        self.sessions = 0
        self.audio_tracks: dict[TrackKey, AudioTrack] = {}
        self.conversations: dict[TrackKey, Conversation] = {}
        self.subscribers = Subscribers()

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
                        await self.add_result_subscriber(event)
                    case moqt.SubscribeRequest():
                        await event.reject(0, f"only {sorted(RESULT_TRACK_NAMES)} can be subscribed")
                    case moqt.SubscribeNamespaceRequest():
                        await event.reject(0, "this server only consumes tracks")
                    case moqt.Disconnected() | moqt.ProtocolViolation():
                        break
        finally:
            self.sessions -= 1
            log.info("session ended")

    async def add_result_subscriber(self, request: moqt.SubscribeRequest) -> None:
        """Result tracks number their groups by turn, so the writer starts at
        group 0 rather than the default wall-clock group id."""
        writer = ResultWriter(await request.accept(first_group_id=TOPOLOGY_GROUP_ID))
        self.subscribers.of((request.namespace, request.name)).append(writer)
        log.info("%s subscriber added for %s", request.name, request.namespace)
        if request.name == PIPELINE_TRACK_NAME:
            topology = pipeline_topology(self.audio_track_name(request.namespace))
            await writer.append(
                TOPOLOGY_GROUP_ID, [json.dumps(topology, ensure_ascii=False).encode()]
            )

    def audio_track_name(self, namespace: str) -> str:
        names = [name for (space, name) in self.audio_tracks if space == namespace]
        return names[0] if names else FALLBACK_AUDIO_TRACK_NAME

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
        conversation = Conversation(key, codec, self.subscribers)
        self.conversations[key] = conversation
        try:
            await conversation.run(reader)
        finally:
            self.conversations.pop(key, None)

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
                    "turns": conversation.pipeline.turns,
                }
                for (namespace, name), conversation in self.conversations.items()
            },
            "subscribers": {
                f"{ns}/{name}": len(writers)
                for (ns, name), writers in self.subscribers.tracks.items()
            },
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
