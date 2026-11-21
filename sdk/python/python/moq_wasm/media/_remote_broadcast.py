import asyncio
import json
import logging
from collections.abc import AsyncIterator, Awaitable, Callable
from functools import partial

from moq_wasm._native import Object
from moq_wasm._once import wait_through_cancellation
from moq_wasm._session import NamespaceWatch, Session
from moq_wasm.media._catalog import CATALOG_TRACK_NAME, Catalog
from moq_wasm.media._event_timeline import EventRecord

logger = logging.getLogger("moq_wasm")

RESUBSCRIBE_DELAY_SECONDS = 1.0
DEFAULT_DELIVERY_TIMEOUT_SECONDS = 2.0


class RemoteBroadcast:
    """A namespace another publisher publishes. Its handler in `watch_broadcasts` is cancelled when
    the namespace is withdrawn, which ends `catalogs()`, `follow()` and `event_timeline()`."""

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

    async def follow(
        self, name: str, *, delivery_timeout: float | None = DEFAULT_DELIVERY_TIMEOUT_SECONDS
    ) -> AsyncIterator[Object]:
        """The track's objects, subscribing again a second after the track ends or its SUBSCRIBE
        fails, as when the page that published it leaves and another in the room takes over. Never
        ends by itself. `delivery_timeout` is the SUBSCRIBE's DELIVERY TIMEOUT in seconds: on a
        congested link, the relay stops sending a group once one of its objects is that old. Pass
        None for a track whose every object matters, such as chat."""
        while True:
            try:
                subscription = await self._session.subscribe(
                    self.namespace, name, delivery_timeout=delivery_timeout
                )
                try:
                    async for track_object in subscription:
                        yield track_object
                finally:
                    await _unsubscribe(subscription)
                logger.info("track %s of %s ended", name, self.namespace)
            except RuntimeError as error:
                logger.info("track %s of %s ended: %s", name, self.namespace, error)
            await asyncio.sleep(RESUBSCRIBE_DELAY_SECONDS)


    async def event_timeline(
        self, name: str, *, priority: int | None = None, delivery_timeout: float | None = None
    ) -> AsyncIterator[EventRecord]:
        """The records of a draft-ietf-moq-msf-02 §8 event timeline track, starting with those of
        the group the relay holds (a joining FETCH), and subscribing again as `follow` does; the
        records read before a SUBSCRIBE are not read again. An object that is not a JSON list of
        records is logged and skipped. `priority` is the Subscriber Priority (128 when None) and
        `delivery_timeout` the DELIVERY TIMEOUT in seconds, none by default: a chat or an episode
        needs every record. Never ends by itself."""
        read_up_to = None
        while True:
            try:
                subscription = await self._session.subscribe_current_group(
                    self.namespace, name, priority=priority, delivery_timeout=delivery_timeout
                )
                resumed_after = read_up_to
                try:
                    async for track_object in subscription:
                        location = (track_object.group_id, track_object.object_id)
                        if not track_object.payload or (resumed_after is not None and location <= resumed_after):
                            continue
                        read_up_to = max(location, read_up_to or location)
                        for record in _decode_records(track_object.payload, name, self.namespace):
                            yield record
                finally:
                    await _unsubscribe(subscription)
                logger.info("track %s of %s ended", name, self.namespace)
            except RuntimeError as error:
                logger.info("track %s of %s ended: %s", name, self.namespace, error)
            await asyncio.sleep(RESUBSCRIBE_DELAY_SECONDS)


