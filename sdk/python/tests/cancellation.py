"""Arrange helpers that cancel a task at the moment a timeout's cancellation races its result."""

import asyncio


async def read(iterator):
    """`anext(iterator)` as a coroutine, for a native iterator whose `__anext__` is a future."""
    return await anext(iterator)


def cancel_as_it_resumes(task: asyncio.Task) -> asyncio.Future[None]:
    """Cancels `task` once the future it awaits resolved, before the task resumes with the result,
    as a timeout that expires then does. Resolves once it cancelled the task."""
    loop = asyncio.get_running_loop()
    cancelled = loop.create_future()

    def poll() -> None:
        # The task's wakeup is scheduled when its future resolves, and runs after this check
        # because the check is always scheduled before it.
        waiting_on = task._fut_waiter  # type: ignore[attr-defined]
        if task.done():
            cancelled.set_exception(AssertionError("the task finished before it was cancelled"))
        elif waiting_on is not None and waiting_on.done():
            task.cancel()
            cancelled.set_result(None)
        else:
            loop.call_soon(poll)

    loop.call_soon(poll)
    return cancelled
