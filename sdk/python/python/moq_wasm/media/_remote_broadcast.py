import asyncio
import json
import logging
import weakref
from collections import deque
from collections.abc import Awaitable, Callable
from functools import partial

from moq_wasm._following import Following
from moq_wasm._native import Object
from moq_wasm._once import retrieve, wait_through_cancellation
from moq_wasm._session import NamespaceWatch, Session
from moq_wasm._subscription import Subscription
from moq_wasm.media._catalog import CATALOG_TRACK_NAME, Catalog
from moq_wasm.media._event_timeline import EventRecord

logger = logging.getLogger("moq_wasm")

RESUBSCRIBE_DELAY_SECONDS = 1.0
DEFAULT_DELIVERY_TIMEOUT_SECONDS = 2.0


class _Owned:
    """What an iteration owns on the relay, kept apart from it so that its finalizer holds nothing
    else: the SUBSCRIBE in flight, kept across a cancelled read for the next read to take its
    answer, and the subscription it took."""

    def __init__(self, session: Session, subscription: Subscription | None = None):
        # The session's loop, which runs the abandonment of an iteration collected on another
        # thread.
        self._loop = session._done.get_loop()
        self.subscribing: asyncio.Future[Subscription] | None = None
        self.subscription = subscription

    def subscribe(self, request: Callable[[], Awaitable[Subscription]]) -> asyncio.Future[Subscription]:
        """The SUBSCRIBE in flight, `request()` started unless one is."""
        if self.subscribing is None:
            self.subscribing = asyncio.ensure_future(request())
            # Its failure goes to the read, and to nobody once the iteration was dropped.
            self.subscribing.add_done_callback(retrieve)
        return self.subscribing

    def take(self) -> Subscription:
        """The answer of the SUBSCRIBE in flight, which is done; raises its failure."""
        assert self.subscribing is not None and self.subscribing.done(), "the SUBSCRIBE was answered"
        subscribing, self.subscribing = self.subscribing, None
        return subscribing.result()

    async def release(self) -> None:
        """Cancels the SUBSCRIBE in flight, which waits for the UNSUBSCRIBE of a track the relay
        accepted, and unsubscribes from the subscription."""
        subscribing, self.subscribing = self.subscribing, None
        if subscribing is not None:
            subscribing.cancel()
            await asyncio.wait([subscribing])
            if not subscribing.cancelled() and subscribing.exception() is None:
                await _unsubscribe(subscribing.result())
        if self.subscription is not None:
            await _unsubscribe(self.subscription)

    def abandon(self) -> None:
        """`release()` without waiting, for an iteration nobody can read any more, on the loop's
        thread: the collection may run in another one."""
        try:
            self._loop.call_soon_threadsafe(self._abandon)
        except RuntimeError:
            # Collected after its event loop closed, as at exit: nothing runs any more.
            pass

    def _abandon(self) -> None:
        if self.subscribing is not None:
            if not self.subscribing.done():
                self.subscribing.cancel()
            elif not self.subscribing.cancelled() and self.subscribing.exception() is None:
                self.subscribing.result().close()
        if self.subscription is not None:
            self.subscription.close()


async def _subscribe(
    session: Session, subscribe: Callable[[list[str], str], Awaitable[Subscription]], namespace: list[str], name: str
) -> Subscription:
    """`subscribe(namespace, name)`. Cancelled, as by closing the iteration it runs for, it raises
    once the relay answered the SUBSCRIBE and what it accepted was unsubscribed, so the close does
    not end before that UNSUBSCRIBE."""
    try:
        return await subscribe(namespace, name)
    except asyncio.CancelledError:
        await wait_through_cancellation(list(session._unsubscribing(namespace, name)))
        raise


