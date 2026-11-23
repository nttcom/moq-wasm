import asyncio
import base64
import hashlib
import signal
import ssl
import uuid
from contextlib import asynccontextmanager
from urllib.parse import urlsplit

import pytest

import moq_wasm
from moq_wasm import _native
from tests.cancellation import cancel_as_it_resumes
from tests.conftest import REPO_ROOT, started_relay
from tests.relay_sessions import TIMEOUT_SECONDS, TRACK, connect, publisher_with_track, room, within

# How long moqt gives a first connection to open its transport (`DIAL_TIMEOUT` of a `wss://` dial)
# and, once open, to exchange SETUP (`SERVER_SETUP_TIMEOUT`).
CONNECT_DEADLINE_SECONDS = 30.0

# moqt ends a subscription this long after its PUBLISH_DONE (PUBLISH_DONE_GRACE_PERIOD).
PUBLISH_DONE_GRACE_SECONDS = 10
# The relay's default drain timeout bounds how long a client keeps the connection a GOAWAY retired.
DRAIN_SECONDS = 30
# A client notices a killed relay at QUIC's idle timeout (30 s), then redials after 1 s.
RECONNECT_SECONDS = 60


async def test_a_published_namespace_and_its_withdrawal_reach_the_prefix_subscriber(relay_url):
    # Arrange
    prefix = room()
    watcher = await connect(relay_url)
    events = await within(watcher.watch_namespaces(prefix))
    publisher = await connect(relay_url)

    # Act
    await publisher.announce([*prefix, "alice"])
    published = await within(events.__anext__())
    await publisher.close()
    withdrawn = await within(events.__anext__())

    # Assert
    assert (published.namespace, published.active) == ([*prefix, "alice"], True)
    assert (withdrawn.namespace, withdrawn.active) == ([*prefix, "alice"], False)
    await watcher.close()


async def test_an_announce_cancelled_as_the_relay_accepted_it_withdraws_the_namespace(relay_url):
    # Arrange
    prefix = room()
    watcher = await connect(relay_url)
    events = await within(watcher.watch_namespaces(prefix))
    publisher = await connect(relay_url)
    announcing = asyncio.create_task(publisher.announce([*prefix, "alice"]))

    # Act
    await within(cancel_as_it_resumes(announcing))
    published = await within(anext(events))
    withdrawn = await within(anext(events))

    # Assert
    assert (published.active, withdrawn.active) == (True, False)
    await publisher.close()
    await watcher.close()


async def test_a_watch_cancelled_as_the_relay_accepted_it_lets_the_session_watch_its_prefix_again(relay_url):
    # Arrange
    prefix = room()
    watcher = await connect(relay_url)
    watching = asyncio.create_task(watcher.watch_namespaces(prefix))
    await within(cancel_as_it_resumes(watching))
    publisher = await connect(relay_url)

    # Act
    again = await within(watcher.watch_namespaces(prefix))
    await publisher.announce([*prefix, "alice"])

    # Assert
    assert (await within(anext(again))).namespace == [*prefix, "alice"]
    await publisher.close()
    await watcher.close()


async def test_a_subscribe_cancelled_as_the_relay_accepted_it_unsubscribes(relay_url):
    # Arrange
    namespace = [*room(), "alice"]
    publisher, track = await publisher_with_track(relay_url, namespace)
    subscriber = await connect(relay_url)
    subscribing = asyncio.create_task(subscriber.subscribe(namespace, TRACK))

    # Act
    await within(cancel_as_it_resumes(subscribing))

    # Assert: the relay stops taking the publisher's objects once nobody subscribes
    async def taken_by_nobody() -> None:
        while await track.write(b"v", new_group=True) is not None:
            await asyncio.sleep(0.05)

    await within(taken_by_nobody())
    await subscriber.close()
    await publisher.close()


async def test_current_group_brings_the_object_the_relay_already_holds(relay_url):
    # Arrange: a first subscriber makes the relay hold the track before the second one comes
    namespace = [*room(), "alice"]
    publisher, track = await publisher_with_track(relay_url, namespace)
    first = await connect(relay_url)
    first_track = await within(first.subscribe(namespace, TRACK))
    await track.write(b"v1", new_group=True)
    assert (await within(first_track.__anext__())).payload == b"v1"
    late = await connect(relay_url)

    # Act
    late_track = await within(late.subscribe_current_group(namespace, TRACK))
    await track.write(b"v2", new_group=True)

    # Assert
    assert [(await within(late_track.__anext__())).payload for _ in range(2)] == [b"v1", b"v2"]
    for session in (late, first, publisher):
        await session.close()


