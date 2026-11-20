import asyncio
import logging
import time
from dataclasses import dataclass, field

from pipecat.frames.frames import UserImageRequestFrame

from moq_wasm._session import Session, TrackWriter
from moq_wasm.codecs import OPUS_SAMPLE_RATE, OpusEncoder, VideoEncoder
from moq_wasm.media import (
    EventRecord,
    EventTimeline,
    LocalBroadcast,
    MediaSource,
    RemoteBroadcast,
    create_broadcast,
    watch_broadcasts,
)
from moq_wasm.pipecat._participant import ParticipantReceiver, ParticipantSink, ReceiveOptions

logger = logging.getLogger("moq_wasm")

OPUS_FRAME_MICROS = 20_000
# How far ahead of the clock the bot's speech may be written. Speech written further ahead would
# reach the browser before an interruption could stop it.
MAX_AUDIO_AHEAD_MICROS = 100_000


@dataclass(frozen=True)
class RoomOptions:
    room: list[str]
    participant_id: str
    receive: ReceiveOptions
    audio_out: bool
    video_out: bool
    video_out_codec: str
    video_out_size: tuple[int, int]
    video_out_framerate: int
    broadcast: list[str]
    data_out_tracks: dict[str, float] = field(default_factory=dict)
    event_timelines_out: dict[str, float] = field(default_factory=dict)
    empty_timeout: float | None = None


@dataclass(frozen=True)
class MoqWasmTrackObject:
    """An object to write on one of the transport's `data_out_tracks`, the `message` of an
    `OutputTransportMessageFrame`."""

    track: str
    payload: bytes
    new_group: bool = True


@dataclass(frozen=True)
class MoqWasmEventRecord:
    """A record to append to one of the transport's `event_timelines_out`, the `message` of an
    `OutputTransportMessageFrame`."""

    track: str
    record: EventRecord


