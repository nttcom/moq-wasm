import argparse
import asyncio

import aiohttp
import moq
from pipecat.pipeline.pipeline import Pipeline
from pipecat.pipeline.worker import PipelineWorker
from pipecat.transports.moq.transport import MOQParams, MOQTransport
from pipecat.workers.runner import WorkerRunner

from moq_chat_moderation.event_timeline import EventTimeline
from moq_chat_moderation.identity_token import GcloudIdentityToken
from moq_chat_moderation.jev import JevClient
from moq_chat_moderation.moderator import ChatModerator

DEFAULT_RELAY_URL = "https://127.0.0.1:4433"
CHAT_BROADCAST_PATH = "anon/moq-chat-moderation/chat"
MODERATOR_BROADCAST_PATH = "anon/moq-chat-moderation/moderator"
CHAT_TRACK = "chat"
EVENT_TIMELINE_TRACK = "eventtimeline"
CHAT_PAGE_WAIT_SECONDS = 365 * 24 * 60 * 60
# djev-run scales to zero; a cold start copies 19 GB of weights before it answers.
JEV_REQUEST_TIMEOUT = aiohttp.ClientTimeout(total=15 * 60)


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description="Judge MoQ chat messages with djev-run")
    parser.add_argument("--relay-url", default=DEFAULT_RELAY_URL)
    parser.add_argument(
        "--insecure",
        action="store_true",
        help="skip TLS verification, for a local relay with a self-signed certificate",
    )
    parser.add_argument("--jev-url", required=True, help="a Jev-compatible /v1/systemone endpoint")
    parser.add_argument(
        "--gcloud-auth",
        action="store_true",
        help="send a gcloud identity token, for a Cloud Run service that requires IAM",
    )
    return parser.parse_args()


def moderator_broadcast(transport: MOQTransport) -> moq.BroadcastProducer:
    # MOQTransport has no public API for an extra track. It rebuilds this broadcast right
    # before every redial and announces it on connect, so the event timeline follows it
    # from on_connected, before a subscriber's round trip through the relay can reach it.
    return transport._client._publish_broadcast


async def main():
    args = parse_args()
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

    async with aiohttp.ClientSession(timeout=JEV_REQUEST_TIMEOUT) as session:
        moderator = ChatModerator(
            JevClient(session, args.jev_url, GcloudIdentityToken() if args.gcloud_auth else None),
            timeline,
        )
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
