import argparse
import asyncio
from functools import partial

import aiohttp
import moq_wasm
from loguru import logger
from moq_bot_rooms.identity_token import IdentityToken, add_identity_token_arguments, identity_token_from_arguments
from moq_bot_rooms.relay import add_relay_arguments
from moq_bot_rooms.rooms import BotRoom
from moq_bot_rooms.server import add_serve_arguments, serve_rooms
from moq_system_one_bot.system_one_client import REQUEST_TIMEOUT
from moq_system_one_bot.system_one_status import SystemOneStatus
from moq_bot_rooms.rooms import GRACE_SECONDS
from moq_system_one_bot.relay import stay_connected
from moq_wasm.media import RemoteBroadcast, create_broadcast, watch_broadcasts
from pipecat.pipeline.pipeline import Pipeline
from pipecat.pipeline.worker import PipelineWorker
from pipecat.workers.runner import WorkerRunner

from moq_chat_moderation.chat import ChatMessage
from moq_chat_moderation.system_one import SystemOneClient
from moq_chat_moderation.moderator import ChatModerator

DEFAULT_PORT = 8097
DEFAULT_MAX_ROOMS = 10
DEMO_NAME = "moq-chat-moderation"
CHAT_TRACK = "chat"
EVENT_TIMELINE_TRACK = "eventtimeline"
STATUS_TRACK = "status"
# The verdicts and the System One status are kept for FETCH as long as a room runs by default
# (--max-room-seconds).
RETENTION_SECONDS = 60 * 60


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description="Judge MoQ chat messages with d1-2B, one bot per chat room")
    add_serve_arguments(
        parser, bot="moq-chat-moderation", default_port=DEFAULT_PORT, default_max_rooms=DEFAULT_MAX_ROOMS
    )
    add_relay_arguments(parser)
    parser.add_argument("--system-one-url", required=True, help="a System One /v1/systemone endpoint, such as system-one-server")
    add_identity_token_arguments(parser)
    return parser.parse_args()


def room_namespace(room: BotRoom) -> list[str]:
    return [room.namespace_root, DEMO_NAME, room.room_id]


async def forward_chat(worker: PipelineWorker, chat: RemoteBroadcast) -> None:
    async for chat_object in chat.follow(CHAT_TRACK):
        try:
            message = ChatMessage.parse(chat_object)
        except ValueError as error:
            logger.warning(f"ignoring an invalid chat object in group {chat_object.group_id}: {error}")
            continue
        await worker.queue_frame(message)


async def moderate_session(
    args: argparse.Namespace,
    http: aiohttp.ClientSession,
    identity_token: IdentityToken | None,
    room: BotRoom,
    session: moq_wasm.Session,
) -> None:
    """Returns once no page has published the room's chat for the grace period."""
    namespace = room_namespace(room)
    chat_namespace = [*namespace, "chat"]
    async with create_broadcast(session, [*namespace, "moderator"]) as moderator:
        timeline = moderator.event_timeline(EVENT_TIMELINE_TRACK, retention_seconds=RETENTION_SECONDS)
        status_track = moderator.publish_track(STATUS_TRACK, retention_seconds=RETENTION_SECONDS)
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
        await moderator.published()
        await status.publish_on(status_track)
        async with asyncio.TaskGroup() as tasks:
            tasks.create_task(runner.run())
            await watch_broadcasts(
                session,
                namespace,
                partial(forward_chat, worker),
                filter=lambda published: published == chat_namespace,
                empty_timeout=GRACE_SECONDS,
            )
            await runner.cancel()


async def moderate_room(
    args: argparse.Namespace, http: aiohttp.ClientSession, identity_token: IdentityToken | None, room: BotRoom
) -> None:
    await stay_connected(args, room, partial(moderate_session, args, http, identity_token, room))


async def main():
    args = parse_args()
    async with aiohttp.ClientSession(timeout=REQUEST_TIMEOUT) as http:
        identity_token = identity_token_from_arguments(args, http, args.system_one_url)
        await serve_rooms(args, partial(moderate_room, args, http, identity_token))


if __name__ == "__main__":
    asyncio.run(main())
