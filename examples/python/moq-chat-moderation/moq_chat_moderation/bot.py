import argparse
import asyncio
import os

import aiohttp
import moq
from pipecat.pipeline.pipeline import Pipeline
from pipecat.pipeline.worker import PipelineWorker
from pipecat.transports.moq.transport import MOQParams, MOQTransport
from pipecat.workers.runner import WorkerRunner

from moq_chat_moderation.event_timeline import EventTimeline
from moq_chat_moderation.jev import JEV_URL, JevClient
from moq_chat_moderation.moderator import ChatModerator

DEFAULT_RELAY_URL = "https://127.0.0.1:4433"
CHAT_BROADCAST_PATH = "anon/moq-chat-moderation/chat"
MODERATOR_BROADCAST_PATH = "anon/moq-chat-moderation/moderator"
CHAT_TRACK = "chat"
EVENT_TIMELINE_TRACK = "eventtimeline"
CHAT_PAGE_WAIT_SECONDS = 365 * 24 * 60 * 60


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description="Judge MoQ chat messages with Jev")
    parser.add_argument("--relay-url", default=DEFAULT_RELAY_URL)
    parser.add_argument(
        "--insecure",
        action="store_true",
        help="skip TLS verification, for a local relay with a self-signed certificate",
    )
    parser.add_argument("--jev-url", default=JEV_URL)
    return parser.parse_args()


def moderator_broadcast(transport: MOQTransport) -> moq.BroadcastProducer:
    # MOQTransport has no public API for an extra track. It rebuilds this broadcast right
    # before every redial and announces it on connect, so the event timeline follows it
    # from on_connected, before a subscriber's round trip through the relay can reach it.
    return transport._client._publish_broadcast


async def main():
    args = parse_args()
    api_key = os.environ.get("JEV_API_KEY")
    if not api_key:
        raise SystemExit("JEV_API_KEY is not set")

    transport = MOQTransport(
        params=MOQParams(
            relay_url=args.relay_url,
            verify_ssl=not args.insecure,
            request_path=CHAT_BROADCAST_PATH,
            response_path=MODERATOR_BROADCAST_PATH,
            transcript_track=CHAT_TRACK,
            audio_in_enabled=False,
            audio_out_enabled=False,
            connection_timeout=CHAT_PAGE_WAIT_SECONDS,
        )
    )
    timeline = EventTimeline(EVENT_TIMELINE_TRACK)

    @transport.event_handler("on_connected")
    async def on_connected(transport: MOQTransport):
        timeline.publish_on(moderator_broadcast(transport))

    async with aiohttp.ClientSession() as session:
        moderator = ChatModerator(JevClient(session, api_key, url=args.jev_url), timeline)
        worker = PipelineWorker(
            Pipeline([moderator, transport.input()]),
            enable_rtvi=False,
            idle_timeout_secs=None,
        )
        runner = WorkerRunner()
        await runner.add_workers(worker)
        await runner.run()


if __name__ == "__main__":
    asyncio.run(main())
