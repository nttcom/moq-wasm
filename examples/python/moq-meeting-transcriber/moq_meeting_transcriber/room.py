import asyncio
import time
from functools import partial

import moq_wasm
from loguru import logger
from moq_bot_rooms.rooms import GRACE_SECONDS, BotRoom
from moq_wasm import TrackWriter
from moq_wasm.media import EventTimeline, RemoteBroadcast, create_broadcast, watch_broadcasts

from moq_meeting_transcriber import status
from moq_meeting_transcriber.participant import Participant
from moq_meeting_transcriber.stt import Recognizer
from moq_meeting_transcriber.transcript import TRACK_NAME, TranscriptRecord

# Two elements under the room, so the Meeting, which takes {root}/{room}/{user}, shows no participant for it.
BOT_NAMESPACE = ["bots", "transcriber"]
# The transcript is kept for FETCH as long as a room runs by default (--max-room-seconds).
TRANSCRIPT_RETENTION_SECONDS = 60 * 60
RECONNECT_DELAY_SECONDS = 2.0
STARTING_NOTICE_AFTER_SECONDS = 2.0


def room_namespace(room: BotRoom) -> list[str]:
    return [room.namespace_root, room.room_id]


def participant_name(namespace: list[str], room: BotRoom) -> str | None:
    """A participant's namespace is the room's with one more element, as the Meeting publishes it."""
    if len(namespace) == 3 and namespace[:2] == room_namespace(room):
        return namespace[2]
    return None


class Room:
    def __init__(self, relay_url: str, verify_certificate: bool, recognizer: Recognizer, room: BotRoom):
        self._relay_url = relay_url
        self._verify_certificate = verify_certificate
        self._recognizer = recognizer
        self._room = room

    async def run(self) -> None:
        """Transcribes the room until it is cancelled or nobody is in it. The session reconnects by
        itself; this retries the first connection. Raises once the room's token cannot be kept fresh,
        or the relay refuses to show the room's participants."""
        while True:
            try:
                session = await moq_wasm.connect(
                    self._relay_url,
                    verify_certificate=self._verify_certificate,
                    token=self._room.relay_token,
                    refresh_token=self._room.token.refresh if self._room.token else None,
                )
            except RuntimeError as error:
                logger.warning(f"room {self._room.key}: cannot connect to {self._relay_url}: {error}")
                await asyncio.sleep(RECONNECT_DELAY_SECONDS)
                continue
            logger.info(f"room {self._room.key}: connected to {self._relay_url}")
            try:
                await session.run(self._transcribe(session))
                return
            finally:
                await session.close()

    async def _transcribe(self, session: moq_wasm.Session) -> None:
        """Returns once the room has had no participant for the grace period."""
        async with create_broadcast(session, [*room_namespace(self._room), *BOT_NAMESPACE]) as bot:
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
        logger.info(f"room {self._room.key}: {self._recognizer.name} is ready")
        await status.write_status(status_track, self._recognizer.name, status.READY)
        # Returning is safe once the room is empty: a page still open sees its request end and asks for
        # the room again, which starts a new bot.
        await watch_broadcasts(
            session,
            room_namespace(self._room),
            partial(self._run_participant, timeline),
            filter=lambda namespace: participant_name(namespace, self._room) is not None,
            empty_timeout=GRACE_SECONDS,
        )

    async def _run_participant(self, timeline: EventTimeline, broadcast: RemoteBroadcast) -> None:
        name = broadcast.namespace[-1]
        logger.info(f"room {self._room.key}: {name} joined")
        try:
            await Participant(broadcast, self._recognizer, timeline).run()
        except RuntimeError as error:
            logger.info(f"room {self._room.key}: {name}'s catalog ended: {error}")
        finally:
            logger.info(f"room {self._room.key}: {name} left")
