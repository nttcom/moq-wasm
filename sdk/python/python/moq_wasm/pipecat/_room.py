import asyncio
import logging
import time
from dataclasses import dataclass, field

from pipecat.frames.frames import UserImageRequestFrame

from moq_wasm._errors import SessionEnded
from moq_wasm._once import wait_through_cancellation
from moq_wasm._session import NamespaceWatch, Session, TrackWriter
from moq_wasm.codecs import OPUS_SAMPLE_RATE, OpusEncoder, VideoEncoder
from moq_wasm.media import (
    EventRecord,
    EventTimeline,
    LocalBroadcast,
    MediaSource,
    RemoteBroadcast,
    create_broadcast,
)
from moq_wasm.media._remote_broadcast import follow_watch
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
        # What the current opening opened, once it did; None while the room is stopped.
        self._opened: _OpenedRoom | None = None
        self._audio_clock = _MediaClock()
        self._video_clock = _MediaClock()
        self._users: set[object] = set()
        # Opens the room once for both processors; None while the room is stopped.
        self._opening: asyncio.Task[_OpenedRoom] | None = None
        # The teardown of the last opening, which the next opening waits for.
        self._closing: asyncio.Task[None] | None = None
        self._receivers: dict[str, ParticipantReceiver] = {}
        # Whether the participants' audio is received, from `start_audio` until the room closes.
        self._audio_streaming = False

    async def start(self, user: object) -> None:
        """Called by the input and the output processors; the first one starts the room, once the
        teardown of its previous opening finished, and each returns once the room is published and
        follows its participants. Raises RuntimeError when the relay refuses the broadcast or the
        SUBSCRIBE_NAMESPACE, leaving the room stopped once it withdrew what it opened. When the last
        caller waiting for the room is cancelled, the room stops opening, or closes if it just
        opened; that caller returns once the teardown finished, even when the opening failed and
        withdraws what it opened, which a later stop and the next opening wait for too."""
        self._users.add(user)
        if self._opening is None:
            self._opening = asyncio.create_task(self._start(self._closing))
        opening = self._opening
        try:
            await asyncio.shield(opening)
        except BaseException:
            self._users.discard(user)
            # Its `stop` finds no user to stop, so the room would stay open with nobody using it.
            if not self._users and self._opening is opening:
                self._end()
            await self._torn_down()
            raise

    async def _start(self, closing: asyncio.Task[None] | None) -> "_OpenedRoom":
        if closing is not None:
            # Unlike `gather`, `wait` leaves the teardown running when this opening is cancelled.
            await asyncio.wait([closing])
        # This opening's failure, which its rollback's failure links to, keeps this frame's locals:
        # the teardown before, failed too, would keep its own opening's encoders in turn.
        del closing
        opened = _OpenedRoom()
        try:
            await self._open(opened)
            if self._opening is asyncio.current_task():
                self._opened = opened
            await self._sink.event("on_connected")
        except BaseException:
            rollback = asyncio.create_task(opened.close())
            if self._opening is asyncio.current_task():
                # The rollback is the room's teardown, which a stop and the next opening wait for.
                self._closing = rollback
                self._opening = None
                self._audio_streaming = False
            if self._opened is opened:
                self._opened = None
            await wait_through_cancellation([rollback])
            raise
        return opened

    async def stop(self, user: object) -> None:
        """Called by the input and the output processors, each as many times as pipecat stops,
        cancels or cleans it up; once neither uses the room, it ends. Every stop made while no
        processor uses the room, also a later one of a processor that stopped it already, returns
        once the teardowns of its openings finished: pipecat's cleanup cancels a stop in progress,
        and the cleanup's stop waits for the teardown that one started. A stop cancelled meanwhile
        leaves the teardown running, and the next `start` waits for it."""
        if user in self._users:
            self._users.discard(user)
            if not self._users:
                self._end()
        await self._torn_down()

    def _end(self) -> None:
        """Starts the teardown of the current opening, which no processor uses any more."""
        opening, self._opening = self._opening, None
        self._opened = None
        self._audio_streaming = False
        if opening is not None:
            self._closing = asyncio.create_task(self._close(opening, self._closing))

    async def _torn_down(self) -> None:
        """Returns once the room's teardown finished, while no processor uses it; cancelling the
        caller leaves the teardown running."""
        closing = self._closing
        if not self._users and closing is not None:
            await asyncio.shield(closing)

    async def _close(self, opening: asyncio.Task["_OpenedRoom"], previous: asyncio.Task[None] | None) -> None:
        """Closes `opening`, then waits for the `previous` teardown, which an opening cancelled
        while it waited for it leaves running, so the next opening waits for both."""
        opening.cancel()
        await asyncio.gather(opening, return_exceptions=True)
        if previous is not None:
            await asyncio.wait([previous])
        # A cancelled or failed opening closed what it opened; one that had finished did not.
        opened = None if opening.cancelled() or opening.exception() is not None else opening.result()
        # A failed teardown's traceback, which the room keeps until the next teardown, keeps these
        # locals: the teardown before, failed too, would keep its own opening's encoders in turn.
        del opening, previous
        if opened is None:
            return
        await opened.close()
        await self._sink.event("on_disconnected")

    async def _open(self, opened: "_OpenedRoom") -> None:
        options = self._options
        broadcast = create_broadcast(self._session, options.broadcast)
        if options.audio_out or options.video_out:
            opened.media = media = broadcast.add_media_source(label=options.participant_id)
            if options.audio_out:
                opened.opus = OpusEncoder()
                media.configure_audio("opus", OPUS_SAMPLE_RATE, 1)
            if options.video_out:
                opened.video = video = VideoEncoder(
                    options.video_out_codec, *options.video_out_size, options.video_out_framerate
                )
                media.configure_video(video.codec_string)
                opened.tasks.append(asyncio.create_task(self._answer_keyframe_requests(media, video)))
        # Set once entered: `close` leaves only a broadcast that was entered, so a configuration
        # error, such as an unsupported video codec, is what `start` raises.
        opened.broadcast = entered = await broadcast.__aenter__()
        for name, retention in options.data_out_tracks.items():
            opened.data_writers[name] = entered.publish_track(name, retention_seconds=retention)
        for name, retention in options.event_timelines_out.items():
            opened.timelines[name] = entered.event_timeline(name, retention_seconds=retention)
        try:
            await entered.published()
        except RuntimeError as error:
            await self._sink.event("on_error", f"cannot publish {entered.namespace}", error)
            raise
        # Followed even when nothing is received, for the participants' events and `on_room_empty`.
        try:
            watch = await self._session.watch_namespaces(options.room)
        except RuntimeError as error:
            await self._sink.event("on_error", f"cannot follow the room {options.room}", error)
            raise
        opened.tasks.append(asyncio.create_task(self._follow_participants(watch)))

    async def start_audio(self) -> None:
        """Receives the audio of the participants in the room and of those joining later, when the
        options receive audio. Until then, and again once the room closed, no audio track is
        subscribed to."""
        self._audio_streaming = True
        for receiver in list(self._receivers.values()):
            await receiver.start_audio()

    async def write_audio(self, pcm: bytes, sample_rate: int) -> None:
        """Encodes `pcm` and waits while the speech written so far runs more than
        `MAX_AUDIO_AHEAD_MICROS` ahead of the clock, as pipecat's `MOQTransport` paces its own."""
        opened = self._opened
        if opened is None or opened.opus is None or opened.media is None:
            return
        for packet in opened.opus.encode(pcm, sample_rate):
            opened.media.push_audio(packet, self._audio_clock.next(OPUS_FRAME_MICROS))
        ahead = self._audio_clock.ahead_micros()
        if ahead > MAX_AUDIO_AHEAD_MICROS:
            await asyncio.sleep((ahead - MAX_AUDIO_AHEAD_MICROS) / 1_000_000)

    async def write_picture(self, image: bytes, size: tuple[int, int], color_format: str) -> None:
        opened = self._opened
        if opened is None or opened.video is None or opened.media is None:
            return
        packets = await asyncio.to_thread(opened.video.encode, image, size, color_format)
        for packet in packets:
            opened.media.push_video(packet, self._video_clock.next(1))

    async def write_message(self, message: object) -> bool:
        """Writes a `MoqWasmTrackObject` or a `MoqWasmEventRecord`; returns False for any other
        message, or one naming a track the transport does not publish."""
        opened = self._opened
        if opened is None:
            return False
        if isinstance(message, MoqWasmTrackObject) and message.track in opened.data_writers:
            await opened.data_writers[message.track].write(message.payload, new_group=message.new_group)
            return True
        if isinstance(message, MoqWasmEventRecord) and message.track in opened.timelines:
            await opened.timelines[message.track].append(message.record)
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

    async def _follow_participants(self, watch: NamespaceWatch) -> None:
        try:
            await follow_watch(
                self._session, watch, self._follow, filter=self._is_participant, empty_timeout=self._options.empty_timeout
            )
        except SessionEnded:
            # `Session.run` reports it.
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
        receiver = ParticipantReceiver(remote, self._options.receive, self._sink, stream_audio=self._audio_streaming)
        self._receivers[participant_id] = receiver
        await self._sink.event("on_client_connected", participant_id)
        try:
            await receiver.run()
        finally:
            if self._receivers.get(participant_id) is receiver:
                del self._receivers[participant_id]
            await self._sink.event("on_client_disconnected", participant_id)


class _OpenedRoom:
    """What one opening of the room opened: its broadcast, its encoders and the tasks following the
    room. Each opening closes only its own, so a teardown still running leaves the next opening's
    alone."""

    def __init__(self):
        self.broadcast: LocalBroadcast | None = None
        self.media: MediaSource | None = None
        self.opus: OpusEncoder | None = None
        self.video: VideoEncoder | None = None
        self.tasks: list[asyncio.Task] = []
        self.data_writers: dict[str, TrackWriter] = {}
        self.timelines: dict[str, EventTimeline] = {}

    async def close(self) -> None:
        for task in self.tasks:
            task.cancel()
        await asyncio.gather(*self.tasks, return_exceptions=True)
        self.tasks.clear()
        self.data_writers.clear()
        self.timelines.clear()
        broadcast, self.broadcast = self.broadcast, None
        if broadcast is not None:
            await broadcast.__aexit__(None, None, None)


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
