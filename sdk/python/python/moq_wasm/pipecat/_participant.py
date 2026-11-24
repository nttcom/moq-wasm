import asyncio
import logging
from collections.abc import Awaitable, Callable
from contextlib import aclosing
from contextvars import ContextVar
from dataclasses import dataclass, field
from typing import Literal, Protocol

from pipecat.frames.frames import UserImageRequestFrame

from moq_wasm._errors import SessionEnded
from moq_wasm._native import Location, Object
from moq_wasm._once import in_thread, retrieve, wait_through_cancellation
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
# Seconds an image request waits for a picture before it is answered with an error.
IMAGE_REQUEST_TIMEOUT_SECONDS = 5.0
# Seconds before its scheduled time a picture may come and still be due, at most half an interval,
# as pipecat's own video input sampler allows for the jitter of the incoming stream.
JITTER_TOLERANCE_SECONDS = 0.05

# Whether the current code runs under the delivery of an error answer: in a `result_callback` or a
# task it started.
_IN_ANSWER: ContextVar[bool] = ContextVar("_IN_ANSWER", default=False)


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
    async def push_picture(self, picture: ReceivedPicture) -> bool:
        """Returns whether the picture was passed on: an input that is paused drops it."""
        ...
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
        # For each image request waiting, the task that answers it with an error once it waited too
        # long.
        self._deadlines: set[asyncio.Task] = set()
        # The tasks activating the sources image requests asked for, once the picks, captures and
        # releases before them finished.
        self._activations: set[asyncio.Task] = set()
        # The tasks answering image requests taken out of a sampler with an error, with their
        # source, or None for the teardown's.
        self._answering: dict[asyncio.Task, str | None] = {}
        self._catalog: Catalog | None = None
        self._picking = asyncio.Lock()
        self._closed = False

    async def run(self) -> None:
        """Receives until cancelled, as `watch_broadcasts` does when the participant leaves, then
        returns once every track it subscribed to is unsubscribed, answering the image requests
        still waiting with an error, and every answer under way was delivered. Ending otherwise, as
        once the session ended, it returns the same way. The teardown runs in a task of its own,
        which cancelling this call, as a stop does after the session's end started the teardown,
        does not cut short: this call returns, or raises the cancellation, once it finished. Reads
        the catalog again a second after it ends or its SUBSCRIBE fails, as when the publisher has
        not written it yet."""
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
                            await self._pick_tracks()
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
        tasks = [*data, *(task for _, task in self._receiving.values()), *self._deadlines, *self._activations]
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
        try:
            await self._answer(None, requests, self._left())
        finally:
            # Also the answers a cancelled deadline, release or track was giving.
            if self._answering:
                await asyncio.wait(list(self._answering))

    async def request_image(self, request: UserImageRequestFrame) -> None:
        """Answers `request` with the next picture of its video source, one at or after the Largest
        Location of its subscription: the older pictures, which a subscription replays from the
        current group for the decoder, are older than the request. A source not yet received
        starts being received, for image requests only. A request no picture answered within
        `IMAGE_REQUEST_TIMEOUT_SECONDS`, as when the participant published no catalog, its catalog
        has no track for the source or the track sends no picture, is answered with an error. Once
        the participant left, the call answers the request with an error itself and returns once
        the sink answered it. Otherwise the request waits from the call on, which returns at once:
        a task of the receiver's activates a source not yet received once the picks, captures and
        releases before it finished, so a request made while its source is released is answered by
        the subscription that activation starts, not with the release's error."""
        if self._closed:
            await self._sink.answer_image_requests([request], self._left())
            return
        source = request.video_source or CAMERA_SOURCE
        sampler = self._sampler(source)
        sampler.add_request(request)
        deadline = asyncio.create_task(self._expire_request(source, sampler, request))
        self._deadlines.add(deadline)
        deadline.add_done_callback(self._deadlines.discard)
        if source not in self._sources:
            self._activate(source)

    def _activate(self, source: str) -> None:
        """Receives `source` for image requests only, in a task of the receiver's that picks its
        track once the picks, captures and releases before it finished, and that the teardown
        waits for."""
        self._sources[source] = None
        activation = asyncio.create_task(self._pick_tracks(source))
        self._activations.add(activation)
        activation.add_done_callback(self._activations.discard)

    async def start_audio(self) -> None:
        """Receives the participant's audio from now on, when the options receive audio."""
        if self._stream_audio:
            return
        self._stream_audio = True
        await self._pick_tracks()

    async def capture(self, source: str, framerate: float | None) -> None:
        """Receives `source` at `framerate` pictures a second, or for image requests only when it is
        None; for a source already received, changes its framerate. Receives nothing once the
        participant left. A source being released starts again once the release finished. Picks
        the track of `source` only, so the failure of another source's selector is not reported
        again, as to an `on_error` handler that captures a fallback source."""
        async with self._picking:
            self._sources[source] = framerate
            self._sampler(source).set_framerate(framerate)
            failures = await self._pick_tracks_locked(source)
        await self._report_pick_failures(failures)

    async def release(self, source: str) -> None:
        """Stops receiving `source` and answers the image requests made on it before this call
        with an error, once the picks, captures and releases before it finished. The requests made
        since keep the source received for image requests only, through an activation, as of a
        request made while the source is not received, which the release starts as it stops the
        source, so cancelling the release does not leave them waiting without a track. A release
        cancelled once it stopped the source still answers the requests made before it with the
        error, not with a picture of the track that activation starts. Returns once every track of
        the source the release stopped is unsubscribed and its own answers were delivered. Called
        from outside any answer's `result_callback`, it also waits for every other answer given on
        the source, also those of an earlier release that was cancelled."""
        sampler = self._sampler(source)
        released = sampler.requests()
        async with self._picking:
            self._sources.pop(source, None)
            key = f"video:{source}"
            self._stop(key)
            # An earlier release may have dropped the sampler taken before the wait, and a request
            # made since went to the one replacing it.
            active = self._samplers.get(source)
            if active is not None:
                active.set_framerate(None)
                if any(all(request is not old for old in released) for request in active.requests()):
                    self._activate(source)
            retiring = self._retiring_tasks(key)
            error = f"{source} of {self.participant_id} released"

            async def withdraw() -> None:
                if retiring:
                    # A request the track was answering goes back to the sampler as the task ends.
                    await asyncio.wait(retiring)
                    # Their tracebacks keep the track's decoder and last picture, which the
                    # release, waiting for the answers, would keep; its own wait took them already.
                    retiring.clear()
                requests = [request for request in released if sampler.remove_request(request)]
                # A source neither received again nor asked for keeps no sampler, so the sources
                # released over a long session do not pile up.
                if source not in self._sources and not sampler.requests() and self._samplers.get(source) is sampler:
                    del self._samplers[source]
                if requests:
                    await self._sink.answer_image_requests(requests, error)

            # A task of the receiver's takes the requests out as the track's task ends, before the
            # track replacing it, which waits for that task, passes a picture on, and answers them,
            # also when this call is cancelled meanwhile.
            withdrawing = self._start_answering(source, withdraw)
            if retiring:
                await asyncio.wait(retiring)
        await asyncio.shield(withdrawing)
        # A release called from an answer's `result_callback` waits for no other answer: that
        # answer waits for it, and so may another one whose callback waits for a release too.
        if _IN_ANSWER.get():
            return
        answering = [task for task, answered in self._answering.items() if answered == source]
        if answering:
            await asyncio.wait(answering)

    def _left(self) -> str:
        return departure_error(self.participant_id)

    async def _answer(self, source: str | None, requests: list[UserImageRequestFrame], error: str) -> None:
        """Answers `requests`, taken out of the sampler of `source`, with `error`, in a task of the
        receiver's: nothing else answers them any more, so the answers run to their end when the
        caller is cancelled, as the teardown cancels a deadline or a stop a release, and a release
        of the source and the teardown wait for them."""
        if requests:
            answering = self._start_answering(source, lambda: self._sink.answer_image_requests(requests, error))
            await asyncio.shield(answering)

    def _start_answering(self, source: str | None, answer: Callable[[], Awaitable[None]]) -> asyncio.Task:
        """Runs `answer()`, which answers image requests of `source` with an error, in a task of
        the receiver's, which a release of the source and the teardown wait for; its failure goes
        to the caller awaiting the task, and to nobody once the caller was cancelled."""
        answering = asyncio.create_task(self._deliver(answer))
        answering.add_done_callback(retrieve)
        self._answering[answering] = source
        answering.add_done_callback(self._answering.pop)
        return answering

    async def _deliver(self, answer: Callable[[], Awaitable[None]]) -> None:
        _IN_ANSWER.set(True)
        await answer()

    async def _pick_tracks(self, only: str | None = None) -> None:
        async with self._picking:
            failures = await self._pick_tracks_locked(only)
        await self._report_pick_failures(failures)

    async def _pick_tracks_locked(self, only: str | None = None) -> list[tuple[str, Exception]]:
        """Picks the tracks of the newest catalog, if any, holding `_picking`: one pick, request,
        capture or release at a time, so a pick from a newer catalog is not undone by one from an
        older catalog, and a source is activated only once its release finished. With `only`, picks
        the track of that video source alone. Returns the sources whose `video_in_track` raised,
        with what it raised, for the caller to report once it released `_picking`, which an
        `on_error` handler capturing or releasing a source takes."""
        failures: list[tuple[str, Exception]] = []
        catalog = self._catalog
        if catalog is None:
            return failures
        if only is None and self._options.audio_in and self._stream_audio:
            audio = catalog.select(role="audio", packaging="loc", codec="opus", prefer="lowest_bitrate")
            await self._receive("audio", "audio", audio, self._receive_audio)
        for source in [source for source in self._sources if only is None or source == only]:
            try:
                track = self._video_track(catalog, source)
            except Exception as error:
                # The selector's failure is the application's: the track received stays.
                logger.exception("picking the %s track of %s failed", source, self.participant_id)
                failures.append((source, error))
                continue
            config = None if track is None else self._video_config(catalog, track)
            known = _KnownConfig(config)
            await self._receive(
                f"video:{source}",
                "video",
                track,
                lambda track, source=source, known=known: self._receive_video(source, track, known),
                init_data=config,
            )
        return failures

    async def _report_pick_failures(self, failures: list[tuple[str, Exception]]) -> None:
        for source, error in failures:
            await self._sink.event("on_error", f"picking the {source} track of {self.participant_id} failed", error)

    def _video_config(self, catalog: Catalog, track: CatalogTrack) -> bytes | None:
        """The track's Video Config the catalog carries as its initialization data, such as an avcC
        record; None for a track without, and for data that is not Base64, which is logged."""
        try:
            return catalog.init_data(track)
        except ValueError as error:
            logger.warning("video config of %s of %s ignored: %s", track.name, self.participant_id, error)
            return None

    async def _expire_request(self, source: str, sampler: "_Sampler", request: UserImageRequestFrame) -> None:
        """Answers `request`, made on `sampler`, with an error once it waited
        `IMAGE_REQUEST_TIMEOUT_SECONDS`, unless a picture or an error answered it before. A picture
        being delivered then answers it, unless the input drops the picture or the track ends
        before it was passed on."""
        await asyncio.sleep(IMAGE_REQUEST_TIMEOUT_SECONDS)
        await sampler.delivered(request)
        if not sampler.remove_request(request):
            return
        if self._catalog is None:
            error = f"{self.participant_id} has published no catalog"
        elif f"video:{source}" not in self._receiving:
            error = f"{self.participant_id} has no {source} track"
        else:
            error = f"no picture of {source} of {self.participant_id} came in time"
        await self._answer(source, [request], error)

    def _video_track(self, catalog: Catalog, source: str) -> CatalogTrack | None:
        """The track of `source` in `catalog`; raises what `video_in_track` raised."""
        if source != CAMERA_SOURCE:
            named = (track for track in catalog.tracks if track.name == source and track.role == "video")
            return next((track for track in named if _decodable(track)), None)
        if self._options.video_in_track is not None:
            return self._options.video_in_track(catalog)
        decodable = Catalog([track for track in catalog.tracks if _decodable(track)])
        return decodable.select(role="video", prefer="lowest_bitrate")

    async def _receive(
        self,
        key: str,
        kind: Literal["audio", "video"],
        track: CatalogTrack | None,
        follow: Callable[[CatalogTrack], Awaitable[None]],
        *,
        init_data: bytes | None = None,
    ) -> None:
        """Receives `track` for `key` with `follow`, replacing the track received for it, unless
        `track`, with the initialization data `init_data` its catalog gives it, is that one. Does
        nothing once the participant left, so a track picked during `run()`'s teardown, as for a
        source captured then, starts no task the teardown does not wait for."""
        if self._closed:
            return
        current = self._receiving.get(key)
        identity = None if track is None else _track_identity(track, init_data)
        if current is not None and current[0] == identity:
            return
        self._stop(key)
        if track is not None:
            source = key.removeprefix("video:") if kind == "video" else None
            receiving = asyncio.create_task(
                self._keep_receiving(track.name, lambda: follow(track), source, self._retiring_tasks(key))
            )
            self._receiving[key] = (_track_identity(track, init_data), receiving)

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
                await self._answer(video_source, self._sampler(video_source).take_requests(), message)
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

    async def _receive_video(self, source: str, track: CatalogTrack, known: "_KnownConfig") -> None:
        """Decodes each subscription to the track with decoding state of its own: the pictures its
        decoders hold back, as for B-frames, come as it ends, and the next subscription, as of a
        publisher that took over and starts its groups at 0 again, starts afresh. The groups without
        a Video Config of their own decode with the one `known` holds, which it learns from the
        objects for the next attempt, as after a decoder error. Raises ValueError before subscribing
        when no decoder takes the track's codec."""
        sampler = self._sampler(source)
        subscribed = self._subscribed(track.name, "video", sampler.new_subscription)
        pictures = follow_pictures(
            self._remote,
            track.name,
            track.codec or "",
            namespace=track.namespace,
            on_subscribed=subscribed,
            config=known.config,
        )
        try:
            async with aclosing(pictures):
                async for decoded in pictures:
                    await self._pass_picture(source, sampler, decoded)
        finally:
            known.config = pictures.config

    async def _pass_picture(self, source: str, sampler: "_Sampler", decoded: DecodedPicture) -> None:
        largest = decoded.largest
        location = decoded.location
        located = (location.group_id, location.object_id)
        fresh = largest is None or located >= (largest.group_id, largest.object_id)
        due, request = sampler.sample(decoded.decoded_at, located, fresh=fresh)
        if not due:
            return
        picture = decoded.picture
        try:
            rgb = await in_thread(lambda: picture.to_ndarray(format="rgb24").tobytes())
            size = (picture.width, picture.height)
            passed = await self._sink.push_picture(
                ReceivedPicture(self.participant_id, source, rgb, size, decoded.location, request)
            )
        except BaseException:
            # The track is replaced or fails before the picture answered the request; the next
            # picture of the source answers it instead.
            if request is not None:
                sampler.return_request(request)
            raise
        if request is not None:
            if passed:
                sampler.answered(request)
            else:
                # A paused input dropped the picture; the next one answers the request instead.
                sampler.return_request(request)

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


