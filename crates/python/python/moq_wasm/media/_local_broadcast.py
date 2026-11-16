import asyncio

from moq_wasm._session import Announcement, Session, TrackWriter
from moq_wasm.media._event_timeline import EventTimeline

DEFAULT_RETENTION_SECONDS = 60.0


class LocalBroadcast:
    """A namespace this session publishes with its tracks. The tracks added before the first
    `await` inside `async with` are registered before the namespace is published, since the relay
    may forward a SUBSCRIBE at once; a track added later is registered on its own, and what is
    written to it before that is dropped."""

    def __init__(self, session: Session, namespace: list[str]):
        self.namespace = namespace
        self._session = session
        self._pending: list[TrackWriter] = []
        # False once the batch registered before the PUBLISH_NAMESPACE is complete.
        self._batching = True
        self._publishing: asyncio.Task[Announcement] | None = None
        self._registering: set[asyncio.Task] = set()

    def publish_track(self, name: str, *, retention_seconds: float = DEFAULT_RETENTION_SECONDS) -> TrackWriter:
        """A track the session answers SUBSCRIBE and FETCH for, keeping what is written for
        `retention_seconds`."""
        writer = TrackWriter(self.namespace, name, retention_seconds)
        if self._batching:
            self._pending.append(writer)
        else:
            task = asyncio.create_task(self._session._register(writer))
            self._registering.add(task)
            task.add_done_callback(self._registering.discard)
        return writer

    def event_timeline(
        self, name: str, *, retention_seconds: float = DEFAULT_RETENTION_SECONDS, replay: bool = False
    ) -> EventTimeline:
        return EventTimeline(self.publish_track(name, retention_seconds=retention_seconds), replay=replay)

    async def __aenter__(self) -> "LocalBroadcast":
        self._publishing = asyncio.create_task(self._publish())
        return self

    async def _publish(self) -> Announcement:
        # Tracks added while a registration awaits join the batch, so all precede the namespace.
        while self._pending:
            await self._session._register(self._pending.pop(0))
        self._batching = False
        return await self._session.announce(self.namespace)

    async def published(self) -> None:
        """Waits until the namespace is published. Raises RuntimeError when the relay refuses it."""
        assert self._publishing is not None, "use the broadcast with async with"
        await asyncio.shield(self._publishing)

    async def __aexit__(self, *exc_info: object) -> None:
        assert self._publishing is not None
        for task in self._registering:
            task.cancel()
        if not self._publishing.done():
            self._publishing.cancel()
            return
        if self._publishing.exception() is None:
            await self._publishing.result().close()


def create_broadcast(session: Session, namespace: list[str]) -> LocalBroadcast:
    """Use it with `async with`: entering publishes the namespace, leaving withdraws it."""
    return LocalBroadcast(session, namespace)
