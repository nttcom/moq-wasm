import argparse
import asyncio
from functools import partial

import aiohttp
import moqt
from loguru import logger
from moq_bot_rooms.identity_token import IdentityToken
from moq_bot_rooms.rooms import GRACE_SECONDS
from moq_bot_rooms.server import add_serve_arguments, serve_rooms
from moq_system_one_bot.system_one_client import REQUEST_TIMEOUT
from moq_system_one_bot.system_one_status import SystemOneStatus
from moq_system_one_bot.identity_token import add_identity_token_arguments, identity_token_from_arguments
from moq_system_one_bot.namespace import until_room_empty, while_published
from pipecat.pipeline.pipeline import Pipeline
from pipecat.pipeline.worker import PipelineWorker
from pipecat.workers.runner import WorkerRunner

from moq_chat_moderation.chat import ChatMessage
from moq_chat_moderation.system_one import SystemOneClient
from moq_system_one_bot.event_timeline import EventTimeline
from moq_chat_moderation.moderator import ChatModerator

DEFAULT_RELAY_URL = "https://127.0.0.1:4433"
DEFAULT_PORT = 8097
DEFAULT_MAX_ROOMS = 10
NAMESPACE_ROOT = ["anon", "moq-chat-moderation"]
CHAT_TRACK = "chat"
EVENT_TIMELINE_TRACK = "eventtimeline"
STATUS_TRACK = "status"
# The verdicts and the System One status are kept for FETCH as long as a room runs by default
# (--max-room-seconds).
RETENTION_SECONDS = 60 * 60
RESUBSCRIBE_DELAY_SECONDS = 1.0
RECONNECT_DELAY_SECONDS = 2.0


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description="Judge MoQ chat messages with d1-2B, one bot per chat room")
    add_serve_arguments(parser, default_port=DEFAULT_PORT, default_max_rooms=DEFAULT_MAX_ROOMS)
    parser.add_argument("--relay-url", default=DEFAULT_RELAY_URL)
    parser.add_argument(
        "--insecure",
        action="store_true",
        help="skip TLS verification, for a local relay with a self-signed certificate",
    )
    parser.add_argument("--system-one-url", required=True, help="a System One /v1/systemone endpoint, such as system-one-server")
    add_identity_token_arguments(parser)
    return parser.parse_args()


def room_namespace(room_id: str) -> list[str]:
    return [*NAMESPACE_ROOT, room_id]


async def forward_chat(session: moqt.Session, chat_namespace: list[str], worker: PipelineWorker) -> None:
    while True:
        try:
            track = await session.subscribe(chat_namespace, CHAT_TRACK)
            async for chat_object in track:
                try:
                    chat = ChatMessage.parse(chat_object)
                except ValueError as error:
                    logger.warning(f"ignoring an invalid chat object in group {chat_object.group_id}: {error}")
                    continue
                await worker.queue_frame(chat)
            logger.info("chat track ended")
        except RuntimeError as error:
            logger.info(f"chat track ended: {error}")
        # The relay ends the track when the page it subscribed to leaves; another page in the room
        # may still publish the chat namespace, so subscribe again.
        await asyncio.sleep(RESUBSCRIBE_DELAY_SECONDS)


async def moderate_session(
    session: moqt.Session, room_id: str, timeline: EventTimeline, status: SystemOneStatus, worker: PipelineWorker
) -> bool:
    """True once no page has published the room's chat for the grace period; False when the relay session closed."""
    moderator_namespace = [*room_namespace(room_id), "moderator"]
    # Registered before the namespace is published, since the relay may forward a SUBSCRIBE at once.
    for track_name, publisher in ((EVENT_TIMELINE_TRACK, timeline), (STATUS_TRACK, status)):
        await publisher.publish_on(
            await session.track(moderator_namespace, track_name, retention_seconds=RETENTION_SECONDS)
        )
    await session.publish_namespace(moderator_namespace)
    chat_namespace = [*room_namespace(room_id), "chat"]
    following = asyncio.create_task(
        while_published(
            session,
            chat_namespace,
            partial(forward_chat, session, chat_namespace, worker),
            unpublished_seconds=GRACE_SECONDS,
        )
    )
    try:
        return await until_room_empty(session, following)
    finally:
        following.cancel()


async def stay_connected(
    args: argparse.Namespace, room_id: str, timeline: EventTimeline, status: SystemOneStatus, worker: PipelineWorker
) -> None:
    while True:
        try:
            session = await moqt.connect(args.relay_url, verify_certificate=not args.insecure)
        except RuntimeError as error:
            logger.warning(f"room {room_id}: cannot connect to {args.relay_url}: {error}")
            await asyncio.sleep(RECONNECT_DELAY_SECONDS)
            continue
        logger.info(f"room {room_id}: connected to {args.relay_url}")
        try:
            if await moderate_session(session, room_id, timeline, status, worker):
                return
            logger.warning(f"room {room_id}: relay session closed")
        except RuntimeError as error:
            logger.warning(f"room {room_id}: relay session failed: {error}")
        finally:
            await session.close()
        await asyncio.sleep(RECONNECT_DELAY_SECONDS)


async def moderate_room(
    args: argparse.Namespace, http: aiohttp.ClientSession, identity_token: IdentityToken | None, room_id: str
) -> None:
    timeline = EventTimeline(replay=True)
    status = SystemOneStatus()
    system_one = SystemOneClient(http, args.system_one_url, status, identity_token)
    worker = PipelineWorker(
        Pipeline([ChatModerator(system_one, timeline)]),
        enable_rtvi=False,
        idle_timeout_secs=None,
    )
    # The room server owns SIGINT and SIGTERM; it stops every room before it exits.
    runner = WorkerRunner(handle_sigint=False)
    await runner.add_workers(worker)
    async with asyncio.TaskGroup() as tasks:
        tasks.create_task(runner.run())
        await stay_connected(args, room_id, timeline, status, worker)
        await runner.cancel()


async def main():
    args = parse_args()
    async with aiohttp.ClientSession(timeout=REQUEST_TIMEOUT) as http:
        identity_token = identity_token_from_arguments(args, http, args.system_one_url)
        await serve_rooms(args, partial(moderate_room, args, http, identity_token))


if __name__ == "__main__":
    asyncio.run(main())