class _Objects(Following[Object]):
    """The objects of one subscription, which it owns, until the subscription ends, as by a stream
    failure, which is logged. Closing or dropping it unsubscribes."""

    def __init__(self, subscription: Subscription, name: str, namespace: list[str]):
        super().__init__(f"track {name}")
        self._subscription = subscription
        self._name = name
        self._namespace = namespace
        self._owned = _Owned(subscription._session, subscription)
        weakref.finalize(self, self._owned.abandon)

    async def _next(self) -> Object:
        try:
            return await anext(self._subscription)
        except StopAsyncIteration:
            logger.info("track %s of %s ended", self._name, self._namespace)
            raise
        except RuntimeError as error:
            logger.info("track %s of %s ended: %s", self._name, self._namespace, error)
            raise StopAsyncIteration from None

    async def _release(self) -> None:
        await self._owned.release()


class _Subscriptions(Following[Following[Object]]):
    """What `RemoteBroadcast.subscriptions` returns. Each subscription it yields is the reader's
    from then on, as one the reader keeps past this one being dropped is: taking the next one
    or closing this closes the one yielded before."""

    def __init__(
        self,
        session: Session,
        namespace: list[str],
        name: str,
        on_subscribed: Callable[[], Awaitable[None]] | None,
        from_group_start: bool,
        priority: int | None,
        delivery_timeout: float | None,
    ):
        super().__init__(f"the subscriptions to {name}")
        self._session = session
        self._namespace = namespace
        self._name = name
        self._on_subscribed = on_subscribed
        self._request = partial(
            session.subscribe_current_group if from_group_start else session.subscribe,
            priority=priority,
            delivery_timeout=delivery_timeout,
        )
        self._owned = _Owned(session)
        # The subscription the relay accepted, until it is yielded; yielded, until the next read.
        self._accepted: _Objects | None = None
        self._yielded: _Objects | None = None
        # When the next SUBSCRIBE goes out, after the track ended or its SUBSCRIBE failed.
        self._resubscribe_at: float | None = None
        weakref.finalize(self, self._owned.abandon)

    async def _next(self) -> Following[Object]:
        loop = asyncio.get_running_loop()
        while self._accepted is None:
            if self._yielded is not None:
                # Its close goes on when the read is cancelled, for the next read to wait for.
                closing = self._yielded._start_closing()
                await asyncio.wait([closing])
                self._yielded = None
                self._resubscribe_at = loop.time() + RESUBSCRIBE_DELAY_SECONDS
                closing.result()
            if self._resubscribe_at is not None:
                await asyncio.sleep(self._resubscribe_at - loop.time())
                self._resubscribe_at = None
            subscribing = self._owned.subscribe(
                lambda: _subscribe(self._session, self._request, self._namespace, self._name)
            )
            await asyncio.wait([subscribing])
            try:
                subscription = self._owned.take()
            except RuntimeError as error:
                logger.info("track %s of %s ended: %s", self._name, self._namespace, error)
                self._resubscribe_at = loop.time() + RESUBSCRIBE_DELAY_SECONDS
                continue
            self._accepted = _Objects(subscription, self._name, self._namespace)
            # What it raises is the caller's own: it ends the iteration, whose close unsubscribes
            # the subscription, before the read raises it; a read cancelled during that close, as
            # by a timeout, leaves the close and the error to the next read. The read cancelled
            # while it runs leaves the subscription to the next read, which does not await it again.
            if self._on_subscribed is not None:
                await self._on_subscribed()
        self._yielded, self._accepted = self._accepted, None
        return self._yielded

    async def _release(self) -> None:
        await self._owned.release()
        closings = [objects._start_closing() for objects in (self._accepted, self._yielded) if objects is not None]
        if closings:
            await asyncio.wait(closings)
        for closing in closings:
            closing.result()


class _Follow(Following[Object]):
    """What `RemoteBroadcast.follow` returns: the objects of each subscription `subscriptions`
    yields, one after the other."""

    def __init__(self, subscriptions: _Subscriptions, name: str):
        super().__init__(f"track {name}")
        self._subscriptions = subscriptions
        self._objects: Following[Object] | None = None

    async def _next(self) -> Object:
        while True:
            if self._objects is None:
                self._objects = await anext(self._subscriptions)
                if self._closing.started:
                    # Closed by the `on_subscribed` callback.
                    raise StopAsyncIteration
            try:
                return await anext(self._objects)
            except StopAsyncIteration:
                self._objects = None

    async def _release(self) -> None:
        await self._subscriptions.aclose()


