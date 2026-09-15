"""MoQT echo server hosted inside a FastAPI application.

Every object received on a published track is written back to all sessions
that subscribed to the same track name. The HTTP side only reports statistics.
"""

import asyncio
import contextlib
import logging
import os
from dataclasses import dataclass, field

import moqt
from fastapi import FastAPI

MOQT_PORT = int(os.environ.get("MOQT_PORT", "4433"))
MOQT_CERT = os.environ.get("MOQT_CERT", "cert.pem")
MOQT_KEY = os.environ.get("MOQT_KEY", "key.pem")

logging.basicConfig(level=logging.INFO, format="%(levelname)s:     %(message)s")
log = logging.getLogger("echo")

TrackKey = tuple[str, str]


@dataclass
class Subscriber:
    writer: moqt.TrackWriter
    current_group: int | None = None

    async def echo(self, obj: moqt.MoqObject) -> bool:
        """Subscribers join at a group boundary (NextGroupStart), so objects
        from a group whose start they missed are skipped."""
        if self.current_group != obj.group_id:
            if obj.object_id != 0:
                return False
            await self.writer.start_group()
            self.current_group = obj.group_id
        await self.writer.write(obj.payload, immutable_extensions=obj.immutable_extensions)
        return True


@dataclass
class Track:
    objects_received: int = 0
    objects_echoed: int = 0
    subscribers: list[Subscriber] = field(default_factory=list)


class EchoHub:
    def __init__(self) -> None:
        self.sessions = 0
        self.tracks: dict[TrackKey, Track] = {}

    def track(self, key: TrackKey) -> Track:
        return self.tracks.setdefault(key, Track())

    async def serve(self, server: moqt.Server) -> None:
        async for session in server:
            self.sessions += 1
            asyncio.ensure_future(self.handle_session(session))

    async def handle_session(self, session: moqt.Session) -> None:
        try:
            async for event in session:
                match event:
                    case moqt.PublishRequest():
                        key = (event.namespace, event.name)
                        reader = await event.accept()
                        log.info("publish accepted %s", key)
                        asyncio.ensure_future(self.pump(key, reader))
                    case moqt.SubscribeRequest():
                        key = (event.namespace, event.name)
                        writer = await event.accept()
                        self.track(key).subscribers.append(Subscriber(writer))
                        log.info("subscribe accepted %s", key)
                    case moqt.PublishNamespaceRequest() | moqt.SubscribeNamespaceRequest():
                        await event.accept()
                    case moqt.Disconnected() | moqt.ProtocolViolation():
                        break
        finally:
            self.sessions -= 1
            log.info("session ended")

    async def pump(self, key: TrackKey, reader: moqt.TrackReader) -> None:
        track = self.track(key)
        async for obj in reader:
            track.objects_received += 1
            for subscriber in list(track.subscribers):
                try:
                    if await subscriber.echo(obj):
                        track.objects_echoed += 1
                except RuntimeError as error:
                    log.info("dropping subscriber of %s: %s", key, error)
                    track.subscribers.remove(subscriber)
        log.info("track ended %s", key)

    def stats(self) -> dict:
        return {
            "sessions": self.sessions,
            "tracks": {
                f"{namespace}/{name}": {
                    "objects_received": track.objects_received,
                    "objects_echoed": track.objects_echoed,
                    "subscribers": len(track.subscribers),
                }
                for (namespace, name), track in self.tracks.items()
            },
        }


hub = EchoHub()


@contextlib.asynccontextmanager
async def lifespan(_: FastAPI):
    server = moqt.listen(MOQT_PORT, MOQT_CERT, MOQT_KEY)
    log.info("MoQT listening on udp/%d (moqt:// and https://)", MOQT_PORT)
    accept_loop = asyncio.ensure_future(hub.serve(server))
    yield
    accept_loop.cancel()


app = FastAPI(title="MoQT echo server", lifespan=lifespan)


@app.get("/")
def stats() -> dict:
    return hub.stats()
