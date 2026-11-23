import asyncio
import logging
import time
from collections.abc import Callable
from dataclasses import dataclass, field

from pipecat.frames.frames import UserImageRequestFrame

from moq_wasm._errors import SessionEnded
from moq_wasm._once import wait_through_cancellation
from moq_wasm._session import NamespaceWatch, Session, TrackWriter
from moq_wasm.codecs import OPUS_BITRATE, OPUS_SAMPLE_RATE, OpusEncoder, VideoEncoder
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
    audio_out_bitrate: int = OPUS_BITRATE
    audio_out_channels: int = 1
    video_out_bitrate: int | None = None
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
        # Bumped by each interruption; a write started before it stops.
        self._audio_generation = 0
        self._video_clock = _MediaClock()
        # Held from the start of a picture's encode until its thread finishes, which a cancelled
        # write does not wait for: the encoder's codec context takes one picture at a time.
        self._encoding = asyncio.Lock()
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
            opened.media = media = broadcast.add_media_source(
                label=options.participant_id,
                video_bitrate=options.video_out_bitrate,
                video_framerate=options.video_out_framerate,
            )
            if options.audio_out:
                opened.opus = self._new_opus_encoder()
                media.configure_audio("opus", OPUS_SAMPLE_RATE, options.audio_out_channels)
            if options.video_out:
                opened.video = video = VideoEncoder(
                    options.video_out_codec,
                    *options.video_out_size,
                    options.video_out_framerate,
                    options.video_out_bitrate,
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

    def _new_opus_encoder(self) -> OpusEncoder:
        return OpusEncoder(self._options.audio_out_bitrate, self._options.audio_out_channels)

    @property
    def media_failed(self) -> bool:
        """Whether publishing the open room's media source failed, which it does for good."""
        return self._opened is not None and self._opened.media_failed

    async def start_audio(self) -> None:
        """Receives the audio of the participants in the room and of those joining later, when the
        options receive audio. Until then, and again once the room closed, no audio track is
        subscribed to."""
        self._audio_streaming = True
        for receiver in list(self._receivers.values()):
            await receiver.start_audio()

    async def write_audio(
        self, pcm: bytes, sample_rate: int, num_channels: int = 1, *, interruptible: bool = True
    ) -> bool:
        """Encodes `pcm` (16-bit, `num_channels` interleaved, mixed to `audio_out_channels`) and
        writes it packet by packet, each once the speech written before it runs at most
        `MAX_AUDIO_AHEAD_MICROS` ahead of the clock, as pipecat's `MOQTransport` paces its own. An
        `interruptible` write stops at the next packet after `interrupt_audio`. Uninterruptible
        speech the encoder holds back is written first, its last packet padded with silence, when
        interruptible speech follows it, so an interruption, which drops what the encoder holds,
        never drops it. Returns whether the room wrote all of the speech: False when it is not open,
        publishes no audio or publishing its media source failed, when an interruption stopped it
        before its last packet, and when the media source dropped a packet or drops what is pushed,
        as once the session ended, which the encoder then does not take, even speech short of a
        packet. Raises what failed publishing the media source, as `_push_media` does."""
        opened = self._opened
        if opened is None or opened.opus is None or not opened.takes_media():
            return False
        generation = self._audio_generation
        packets: list[tuple[bytes, bool]] = []
        if interruptible and opened.opus_holds_interruptible is False:
            finished, opened.opus = opened.opus, self._new_opus_encoder()
            packets = [(packet, False) for packet in finished.finish()]
        opened.opus_holds_interruptible = interruptible
        packets.extend((packet, interruptible) for packet in opened.opus.encode(pcm, sample_rate, num_channels))
        for packet, packet_interruptible in packets:
            ahead = self._audio_clock.ahead_micros()
            if ahead > MAX_AUDIO_AHEAD_MICROS:
                await asyncio.sleep((ahead - MAX_AUDIO_AHEAD_MICROS) / 1_000_000)
            if packet_interruptible and generation != self._audio_generation:
                return False
            if not self._push_media(
                opened, lambda media: media.push_audio(packet, self._audio_clock.next(OPUS_FRAME_MICROS))
            ):
                return False
        return True

    def finish_audio(self) -> None:
        """Writes what the Opus encoder holds back of the speech, its last packet padded with
        silence, as the speech ends; the next write starts a fresh encoder. Raises what failed
        publishing the media source, as `_push_media` does."""
        opened = self._opened
        if opened is None or opened.opus is None or opened.media_failed:
            return
        finished, opened.opus = opened.opus, self._new_opus_encoder()
        opened.opus_holds_interruptible = None
        for packet in finished.finish():
            self._push_media(opened, lambda media: media.push_audio(packet, self._audio_clock.next(OPUS_FRAME_MICROS)))

    def interrupt_audio(self) -> None:
        """Stops the interruptible speech being written and drops what the encoder holds of it; what
        it holds of uninterruptible speech is kept for the next write."""
        self._audio_generation += 1
        opened = self._opened
        if opened is not None and opened.opus is not None and opened.opus_holds_interruptible:
            opened.opus = self._new_opus_encoder()
            opened.opus_holds_interruptible = None

    def speech_end_micros(self, pending_micros: float) -> int | None:
        """The timestamp at which the speech written so far ends once the speech the Opus encoder
        holds back and `pending_micros` more, which has not reached it yet, are written: the
        timestamp of a picture synchronized with the speech before it. The packets of what the
        encoder holds follow the last packet written on the same clock, or start at the clock after
        a pause. None when the room is not open or publishes no audio."""
        opened = self._opened
        if opened is None or opened.opus is None:
            return None
        next_packet = self._audio_clock.following(OPUS_FRAME_MICROS)
        return next_packet + round(opened.opus.held_micros + pending_micros)

    async def write_picture(
        self, image: bytes, size: tuple[int, int], color_format: str, *, timestamp_micros: int | None = None
    ) -> bool:
        """Encodes the picture in a thread and pushes it as the encode finishes, before the next
        picture's encode starts, at `timestamp_micros`, or the clock without one; a timestamp
        before the previous picture's is moved right after it, so the track's timestamps never run
        backwards. A write cancelled before then drops the picture, and the next picture is encoded
        as a keyframe, since a subscriber's decoder misses the dropped one; a write cancelled after
        then has pushed its picture. Raises what failed the encode, as for a picture in a color
        format the encoder does not take, and what failed publishing the media source, as
        `_push_media` does. Returns whether the room wrote the picture: False when it is not open,
        publishes no video or publishing its media source failed, and when the media source dropped
        it, as once the session ended."""
        opened = self._opened
        if opened is None or opened.video is None or not opened.takes_media():
            return False
        video = opened.video
        await self._encoding.acquire()
        dropped = False
        pushed = True
        failure: RuntimeError | None = None

        def finished(encoded: asyncio.Future[list[bytes]]) -> None:
            # Decides once, under the lock, whether the picture is pushed or replaced by a keyframe.
            nonlocal failure, pushed
            try:
                if encoded.cancelled() or encoded.exception() is not None or dropped:
                    if dropped:
                        video.request_keyframe()
                    return
                for packet in encoded.result():
                    timestamp = self._video_clock.next(1, at=timestamp_micros)
                    if not self._push_media(opened, lambda media: media.push_video(packet, timestamp)):
                        pushed = False
                        return
            except RuntimeError as error:
                failure = error
            finally:
                self._encoding.release()

        encoding = asyncio.ensure_future(asyncio.to_thread(video.encode, image, size, color_format))
        encoding.add_done_callback(finished)
        try:
            await asyncio.shield(encoding)
        except asyncio.CancelledError:
            dropped = True
            raise
        # `finished` ran before this resumed: it was added to `encoding` before the shield was.
        if failure is not None:
            raise failure
        return pushed

    @staticmethod
    def _push_media(opened: "_OpenedRoom", push: Callable[[MediaSource], bool]) -> bool:
        """Pushes into the room's media source, unless publishing it failed; returns whether the
        source took the push. A push raises RuntimeError once publishing the source failed, which it
        does for good: the push that found it raises it, and the audio and pictures after it are
        dropped, so it is reported once."""
        if opened.media is None or opened.media_failed:
            return False
        try:
            return push(opened.media)
        except RuntimeError:
            opened.media_failed = True
            raise

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
        """`ParticipantReceiver.request_image` for the participant `request` names. A request for
        a participant not in the room is answered with an error at once, and the call returns once
        its `result_callback` did."""
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
        # Whether the samples `opus` holds back, short of a packet, are of interruptible speech;
        # None while it holds none. Interruptible samples may follow uninterruptible ones only once
        # those were written.
        self.opus_holds_interruptible: bool | None = None
        self.video: VideoEncoder | None = None
        # Whether publishing `media` failed, which it does for good.
        self.media_failed = False
        self.tasks: list[asyncio.Task] = []
        self.data_writers: dict[str, TrackWriter] = {}
        self.timelines: dict[str, EventTimeline] = {}

    def takes_media(self) -> bool:
        """Whether the media source takes what is pushed into it: publishing it did not fail, and it
        does not drop it, as once the session ended. Checked before an encoder takes the media, as
        speech short of a packet pushes nothing that would find the source stopped."""
        return self.media is not None and not self.media_failed and not self.media._drops_pushes()

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
    """Timestamps in microseconds that never run backwards: a frame right after the previous one
    follows it by its duration, even ahead of the clock as when speech is generated faster than
    real time, and one after a pause starts at the clock. The clock is the wall clock when it was
    made, advanced by the monotonic clock, so a wall-clock step changes neither the pacing nor the
    order."""

    def __init__(self):
        self._last: int | None = None

    def next(self, duration_micros: int, *, at: int | None = None) -> int:
        """The next frame's timestamp: `at` when given, moved 1 µs after the previous frame's when
        it is not after it; otherwise `following(duration_micros)`."""
        if at is None:
            timestamp = self.following(duration_micros)
        else:
            timestamp = at if self._last is None else max(at, self._last + 1)
        self._last = timestamp
        return timestamp

    def following(self, duration_micros: int) -> int:
        """The timestamp `next` would give the next frame now, after a previous one of
        `duration_micros`."""
        now = _now_micros()
        return now if self._last is None else max(self._last + duration_micros, now)

    def ahead_micros(self) -> int:
        return 0 if self._last is None else self._last - _now_micros()


_WALL_MINUS_MONOTONIC_NS = time.time_ns() - time.monotonic_ns()


def _now_micros() -> int:
    return (time.monotonic_ns() + _WALL_MINUS_MONOTONIC_NS) // 1000