class _EventRecords(Following[EventRecord]):
    """What `RemoteBroadcast.event_timeline` returns: the records of the objects of each
    subscription `subscriptions` yields, skipping those a later subscription's joining FETCH brings
    again."""

    def __init__(self, subscriptions: _Subscriptions, name: str, namespace: list[str]):
        super().__init__(f"the event timeline {name}")
        self._subscriptions = subscriptions
        self._name = name
        self._namespace = namespace
        self._objects: Following[Object] | None = None
        # The location of the newest object read, and that of the newest one read before the
        # current subscription, whose joining FETCH brings it and those before it again.
        self._read_up_to: tuple[int, int] | None = None
        self._resumed_after: tuple[int, int] | None = None
        # The records of an object read, not yielded yet.
        self._records: deque[EventRecord] = deque()

    async def _next(self) -> EventRecord:
        while not self._records:
            if self._objects is None:
                self._objects = await anext(self._subscriptions)
                self._resumed_after = self._read_up_to
            try:
                track_object = await anext(self._objects)
            except StopAsyncIteration:
                self._objects = None
                continue
            location = (track_object.group_id, track_object.object_id)
            if not track_object.payload or (self._resumed_after is not None and location <= self._resumed_after):
                continue
            self._read_up_to = max(location, self._read_up_to or location)
            self._records.extend(_decode_records(track_object.payload, self._name, self._namespace))
        return self._records.popleft()

    async def _release(self) -> None:
        await self._subscriptions.aclose()


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


class _Catalogs(Following[Catalog]):
    """What `RemoteBroadcast.catalogs` returns."""

    def __init__(self, broadcast: "RemoteBroadcast"):
        super().__init__("the catalogs")
        self._broadcast = broadcast
        self._owned = _Owned(broadcast._session)
        weakref.finalize(self, self._owned.abandon)

    async def _next(self) -> Catalog:
        broadcast = self._broadcast
        session = broadcast._session
        if self._owned.subscription is None:
            subscribing = self._owned.subscribe(
                lambda: _subscribe(session, session.subscribe_current_group, broadcast.namespace, CATALOG_TRACK_NAME)
            )
            await asyncio.wait([subscribing])
            self._owned.subscription = self._owned.take()
        while True:
            catalog_object = await anext(self._owned.subscription)
            newest_group = broadcast._newest_catalog_group
            if newest_group is not None and catalog_object.group_id < newest_group:
                logger.debug("catalog of %s in older group %d ignored", broadcast.namespace, catalog_object.group_id)
                continue
            try:
                catalog = Catalog.parse(catalog_object.payload)
            except ValueError as error:
                logger.warning("catalog of %s ignored: %s", broadcast.namespace, error)
                continue
            broadcast._newest_catalog_group = catalog_object.group_id
            return catalog

    async def _release(self) -> None:
        await self._owned.release()


