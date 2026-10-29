from typing import Protocol


class Publication(Protocol):
    async def write(self, payload: bytes, *, new_group: bool = False) -> None: ...