class Room:
    """What `MoqWasmTransport` does on its session: publishes its own broadcast `broadcast` and
    follows every other broadcast one level under `room` as a participant."""

    def __init__(self, session: Session, options: RoomOptions, sink: ParticipantSink):
        self._session = session
        self._options = options
        self._sink = sink
        self._broadcast: LocalBroadcast | None = None
        self._media: MediaSource | None = None
        self._opus: OpusEncoder | None = None
        self._video: VideoEncoder | None = None
        self._audio_clock = _MediaClock()
        self._video_clock = _MediaClock()
        self._tasks: list[asyncio.Task] = []
        self._users: set[object] = set()
        self._receivers: dict[str, ParticipantReceiver] = {}
        self._data_writers: dict[str, TrackWriter] = {}
        self._timelines: dict[str, EventTimeline] = {}

    async def start(self, user: object) -> None:
        """Called by the input and the output processors; the first one starts the room. Raises
        RuntimeError when the relay refuses the broadcast, leaving the room stopped."""
        if self._users:
            self._users.add(user)
            return
        self._users.add(user)
        try:
            await self._open()
        except BaseException:
            self._users.clear()
            await self._close()
            raise
        await self._sink.event("on_connected")

    async def stop(self, user: object) -> None:
        """Called by the input and the output processors, each as many times as pipecat stops or
        cancels it; once neither uses the room, it ends."""
        if user not in self._users:
            return
        self._users.discard(user)
        if self._users:
            return
        await self._close()
        await self._sink.event("on_disconnected")

    async def _open(self) -> None:
        options = self._options
        self._broadcast = create_broadcast(self._session, options.broadcast)
        if options.audio_out or options.video_out:
            self._media = self._broadcast.add_media_source(label=options.participant_id)
        if options.audio_out:
            self._opus = OpusEncoder()
            self._media.configure_audio("opus", OPUS_SAMPLE_RATE, 1)
        if options.video_out:
            self._video = VideoEncoder(options.video_out_codec, *options.video_out_size, options.video_out_framerate)
            self._media.configure_video(self._video.codec_string)
            self._tasks.append(asyncio.create_task(self._answer_keyframe_requests(self._media, self._video)))
        await self._broadcast.__aenter__()
        for name, retention in options.data_out_tracks.items():
            self._data_writers[name] = self._broadcast.publish_track(name, retention_seconds=retention)
        for name, retention in options.event_timelines_out.items():
            self._timelines[name] = self._broadcast.event_timeline(name, retention_seconds=retention)
        try:
            await self._broadcast.published()
        except RuntimeError as error:
            await self._sink.event("on_error", f"cannot publish {self._broadcast.namespace}", error)
            raise
        if options.receive.receives_anything:
            self._tasks.append(asyncio.create_task(self._follow_participants()))

    async def _close(self) -> None:
        for task in self._tasks:
            task.cancel()
        await asyncio.gather(*self._tasks, return_exceptions=True)
        self._tasks.clear()
        self._data_writers.clear()
        self._timelines.clear()
        broadcast, self._broadcast = self._broadcast, None
        if broadcast is not None:
            await broadcast.__aexit__(None, None, None)

    async def write_audio(self, pcm: bytes, sample_rate: int) -> None:
        """Encodes `pcm` and waits while the speech written so far runs more than
        `MAX_AUDIO_AHEAD_MICROS` ahead of the clock, as pipecat's `MOQTransport` paces its own."""
        if self._opus is None or self._media is None:
            return
        for packet in self._opus.encode(pcm, sample_rate):
            self._media.push_audio(packet, self._audio_clock.next(OPUS_FRAME_MICROS))
        ahead = self._audio_clock.ahead_micros()
        if ahead > MAX_AUDIO_AHEAD_MICROS:
            await asyncio.sleep((ahead - MAX_AUDIO_AHEAD_MICROS) / 1_000_000)

    async def write_picture(self, image: bytes, size: tuple[int, int], color_format: str) -> None:
        if self._video is None or self._media is None:
            return
        packets = await asyncio.to_thread(self._video.encode, image, size, color_format)
        for packet in packets:
            self._media.push_video(packet, self._video_clock.next(1))

    async def write_message(self, message: object) -> bool:
        """Writes a `MoqWasmTrackObject` or a `MoqWasmEventRecord`; returns False for any other
        message, or one naming a track the transport does not publish."""
        if isinstance(message, MoqWasmTrackObject) and message.track in self._data_writers:
            await self._data_writers[message.track].write(message.payload, new_group=message.new_group)
            return True
        if isinstance(message, MoqWasmEventRecord) and message.track in self._timelines:
            await self._timelines[message.track].append(message.record)
            return True
        return False

    async def capture(self, participant_id: str, source: str, framerate: float | None) -> None:
        receiver = self._receivers.get(participant_id)
        if receiver is None:
            logger.warning("cannot capture %s of %s: not in the room", source, participant_id)
            return
        await receiver.capture(source, framerate)

    async def release(self, participant_id: str, source: str) -> None:
        receiver = self._receivers.get(participant_id)
        if receiver is not None:
            await receiver.release(source)

    async def request_image(self, request: UserImageRequestFrame) -> None:
        receiver = self._receivers.get(request.user_id)
        if receiver is None:
            await self._sink.answer_image_requests([request], f"no participant {request.user_id}")
            return
        await receiver.request_image(request)

    async def _follow_participants(self) -> None:
        options = self._options
        try:
            await watch_broadcasts(
                self._session, options.room, self._follow, filter=self._is_participant, empty_timeout=options.empty_timeout
            )
        except RuntimeError as error:
            logger.exception("cannot watch %s", options.room)
            await self._sink.event("on_error", f"cannot watch {options.room}", error)
            return
        await self._sink.event("on_room_empty")

    def _is_participant(self, namespace: list[str]) -> bool:
        room = self._options.room
        return len(namespace) == len(room) + 1 and namespace[: len(room)] == room

    async def _answer_keyframe_requests(self, media: MediaSource, encoder: VideoEncoder) -> None:
        try:
            while True:
                await media.keyframe_request()
                encoder.request_keyframe()
        except RuntimeError as error:
            logger.info("keyframe requests ended: %s", error)

    async def _follow(self, remote: RemoteBroadcast) -> None:
        participant_id = remote.namespace[-1]
        receiver = ParticipantReceiver(remote, self._options.receive, self._sink)
        self._receivers[participant_id] = receiver
        await self._sink.event("on_client_connected", participant_id)
        try:
            await receiver.run()
        finally:
            if self._receivers.get(participant_id) is receiver:
                del self._receivers[participant_id]
            await self._sink.event("on_client_disconnected", participant_id)


class _MediaClock:
    """Timestamps in wall-clock microseconds that never run backwards: a frame right after the
    previous one follows it by its duration, even ahead of the clock as when speech is generated
    faster than real time, and one after a pause starts at the clock."""

    def __init__(self):
        self._last: int | None = None

    def next(self, duration_micros: int) -> int:
        now = _now_micros()
        timestamp = now if self._last is None else max(self._last + duration_micros, now)
        self._last = timestamp
        return timestamp

    def ahead_micros(self) -> int:
        return 0 if self._last is None else self._last - _now_micros()


def _now_micros() -> int:
    return time.time_ns() // 1000
