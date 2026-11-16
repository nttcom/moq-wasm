"""Arrange helpers for the tests that run sessions against the relay of `tests.conftest`."""

import asyncio
import uuid

import moq_wasm

TIMEOUT_SECONDS = 10
TRACK = "timeline"


def room() -> list[str]:
    return ["anon", f"room-{uuid.uuid4().hex[:8]}"]


async def within(awaitable):
    return await asyncio.wait_for(awaitable, TIMEOUT_SECONDS)


async def connect(relay_url: str) -> moq_wasm.Session:
    # The relay serves this workspace's self-signed certificate.
    return await moq_wasm.connect(relay_url, verify_certificate=False)


async def publisher_with_track(relay_url: str, namespace: list[str]):
    session = await connect(relay_url)
    # Registered before the namespace, since the relay may forward a SUBSCRIBE at once.
    track = await session.publish_track(namespace, TRACK, retention_seconds=60)
    await session.announce(namespace)
    return session, track
