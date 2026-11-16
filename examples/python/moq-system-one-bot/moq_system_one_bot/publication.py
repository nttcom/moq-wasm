from collections.abc import Awaitable, Callable
from typing import Protocol


class Publication(Protocol):
    """A track the bot writes: a `moq_wasm.TrackWriter`, or a recording in tests."""

    async def write(self, payload: bytes, *, new_group: bool = False) -> None: ...

    def on_connected(self, callback: Callable[[], Awaitable[None]]) -> None: ...