async def test_a_subscribe_to_a_track_the_publisher_does_not_have_fails(relay_url):
    # Arrange
    namespace = [*room(), "alice"]
    publisher, _ = await publisher_with_track(relay_url, namespace)
    subscriber = await connect(relay_url)

    # Act / Assert
    with pytest.raises(RuntimeError, match="TrackDoesNotExist"):
        await within(subscriber.subscribe(namespace, "unknown"))
    for session in (subscriber, publisher):
        await session.close()


async def test_close_resolves_closed_and_fails_later_calls(relay_url):
    # Arrange
    namespace = [*room(), "alice"]
    session = await _native.connect(relay_url, verify_certificate=False)

    # Act
    await within(session.close())

    # Assert: draft-14 fails writing the control stream; draft-18 fails opening the request's stream
    await within(session.closed())
    for request in (
        session.subscribe(namespace, TRACK),
        session.watch_namespaces(namespace),
        session.announce(namespace),
    ):
        with pytest.raises(RuntimeError, match="connection lost|the session closed"):
            await within(request)


async def test_a_session_offering_draft18_works_with_a_draft18_relay(draft18_relay_url):
    # Arrange
    namespace = [*room(), "alice"]
    session = await moq_wasm.connect(draft18_relay_url, verify_certificate=False, versions=["draft-18"])

    # Act / Assert: the relay accepts the request, so the session speaks its draft
    await within(session.announce(namespace))
    await session.close()


async def test_an_unknown_version_name_is_a_value_error():
    # Act / Assert
    with pytest.raises(ValueError, match="draft-99"):
        await moq_wasm.connect("moqt://127.0.0.1:4433", versions=["draft-99"])


def temporary_app_id() -> str:
    return f"temp-{uuid.uuid4().hex}"


async def test_a_refreshed_token_keeps_the_session_working_under_its_app(relay_url, temporary_app_token):
    # Arrange
    app_id = temporary_app_id()
    session = await _native.connect(
        relay_url, verify_certificate=False, token=temporary_app_token(app_id), refreshes_token=True
    )

    # Act
    await within(session.refresh_token(temporary_app_token(app_id), app_id=app_id))

    # Assert: a request after the refresh is still authorized under the app's root
    await within(session.announce([app_id, "room", "alice"]))
    await session.close()


# draft-14's and draft-18's EXPIRED_AUTH_TOKEN termination code.
EXPIRED_AUTH_TOKEN = 0x18


async def test_a_token_that_expires_without_refresh_token_ends_the_session_at_its_expiry(
    relay_url, temporary_app_token
):
    # Arrange
    app_id = temporary_app_id()
    session = await moq_wasm.connect(relay_url, verify_certificate=False, token=temporary_app_token(app_id, ttl="2s"))
    loop = asyncio.get_running_loop()
    started = loop.time()

    # Act
    with pytest.raises(moq_wasm.ConnectionLost) as lost:
        await within(session.run(asyncio.Event().wait()))

    # Assert: the relay closed the session at the expiry, and the session did not present the token again
    assert lost.value.close_info.code == EXPIRED_AUTH_TOKEN
    assert loop.time() - started < 2 + 3


@asynccontextmanager
async def silent_relay():
    """A `wss://` URL whose server completes the TLS and WebSocket handshakes, then never answers."""
    keys = REPO_ROOT / "services" / "relay" / "keys"
    tls = ssl.create_default_context(ssl.Purpose.CLIENT_AUTH)
    tls.load_cert_chain(keys / "cert.pem", keys / "key.pem")

    async def upgrade(reader: asyncio.StreamReader, writer: asyncio.StreamWriter) -> None:
        request = (await reader.readuntil(b"\r\n\r\n")).decode()
        lines = (line.split(": ", 1) for line in request.split("\r\n")[1:] if ": " in line)
        headers = {name.lower(): value for name, value in lines}
        key = headers["sec-websocket-key"]
        accept = base64.b64encode(hashlib.sha1((key + "258EAFA5-E914-47DA-95CA-C5AB0DC85B11").encode()).digest())
        protocol = headers["sec-websocket-protocol"].split(",")[0].strip()
        writer.write(
            b"HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n"
            + b"Sec-WebSocket-Accept: " + accept + b"\r\nSec-WebSocket-Protocol: " + protocol.encode() + b"\r\n\r\n"
        )
        await writer.drain()
        await reader.read()

    server = await asyncio.start_server(upgrade, "127.0.0.1", 0, ssl=tls)
    try:
        yield f"wss://127.0.0.1:{server.sockets[0].getsockname()[1]}"
    finally:
        server.close()


