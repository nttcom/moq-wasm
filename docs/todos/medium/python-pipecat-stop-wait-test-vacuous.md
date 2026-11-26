# [S] Make the failed-start stop test fail when `Room.stop` skips its teardown wait

Next: in `sdk/python/tests/test_pipecat.py:1922` (`test_stopping_a_room_whose_start_failed_waits_until_it_withdrew_its_broadcast`), call `failing.stop` before `starting` returns.

## Goal

Dropping `await self._torn_down()` from `Room.stop` (`sdk/python/python/moq_wasm/pipecat/_room.py`) fails the test.
Today the test passes 20/20 with that line dropped: the cancelled `start` already waits for the rollback in `gather(starting)`, before `stop` runs.
