import argparse
import asyncio
import re
import signal
from collections.abc import Awaitable, Callable, Mapping

from aiohttp import web
from loguru import logger

from moq_bot_rooms.open_hours import OpenHours
from moq_bot_rooms.rooms import Membership, RoomEnd, Rooms, RunBot

# One namespace element, so a room ID cannot reach outside the bot's namespace.
ROOM_ID_PATTERN = re.compile(r"[a-z0-9-]{1,32}")
HEARTBEAT_SECONDS = 15.0
HOURS_CHECK_SECONDS = 10.0


def add_serve_arguments(parser: argparse.ArgumentParser, *, default_port: int, default_max_rooms: int) -> None:
    parser.add_argument("--host", default="127.0.0.1")
    parser.add_argument("--port", type=int, default=default_port)
    parser.add_argument("--max-rooms", type=int, default=default_max_rooms, help="rooms that may run at once")
    parser.add_argument(
        "--max-room-seconds",
        type=float,
        default=60 * 60,
        help="how long a room may run before its bot leaves",
    )
    parser.add_argument(
        "--open-hours",
        type=OpenHours.parse,
        help="weekly hours to run rooms in, such as 'Mon-Fri 09:00-17:30 +09:00'; always open without it",
    )
    parser.add_argument(
        "--allowed-origin",
        action="append",
        default=[],
        help="a page origin that may request rooms, repeatable; any origin without it",
    )


def room_app(
    rooms: Rooms,
    allowed_origins: list[str],
    only_room: str | None = None,
    heartbeat_seconds: float = HEARTBEAT_SECONDS,
    room_id_pattern: re.Pattern[str] = ROOM_ID_PATTERN,
    bot_from_query: Callable[[Mapping[str, str]], RunBot] | None = None,
) -> web.Application:
    """`GET /rooms/{room}` holds the room while the page keeps the event stream open, which starts
    with a `member` event naming the hold. `POST /rooms/{room}/leave?member=` ends that hold at once.
    `POST /rooms/{room}/stop?member=` ends the room for every page that holds it.

    With `bot_from_query`, the query of the request that starts a room picks the room's bot; it
    raises `ValueError` for a query it refuses, which the page gets as 400.
    """

    async def room_events(request: web.Request) -> web.StreamResponse:
        origin = request.headers.get("Origin")
        # CORS only hides the answer from a page; the bot would start anyway.
        if allowed_origins and origin not in allowed_origins:
            raise web.HTTPForbidden()
        room_id = request.match_info["room"]
        if not room_id_pattern.fullmatch(room_id) or (only_room is not None and room_id != only_room):
            raise web.HTTPNotFound()
        try:
            run_bot = bot_from_query(request.query) if bot_from_query else None
        except ValueError as error:
            raise web.HTTPBadRequest(text=str(error)) from error
        response = web.StreamResponse(
            headers={
                "Content-Type": "text/event-stream",
                "Cache-Control": "no-cache",
                "Access-Control-Allow-Origin": origin if allowed_origins else "*",
            }
        )
        await response.prepare(request)
        membership = rooms.join(room_id, run_bot)
        try:
            await response.write(f"event: member\ndata: {membership.id}\n\n".encode())
            reason = await hold(response, membership, heartbeat_seconds)
            if reason is not None:
                # EventSource drops an event whose data is empty.
                await response.write(f"event: {reason}\ndata: {reason}\n\n".encode())
        except ConnectionResetError:
            pass
        finally:
            membership.leave()
        return response

    def member_action(action: Callable[[str], Awaitable[bool]]):
        async def handle(request: web.Request) -> web.Response:
            if allowed_origins and request.headers.get("Origin") not in allowed_origins:
                raise web.HTTPForbidden()
            if not await action(request.query.get("member", "")):
                raise web.HTTPNotFound()
            return web.Response(status=204)

        return handle

    app = web.Application()
    app.router.add_get("/rooms/{room}", room_events)
    # The page sends these without a body or headers, so the browser needs no CORS preflight.
    app.router.add_post("/rooms/{room}/leave", member_action(rooms.leave))
    app.router.add_post("/rooms/{room}/stop", member_action(rooms.stop))
    return app


async def hold(response: web.StreamResponse, membership: Membership, heartbeat_seconds: float) -> RoomEnd | None:
    """Waits for the room to end. The heartbeat fails to write once the page is gone, which is how
    a closed tab is noticed when no FIN arrives."""
    while True:
        try:
            return await asyncio.wait_for(membership.ended(), heartbeat_seconds)
        except TimeoutError:
            await response.write(b": heartbeat\n\n")


async def serve_rooms(
    args: argparse.Namespace,
    run_bot: RunBot,
    only_room: str | None = None,
    room_id_pattern: re.Pattern[str] = ROOM_ID_PATTERN,
    bot_from_query: Callable[[Mapping[str, str]], RunBot] | None = None,
) -> None:
    """Serves rooms until SIGINT or SIGTERM, then stops every bot before it returns."""
    rooms = Rooms(
        run_bot,
        max_rooms=args.max_rooms,
        max_room_seconds=args.max_room_seconds,
        open_hours=args.open_hours,
    )
    runner = web.AppRunner(
        room_app(rooms, args.allowed_origin, only_room, room_id_pattern=room_id_pattern, bot_from_query=bot_from_query)
    )
    await runner.setup()
    await web.TCPSite(runner, args.host, args.port).start()
    logger.info(f"serving rooms on http://{args.host}:{args.port}")

    stopping = asyncio.Event()
    loop = asyncio.get_running_loop()
    for signum in (signal.SIGINT, signal.SIGTERM):
        loop.add_signal_handler(signum, stopping.set)
    checking_hours = asyncio.create_task(close_outside_hours(rooms))
    try:
        await stopping.wait()
    finally:
        checking_hours.cancel()
        await rooms.shutdown()
        await runner.cleanup()


async def close_outside_hours(rooms: Rooms) -> None:
    while True:
        await asyncio.sleep(HOURS_CHECK_SECONDS)
        rooms.close_outside_hours()
