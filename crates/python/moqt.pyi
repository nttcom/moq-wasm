"""Python bindings for this repository's MoQT implementation (docs/architecture/python)."""

from collections.abc import AsyncIterator

async def connect(url: str, *, verify_certificate: bool = True) -> Session:
    """`moqt://` connects over raw QUIC, `https://` over WebTransport. The session follows the relay's GOAWAY."""

class Session:
    async def subscribe_namespace(self, prefix: list[str]) -> NamespaceEvents:
        """Namespaces published and withdrawn under `prefix`, including those already published."""
    async def subscribe(self, namespace: list[str], name: str, *, current_group: bool = False) -> Track:
        """With `current_group`, the objects of the group the relay holds come first (a joining FETCH),
        then the live ones; without it, only the objects published after the SUBSCRIBE."""
    async def publish_namespace(self, namespace: list[str]) -> None: ...
    async def track(self, namespace: list[str], name: str, *, retention_seconds: float) -> TrackPublication:
        """Registers a track that the session answers SUBSCRIBE and FETCH for. Register it before
        `publish_namespace`, since the relay may forward a SUBSCRIBE at once."""
    async def closed(self) -> None:
        """Waits until the relay closes the session."""
    async def close(self) -> None: ...

class NamespaceEvent:
    namespace: list[str]
    active: bool
    """False once the namespace was withdrawn (PUBLISH_NAMESPACE_DONE)."""

class NamespaceEvents(AsyncIterator[NamespaceEvent]):
    def __aiter__(self) -> NamespaceEvents: ...
    async def __anext__(self) -> NamespaceEvent: ...

class Object:
    group_id: int
    object_id: int
    payload: bytes

class Track(AsyncIterator[Object]):
    def __aiter__(self) -> Track: ...
    async def __anext__(self) -> Object:
        """Raises StopAsyncIteration once the publisher ends the track."""
    async def unsubscribe(self) -> None: ...

class TrackPublication:
    async def write(self, payload: bytes, *, new_group: bool = False) -> None:
        """The object is cached for FETCH whether or not a subscriber takes it."""