async def test_a_relay_that_never_sets_the_session_up_fails_the_connection():
    # Arrange
    async with silent_relay() as url:
        # Act / Assert: the server never sends its QMux transport parameters, so one of moqt's 30 s
        # deadlines, the dial's or QMux's idle timeout, whichever fires first, fails the connect
        with pytest.raises(RuntimeError, match="timed out|nothing received"):
            await asyncio.wait_for(
                moq_wasm.connect(url, verify_certificate=False), CONNECT_DEADLINE_SECONDS + TIMEOUT_SECONDS
            )


@asynccontextmanager
async def slow_relay(relay_url: str, delay: float):
    """A `wss://` URL that reaches the relay's WebSocket listener once each connection has waited
    `delay` seconds, as a relay starting from zero behind a slow link takes long to answer."""
    relay_port = urlsplit(relay_url).port

    async def forward(reader: asyncio.StreamReader, writer: asyncio.StreamWriter) -> None:
        try:
            while data := await reader.read(65536):
                writer.write(data)
                await writer.drain()
        finally:
            writer.close()

    async def delay_then_forward(client_reader: asyncio.StreamReader, client_writer: asyncio.StreamWriter) -> None:
        await asyncio.sleep(delay)
        relay_reader, relay_writer = await asyncio.open_connection("127.0.0.1", relay_port)
        await asyncio.gather(forward(client_reader, relay_writer), forward(relay_reader, client_writer))

    server = await asyncio.start_server(delay_then_forward, "127.0.0.1", 0)
    try:
        yield f"wss://127.0.0.1:{server.sockets[0].getsockname()[1]}"
    finally:
        server.close()


async def test_a_wss_relay_that_answers_after_12_s_connects_within_the_connect_deadline(relay_url):
    # Arrange: a relay that takes over 10 s, well within moqt's 30 s dial deadline
    async with slow_relay(relay_url, delay=12.0) as url:
        # Act
        session = await asyncio.wait_for(
            moq_wasm.connect(url, verify_certificate=False), CONNECT_DEADLINE_SECONDS + TIMEOUT_SECONDS
        )

        # Assert
        await within(session.announce([*room(), "alice"]))
        await session.close()


async def test_a_token_of_another_app_is_refused_as_a_refresh(relay_url, temporary_app_token):
    # Arrange
    app_id = temporary_app_id()
    session = await _native.connect(
        relay_url, verify_certificate=False, token=temporary_app_token(app_id), refreshes_token=True
    )

    # Act / Assert
    with pytest.raises(RuntimeError):
        await within(session.refresh_token(temporary_app_token(temporary_app_id()), app_id=app_id))
    await session.close()


async def test_a_session_connected_without_refreshes_token_cannot_refresh_its_token(relay_url, temporary_app_token):
    # Arrange
    app_id = temporary_app_id()
    session = await _native.connect(relay_url, verify_certificate=False, token=temporary_app_token(app_id))

    # Act / Assert
    with pytest.raises(RuntimeError, match="without refreshes_token"):
        session.refresh_token(temporary_app_token(app_id), app_id=app_id)
    await session.close()


async def test_a_session_without_a_token_cannot_refresh_one(relay_url, temporary_app_token):
    # Arrange
    app_id = temporary_app_id()
    session = await _native.connect(relay_url, verify_certificate=False)

    # Act / Assert
    with pytest.raises(RuntimeError, match="without a token"):
        session.refresh_token(temporary_app_token(app_id), app_id=app_id)
    await session.close()


