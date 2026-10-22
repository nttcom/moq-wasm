import asyncio

from moq_d3_flash_bot.namespace import until_room_empty, while_published
from tests.helpers import FakeNamespaceSession, FakeRun, settle

CAMERA_NAMESPACE = ["anon", "moq-camera-detection", "room-1", "camera"]
DETECTOR_NAMESPACE = ["anon", "moq-camera-detection", "room-1", "detector"]
SHORT_UNPUBLISHED_SECONDS = 0.1


async def test_run_lasts_while_the_namespace_is_published():
    # Arrange
    session = FakeNamespaceSession()
    run = FakeRun()
    following = asyncio.create_task(while_published(session, CAMERA_NAMESPACE, run))
    session.publish(DETECTOR_NAMESPACE)
    session.publish(CAMERA_NAMESPACE)
    await settle()
    assert (run.starts, run.running) == (1, 1)

    # Act
    session.publish(CAMERA_NAMESPACE, active=False)
    await settle()

    # Assert: the namespace is followed under its parent, and a sibling namespace starts nothing
    assert session.prefixes == [CAMERA_NAMESPACE[:-1]]
    assert (run.starts, run.running) == (1, 0)
    following.cancel()


async def test_run_starts_again_when_the_namespace_is_published_anew():
    # Arrange
    session = FakeNamespaceSession()
    run = FakeRun()
    following = asyncio.create_task(while_published(session, CAMERA_NAMESPACE, run))
    session.publish(CAMERA_NAMESPACE)
    await settle()
    session.publish(CAMERA_NAMESPACE, active=False)
    await settle()

    # Act
    session.publish(CAMERA_NAMESPACE)
    await settle()

    # Assert
    assert (run.starts, run.running) == (2, 1)
    following.cancel()


async def test_cancelling_the_follower_cancels_the_run():
    # Arrange
    session = FakeNamespaceSession()
    run = FakeRun()
    following = asyncio.create_task(while_published(session, CAMERA_NAMESPACE, run))
    session.publish(CAMERA_NAMESPACE)
    await settle()

    # Act
    following.cancel()
    await settle()

    # Assert
    assert run.running == 0


async def test_returns_once_the_namespace_has_gone_unpublished_for_long_enough():
    # Arrange
    session = FakeNamespaceSession()
    run = FakeRun()
    following = asyncio.create_task(
        while_published(session, CAMERA_NAMESPACE, run, unpublished_seconds=SHORT_UNPUBLISHED_SECONDS)
    )
    session.publish(CAMERA_NAMESPACE)
    await settle()
    held = following.done()

    # Act
    session.publish(CAMERA_NAMESPACE, active=False)
    session.publish(DETECTOR_NAMESPACE)
    await asyncio.wait_for(following, 1)

    # Assert: the published namespace holds it, a sibling's event does not extend the wait, and returning stops the run
    assert not held
    assert run.running == 0


async def test_room_is_empty_when_the_pages_namespace_goes_unpublished_first():
    # Arrange
    session = FakeNamespaceSession()
    run = FakeRun()
    presence = asyncio.create_task(
        while_published(session, CAMERA_NAMESPACE, run, unpublished_seconds=SHORT_UNPUBLISHED_SECONDS)
    )

    # Act
    empty = await asyncio.wait_for(until_room_empty(session, presence), 1)

    # Assert: a namespace never published counts from the start
    assert empty
    assert run.starts == 0


async def test_room_is_not_empty_when_the_relay_session_closes_first():
    # Arrange
    session = FakeNamespaceSession()
    presence = asyncio.create_task(while_published(session, CAMERA_NAMESPACE, FakeRun()))
    session.publish(CAMERA_NAMESPACE)

    # Act
    session.close()
    empty = await asyncio.wait_for(until_room_empty(session, presence), 1)

    # Assert
    assert not empty
    presence.cancel()
