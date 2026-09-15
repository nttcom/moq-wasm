import asyncio
import socket
import subprocess

import pytest

import moqt

TIMEOUT_SEC = 5


@pytest.fixture(scope="session")
def self_signed_cert(tmp_path_factory):
    cert_dir = tmp_path_factory.mktemp("certs")
    cert_path = cert_dir / "cert.pem"
    key_path = cert_dir / "key.pem"
    subprocess.run(
        [
            "openssl", "req", "-x509", "-nodes", "-days", "1",
            "-newkey", "ec", "-pkeyopt", "ec_paramgen_curve:prime256v1",
            "-keyout", str(key_path), "-out", str(cert_path),
            "-subj", "/CN=localhost",
            "-addext", "subjectAltName=DNS:localhost,IP:127.0.0.1",
        ],
        check=True,
        capture_output=True,
    )
    return str(cert_path), str(key_path)


def free_udp_port() -> int:
    with socket.socket(socket.AF_INET, socket.SOCK_DGRAM) as sock:
        sock.bind(("127.0.0.1", 0))
        return sock.getsockname()[1]


@pytest.fixture
def server(self_signed_cert):
    port = free_udp_port()
    return port, moqt.listen(port, *self_signed_cert)


def client_url(port: int, transport: str) -> str:
    if transport == "quic":
        return f"moqt://127.0.0.1:{port}"
    return f"https://127.0.0.1:{port}/moq"


async def connect_pair(server, transport: str):
    port, listener = server
    return await asyncio.wait_for(
        asyncio.gather(
            moqt.connect(client_url(port, transport), insecure=True),
            listener.accept(),
        ),
        TIMEOUT_SEC,
    )


async def wait(awaitable):
    return await asyncio.wait_for(awaitable, TIMEOUT_SEC)


@pytest.mark.parametrize("transport", ["quic", "webtransport"])
async def test_subscriber_receives_objects_written_by_server(server, transport):
    # Arrange
    client, server_session = await connect_pair(server, transport)
    subscribe = asyncio.ensure_future(client.subscribe("room/main", "video"))
    request = await wait(server_session.next_event())
    assert isinstance(request, moqt.SubscribeRequest)
    assert (request.namespace, request.name) == ("room/main", "video")

    # Act
    writer = await wait(request.accept(first_group_id=10))
    reader = await wait(subscribe)
    await writer.start_group()
    await writer.write(b"hello", immutable_extensions=[b"ext"])
    await writer.write(b"world")
    await writer.finish()
    first = await wait(reader.next_object())
    second = await wait(reader.next_object())

    # Assert
    assert (first.group_id, first.object_id, first.payload) == (10, 0, b"hello")
    assert first.immutable_extensions == [b"ext"]
    assert (second.group_id, second.object_id, second.payload) == (10, 1, b"world")
    assert second.immutable_extensions == []


async def test_server_reads_track_published_by_client(server):
    # Arrange
    client, server_session = await connect_pair(server, "quic")
    publish = asyncio.ensure_future(client.publish("room/main", "audio", first_group_id=7))
    request = await wait(server_session.next_event())
    assert isinstance(request, moqt.PublishRequest)
    assert (request.namespace, request.name) == ("room/main", "audio")

    # Act
    reader = await wait(request.accept())
    writer = await wait(publish)
    await writer.write_group(b"catalog")
    received = await wait(reader.next_object())

    # Assert
    assert (received.group_id, received.object_id, received.payload) == (7, 0, b"catalog")


async def test_rejected_publish_raises_on_the_publisher(server):
    # Arrange
    client, server_session = await connect_pair(server, "quic")
    publish = asyncio.ensure_future(client.publish("room/main", "audio"))
    request = await wait(server_session.next_event())

    # Act
    await wait(request.reject(1, "not allowed"))

    # Assert
    with pytest.raises(RuntimeError):
        await wait(publish)


async def test_namespace_requests_round_trip(server):
    # Arrange
    client, server_session = await connect_pair(server, "quic")

    # Act
    publish_namespace = asyncio.ensure_future(client.publish_namespace("room/main"))
    request = await wait(server_session.next_event())
    await wait(request.accept())
    await wait(publish_namespace)

    # Assert
    assert isinstance(request, moqt.PublishNamespaceRequest)
    assert request.namespace == "room/main"


async def test_request_cannot_be_answered_twice(server):
    # Arrange
    client, server_session = await connect_pair(server, "quic")
    asyncio.ensure_future(client.subscribe_namespace("room"))
    request = await wait(server_session.next_event())
    await wait(request.accept())

    # Act / Assert
    with pytest.raises(RuntimeError):
        await wait(request.reject(1, "twice"))