def _decode_records(payload: bytes, name: str, namespace: list[str]) -> list[EventRecord]:
    try:
        records = json.loads(payload)
    except ValueError as error:
        logger.warning("event timeline object of %s in %s ignored: %s", name, namespace, error)
        return []
    if not isinstance(records, list) or not all(isinstance(record, dict) for record in records):
        logger.warning("event timeline object of %s in %s ignored: not a list of records", name, namespace)
        return []
    return records


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
    those published before the call included, except the ones this session publishes, from the
    start of their PUBLISH_NAMESPACE until they were withdrawn. `filter` is asked when a namespace
    is published, so a handler it admitted ends when its namespace is withdrawn even if `filter`
    would refuse the namespace by then. The task is cancelled when the namespace is withdrawn, and
    started anew when it is published again, once the cancelled one finished. Before returning, it
    cancels the handlers still running and waits until every handler finished. A handler's
    exception is logged. Runs until cancelled or the session ends, raising what ended it
    (`moq_wasm.SessionEnded`); with `empty_timeout`, returns once no broadcast has been published
    for that many seconds, from the start or since the last one was withdrawn. Raises RuntimeError
    when the relay refuses the SUBSCRIBE_NAMESPACE."""
    watch = await session.watch_namespaces(prefix)
    await follow_watch(session, watch, handler, filter=filter, empty_timeout=empty_timeout)


async def follow_watch(
    session: Session,
    watch: NamespaceWatch,
    handler: Callable[[RemoteBroadcast], Awaitable[None]],
    *,
    filter: Callable[[list[str]], bool] | None = None,
    empty_timeout: float | None = None,
) -> None:
    """`watch_broadcasts` on a watch the caller already has, so it knows the relay accepted the
    SUBSCRIBE_NAMESPACE before the broadcasts are followed. Closes the watch at the end."""
    running: dict[tuple[str, ...], asyncio.Task] = {}
    # The handlers of withdrawn namespaces that are still finishing.
    leaving: dict[tuple[str, ...], asyncio.Task] = {}
    loop = asyncio.get_running_loop()
    empty_until = None if empty_timeout is None else loop.time() + empty_timeout
    try:
        while True:
            try:
                async with asyncio.timeout_at(None if running else empty_until):
                    event = await anext(watch)
            except TimeoutError:
                return
            except StopAsyncIteration:
                session._raise_if_ended()
                return
            key = tuple(event.namespace)
            if event.active:
                if session._publishes(event.namespace) or (filter is not None and not filter(event.namespace)):
                    continue
                previous = running.pop(key, None)
                if previous is not None:
                    previous.cancel()
                else:
                    # Already cancelled; cancelling it again would cut its teardown short.
                    previous = leaving.pop(key, None)
                broadcast = RemoteBroadcast(session, event.namespace)
                running[key] = asyncio.create_task(_run(handler, broadcast, previous))
                # A cancelled handler's traceback keeps its locals, as its pipeline: kept here
                # until the next event, they would outlive it.
                previous = None
            else:
                # A withdrawal retires the handler its publication started, whether or not `filter`
                # would still accept the namespace.
                previous = running.pop(key, None)
                if previous is None:
                    continue
                previous.cancel()
                if not previous.done():
                    leaving[key] = previous
                    previous.add_done_callback(partial(_forget, leaving, key))
                previous = None
                if not running and empty_timeout is not None:
                    empty_until = loop.time() + empty_timeout
    finally:
        for task in running.values():
            task.cancel()
        try:
            # A handler waiting for the one before it finishes after it, so that one is waited for
            # too.
            await wait_through_cancellation([*running.values(), *leaving.values()])
        finally:
            await watch.close()


def _forget(tasks: dict[tuple[str, ...], asyncio.Task], key: tuple[str, ...], task: asyncio.Task) -> None:
    if tasks.get(key) is task:
        del tasks[key]


async def _run(
    handler: Callable[[RemoteBroadcast], Awaitable[None]],
    broadcast: RemoteBroadcast,
    previous: asyncio.Task | None,
) -> None:
    """Runs `handler` once the handler of the namespace's previous publication finished, so that
    one's teardown comes before this one starts. Cancelled before that, it ends without running
    `handler`, but only once the previous handler finished, whose teardown its cancellation does not
    interrupt."""
    if previous is not None:
        await wait_through_cancellation([previous])
        # Its traceback keeps its handler's locals, and the handler it waited for in turn.
        previous = None
    try:
        await handler(broadcast)
    except Exception:
        logger.exception("handler of %s failed", broadcast.namespace)
