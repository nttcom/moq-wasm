import asyncio
import logging
import time
from collections.abc import Awaitable, Callable
from dataclasses import dataclass
from typing import Literal, Protocol

from moq_wasm._session import Session
from moq_wasm.media import Catalog, CatalogTrack, LocalBroadcast, MediaSource, RemoteBroadcast, create_broadcast, watch_broadcasts
from moq_wasm.pipecat._codecs import OPUS_SAMPLE_RATE, OpusDecoder, OpusEncoder, VideoDecoder, VideoEncoder

logger = logging.getLogger("moq_wasm")

OPUS_FRAME_MICROS = 20_000
# How far ahead of the clock the bot's speech may be written. Speech written further ahead would
# reach the browser before an interruption could stop it.
MAX_AUDIO_AHEAD_MICROS = 100_000


@dataclass(frozen=True)
class MoqWasmTrack:
    """A participant's track the transport subscribed to, for `on_track_subscribed`."""

    participant_id: str
    name: str
    kind: Literal["audio", "video"]


@dataclass(frozen=True)
class RoomOptions:
    room: list[str]
    participant_id: str
    audio_in: bool
    video_in: bool
    video_in_max_fps: float
    video_in_track: Callable[[Catalog], CatalogTrack | None] | None
    audio_out: bool
    video_out: bool
    video_out_codec: str
    video_out_size: tuple[int, int]
    video_out_framerate: int


class RoomSink(Protocol):
    """Where the room's media and events go: the transport and its input processor."""

    def audio_in_sample_rate(self) -> int: ...
    async def push_audio(self, pcm: bytes, sample_rate: int, participant_id: str) -> None: ...
    async def push_picture(self, rgb: bytes, size: tuple[int, int], participant_id: str) -> None: ...
    async def event(self, name: str, *args: object) -> None: ...


class Room:
    """What `MoqWasmTransport` does on its session: publishes its own broadcast `[*room,
    participant_id]` and follows every other broadcast one level under `room` as a participant."""

    def __init__(self, session: Session, options: RoomOptions, sink: RoomSink):
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
        if options.audio_in or options.video_in:
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
        while True:
            await media.keyframe_request()
            encoder.request_keyframe()

    async def _follow(self, remote: RemoteBroadcast) -> None:
        participant_id = remote.namespace[-1]
        await self._sink.event("on_client_connected", participant_id)
        receiving: dict[str, tuple[str, asyncio.Task]] = {}
        try:
            async for catalog in remote.catalogs():
                if self._options.audio_in:
                    audio = catalog.select(role="audio", packaging="loc", codec="opus", prefer="lowest_bitrate")
                    await self._receive(participant_id, receiving, "audio", audio, lambda track: self._receive_audio(remote, track))
                if self._options.video_in:
                    video = self._video_track(catalog)
                    await self._receive(participant_id, receiving, "video", video, lambda track: self._receive_video(remote, track))
        except RuntimeError as error:
            logger.info("catalog of %s ended: %s", remote.namespace, error)
        finally:
            for _, task in receiving.values():
                task.cancel()
            await self._sink.event("on_client_disconnected", participant_id)

    def _video_track(self, catalog: Catalog) -> CatalogTrack | None:
        if self._options.video_in_track is not None:
            return self._options.video_in_track(catalog)
        decodable = Catalog([track for track in catalog.tracks if _decodable(track)])
        return decodable.select(role="video", packaging="loc", prefer="lowest_bitrate")

    async def _receive(
        self,
        participant_id: str,
        receiving: dict[str, tuple[str, asyncio.Task]],
        kind: Literal["audio", "video"],
        track: CatalogTrack | None,
        receive: Callable[[CatalogTrack], Awaitable[None]],
    ) -> None:
        current = receiving.get(kind)
        if current is not None and track is not None and current[0] == track.name:
            return
        if current is not None:
            current[1].cancel()
            del receiving[kind]
        if track is not None:
            receiving[kind] = (track.name, asyncio.create_task(self._report_failure(participant_id, track, receive)))
            await self._sink.event("on_track_subscribed", MoqWasmTrack(participant_id, track.name, kind))

    async def _report_failure(
        self, participant_id: str, track: CatalogTrack, receive: Callable[[CatalogTrack], Awaitable[None]]
    ) -> None:
        """Ends the participant's track on a failure, such as a frame PyAV cannot decode, and hands
        it to `on_error`; the next catalog naming another track starts afresh."""
        try:
            await receive(track)
        except Exception as error:
            logger.exception("receiving %s of %s failed", track.name, participant_id)
            await self._sink.event("on_error", f"receiving {track.name} of {participant_id} failed", error)

    async def _receive_audio(self, remote: RemoteBroadcast, track: CatalogTrack) -> None:
        participant_id = remote.namespace[-1]
        sample_rate = self._sink.audio_in_sample_rate()
        decoder = OpusDecoder(sample_rate)
        async for audio_object in remote.follow(track.name):
            pcm = decoder.decode(audio_object.payload)
            if pcm:
                await self._sink.push_audio(pcm, sample_rate, participant_id)

    async def _receive_video(self, remote: RemoteBroadcast, track: CatalogTrack) -> None:
        participant_id = remote.namespace[-1]
        interval = 1 / self._options.video_in_max_fps
        next_picture_at = 0.0
        decoder: VideoDecoder | None = None
        group_id: int | None = None
        async for video_object in remote.follow(track.name):
            if video_object.group_id != group_id:
                decoder = VideoDecoder(track.codec or "")
                group_id = video_object.group_id
            picture = await asyncio.to_thread(decoder.decode, video_object.payload)
            now = time.monotonic()
            if picture is None or now < next_picture_at:
                continue
            next_picture_at = now + interval
            rgb = await asyncio.to_thread(lambda: picture.to_ndarray(format="rgb24").tobytes())
            await self._sink.push_picture(rgb, (picture.width, picture.height), participant_id)


def _decodable(track: CatalogTrack) -> bool:
    try:
        VideoDecoder(track.codec or "")
        return True
    except ValueError:
        return False


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
