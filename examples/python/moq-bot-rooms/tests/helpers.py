import asyncio
from dataclasses import dataclass, field
from datetime import datetime

from aiohttp import web

from moq_bot_rooms.open_hours import OpenHours
from moq_bot_rooms.rooms import Rooms

SHORT_GRACE_SECONDS = 0.05
SETTLE_SECONDS = 0.15
WEEKDAY_HOURS = OpenHours.parse("Mon-Fri 09:00-17:30 +09:00")
# 2026-10-19 is a Monday.
OPEN_MOMENT = datetime.fromisoformat("2026-10-19T10:00:00+09:00")
CLOSED_MOMENT = datetime.fromisoformat("2026-10-19T18:00:00+09:00")


class FakeBots:
    """Bots that run until they are cancelled, or fail at once for a room in `failing`."""

    def __init__(self, failing: set[str] | None = None):
        self.started: list[str] = []
        self.running: set[str] = set()
        self._failing = failing or set()

    async def run(self, room_id: str) -> None:
        self.started.append(room_id)
        if room_id in self._failing:
            raise RuntimeError("bot failed")
        self.running.add(room_id)
        try:
            await asyncio.Event().wait()
        finally:
            self.running.discard(room_id)


class FakeNow:
    def __init__(self, moment: datetime):
        self.moment = moment

    def __call__(self) -> datetime:
        return self.moment


def make_rooms(
    bots: FakeBots,
    *,
    max_rooms: int = 10,
    max_room_seconds: float = 60,
    open_hours: OpenHours | None = None,
    now: FakeNow | None = None,
) -> Rooms:
    return Rooms(
        bots.run,
        max_rooms=max_rooms,
        max_room_seconds=max_room_seconds,
        open_hours=open_hours,
        grace_seconds=SHORT_GRACE_SECONDS,
        now=now or FakeNow(OPEN_MOMENT),
    )


async def settle() -> None:
    await asyncio.sleep(SETTLE_SECONDS)


@dataclass
class FakeMetadataServer:
    url: str = ""
    requests: list[tuple[str | None, str | None]] = field(default_factory=list)

    async def handle(self, request: web.Request) -> web.Response:
        self.requests.append((request.headers.get("Metadata-Flavor"), request.query.get("audience")))
        return web.Response(text="metadata-token")
