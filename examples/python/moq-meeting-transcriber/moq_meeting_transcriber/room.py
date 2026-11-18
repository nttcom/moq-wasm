import asyncio
import time
from collections.abc import Awaitable, Callable
from functools import partial

import moq_wasm
from loguru import logger
from moq_wasm import TrackWriter
from moq_wasm.media import EventTimeline, RemoteBroadcast, create_broadcast, watch_broadcasts

from moq_meeting_transcriber import status
from moq_meeting_transcriber.participant import Participant
from moq_meeting_transcriber.stt import Recognizer
from moq_meeting_transcriber.transcript import TRACK_NAME, TranscriptRecord

# Two elements under the room, so the Meeting, which takes {root}/{room}/{user}, shows no participant for it.
BOT_NAMESPACE = ["bots", "transcriber"]
# The transcript is kept for FETCH as long as the bot service lets a room's transcriber run.
TRANSCRIPT_RETENTION_SECONDS = 60 * 60
RECONNECT_DELAY_SECONDS = 2.0
STARTING_NOTICE_AFTER_SECONDS = 2.0


def participant_name(namespace: list[str], room: list[str]) -> str | None:
    """A participant's namespace is the room's with one more element, as the Meeting publishes it."""
    if len(namespace) == len(room) + 1 and namespace[: len(room)] == room:
        return namespace[-1]
    return None


class Room:
    """`connect` opens a session to the relay; `namespace` is the room's, `<app>/<room>`."""

    def __init__(
        self,
        connect: Callable[[], Awaitable[moq_wasm.Session]],
        namespace: list[str],
        recognizer: Recognizer,
        grace_seconds: float,
    ):
        self._connect = connect
        self._namespace = namespace
        self._key = "/".join(namespace)
        self._recognizer = recognizer
        self._grace_seconds = grace_seconds

    async def run(self) -> None:
        """Transcribes the room until it is cancelled or nobody is in it. The session reconnects by
        itself; this retries the first connection. Raises once the room's token cannot be kept fresh,
        or the relay refuses to show the room's participants."""
        while True:
            try:
                session = await self._connect()
            except RuntimeError as error:
                logger.warning(f"room {self._key}: cannot connect to the relay: {error}")
                await asyncio.sleep(RECONNECT_DELAY_SECONDS)
                continue
            logger.info(f"room {self._key}: connected to the relay")
            try:
                await session.run(self._transcribe(session))
                return
            finally:
                await session.close()

    async def _transcribe(self, session: moq_wasm.Session) -> None:
        """Returns once the room has had no participant for the grace period."""
        async with create_broadcast(session, [*self._namespace, *BOT_NAMESPACE]) as bot:
            timeline = bot.event_timeline(TRACK_NAME, retention_seconds=TRANSCRIPT_RETENTION_SECONDS)
            status_track = bot.publish_track(status.TRACK_NAME, retention_seconds=TRANSCRIPT_RETENTION_SECONDS)
            await bot.published()
            await status.write_status(status_track, self._recognizer.name, status.STARTING)
            await self._follow_participants(session, timeline, status_track)

    async def _follow_participants(
        self, session: moq_wasm.Session, timeline: EventTimeline, status_track: TrackWriter
    ) -> None:
        # Audio is subscribed only once the recognizer answers, so a GPU service starting from zero
        # does not receive every utterance of its startup minutes at once.
        ready = asyncio.ensure_future(self._recognizer.ready())
        try:
            done, _ = await asyncio.wait([ready], timeout=STARTING_NOTICE_AFTER_SECONDS)
            if not done:
                notice = f"{self._recognizer.name} を起動しています。書き起こしが始まるまで時間がかかることがあります"
                await timeline.append(TranscriptRecord(int(time.time() * 1000), "transcriber", notice).to_json())
            await ready
        finally:
            ready.cancel()
        logger.info(f"room {self._key}: {self._recognizer.name} is ready")
        await status.write_status(status_track, self._recognizer.name, status.READY)
        await watch_broadcasts(
            session,
            self._namespace,
            partial(self._run_participant, timeline),
            filter=lambda namespace: participant_name(namespace, self._namespace) is not None,
            empty_timeout=self._grace_seconds,
        )

    async def _run_participant(self, timeline: EventTimeline, broadcast: RemoteBroadcast) -> None:
        name = broadcast.namespace[-1]
        logger.info(f"room {self._key}: {name} joined")
        try:
            await Participant(broadcast, self._recognizer, timeline).run()
        except RuntimeError as error:
            logger.info(f"room {self._key}: {name}'s catalog ended: {error}")
        finally:
            logger.info(f"room {self._key}: {name} left")
