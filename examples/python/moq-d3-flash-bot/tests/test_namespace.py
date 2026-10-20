import asyncio

from moq_d3_flash_bot.namespace import while_published
from tests.helpers import FakeNamespaceSession, FakeRun, settle

CAMERA_NAMESPACE = ["anon", "moq-camera-detection", "room-1", "camera"]
DETECTOR_NAMESPACE = ["anon", "moq-camera-detection", "room-1", "detector"]


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
