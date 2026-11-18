import asyncio
import logging
import time
from collections.abc import Awaitable, Callable
from dataclasses import dataclass
from typing import Literal, Protocol

from moq_wasm.codecs import OpusDecoder, VideoDecoder
from moq_wasm.media import Catalog, CatalogTrack, RemoteBroadcast

logger = logging.getLogger("moq_wasm")


@dataclass(frozen=True)
class MoqWasmTrack:
    """A participant's track the transport subscribed to, for `on_track_subscribed`."""

    participant_id: str
    name: str
    kind: Literal["audio", "video"]


@dataclass(frozen=True)
class ReceiveOptions:
    audio_in: bool
    video_in: bool
    video_in_max_fps: float
    video_in_track: Callable[[Catalog], CatalogTrack | None] | None


class ParticipantSink(Protocol):
    """Where a participant's decoded media and the events about it go."""

    def audio_in_sample_rate(self) -> int: ...
    async def push_audio(self, pcm: bytes, sample_rate: int, participant_id: str) -> None: ...
    async def push_picture(self, rgb: bytes, size: tuple[int, int], participant_id: str) -> None: ...
    async def event(self, name: str, *args: object) -> None: ...


async def receive_participant(remote: RemoteBroadcast, options: ReceiveOptions, sink: ParticipantSink) -> None:
    """Pushes the audio and video of the tracks picked from each catalog of `remote` into `sink`
    until the catalog ends."""
    participant_id = remote.namespace[-1]
    receiving: dict[str, tuple[str, asyncio.Task]] = {}

    async def receive(kind: Literal["audio", "video"], track: CatalogTrack | None, follow) -> None:
        current = receiving.get(kind)
        if current is not None and track is not None and current[0] == track.name:
            return
        if current is not None:
            current[1].cancel()
            del receiving[kind]
        if track is not None:
            receiving[kind] = (track.name, asyncio.create_task(_report_failure(participant_id, track, follow, sink)))
            await sink.event("on_track_subscribed", MoqWasmTrack(participant_id, track.name, kind))

    try:
        async for catalog in remote.catalogs():
            if options.audio_in:
                audio = catalog.select(role="audio", packaging="loc", codec="opus", prefer="lowest_bitrate")
                await receive("audio", audio, lambda track: _receive_audio(remote, track, sink))
            if options.video_in:
                video = _video_track(catalog, options)
                await receive("video", video, lambda track: _receive_video(remote, track, options, sink))
    except RuntimeError as error:
        logger.info("catalog of %s ended: %s", remote.namespace, error)
    finally:
        for _, task in receiving.values():
            task.cancel()


def _video_track(catalog: Catalog, options: ReceiveOptions) -> CatalogTrack | None:
    if options.video_in_track is not None:
        return options.video_in_track(catalog)
    decodable = Catalog([track for track in catalog.tracks if _decodable(track)])
    return decodable.select(role="video", packaging="loc", prefer="lowest_bitrate")


async def _report_failure(
    participant_id: str,
    track: CatalogTrack,
    follow: Callable[[CatalogTrack], Awaitable[None]],
    sink: ParticipantSink,
) -> None:
    """Ends the participant's track on a failure, such as a frame PyAV cannot decode, and hands it
    to `on_error`; the next catalog naming another track starts afresh."""
    try:
        await follow(track)
    except Exception as error:
        logger.exception("receiving %s of %s failed", track.name, participant_id)
        await sink.event("on_error", f"receiving {track.name} of {participant_id} failed", error)


async def _receive_audio(remote: RemoteBroadcast, track: CatalogTrack, sink: ParticipantSink) -> None:
    participant_id = remote.namespace[-1]
    sample_rate = sink.audio_in_sample_rate()
    decoder = OpusDecoder(sample_rate)
    async for audio_object in remote.follow(track.name):
        pcm = decoder.decode(audio_object.payload)
        if pcm:
            await sink.push_audio(pcm, sample_rate, participant_id)


async def _receive_video(
    remote: RemoteBroadcast, track: CatalogTrack, options: ReceiveOptions, sink: ParticipantSink
) -> None:
    participant_id = remote.namespace[-1]
    interval = 1 / options.video_in_max_fps
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
        await sink.push_picture(rgb, (picture.width, picture.height), participant_id)


def _decodable(track: CatalogTrack) -> bool:
    try:
        VideoDecoder(track.codec or "")
        return True
    except ValueError:
        return False
