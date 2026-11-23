import asyncio
import logging
from collections.abc import Awaitable, Callable
from contextlib import aclosing
from dataclasses import dataclass, field
from typing import Literal, Protocol

from pipecat.frames.frames import UserImageRequestFrame

from moq_wasm._errors import SessionEnded
from moq_wasm._native import Location, Object
from moq_wasm._once import retrieve, wait_through_cancellation
from moq_wasm.codecs import DecodedPicture, OpusDecoder, VideoDecoder, follow_pictures
from moq_wasm.media import Catalog, CatalogTrack, Following, RemoteBroadcast
from moq_wasm.media._remote_broadcast import DEFAULT_DELIVERY_TIMEOUT_SECONDS, RESUBSCRIBE_DELAY_SECONDS

logger = logging.getLogger("moq_wasm")

# The video source naming a participant's default video track, as pipecat names a user's camera.
CAMERA_SOURCE = "camera"
# Seconds before a failed audio or video track is received again; the delay doubles with each
# failure in a row, up to the maximum.
RETRY_DELAY_SECONDS = 1.0
MAX_RETRY_DELAY_SECONDS = 30.0


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
    # The channels of the PCM the audio is decoded to, mono or stereo.
    audio_in_channels: int = 1

    @property
    def receives_media(self) -> bool:
        return self.audio_in or self.video_in or bool(self.video_sources)


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
    catalog, the data tracks the options name, and the pictures answering image requests. With
    `stream_audio` False, the audio track is neither subscribed to nor decoded until
    `start_audio`."""

    def __init__(
        self, remote: RemoteBroadcast, options: ReceiveOptions, sink: ParticipantSink, *, stream_audio: bool = True
    ):
        self.participant_id = remote.namespace[-1]
        self._remote = remote
        self._options = options
        self._sink = sink
        self._stream_audio = stream_audio
        self._sources = dict(options.video_sources)
        self._samplers: dict[str, _Sampler] = {}
        # What identifies the track each key ("audio", "video:<source>") receives
        # (`_track_identity`), and its task.
        self._receiving: dict[str, tuple[tuple, asyncio.Task]] = {}
        # The cancelled tasks of replaced or released tracks, by key, until they finished
        # unsubscribing.
        self._retiring: dict[asyncio.Task, str] = {}
        self._catalog: Catalog | None = None
        self._picking = asyncio.Lock()
        self._closed = False

    async def run(self) -> None:
        """Receives until cancelled, as `watch_broadcasts` does when the participant leaves, then
        returns once every track it subscribed to is unsubscribed, answering the image requests
        still waiting with an error. Ending otherwise, as once the session ended, it returns the
        same way. The teardown runs in a task of its own, which cancelling this call, as a stop
        does after the session's end started the teardown, does not cut short: this call returns,
        or raises the cancellation, once it finished. Reads the catalog again a second after it
        ends or its SUBSCRIBE fails, as when the publisher has not written it yet."""
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
            tearing_down = asyncio.create_task(self._tear_down(data))
            # Its failure is this call's, and nobody's once this call was cancelled.
            tearing_down.add_done_callback(retrieve)
            await wait_through_cancellation([tearing_down])
            tearing_down.result()

    async def _tear_down(self, data: list[asyncio.Task]) -> None:
        tasks = [*data, *(task for _, task in self._receiving.values())]
        for task in tasks:
            task.cancel()
        # A picture task hands back the request it was answering as it ends. A replaced or released
        # track's task, cancelled already, is not cancelled again, which would cut its UNSUBSCRIBE
        # short.
        await asyncio.gather(*tasks, *self._retiring, return_exceptions=True)
        # A cancelled task's traceback keeps its locals, as its decoder, last picture or last
        # object: a receiver the application still holds once it left keeps none of them.
        self._receiving.clear()
        data.clear()
        tasks.clear()
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

    async def start_audio(self) -> None:
        """Receives the participant's audio from now on, when the options receive audio."""
        if self._stream_audio:
            return
        self._stream_audio = True
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
        return departure_error(self.participant_id)

    async def _pick_tracks(self, catalog: Catalog) -> None:
        # A request for a new source picks its track from the catalog it knows while run() may pick
        # from a newer one; one at a time, the newer catalog's picks are not undone.
        async with self._picking:
            if catalog is not self._catalog:
                return
            if self._options.audio_in and self._stream_audio:
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
        """Receives `track` for `key` with `follow`, replacing the track received for it, unless
        `track` is that one. Does nothing once the participant left, so a track picked during
        `run()`'s teardown, as for a source captured then, starts no task the teardown does not
        wait for."""
        if self._closed:
            return
        current = self._receiving.get(key)
        identity = None if track is None else _track_identity(track)
        if current is not None and current[0] == identity:
            return
        self._stop(key)
        if track is not None:
            source = key.removeprefix("video:") if kind == "video" else None
            receiving = asyncio.create_task(
                self._keep_receiving(track.name, lambda: follow(track), source, self._retiring_tasks(key))
            )
            self._receiving[key] = (_track_identity(track), receiving)

    def _stop(self, key: str) -> None:
        """Cancels the task receiving the track of `key` and keeps it in `_retiring` until it
        finished unsubscribing, so the teardown, a release and the task of a track replacing it wait
        for it."""
        current = self._receiving.pop(key, None)
        if current is None:
            return
        current[1].cancel()
        self._retiring[current[1]] = key
        current[1].add_done_callback(self._retire)

    def _retiring_tasks(self, key: str) -> list[asyncio.Task]:
        """The cancelled tasks of the tracks `key` received that have not finished unsubscribing."""
        return [task for task, retiring_key in self._retiring.items() if retiring_key == key]

    def _retire(self, task: asyncio.Task) -> None:
        self._retiring.pop(task, None)

    async def _keep_receiving(
        self,
        track_name: str,
        receive: Callable[[], Awaitable[None]],
        video_source: str | None,
        predecessors: list[asyncio.Task],
    ) -> None:
        """Receives an audio or video track for as long as it is picked, once the tasks of the
        tracks it replaces finished unsubscribing. After a failure, such as a frame PyAV cannot
        decode, receives it again with a fresh decoder `RETRY_DELAY_SECONDS` later, twice as long
        after each failure in a row, up to `MAX_RETRY_DELAY_SECONDS`; an attempt that lasted that
        long starts the delays over. Ends once the session ended."""
        try:
            if predecessors:
                await asyncio.wait(predecessors)
        finally:
            # A cancelled predecessor's traceback keeps its locals, as its decoder and last picture,
            # and its own predecessors in turn: kept for this track's whole life, they would
            # outlive it.
            predecessors.clear()
        loop = asyncio.get_running_loop()
        delay = RETRY_DELAY_SECONDS
        while True:
            started = loop.time()
            if not await self._report_failure(track_name, receive, video_source):
                return
            if loop.time() - started >= MAX_RETRY_DELAY_SECONDS:
                delay = RETRY_DELAY_SECONDS
            await asyncio.sleep(delay)
            delay = min(delay * 2, MAX_RETRY_DELAY_SECONDS)

    async def _report_failure(
        self, track_name: str, receive: Callable[[], Awaitable[None]], video_source: str | None = None
    ) -> bool:
        """Runs `receive`. On a failure, hands it to `on_error` and answers the image requests
        waiting on `video_source` with an error. Returns whether it failed while the session is
        still open, so the track may be received again."""
        try:
            await receive()
            return False
        except Exception as error:
            logger.exception("receiving %s of %s failed", track_name, self.participant_id)
            message = f"receiving {track_name} of {self.participant_id} failed"
            if video_source is not None:
                await self._sink.answer_image_requests(self._sampler(video_source).take_requests(), message)
            await self._sink.event("on_error", message, error)
            return not isinstance(error, SessionEnded)

    def _follow(
        self,
        name: str,
        namespace: list[str] | None,
        kind: Literal["audio", "video", "data"],
        on_subscribed: Callable[[], None] | None = None,
    ) -> Following[Object]:
        """`RemoteBroadcast.follow`, reporting `on_track_subscribed` once each SUBSCRIBE is
        accepted, after calling `on_subscribed`, with no DELIVERY TIMEOUT for a data track, such as
        chat, whose every object matters. Read it in `aclosing`: a generator left at a `yield`, as when
        its reader is cancelled while it handles an object, unsubscribes only once closed, and a
        replaced track must be unsubscribed by the time its task ends."""
        return self._remote.follow(
            name,
            namespace=namespace,
            on_subscribed=self._subscribed(name, kind, on_subscribed),
            delivery_timeout=None if kind == "data" else DEFAULT_DELIVERY_TIMEOUT_SECONDS,
        )

    def _subscribed(
        self, name: str, kind: Literal["audio", "video", "data"], on_subscribed: Callable[[], None] | None = None
    ) -> Callable[[], Awaitable[None]]:
        async def subscribed() -> None:
            if on_subscribed is not None:
                on_subscribed()
            await self._sink.event("on_track_subscribed", MoqWasmTrack(self.participant_id, name, kind))

        return subscribed

    async def _receive_audio(self, track: CatalogTrack) -> None:
        """Decodes the track's objects in order: one older than the newest decoded, as of a group
        whose stream was delayed, would play after newer audio and is dropped. The decoder starts
        afresh at each subscription and after missing objects, since its state is of audio that
        does not continue there, and at the sink's sample rate whenever it changes, as when the
        pipeline that resumes a stopped input runs at another rate."""
        sample_rate = 0
        decoder: OpusDecoder | None = None
        # The location of the newest object decoded on the current subscription.
        newest: tuple[int, int] | None = None

        def subscribed() -> None:
            nonlocal newest
            newest = None

        async with aclosing(self._follow(track.name, track.namespace, "audio", subscribed)) as objects:
            async for audio_object in objects:
                location = (audio_object.group_id, audio_object.object_id)
                if newest is not None and location <= newest:
                    continue
                following = None if newest is None else ((newest[0], newest[1] + 1), (newest[0] + 1, 0))
                rate = self._sink.audio_in_sample_rate()
                if decoder is None or following is None or location not in following or rate != sample_rate:
                    sample_rate = rate
                    decoder = OpusDecoder(sample_rate, self._options.audio_in_channels)
                newest = location
                pcm = decoder.decode(audio_object.payload)
                if pcm:
                    await self._sink.push_audio(pcm, sample_rate, self.participant_id)

    async def _receive_video(self, source: str, track: CatalogTrack) -> None:
        """Decodes each subscription to the track afresh, as of a publisher that took over and
        starts its groups at 0 again."""
        sampler = self._sampler(source)
        subscribed = self._subscribed(track.name, "video")
        pictures = follow_pictures(
            self._remote, track.name, track.codec or "", namespace=track.namespace, on_subscribed=subscribed
        )
        async with aclosing(pictures):
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
        # A data track has no decoder to start afresh, so a failed one stays ended.
        await self._report_failure(track_name, lambda: self._forward_data(track_name))

    async def _forward_data(self, track_name: str) -> None:
        async with aclosing(self._follow(track_name, None, "data")) as objects:
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


def departure_error(participant_id: str) -> str:
    """The error answering the image requests of a participant who left."""
    return f"{participant_id} left"


def _track_identity(track: CatalogTrack) -> tuple:
    """What a newer catalog's track must share with the one being received for it not to be
    replaced: its namespace, name, packaging and codec."""
    return (track.namespace, track.name, track.packaging, track.codec)


def _decodable(track: CatalogTrack) -> bool:
    try:
        VideoDecoder(track.codec or "")
        return True
    except ValueError:
        return False
