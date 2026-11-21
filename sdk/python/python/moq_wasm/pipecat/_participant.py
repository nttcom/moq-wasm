import asyncio
import logging
from collections.abc import Awaitable, Callable
from contextlib import aclosing
from dataclasses import dataclass, field
from typing import Literal, Protocol

from pipecat.frames.frames import UserImageRequestFrame

from moq_wasm._native import Location
from moq_wasm.codecs import DecodedPicture, OpusDecoder, VideoDecoder, follow_pictures
from moq_wasm.media import Catalog, CatalogTrack, RemoteBroadcast
from moq_wasm.media._remote_broadcast import RESUBSCRIBE_DELAY_SECONDS

logger = logging.getLogger("moq_wasm")

# The video source naming a participant's default video track, as pipecat names a user's camera.
CAMERA_SOURCE = "camera"


@dataclass(frozen=True)
class MoqWasmTrack:
    """A participant's track the transport subscribed to, for `on_track_subscribed`."""

    participant_id: str
    name: str
    kind: Literal["audio", "video", "data"]


@dataclass(frozen=True)
class MoqWasmTrackMessage:
    """An object of a participant's data track, the `message` of an `InputTransportMessageFrame`."""

    participant_id: str
    track: str
    payload: bytes
    location: Location


@dataclass(frozen=True)
class ReceiveOptions:
    audio_in: bool
    # Pictures a second each video source passes on, or None for only those answering image requests.
    video_sources: dict[str, float | None]
    video_in_track: Callable[[Catalog], CatalogTrack | None] | None
    data_tracks: list[str] = field(default_factory=list)
    # Whether video sources not in `video_sources` may be asked for later.
    video_in: bool = False

    @property
    def receives_media(self) -> bool:
        return self.audio_in or self.video_in or bool(self.video_sources)

    @property
    def receives_anything(self) -> bool:
        return self.receives_media or bool(self.data_tracks)


@dataclass(frozen=True)
class ReceivedPicture:
    participant_id: str
    video_source: str
    rgb: bytes
    size: tuple[int, int]
    location: Location
    request: UserImageRequestFrame | None


class ParticipantSink(Protocol):
    """Where a participant's decoded media, data objects and the events about them go."""

    def audio_in_sample_rate(self) -> int: ...
    async def push_audio(self, pcm: bytes, sample_rate: int, participant_id: str) -> None: ...
    async def push_picture(self, picture: ReceivedPicture) -> None: ...
    async def push_message(self, message: MoqWasmTrackMessage) -> None: ...
    async def answer_image_requests(self, requests: list[UserImageRequestFrame], error: str) -> None: ...
    async def event(self, name: str, *args: object) -> None: ...


