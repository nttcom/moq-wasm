import asyncio
from collections.abc import Awaitable, Callable

import moqt


async def while_published(session: moqt.Session, namespace: list[str], run: Callable[[], Awaitable[None]]) -> None:
    """Runs `run` for as long as `namespace` is published, and again each time it is published anew."""
    running: asyncio.Task | None = None
    try:
        async for event in await session.subscribe_namespace(namespace[:-1]):
            if event.namespace != namespace:
                continue
            if running is not None:
                running.cancel()
            running = asyncio.create_task(run()) if event.active else None
    finally:
        if running is not None:
            running.cancel()
