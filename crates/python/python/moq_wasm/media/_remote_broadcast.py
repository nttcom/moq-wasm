import asyncio
import logging
from collections.abc import AsyncIterator, Awaitable, Callable

from moq_wasm._native import Object
from moq_wasm._session import Session
from moq_wasm.media._catalog import CATALOG_TRACK_NAME, Catalog

logger = logging.getLogger("moq_wasm")

RESUBSCRIBE_DELAY_SECONDS = 1.0


class RemoteBroadcast:
    """A namespace another publisher publishes. Its handler in `watch_broadcasts` is cancelled when
    the namespace is withdrawn, which ends `catalogs()` and `follow()`."""

    def __init__(self, session: Session, namespace: list[str]):
        self.namespace = namespace
        self._session = session

    async def catalogs(self) -> AsyncIterator[Catalog]:
        """Each catalog the publisher writes, starting from the one the relay holds (a joining
        FETCH). A catalog that does not parse is logged and skipped. Ends with the catalog track;
        raises RuntimeError when the SUBSCRIBE fails."""
        subscription = await self._session.subscribe_current_group(self.namespace, CATALOG_TRACK_NAME)
        try:
            async for catalog_object in subscription:
                try:
                    yield Catalog.parse(catalog_object.payload)
                except ValueError as error:
                    logger.warning("catalog of %s ignored: %s", self.namespace, error)
        finally:
            await _unsubscribe(subscription)

    async def follow(self, name: str) -> AsyncIterator[Object]:
        """The track's objects, subscribing again a second after the track ends or its SUBSCRIBE
        fails, as when the page that published it leaves and another in the room takes over. Never
        ends by itself."""
        while True:
            try:
                subscription = await self._session.subscribe(self.namespace, name)
                try:
                    async for track_object in subscription:
                        yield track_object
                finally:
                    await _unsubscribe(subscription)
                logger.info("track %s of %s ended", name, self.namespace)
            except RuntimeError as error:
                logger.info("track %s of %s ended: %s", name, self.namespace, error)
            await asyncio.sleep(RESUBSCRIBE_DELAY_SECONDS)


async def _unsubscribe(subscription) -> None:
    try:
        await subscription.close()
    except RuntimeError as error:
        logger.debug("UNSUBSCRIBE failed: %s", error)


async def watch_broadcasts(
    session: Session,
    prefix: list[str],
    handler: Callable[[RemoteBroadcast], Awaitable[None]],
    *,
    filter: Callable[[list[str]], bool] | None = None,
    empty_timeout: float | None = None,
) -> None:
    """Runs `handler` as a task for each namespace published under `prefix` that `filter` accepts,
    those published before the call included, except the ones this session publishes. The task is
    cancelled when the namespace is withdrawn, and started anew when it is published again. A
    handler's exception is logged. Runs until cancelled or the session closes; with
    `empty_timeout`, returns once no broadcast has been published for that many seconds, from the
    start or since the last one was withdrawn. Raises RuntimeError when the relay refuses the
    SUBSCRIBE_NAMESPACE."""
    watch = await session.watch_namespaces(prefix)
    running: dict[tuple[str, ...], asyncio.Task] = {}
    loop = asyncio.get_running_loop()
    empty_until = None if empty_timeout is None else loop.time() + empty_timeout
    try:
        while True:
            try:
                async with asyncio.timeout_at(None if running else empty_until):
                    event = await anext(watch)
            except (TimeoutError, StopAsyncIteration):
                return
            if session._publishes(event.namespace) or (filter is not None and not filter(event.namespace)):
                continue
            key = tuple(event.namespace)
            if key in running:
                running.pop(key).cancel()
            if event.active:
                running[key] = asyncio.create_task(_run(handler, RemoteBroadcast(session, event.namespace)))
            elif not running and empty_timeout is not None:
                empty_until = loop.time() + empty_timeout
    finally:
        watch.close()
        for task in running.values():
            task.cancel()


async def _run(handler: Callable[[RemoteBroadcast], Awaitable[None]], broadcast: RemoteBroadcast) -> None:
    try:
        await handler(broadcast)
    except Exception:
        logger.exception("handler of %s failed", broadcast.namespace)
