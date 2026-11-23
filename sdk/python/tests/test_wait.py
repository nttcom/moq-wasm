import asyncio
import gc
import weakref

from moq_wasm._wait import wait


async def test_a_task_that_waited_for_the_first_of_its_futures_is_not_kept_by_the_one_still_pending():
    # Arrange: a future that outlives the wait, as the session's end
    pending = asyncio.get_running_loop().create_future()
    first = asyncio.ensure_future(asyncio.sleep(0))
    waiting = asyncio.create_task(wait([first, pending], return_when=asyncio.FIRST_COMPLETED))
    await waiting
    task = weakref.ref(waiting)

    # Act: once the step that took the task's result, whose handle holds the task, ended
    del waiting
    await asyncio.sleep(0)
    gc.collect()

    # Assert
    assert task() is None
    pending.cancel()


async def test_a_cancelled_wait_leaves_its_task_unkept_by_the_futures_still_pending():
    # Arrange
    pending = asyncio.get_running_loop().create_future()
    waiting = asyncio.create_task(wait([pending]))
    await asyncio.sleep(0)
    waiting.cancel()
    await asyncio.gather(waiting, return_exceptions=True)
    task = weakref.ref(waiting)

    # Act: once the step that took the task's result, whose handle holds the task, ended
    del waiting
    await asyncio.sleep(0)
    gc.collect()

    # Assert
    assert task() is None
    pending.cancel()