class ParticipantReceiver:
    """Receives one participant's broadcast into a sink: the audio and video tracks picked from each
    catalog, the data tracks the options name, and the pictures answering image requests."""

    def __init__(self, remote: RemoteBroadcast, options: ReceiveOptions, sink: ParticipantSink):
        self.participant_id = remote.namespace[-1]
        self._remote = remote
        self._options = options
        self._sink = sink
        self._sources = dict(options.video_sources)
        self._samplers: dict[str, _Sampler] = {}
        self._receiving: dict[str, tuple[str, asyncio.Task]] = {}
        self._catalog: Catalog | None = None
        self._picking = asyncio.Lock()
        self._closed = False

    async def run(self) -> None:
        """Receives until cancelled, as `watch_broadcasts` does when the participant leaves, then
        answers the image requests still waiting with an error. Reads the catalog again a second after
        it ends or its SUBSCRIBE fails, as when the publisher has not written it yet."""
        data = [asyncio.create_task(self._receive_data(track)) for track in self._options.data_tracks]
        try:
            # When only data tracks are read, the participant may publish no catalog, so none is asked for.
            if not self._options.receives_media:
                await asyncio.Event().wait()
            while True:
                try:
                    async with aclosing(self._remote.catalogs()) as catalogs:
                        async for catalog in catalogs:
                            self._catalog = catalog
                            await self._pick_tracks(catalog)
                    logger.info("catalog of %s ended", self._remote.namespace)
                except RuntimeError as error:
                    logger.info("catalog of %s ended: %s", self._remote.namespace, error)
                await asyncio.sleep(RESUBSCRIBE_DELAY_SECONDS)
        finally:
            self._closed = True
            for task in data:
                task.cancel()
            for _, task in self._receiving.values():
                task.cancel()
            requests = [request for sampler in self._samplers.values() for request in sampler.take_requests()]
            await self._sink.answer_image_requests(requests, self._left())

    async def request_image(self, request: UserImageRequestFrame) -> None:
        """Answers `request` with the next picture of its video source; a source not yet received
        starts being received, for image requests only."""
        if self._closed:
            await self._sink.answer_image_requests([request], self._left())
            return
        source = request.video_source or CAMERA_SOURCE
        self._sampler(source).add_request(request)
        if source not in self._sources:
            self._sources[source] = None
            if self._catalog is not None:
                await self._pick_tracks(self._catalog)

    async def capture(self, source: str, framerate: float | None) -> None:
        """Receives `source` at `framerate` pictures a second, or for image requests only when it is
        None; for a source already received, changes its framerate."""
        self._sources[source] = framerate
        self._sampler(source).set_framerate(framerate)
        if self._catalog is not None:
            await self._pick_tracks(self._catalog)

    async def release(self, source: str) -> None:
        """Stops receiving `source` and answers the image requests waiting on it with an error."""
        async with self._picking:
            if source not in self._sources and source not in self._samplers:
                return
            self._sources.pop(source, None)
            current = self._receiving.pop(f"video:{source}", None)
            if current is not None:
                current[1].cancel()
                # A request the track was answering goes back to the sampler as the task ends.
                await asyncio.gather(current[1], return_exceptions=True)
            sampler = self._samplers.pop(source, None)
        if sampler is not None:
            await self._sink.answer_image_requests(sampler.take_requests(), f"{source} of {self.participant_id} released")

    def _left(self) -> str:
        return f"{self.participant_id} left"

    async def _pick_tracks(self, catalog: Catalog) -> None:
        # A request for a new source picks its track from the catalog it knows while run() may pick
        # from a newer one; one at a time, the newer catalog's picks are not undone.
        async with self._picking:
            if catalog is not self._catalog:
                return
            if self._options.audio_in:
                audio = catalog.select(role="audio", packaging="loc", codec="opus", prefer="lowest_bitrate")
                await self._receive("audio", "audio", audio, self._receive_audio)
            for source in list(self._sources):
                track = self._video_track(catalog, source)
                await self._receive(
                    f"video:{source}", "video", track, lambda track, source=source: self._receive_video(source, track)
                )

    def _video_track(self, catalog: Catalog, source: str) -> CatalogTrack | None:
        if source != CAMERA_SOURCE:
            return next((track for track in catalog.tracks if track.name == source and track.role == "video"), None)
        if self._options.video_in_track is not None:
            return self._options.video_in_track(catalog)
        decodable = Catalog([track for track in catalog.tracks if _decodable(track)])
        return decodable.select(role="video", packaging="loc", prefer="lowest_bitrate")

    async def _receive(
        self,
        key: str,
        kind: Literal["audio", "video"],
        track: CatalogTrack | None,
        follow: Callable[[CatalogTrack], Awaitable[None]],
    ) -> None:
        current = self._receiving.get(key)
        if current is not None and track is not None and current[0] == track.name:
            return
        if current is not None:
            current[1].cancel()
            del self._receiving[key]
        if track is not None:
            source = key.removeprefix("video:") if kind == "video" else None
            receiving = asyncio.create_task(self._report_failure(track.name, lambda: follow(track), source))
            self._receiving[key] = (track.name, receiving)
            await self._sink.event("on_track_subscribed", MoqWasmTrack(self.participant_id, track.name, kind))

    async def _report_failure(
        self, track_name: str, receive: Callable[[], Awaitable[None]], video_source: str | None = None
    ) -> None:
        """Ends the participant's track on a failure, such as a frame PyAV cannot decode, hands it to
        `on_error` and answers the image requests waiting on its video source with an error. An audio
        or video track starts afresh when the next catalog names another track; a data track stays
        ended."""
        try:
            await receive()
        except Exception as error:
            logger.exception("receiving %s of %s failed", track_name, self.participant_id)
            message = f"receiving {track_name} of {self.participant_id} failed"
            if video_source is not None:
                await self._sink.answer_image_requests(self._sampler(video_source).take_requests(), message)
            await self._sink.event("on_error", message, error)

    async def _receive_audio(self, track: CatalogTrack) -> None:
        sample_rate = self._sink.audio_in_sample_rate()
        decoder = OpusDecoder(sample_rate)
        async with aclosing(self._remote.follow(track.name)) as objects:
            async for audio_object in objects:
                pcm = decoder.decode(audio_object.payload)
                if pcm:
                    await self._sink.push_audio(pcm, sample_rate, self.participant_id)

    async def _receive_video(self, source: str, track: CatalogTrack) -> None:
        """Decodes each subscription to the track afresh, as of a publisher that took over and
        starts its groups at 0 again."""
        sampler = self._sampler(source)
        async with aclosing(follow_pictures(self._remote, track.name, track.codec or "")) as pictures:
            async for decoded in pictures:
                await self._pass_picture(source, sampler, decoded)

    async def _pass_picture(self, source: str, sampler: "_Sampler", decoded: DecodedPicture) -> None:
        due, request = sampler.sample(decoded.decoded_at)
        if not due:
            return
        picture = decoded.picture
        try:
            rgb = await asyncio.to_thread(lambda: picture.to_ndarray(format="rgb24").tobytes())
            size = (picture.width, picture.height)
            await self._sink.push_picture(
                ReceivedPicture(self.participant_id, source, rgb, size, decoded.location, request)
            )
        except BaseException:
            # The track is replaced or fails before the picture answered the request; the next
            # picture of the source answers it instead.
            if request is not None:
                sampler.return_request(request)
            raise

    async def _receive_data(self, track_name: str) -> None:
        await self._sink.event("on_track_subscribed", MoqWasmTrack(self.participant_id, track_name, "data"))
        await self._report_failure(track_name, lambda: self._forward_data(track_name))

    async def _forward_data(self, track_name: str) -> None:
        async with aclosing(self._remote.follow(track_name, delivery_timeout=None)) as objects:
            async for data_object in objects:
                location = Location(data_object.group_id, data_object.object_id)
                await self._sink.push_message(
                    MoqWasmTrackMessage(self.participant_id, track_name, data_object.payload, location)
                )

    def _sampler(self, source: str) -> "_Sampler":
        if source not in self._samplers:
            self._samplers[source] = _Sampler(self._sources.get(source))
        return self._samplers[source]


class _Sampler:
    """Decides which pictures of a video source are passed on: one every 1 / `framerate` seconds,
    none when `framerate` is None, and the next picture after each image request either way."""

    def __init__(self, framerate: float | None):
        self.set_framerate(framerate)
        self._next_at = 0.0
        self._requests: list[UserImageRequestFrame] = []

    def set_framerate(self, framerate: float | None) -> None:
        self._interval = None if framerate is None else 1 / framerate

    def add_request(self, request: UserImageRequestFrame) -> None:
        self._requests.append(request)

    def return_request(self, request: UserImageRequestFrame) -> None:
        self._requests.insert(0, request)

    def take_requests(self) -> list[UserImageRequestFrame]:
        requests, self._requests = self._requests, []
        return requests

    def sample(self, now: float) -> tuple[bool, UserImageRequestFrame | None]:
        request = self._requests.pop(0) if self._requests else None
        if request is None and (self._interval is None or now < self._next_at):
            return False, None
        if self._interval is not None:
            self._next_at = now + self._interval
        return True, request


def _decodable(track: CatalogTrack) -> bool:
    try:
        VideoDecoder(track.codec or "")
        return True
    except ValueError:
        return False