class RemoteBroadcast:
    """A remote publisher's broadcast. In `watch_broadcasts`, withdrawal cancels the handler (ending its
    `catalogs()`, `follow()`, and `event_timeline()` iterators); republishing yields a new `RemoteBroadcast`."""

    def __init__(self, session: Session, namespace: list[str]):
        self.namespace = list(namespace)
        self._session = session
        self._newest_catalog_group: int | None = None

    def catalogs(self) -> Following[Catalog]:
        """Yield parsed MSF catalogs starting from the relay's current group (joining `FETCH`).

        Unparseable catalogs and groups older than the newest catalog yielded by this or an earlier call on the
        broadcast are skipped (draft-ietf-moq-msf-02 §5). Ends with the track; raises `RuntimeError` if
        `SUBSCRIBE` fails. Use with `contextlib.aclosing`.
        """
        return _Catalogs(self)

    def follow(
        self,
        name: str,
        *,
        namespace: list[str] | None = None,
        on_subscribed: Callable[[], Awaitable[None]] | None = None,
        delivery_timeout: float | None = DEFAULT_DELIVERY_TIMEOUT_SECONDS,
    ) -> Following[Object]:
        """Yield objects from track `name` (defaulting to this broadcast's namespace), retrying 1 s after track end or failure.

        - `on_subscribed`: awaited after each accepted `SUBSCRIBE`.
        - `delivery_timeout`: seconds before the relay drops stale groups (`None` for lossless tracks like chat).

        Ends only on `SessionEnded` or an error from `on_subscribed`. Use `subscriptions()` if you need
        to detect subscription boundaries; use with `contextlib.aclosing`.
        """
        return _Follow(self._subscriptions(name, namespace, on_subscribed, False, None, delivery_timeout), name)

    def subscriptions(
        self,
        name: str,
        *,
        namespace: list[str] | None = None,
        on_subscribed: Callable[[], Awaitable[None]] | None = None,
        from_group_start: bool = False,
        delivery_timeout: float | None = DEFAULT_DELIVERY_TIMEOUT_SECONDS,
    ) -> Following[Following[Object]]:
        """Like `follow()`, but yields one inner `Following[Object]` per subscription so stateful readers
        (such as video decoders) can reset when a publisher restarts.

        Advancing to the next subscription or closing unsubscribes the previous one; each yielded inner iterator
        owns its subscription (staying subscribed until closed or dropped even if the outer iterator is dropped).
        Cancelling a read does not end either iterator; cancelling during `on_subscribed` leaves the accepted
        subscription for the next read without re-running `on_subscribed`. Read each inside `contextlib.aclosing`.
        With `from_group_start=True`, joins via `subscribe_current_group` (joining `FETCH`) to start at the
        current group's keyframe and Video Config.
        """
        return self._subscriptions(name, namespace, on_subscribed, from_group_start, None, delivery_timeout)

    def event_timeline(
        self, name: str, *, priority: int | None = None, delivery_timeout: float | None = None
    ) -> Following[EventRecord]:
        """Yield records from an MSF event timeline track (draft-ietf-moq-msf-02 §8), starting via joining `FETCH`.

        Deduplicates records across resubscribes and skips malformed JSON objects.
        `priority` defaults to 128; `delivery_timeout=None` is lossless. Use with `contextlib.aclosing`.
        """
        subscriptions = self._subscriptions(name, None, None, True, priority, delivery_timeout)
        return _EventRecords(subscriptions, name, list(self.namespace))

    def _subscriptions(
        self,
        name: str,
        namespace: list[str] | None,
        on_subscribed: Callable[[], Awaitable[None]] | None,
        from_group_start: bool,
        priority: int | None,
        delivery_timeout: float | None,
    ) -> _Subscriptions:
        namespace = list(self.namespace if namespace is None else namespace)
        return _Subscriptions(
            self._session, namespace, name, on_subscribed, from_group_start, priority, delivery_timeout
        )


async def _unsubscribe(subscription: Subscription) -> None:
    """Waits until the UNSUBSCRIBE was sent, even when the caller is cancelled meanwhile, as a
    replaced track's reader is; then raises the caller's cancellation."""
    closing = subscription.close()
    try:
        await wait_through_cancellation([closing])
    finally:
        failure = closing.exception()
    if isinstance(failure, RuntimeError):
        logger.debug("UNSUBSCRIBE failed: %s", failure)
    elif failure is not None:
        raise failure


async def watch_broadcasts(
    session: Session,
    prefix: list[str],
    handler: Callable[[RemoteBroadcast], Awaitable[None]],
    *,
    filter: Callable[[list[str]], bool] | None = None,
    empty_timeout: float | None = None,
) -> None:
    """Run `handler(RemoteBroadcast)` as a task for each remote namespace under `prefix` accepted by `filter`
    (including namespaces already published before the call, skipping this session's own).

    Cancels a handler on withdrawal, waits for teardown before restarting if republished, and logs handler
    exceptions. Raises `SessionEnded` if the session ends, returns if `empty_timeout` elapses with no active
    broadcasts, and raises `RuntimeError` if `SUBSCRIBE_NAMESPACE` fails.
    """
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
