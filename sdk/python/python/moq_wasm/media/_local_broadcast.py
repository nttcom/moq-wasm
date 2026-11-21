import asyncio
import logging
from collections.abc import Iterable

from moq_wasm import _native
from moq_wasm._once import Once
from moq_wasm._session import (
    DEFAULT_PRIORITY,
    DEFAULT_RETENTION_SECONDS,
    Announcement,
    Session,
    TrackWriter,
    _namespace_key,
)
from moq_wasm.media._catalog import CATALOG_TRACK_NAME
from moq_wasm.media._event_timeline import EventTimeline
from moq_wasm.media._media_source import DEFAULT_AUDIO, MediaSource

logger = logging.getLogger("moq_wasm")

# How long the native media broadcast keeps the objects of a track other than the catalog, of which
# it keeps the newest only (`ensure_origin_track` in crates/broadcast/src/relay_connection.rs).
MEDIA_RETENTION_SECONDS = 60.0


class LocalBroadcast:
    """A namespace this session publishes with its tracks. The tracks added before the first
    `await` inside `async with` are registered before the namespace is published, since the relay
    may forward a SUBSCRIBE at once; a track added later is registered on its own, and what is
    written to it before that is dropped. Once the broadcast was left, adding a track or a media
    source raises RuntimeError. When the namespace or the batch cannot be published, every track
    of the broadcast, one registered before included, raises that failure from its writes, and
    leaving a block that exited normally raises it unless it was raised to the caller before."""

    def __init__(self, session: Session, namespace: list[str]):
        # Immutable, so a caller changing a list cannot redirect the withdrawal.
        self._namespace = tuple(namespace)
        self._session = session
        self._pending: list[TrackWriter] = []
        # False once the batch registered before the PUBLISH_NAMESPACE is complete.
        self._batching = True
        self._publishing: asyncio.Task[Announcement] | None = None
        self._registering: set[asyncio.Task] = set()
        # Every track the broadcast added and whose close has not finished, closed when it is left.
        self._writers: list[TrackWriter] = []
        self._media_sources: list[MediaSource] = []
        # The tracks the media broadcast publishes: the catalog, each source's and rendition's, and
        # those its catalog lists next to them.
        self._media_tracks: set[str] = set()
        # Those of `_media_tracks` claimed only as listed next to a source, not as a source's own.
        self._implied_tracks: set[str] = set()
        self._media_broadcast: _native.MediaBroadcast | None = None
        # Set as leaving starts, so nothing is added to a broadcast that is being withdrawn.
        self._left = False
        # What failed the publishing, which settles the tracks and media sources waiting for it.
        self._failure: Exception | None = None
        # Whether `_failure` was raised to the caller, by `published()`, a track or a media source.
        self._failure_raised = False
        self._leaving: Once[None] = Once()

    @property
    def namespace(self) -> list[str]:
        """A copy: changing it changes nothing the broadcast does."""
        return list(self._namespace)

    def publish_track(
        self,
        name: str,
        *,
        retention_seconds: float = DEFAULT_RETENTION_SECONDS,
        datagram: bool = False,
        priority: int = DEFAULT_PRIORITY,
    ) -> TrackWriter:
        """A track the session answers SUBSCRIBE and FETCH for, keeping what is written for
        `retention_seconds`; with `datagram`, sent in datagrams; `priority` is its Publisher
        Priority, as in `moq_wasm.Session.publish_track`. It takes no `publish`: the tracks
        declared before the first `await` are registered before the namespace is published, and a
        PUBLISH must follow the namespace. Raises RuntimeError once the broadcast was left, and
        while another writer of the broadcast publishes the track; once the broadcast was entered,
        while another writer of the session does. A track declared before that is the session's
        only once entering it succeeded, which raises RuntimeError while another writer of the
        session publishes it. With a `retention_seconds` other than the one the track was
        registered with first in the session, or the other `datagram` than the one the track sent
        its objects with, its registration fails with ValueError, which `registered()` raises."""
        self._raise_if_left()
        if self._publishing is not None:
            writer = self._session._track_writer(
                self.namespace, name, retention_seconds, publish=False, datagram=datagram, priority=priority
            )
        else:
            if name in self._declared_tracks():
                raise RuntimeError(f"the broadcast already publishes track {name} of {self.namespace}")
            writer = TrackWriter(
                self._session, self.namespace, name, retention_seconds, datagram=datagram, priority=priority
            )
            self._session._on_end_while_held(writer, TrackWriter._end_with)
        self._writers.append(writer)
        writer._on_closed.append(lambda: self._forget(writer))
        if self._failure is not None:
            writer._end_with(self._failure, raised=self._raised_failure)
        elif self._batching:
            self._pending.append(writer)
        else:
            task = asyncio.create_task(self._register_late(writer))
            self._registering.add(task)
            task.add_done_callback(self._registering.discard)
        return writer

    async def _register_late(self, writer: TrackWriter) -> None:
        """Registers a track added after the namespace was published; `writer.registered()` raises
        what failed it. A writer that will not be registered, as one whose registration failed or
        was cancelled by leaving, has nothing left to close, and the broadcast forgets it. One
        registered whose opening failed or was cancelled is closing, and the broadcast keeps it
        until its close finished, so leaving waits for that."""
        try:
            await self._session._register(writer)
        except BaseException as error:
            if writer._writer is None:
                self._forget(writer)
            if not isinstance(error, Exception):
                raise
            logger.warning("track %s of %s was not registered: %r", writer.name, self.namespace, error)

    def _forget(self, writer: TrackWriter) -> None:
        """Unlinks a writer the broadcast has nothing left to close for: its close finished, or it
        will not be registered, whichever comes first, as when it is closed while its registration
        fails. A closed writer still waiting for the batch, which would not register it, leaves the
        batch too."""
        if writer in self._writers:
            self._writers.remove(writer)
        if writer._closed and writer in self._pending:
            self._pending.remove(writer)

    def event_timeline(self, name: str, *, retention_seconds: float = DEFAULT_RETENTION_SECONDS) -> EventTimeline:
        return EventTimeline(self.publish_track(name, retention_seconds=retention_seconds))

    def add_media_source(
        self,
        video: str = "video",
        audio: str = "audio",
        *,
        label: str,
        video_bitrate: int | None = None,
        video_framerate: int | None = None,
        cmaf: bool = False,
        on_demand: bool = True,
    ) -> MediaSource:
        """A source of encoded video and audio on the tracks `video` and `audio`, listed in the
        broadcast's MSF catalog (`catalog`) with the media timeline `<video>_timeline`, and with
        `cmaf`, `<video>_cmaf` and `<audio>_cmaf`; `cmaf` and `on_demand` as in
        `_native.MediaBroadcast.add_source`. With the default `video` or `audio`, the catalog also
        lists the `chat` track, and with the default `video` an `audio` placeholder while no source
        has an `audio`: the broadcast claims them as well, and ends them once the catalog no longer
        lists them. Add it before the first `await` inside `async with`, since the media broadcast
        publishes the namespace; raises RuntimeError after that, once the broadcast was left, and
        while another track of the broadcast, or once it was entered of the session, has the name
        of one of its tracks. Raises ValueError for a track the session registered before in
        datagrams, which the media broadcast's subscribers cannot read by group, or with another
        retention than the media broadcast gives it (the newest object for the catalog, 60 s for
        the others), as the native origin keeps both, for a negative `video_bitrate` or
        `video_framerate`, or one too large, and TypeError for an option of another type, such as
        a float `video_framerate`."""
        self._raise_if_left()
        if self._failure is not None:
            raise RuntimeError("the broadcast was not published") from self._failure
        if not self._batching:
            raise RuntimeError("add media sources before the broadcast is published")
        source = MediaSource(
            video,
            audio,
            label=label,
            video_bitrate=video_bitrate,
            video_framerate=video_framerate,
            cmaf=cmaf,
            on_demand=on_demand,
        )
        self._claim_source_tracks(source)
        source._claim_tracks = self._claim_media_tracks
        if self._publishing is not None:
            self._session._on_end(source._stop_with_session)
        else:
            self._session._on_end_while_held(source, MediaSource._stop_with_session)
        self._media_sources.append(source)
        return source

    def _claim_source_tracks(self, source: MediaSource) -> None:
        """Claims the tracks the media broadcast publishes for `source`: its own, the catalog with
        the first source, and those the catalog lists next to it, which sources share. A source's
        own audio takes the place of an audio placeholder; another of its tracks named as a track
        the catalog lists next to a source is refused, as is such a track named as another source's
        own one, while its placeholder is not. Another track named as the placeholder, as a video
        named `audio`, is refused in either order, since the native catalog lists the placeholder
        while no source's audio has its name (`audio_placeholder` in
        crates/broadcast/src/broadcast.rs). A source naming two of its own tracks alike is refused
        before its audio takes a placeholder's place, which would leave one name to both."""
        own = [*([] if self._media_sources else [CATALOG_TRACK_NAME]), *source._track_names()]
        self._refuse_duplicates(own)
        replacing = {name for name in own if name in self._implied_tracks}
        for name in replacing - ({DEFAULT_AUDIO} & {source.audio}):
            raise RuntimeError(f"the broadcast already publishes track {name} of {self.namespace}")
        placeholder_taken = any(added.audio == DEFAULT_AUDIO for added in self._media_sources)
        implied = []
        for name in source._implied_track_names():
            if name in self._implied_tracks or (name == DEFAULT_AUDIO and placeholder_taken):
                continue
            implied.append(name)
        self._claim_media_tracks([*(name for name in own if name not in replacing), *implied])
        self._implied_tracks -= replacing
        self._implied_tracks.update(implied)

    def _claim_media_tracks(self, names: list[str]) -> None:
        """Claims tracks the media broadcast publishes. Raises RuntimeError, claiming none of them,
        once the broadcast was left, and while another track of the broadcast, or once it was
        entered of the session, has one of the names, and ValueError for a name the session
        registered in datagrams or with another retention."""
        self._raise_if_left()
        self._refuse_duplicates(names)
        self._session._refuse_other_registrations(self._namespace, _media_retentions(names))
        if self._publishing is not None:
            self._session._claim_tracks(self._namespace, {name: self for name in names})
        else:
            declared = self._declared_tracks()
            for name in names:
                if name in declared:
                    raise RuntimeError(f"the broadcast already publishes track {name} of {self.namespace}")
        self._media_tracks.update(names)

    def _refuse_duplicates(self, names: list[str]) -> None:
        if len(set(names)) < len(names):
            raise RuntimeError(f"a media source of {self.namespace} names two of its tracks alike: {names}")

    def _declared_tracks(self) -> dict[str, object]:
        """The owner of each track declared on the broadcast, which entering it claims in the
        session."""
        owners: dict[str, object] = {name: self for name in self._media_tracks}
        owners.update((writer.name, writer) for writer in self._writers if not writer._closed)
        return owners

    def _raise_if_left(self) -> None:
        if self._left:
            raise RuntimeError(f"the broadcast of {self.namespace} was left")

    async def __aenter__(self) -> "LocalBroadcast":
        """Raises RuntimeError for a broadcast entered before, while another broadcast or
        announcement of the session publishes the namespace, until it finished withdrawing it, and
        while another writer of the session publishes a track declared on the broadcast, and
        ValueError for a media source's track the session registered in datagrams or with another
        retention since. Until it was entered, the session does not keep the broadcast or what was
        declared on it alive, though it ends the tracks and media sources the app holds when it
        ends, so a broadcast that could not be entered leaves nothing behind."""
        if self._publishing is not None:
            raise RuntimeError(f"the broadcast of {self.namespace} was entered before")
        self._session._raise_if_ended()
        # The session may have registered one otherwise since it was claimed.
        self._session._refuse_other_registrations(self._namespace, _media_retentions(self._media_tracks))
        self._session._own(self.namespace)
        try:
            self._session._claim_tracks(self._namespace, self._declared_tracks())
        except BaseException:
            self._session._disown(self.namespace)
            raise
        # The session holds the handles declared before from now on, as it does those added later.
        for writer in self._writers:
            self._session._forget_end_while_held(writer)
            writer._end_with_session()
        for source in self._media_sources:
            self._session._forget_end_while_held(source)
            self._session._on_end(source._stop_with_session)
        self._publishing = asyncio.create_task(self._publish())
        return self

    async def _publish(self) -> Announcement:
        try:
            # Tracks added while a registration awaits join the batch, so all precede the namespace.
            while self._pending:
                await self._session._register(self._pending.pop(0))
            self._batching = False
            if self._media_sources:
                return await self._session._announce(
                    self.namespace, lambda native: self._build_media(native).announce(), owns=False
                )
            return await self._session._announce(
                self.namespace, lambda native: native.announce(self.namespace), owns=False
            )
        except Exception as error:
            self._fail(error)
            raise

    def _fail(self, error: Exception) -> None:
        """The namespace will not be published: what waits for it raises `error` at once, and so do
        the writes of every track, as the relay takes nothing of a namespace it refused."""
        self._failure = error
        for writer in self._writers:
            writer._end_with(error, raised=self._raised_failure)
        self._pending.clear()
        for source in self._media_sources:
            source._detach(error, failure=True, raised=self._raised_failure)

    def _raised_failure(self) -> None:
        self._failure_raised = True

    def _build_media(self, native: _native.Session) -> _native.MediaBroadcast:
        broadcast = native.media_broadcast(self.namespace)
        self._media_broadcast = broadcast
        try:
            for source in self._media_sources:
                source._build(broadcast, self._session._done)
        except Exception:
            # A source built before may have published the namespace with its catalog.
            self._session._release_later(_namespace_key(self.namespace), self._withdraw_media(broadcast))
            raise
        return broadcast

    async def _withdraw_media(self, broadcast: _native.MediaBroadcast) -> None:
        """Closes a media broadcast that failed to build, then withdraws its namespace; its sources
        were detached by the failure. A source built before publishes on its own, from its media
        configuration: the namespace, its tracks and its catalog. Closing waits for that and ends
        them, so the withdrawal follows it: sent while the source's PUBLISH_NAMESPACE was under way,
        the withdrawal would leave the namespace published, and while its PUBLISH was, the relay
        would leave that unanswered, and the close waiting for it would hang until the session
        ended."""
        try:
            await broadcast.close()
        except Exception as error:
            logger.debug("closing the media broadcast of %s failed: %r", self.namespace, error)
        try:
            if not self._session._done.done():
                await self._session._native.unannounce(self.namespace)
        except Exception as error:
            # Refused, as for a namespace no source published.
            logger.debug("withdrawing the namespace %s failed: %r", self.namespace, error)

    async def published(self) -> None:
        """Waits until the namespace is published. Raises RuntimeError when the relay refuses it or
        a track of the batch registered before it; the tracks and media sources then raise that
        error too."""
        assert self._publishing is not None, "use the broadcast with async with"
        try:
            await asyncio.shield(self._publishing)
        except Exception as error:
            if error is self._failure:
                self._raised_failure()
            raise

    async def __aexit__(self, *exc_info: object) -> None:
        """Withdraws the namespace and closes every track and media source, each even when an
        earlier step raised; raises the first step's error. When the block exited normally, it
        raises what kept the broadcast from being published, unless that was raised to the caller
        before, so a refused broadcast does not look as if it ran. The session may publish the
        namespace again once the leaving finished: cancelled, this leaves it running, and a later
        call waits for it."""
        assert self._publishing is not None
        if not self._leaving.started:
            # At once, so a publishing that has not started yet does not start.
            self._left = True
            for task in self._registering:
                task.cancel()
            self._publishing.cancel()
        await self._leaving.run(self._leave_and_disown)
        if exc_info[0] is None and self._failure is not None and not self._failure_raised:
            self._raised_failure()
            raise self._failure

    async def _leave_and_disown(self) -> None:
        try:
            await self._leave()
        finally:
            # Recorded while the broadcast still owns its tracks, so no other writer registers one
            # before.
            self._session._record_media_tracks(self._namespace, _media_retentions(self._registered_media_tracks()))
            self._session._release_tracks(self._namespace, {name: self for name in self._media_tracks})
            self._session._disown(self.namespace)

    def _registered_media_tracks(self) -> set[str]:
        """The tracks the native media broadcast registered in the session's origin, or may have,
        as a subscriber may take the catalog, and the tracks it lists, at any time: the catalog once
        the media broadcast was built, and those its sources' configurations had it list."""
        if self._media_broadcast is None:
            return set()
        names = {CATALOG_TRACK_NAME}
        for source in self._media_sources:
            names.update(source._registered_track_names())
        return names

    async def _leave(self) -> None:
        assert self._publishing is not None
        failures: list[BaseException] = []
        await asyncio.gather(*self._registering, return_exceptions=True)
        while self._pending:
            self._pending.pop()._abandon()
        try:
            await asyncio.wait([self._publishing])
            # What the publishing left to withdraw comes first: a PUBLISH_NAMESPACE its
            # cancellation left running, once answered, or a media broadcast that failed to build.
            await self._session._released(_namespace_key(self.namespace))
            if not self._publishing.cancelled() and self._publishing.exception() is None:
                await self._publishing.result().close()
        except BaseException as error:
            failures.append(error)
        # Ends the subscriptions the relay still forwards, and the handles, so a writer kept past
        # the broadcast cannot write into a later broadcast of the same namespace.
        for writer in list(self._writers):
            try:
                await writer.close()
            except BaseException as error:
                failures.append(error)
        for source in self._media_sources:
            source._detach()
            self._session._forget_end(source._stop_with_session)
        if self._media_broadcast is not None:
            try:
                await self._media_broadcast.close()
            except BaseException as error:
                failures.append(error)
        if failures:
            raise failures[0]


def _media_retentions(names: Iterable[str]) -> dict[str, float]:
    """The retention, in seconds, the native media broadcast gives each of its tracks `names`."""
    return {name: 0.0 if name == CATALOG_TRACK_NAME else MEDIA_RETENTION_SECONDS for name in names}


def create_broadcast(session: Session, namespace: list[str]) -> LocalBroadcast:
    """Use it with `async with`: entering publishes the namespace, leaving withdraws it."""
    return LocalBroadcast(session, namespace)