@dataclass
class _KnownConfig:
    """The Video Config of a picked video track, kept across the attempts to receive it: the one its
    catalog carries, then the one of the newest object that carried one."""

    config: bytes | None


@dataclass
class _Waiting:
    """An image request waiting for a picture, and the location of the newest picture of the
    subscription seen as it came, which a picture answering it must be newer than; None for none."""

    request: UserImageRequestFrame
    seen: tuple[int, int] | None


class _Sampler:
    """Decides which pictures of a video source are passed on: one every 1 / `framerate` seconds on
    the schedule of pipecat's own sampler, which a picture up to `JITTER_TOLERANCE_SECONDS` early
    is due on, none when `framerate` is None, and, after each image request either way, the next
    fresh picture from a newer location than any picture of the subscription seen before the
    request, so a group whose stream was delayed does not answer it with an older picture. A request
    handed to a picture is being delivered until the picture was passed on (`answered`) or the
    request comes back (`return_request`)."""

    def __init__(self, framerate: float | None):
        self.set_framerate(framerate)
        self._next_at = 0.0
        self._requests: list[_Waiting] = []
        # The requests being delivered, each with the event set once its delivery ended.
        self._delivering: list[tuple[_Waiting, asyncio.Event]] = []
        # The location of the newest picture of the current subscription seen; a subscription, as
        # of a publisher that took over, may start its groups at 0 again.
        self._newest: tuple[int, int] | None = None

    def set_framerate(self, framerate: float | None) -> None:
        self._interval = None if framerate is None else 1 / framerate

    def new_subscription(self) -> None:
        """Forgets the pictures seen, whose locations the new subscription's do not follow."""
        self._newest = None
        for waiting in self._requests:
            waiting.seen = None

    def add_request(self, request: UserImageRequestFrame) -> None:
        self._requests.append(_Waiting(request, self._newest))

    def return_request(self, request: UserImageRequestFrame) -> None:
        """Takes back a request whose picture was not passed on, ahead of the others."""
        waiting = self._end_delivery(request)
        self._requests.insert(0, waiting if waiting is not None else _Waiting(request, None))

    def answered(self, request: UserImageRequestFrame) -> None:
        """Ends the delivery of a request whose picture was passed on."""
        self._end_delivery(request)

    async def delivered(self, request: UserImageRequestFrame) -> None:
        """Returns once `request` is not being delivered: answered, waiting or taken out."""
        while (delivery := self._delivery(request)) is not None:
            await delivery.wait()

    def _delivery(self, request: UserImageRequestFrame) -> asyncio.Event | None:
        return next((ended for delivering, ended in self._delivering if delivering.request is request), None)

    def _end_delivery(self, request: UserImageRequestFrame) -> _Waiting | None:
        for index, (delivering, ended) in enumerate(self._delivering):
            if delivering.request is request:
                del self._delivering[index]
                ended.set()
                return delivering
        return None

    def remove_request(self, request: UserImageRequestFrame) -> bool:
        """Takes `request` out if it is waiting; returns whether it was."""
        for index, waiting in enumerate(self._requests):
            if waiting.request is request:
                del self._requests[index]
                return True
        return False

    def requests(self) -> list[UserImageRequestFrame]:
        """The requests waiting or being delivered."""
        return [waiting.request for waiting in self._requests] + [waiting.request for waiting, _ in self._delivering]

    def take_requests(self) -> list[UserImageRequestFrame]:
        requests, self._requests = self._requests, []
        return [waiting.request for waiting in requests]

    def sample(
        self, now: float, location: tuple[int, int], *, fresh: bool = True
    ) -> tuple[bool, UserImageRequestFrame | None]:
        """Whether the picture decoded at `now` from the object at `location` (group and object
        id) is passed on, and the request it answers: only a `fresh` one, not replayed from before
        its subscription, from a newer location than any seen before the request. A group's
        pictures come in presentation order, which may differ from their objects' (B-frames), so a
        picture of an earlier object than one seen leaves the request to the next."""
        first = self._requests[0] if self._requests else None
        answers = first is not None and fresh and (first.seen is None or location > first.seen)
        self._newest = location if self._newest is None else max(self._newest, location)
        waiting = self._requests.pop(0) if answers else None
        interval = self._interval
        if interval is None:
            if waiting is None:
                return False, None
        else:
            tolerance = min(JITTER_TOLERANCE_SECONDS, interval / 2)
            if waiting is None and now < self._next_at - tolerance:
                return False, None
            if waiting is not None or now - self._next_at > interval:
                # A fresh schedule from the first picture, one answering a request, or the first
                # after a stall, which passes on one picture instead of a burst of missed ones.
                self._next_at = now + interval
            else:
                # One interval after this picture's scheduled time, not its arrival, so jitter costs
                # no picture; after its arrival when it came late, so a slower stream passes all.
                self._next_at = max(self._next_at, now - tolerance) + interval
        if waiting is not None:
            self._delivering.append((waiting, asyncio.Event()))
        return True, None if waiting is None else waiting.request


def departure_error(participant_id: str) -> str:
    """The error answering the image requests of a participant who left."""
    return f"{participant_id} left"


def _track_identity(track: CatalogTrack, init_data: bytes | None) -> tuple:
    """What a newer catalog's track must share with the one being received for it not to be
    replaced: its namespace, name, packaging and codec, and the initialization data the catalog
    gives it, which configures its decoder."""
    return (track.namespace, track.name, track.packaging, track.codec, init_data)


def _decodable(track: CatalogTrack) -> bool:
    """Whether `decode_pictures` takes the track: LOC packaging, and a codec PyAV decodes."""
    if track.packaging != "loc":
        return False
    try:
        VideoDecoder(track.codec or "")
        return True
    except ValueError:
        return False
