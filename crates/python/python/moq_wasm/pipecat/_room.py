import asyncio
import logging
import time
from dataclasses import dataclass

from moq_wasm._session import Session
from moq_wasm.codecs import OPUS_SAMPLE_RATE, OpusEncoder, VideoEncoder
from moq_wasm.media import LocalBroadcast, MediaSource, RemoteBroadcast, create_broadcast, watch_broadcasts
from moq_wasm.pipecat._participant import ParticipantSink, ReceiveOptions, receive_participant

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


class Room:
    """What `MoqWasmTransport` does on its session: publishes its own broadcast `[*room,
    participant_id]` and follows every other broadcast one level under `room` as a participant."""

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
        self._broadcast = create_broadcast(self._session, [*options.room, options.participant_id])
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
        try:
            await self._broadcast.published()
        except RuntimeError as error:
            await self._sink.event("on_error", f"cannot publish {self._broadcast.namespace}", error)
            raise
        if options.receive.audio_in or options.receive.video_in:
            self._tasks.append(
                asyncio.create_task(
                    watch_broadcasts(self._session, options.room, self._follow, filter=self._is_participant)
                )
            )

    async def _close(self) -> None:
        for task in self._tasks:
            task.cancel()
        await asyncio.gather(*self._tasks, return_exceptions=True)
        self._tasks.clear()
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
        await self._sink.event("on_client_connected", participant_id)
        try:
            await receive_participant(remote, self._options.receive, self._sink)
        finally:
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