async def test_the_subscription_ends_when_the_publisher_closes_inside_an_open_group(relay_url):
    # Arrange: the group stays open until the next write starts a new one
    namespace = [*room(), "alice"]
    publisher, track = await publisher_with_track(relay_url, namespace)
    subscriber = await connect(relay_url)
    objects = await within(subscriber.subscribe(namespace, TRACK))
    await track.write(b"v1", new_group=True)
    assert (await within(objects.__anext__())).payload == b"v1"

    # Act
    await publisher.close()

    # Assert: the reset of the open group does not raise; PUBLISH_DONE ends the track
    with pytest.raises(StopAsyncIteration):
        await asyncio.wait_for(objects.__anext__(), PUBLISH_DONE_GRACE_SECONDS + TIMEOUT_SECONDS)
    await subscriber.close()


async def test_a_subscriber_after_an_unsubscribe_inside_an_open_group_gets_its_later_objects(relay_url):
    # Arrange: the relay's UNSUBSCRIBE makes the publisher leave the open group
    namespace = [*room(), "alice"]
    publisher, track = await publisher_with_track(relay_url, namespace)
    first = await connect(relay_url)
    first_objects = await within(first.subscribe(namespace, TRACK))
    await track.write(b"v1", new_group=True)
    assert (await within(first_objects.__anext__())).payload == b"v1"
    await first_objects.close()
    with pytest.raises(StopAsyncIteration):
        await within(first_objects.__anext__())
    second = await connect(relay_url)
    second_objects = await within(second.subscribe(namespace, TRACK))

    # Act
    await track.write(b"v2")

    # Assert
    assert (await within(second_objects.__anext__())).payload == b"v2"
    for session in (second, first, publisher):
        await session.close()


async def test_current_group_brings_the_rest_of_the_group_being_appended(relay_url):
    # Arrange
    namespace = [*room(), "alice"]
    publisher, track = await publisher_with_track(relay_url, namespace)
    await track.write(b"v1", new_group=True)
    await track.write(b"v2")
    late = await connect(relay_url)

    # Act
    late_objects = await within(late.subscribe_current_group(namespace, TRACK))
    await track.write(b"v3")

    # Assert
    assert [(await within(late_objects.__anext__())).payload for _ in range(3)] == [b"v1", b"v2", b"v3"]
    for session in (late, publisher):
        await session.close()


async def test_a_subscription_continues_on_the_relay_a_draining_relay_names(relays_sharing_redis):
    # Arrange: the publisher stays on relay B
    relay_a, relay_b = relays_sharing_redis
    namespace = [*room(), "alice"]
    publisher, track = await publisher_with_track(relay_b.url, namespace)
    subscriber = await connect(relay_a.url)
    objects = await within(subscriber.subscribe(namespace, TRACK))
    await track.write(b"v1", new_group=True)
    assert (await within(objects.__anext__())).payload == b"v1"

    # Act: v3 is written once relay A is gone, so only relay B can bring it
    relay_a.process.send_signal(signal.SIGINT)
    await track.write(b"v2", new_group=True)
    await asyncio.wait_for(asyncio.to_thread(relay_a.process.wait), DRAIN_SECONDS)
    await track.write(b"v3", new_group=True)

    # Assert
    assert [(await within(objects.__anext__())).payload for _ in range(2)] == [b"v2", b"v3"]
    for session in (subscriber, publisher):
        await session.close()


async def test_a_session_serves_its_track_again_on_a_restarted_relay(relay_auth):
    # Arrange
    namespace = [*room(), "alice"]
    with started_relay(relay_auth, "relay-restart") as relay:
        publisher, track = await publisher_with_track(relay.url, namespace)
        # A crash: a terminated relay would drain its clients first.
        relay.process.kill()
        relay.process.wait()

    # Act: the publisher's native session reconnects to the relay started on the same port
    with started_relay(relay_auth, "relay-restart", port=relay.port):
        subscriber = await connect(relay.url)
        watch = await within(subscriber.watch_namespaces(namespace[:-1]))
        announced = await asyncio.wait_for(anext(watch), RECONNECT_SECONDS)
        objects = await within(subscriber.subscribe(namespace, TRACK))
        await track.write(b"after restart", new_group=True)

        # Assert
        assert announced.namespace == namespace
        assert (await within(anext(objects))).payload == b"after restart"
        for session in (subscriber, publisher):
            await session.close()
