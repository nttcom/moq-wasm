import asyncio
import secrets
from collections.abc import Awaitable, Callable
from datetime import datetime, timezone
from enum import StrEnum

from loguru import logger

from moq_bot_rooms.open_hours import OpenHours

GRACE_SECONDS = 30.0


class RoomEnd(StrEnum):
    """Why a page's hold on a room ended; the server sends it to the page as the event name."""

    FULL = "full"
    CLOSED = "closed"
    EXPIRED = "expired"
    FAILED = "failed"
    STOPPED = "stopped"


# Runs the bot for a room ID until it is cancelled, or returns once nobody is in the room. The
# server cannot rely on a page's request to tell it the page left: Cloud Run keeps the request to
# the container open after the browser goes away.
RunBot = Callable[[str], Awaitable[None]]


class _Room:
    def __init__(self, room_id: str):
        self.room_id = room_id
        self.members = 0
        # Resolves to None when the room stops without a reason to tell its pages: nobody
        # holds it any more, or the server is shutting down.
        self.ended: asyncio.Future[RoomEnd | None] = asyncio.get_running_loop().create_future()
        self.task: asyncio.Task | None = None
        self.expiry: asyncio.TimerHandle | None = None
        self.removal: asyncio.TimerHandle | None = None


class Membership:
    """A page's hold on a room, from `Rooms.join` until `leave`."""

    def __init__(self, ended: asyncio.Future, on_leave: Callable[[bool], None] = lambda at_once: None):
        # Sent to the page, which passes it to `Rooms.leave` to end this hold.
        self.id = secrets.token_urlsafe(12)
        self._ended = ended
        self._left = asyncio.get_running_loop().create_future()
        self._on_leave = on_leave

    @classmethod
    def refused(cls, reason: RoomEnd | None) -> "Membership":
        ended = asyncio.get_running_loop().create_future()
        ended.set_result(reason)
        return cls(ended)

    async def ended(self) -> RoomEnd | None:
        """Why the room ended; None if it ended without a reason or the page left."""
        await asyncio.wait([self._ended, self._left], return_when=asyncio.FIRST_COMPLETED)
        return self._ended.result() if self._ended.done() else None

    def leave(self, *, at_once: bool = False) -> None:
        """`at_once` skips the grace period, for a page that said it is leaving rather than one
        whose request dropped and may come back."""
        on_leave, self._on_leave = self._on_leave, lambda at_once: None
        if not self._left.done():
            self._left.set_result(None)
        on_leave(at_once)


class Rooms:
    """Runs one bot per room while at least one page holds the room
    (docs/architecture/ai-bots/on-demand-rooms.md)."""

    def __init__(
        self,
        run_bot: RunBot,
        *,
        max_rooms: int,
        max_room_seconds: float,
        open_hours: OpenHours | None = None,
        grace_seconds: float = GRACE_SECONDS,
        now: Callable[[], datetime] = lambda: datetime.now(timezone.utc),
    ):
        self._run_bot = run_bot
        self._max_rooms = max_rooms
        self._max_room_seconds = max_room_seconds
        self._open_hours = open_hours
        self._grace_seconds = grace_seconds
        self._now = now
        self._rooms: dict[str, _Room] = {}
        self._members: dict[str, tuple[Membership, _Room]] = {}
        self._shutting_down = False

    def join(self, room_id: str, run_bot: RunBot | None = None) -> Membership:
        """`run_bot`, when given, runs the room's bot instead of the default one if this join
        starts the room; a join to a running room leaves its bot as it is."""
        if self._shutting_down:
            return Membership.refused(None)
        room = self._rooms.get(room_id)
        if room is None:
            if not self._is_open():
                return Membership.refused(RoomEnd.CLOSED)
            if sum(not other.ended.done() for other in self._rooms.values()) >= self._max_rooms:
                return Membership.refused(RoomEnd.FULL)
            room = self._start(room_id, run_bot or self._run_bot)
        elif room.ended.done():
            # The room ended while pages held it and is remembered until they are gone, so a
            # page that reconnects learns why instead of starting the room again.
            return Membership.refused(room.ended.result())
        room.members += 1
        if room.removal is not None:
            room.removal.cancel()
            room.removal = None
        membership = Membership(room.ended, lambda at_once: self._leave(room, membership.id, at_once))
        self._members[membership.id] = (membership, room)
        return membership

    async def leave(self, member_id: str) -> bool:
        """Ends the hold a page names, and the room at once if it was the last; False for an ID
        no page holds. Returns once the room's bot has left, so the teardown runs inside the
        page's request: Cloud Run gives the instance CPU only while a request is in flight."""
        if member_id not in self._members:
            return False
        membership, room = self._members[member_id]
        membership.leave(at_once=True)
        if room.ended.done():
            await asyncio.wait([room.task])
        return True

    async def stop(self, member_id: str) -> bool:
        """Ends the named hold's room for every page that holds it; False for an ID no page holds.
        Returns once the room's bot has left, and forgets the room so that the next join starts a new
        bot."""
        if member_id not in self._members:
            return False
        _, room = self._members[member_id]
        self._end(room, RoomEnd.STOPPED)
        await asyncio.wait([room.task])
        self._remove(room)
        return True

    def close_outside_hours(self) -> None:
        if self._is_open():
            return
        for room in list(self._rooms.values()):
            self._end(room, RoomEnd.CLOSED)

    async def shutdown(self) -> None:
        self._shutting_down = True
        rooms = list(self._rooms.values())
        for room in rooms:
            self._end(room, None)
        await asyncio.gather(*(room.task for room in rooms), return_exceptions=True)

    def _is_open(self) -> bool:
        return self._open_hours is None or self._open_hours.contains(self._now())

    def _start(self, room_id: str, run_bot: RunBot) -> _Room:
        room = _Room(room_id)
        self._rooms[room_id] = room
        room.task = asyncio.create_task(self._run(room, run_bot))
        room.expiry = asyncio.get_running_loop().call_later(self._max_room_seconds, self._end, room, RoomEnd.EXPIRED)
        logger.info(f"room {room_id}: bot started")
        return room

    async def _run(self, room: _Room, run_bot: RunBot) -> None:
        try:
            await run_bot(room.room_id)
        except asyncio.CancelledError:
            raise
        except Exception:
            logger.exception(f"room {room.room_id}: bot failed")
            self._end(room, RoomEnd.FAILED)
            return
        logger.info(f"room {room.room_id}: nobody is in the room")
        # The requests still held get no reason, so a page that is still there reconnects and starts a new bot.
        self._remove(room)

    def _end(self, room: _Room, reason: RoomEnd | None) -> None:
        if room.ended.done():
            return
        room.ended.set_result(reason)
        room.expiry.cancel()
        if room.task is not asyncio.current_task():
            room.task.cancel()
        logger.info(f"room {room.room_id}: bot stopped" + (f" ({reason})" if reason else ""))

    def _leave(self, room: _Room, member_id: str, at_once: bool) -> None:
        del self._members[member_id]
        room.members -= 1
        if room.members > 0:
            return
        if at_once:
            self._remove(room)
        else:
            room.removal = asyncio.get_running_loop().call_later(self._grace_seconds, self._remove, room)

    def _remove(self, room: _Room) -> None:
        self._end(room, None)
        if self._rooms.get(room.room_id) is room:
            del self._rooms[room.room_id]
