import asyncio

import moqt
from loguru import logger

from moq_meeting_transcriber.participant import Participant
from moq_meeting_transcriber.stt import SharedWhisper
from moq_meeting_transcriber.transcript import TRACK_NAME, TranscriptTimeline

# The anonymous app's namespace root; a room under another app's root needs a token the bot lacks.
NAMESPACE_ROOT = "anon"
# Two elements under the room, so the Meeting, which takes {root}/{room}/{user}, shows no participant for it.
BOT_NAMESPACE = ["bots", "transcriber"]
# The transcript is kept for FETCH as long as a room runs by default (--max-room-seconds).
TRANSCRIPT_RETENTION_SECONDS = 60 * 60
RECONNECT_DELAY_SECONDS = 2.0


def room_namespace(room_id: str) -> list[str]:
    return [NAMESPACE_ROOT, room_id]


def participant_name(namespace: list[str], room_id: str) -> str | None:
    """A participant's namespace is the room's with one more element, as the Meeting publishes it."""
    if len(namespace) == 3 and namespace[:2] == room_namespace(room_id):
        return namespace[2]
    return None


class Room:
    def __init__(self, relay_url: str, verify_certificate: bool, whisper: SharedWhisper, room_id: str):
        self._relay_url = relay_url
        self._verify_certificate = verify_certificate
        self._whisper = whisper
        self._room_id = room_id

    async def run(self) -> None:
        """Transcribes the room until it is cancelled, reconnecting when the relay session ends."""
        while True:
            try:
                session = await moqt.connect(self._relay_url, verify_certificate=self._verify_certificate)
            except RuntimeError as error:
                logger.warning(f"room {self._room_id}: cannot connect to {self._relay_url}: {error}")
                await asyncio.sleep(RECONNECT_DELAY_SECONDS)
                continue
            logger.info(f"room {self._room_id}: connected to {self._relay_url}")
            try:
                await self._transcribe(session)
            finally:
                await session.close()
            logger.warning(f"room {self._room_id}: relay session closed")
            await asyncio.sleep(RECONNECT_DELAY_SECONDS)

    async def _transcribe(self, session: moqt.Session) -> None:
        bot_namespace = [*room_namespace(self._room_id), *BOT_NAMESPACE]
        # Registered before the namespace is published, since the relay may forward a SUBSCRIBE at once.
        publication = await session.track(bot_namespace, TRACK_NAME, retention_seconds=TRANSCRIPT_RETENTION_SECONDS)
        timeline = TranscriptTimeline(publication)
        await timeline.open()
        await session.publish_namespace(bot_namespace)
        following = asyncio.create_task(self._follow_participants(session, timeline))
        # The binding's methods return futures, not coroutines.
        closed = asyncio.ensure_future(session.closed())
        try:
            await asyncio.wait([following, closed], return_when=asyncio.FIRST_COMPLETED)
            if following.done():
                # Raises what stopped following the participants, so the room fails instead of going quiet.
                following.result()
        finally:
            following.cancel()
            closed.cancel()

    async def _follow_participants(self, session: moqt.Session, timeline: TranscriptTimeline) -> None:
        participants: dict[str, asyncio.Task] = {}
        try:
            async for event in await session.subscribe_namespace(room_namespace(self._room_id)):
                name = participant_name(event.namespace, self._room_id)
                if name is None:
                    continue
                if name in participants:
                    participants.pop(name).cancel()
                if event.active:
                    participant = Participant(session, event.namespace, self._whisper, timeline)
                    participants[name] = asyncio.create_task(self._run_participant(name, participant))
                else:
                    logger.info(f"room {self._room_id}: {name} left")
        finally:
            for task in participants.values():
                task.cancel()

    async def _run_participant(self, name: str, participant: Participant) -> None:
        logger.info(f"room {self._room_id}: {name} joined")
        try:
            await participant.run()
        except RuntimeError as error:
            logger.info(f"room {self._room_id}: {name}'s catalog ended: {error}")
