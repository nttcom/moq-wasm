import asyncio
import sys
from collections.abc import Collection
from typing import Any


async def wait(
    futures: Collection[asyncio.Future[Any]],
    *,
    timeout: float | None = None,
    return_when: str = asyncio.ALL_COMPLETED,
) -> tuple[set[asyncio.Future[Any]], set[asyncio.Future[Any]]]:
    """`asyncio.wait`, for a wait that may return, or be cancelled, while a future that outlives it
    is still pending, as the session's end. On CPython 3.14.0 to 3.14.6, `asyncio.wait` keeps the
    waiting task referenced from each future still pending then: the task, once ended, would stay
    with the locals its traceback keeps until that future finished. This drops those references,
    as 3.14.7 does."""
    try:
        return await asyncio.wait(futures, timeout=timeout, return_when=return_when)
    finally:
        if sys.version_info >= (3, 14):
            task = asyncio.current_task()
            # A coroutine the event loop runs always runs in a task.
            assert task is not None
            for future in futures:
                asyncio.future_discard_from_awaited_by(future, task)
