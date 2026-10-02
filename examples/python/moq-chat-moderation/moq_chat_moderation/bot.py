import argparse
import asyncio
import signal

import aiohttp
import moq
from loguru import logger
from pipecat.pipeline.pipeline import Pipeline
from pipecat.pipeline.worker import PipelineWorker
from pipecat.transports.moq.transport import MOQParams, MOQTransport
from pipecat.workers.runner import WorkerRunner

from moq_chat_moderation.djev_status import DjevStatus
from moq_chat_moderation.event_timeline import EventTimeline
from moq_chat_moderation.identity_token import gcloud_identity_token, metadata_identity_token
from moq_chat_moderation.jev import JevClient
from moq_chat_moderation.moderator import ChatModerator

DEFAULT_RELAY_URL = "https://127.0.0.1:4433"
CHAT_BROADCAST_PATH = "anon/moq-chat-moderation/chat"
MODERATOR_BROADCAST_PATH = "anon/moq-chat-moderation/moderator"
CHAT_TRACK = "chat"
EVENT_TIMELINE_TRACK = "eventtimeline"
STATUS_TRACK = "status"
CHAT_PAGE_WAIT_SECONDS = 365 * 24 * 60 * 60
REBUILD_DELAY_SECONDS = 2.0
# djev-run scales to zero; a cold start copies 19 GB of weights before it answers.
JEV_REQUEST_TIMEOUT = aiohttp.ClientTimeout(total=10 * 60)


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description="Judge MoQ chat messages with djev-run")
    parser.add_argument("--relay-url", default=DEFAULT_RELAY_URL)
    parser.add_argument(
        "--insecure",
        action="store_true",
        help="skip TLS verification, for a local relay with a self-signed certificate",
    )
    parser.add_argument("--jev-url", required=True, help="a Jev-compatible /v1/systemone endpoint")
    auth = parser.add_mutually_exclusive_group()
    auth.add_argument(
        "--gcloud-auth",
        action="store_true",
        help="send a gcloud identity token, for a Cloud Run service that requires IAM",
    )
    auth.add_argument(
        "--metadata-auth",
        action="store_true",
        help="send the GCE metadata server's identity token, for the bot VM",
    )
    return parser.parse_args()


def moderator_broadcast(transport: MOQTransport) -> moq.BroadcastProducer:
    # MOQTransport has no public API for an extra track. It rebuilds this broadcast right
    # before every redial and announces it on connect, so the event timeline follows it
    # from on_connected, before a subscriber's round trip through the relay can reach it.
    return transport._client._publish_broadcast


async def moderate_until_transport_fails(
    args: argparse.Namespace, jev: JevClient, timeline: EventTimeline, status: DjevStatus
) -> bool:
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
    runner = WorkerRunner()
    failed = False

    @transport.event_handler("on_connected")
    async def on_connected(transport: MOQTransport):
        broadcast = moderator_broadcast(transport)
        timeline.publish_on(broadcast)
        status.publish_on(broadcast)

    # MOQTransport stops for good after a failure it does not count as the peer leaving.
    @transport.event_handler("on_error")
    async def on_error(transport: MOQTransport, message: str, error: Exception):
        nonlocal failed
        failed = True
        await runner.cancel(reason=message)

    await runner.add_workers(
        PipelineWorker(
            Pipeline([ChatModerator(jev, timeline), transport.input()]),
            enable_rtvi=False,
            idle_timeout_secs=None,
        )
    )
    await runner.run()
    return failed


async def main():
    args = parse_args()
    timeline = EventTimeline(EVENT_TIMELINE_TRACK)
    status = DjevStatus(STATUS_TRACK)
    async with aiohttp.ClientSession(timeout=JEV_REQUEST_TIMEOUT) as session:
        identity_token = (
            gcloud_identity_token()
            if args.gcloud_auth
            else metadata_identity_token(session, args.jev_url)
            if args.metadata_auth
            else None
        )
        jev = JevClient(session, args.jev_url, status, identity_token)
        while await moderate_until_transport_fails(args, jev, timeline, status):
            # The finished runner leaves its SIGINT handler installed; restore the default one so
            # Ctrl-C still stops the bot until the next runner takes over.
            asyncio.get_running_loop().remove_signal_handler(signal.SIGINT)
            logger.warning(f"MOQ transport stopped; building a new one in {REBUILD_DELAY_SECONDS}s")
            await asyncio.sleep(REBUILD_DELAY_SECONDS)


if __name__ == "__main__":
    asyncio.run(main())
