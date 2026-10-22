import asyncio
from collections.abc import Awaitable, Callable

import moqt


async def while_published(
    session: moqt.Session,
    namespace: list[str],
    run: Callable[[], Awaitable[None]],
    *,
    unpublished_seconds: float | None = None,
) -> None:
    """Runs `run` for as long as `namespace` is published, and again each time it is published anew.
    With `unpublished_seconds`, returns once the namespace has gone that long unpublished, from the
    start or since it was withdrawn."""
    loop = asyncio.get_running_loop()
    running: asyncio.Task | None = None
    unpublished_until = None if unpublished_seconds is None else loop.time() + unpublished_seconds
    events = aiter(await session.subscribe_namespace(namespace[:-1]))
    try:
        while True:
            try:
                async with asyncio.timeout_at(None if running else unpublished_until):
                    event = await anext(events)
            except (TimeoutError, StopAsyncIteration):
                return
            if event.namespace != namespace:
                continue
            if running is not None:
                running.cancel()
            running = asyncio.create_task(run()) if event.active else None
            if running is None and unpublished_seconds is not None:
                unpublished_until = loop.time() + unpublished_seconds
    finally:
        if running is not None:
            running.cancel()


async def until_room_empty(session: moqt.Session, presence: asyncio.Task) -> bool:
    """Waits until `presence` returns or the relay session closes. `presence` is a `while_published`
    task with `unpublished_seconds` on the namespace the room's pages publish. True if `presence`
    returned first (nobody is in the room), False if the session closed first; re-raises an
    exception from `presence`."""
    closed = asyncio.ensure_future(session.closed())
    try:
        await asyncio.wait([presence, closed], return_when=asyncio.FIRST_COMPLETED)
    finally:
        closed.cancel()
    if not presence.done():
        return False
    presence.result()
    return True
