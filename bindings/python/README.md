# moqt (Python bindings)

asyncio bindings for the `moqt` crate. One `Session` type covers raw QUIC and
WebTransport: the URL scheme picks the transport on the client, and a
`Server` accepts both on one port.

## Build

```bash
uv venv && source .venv/bin/activate
uv pip install maturin pytest pytest-asyncio
maturin develop
pytest
```

## Usage

```python
import asyncio
import moqt

async def subscribe():
    session = await moqt.connect("moqt://relay.example:4433")
    reader = await session.subscribe("room/main", "video")
    async for obj in reader:
        print(obj.group_id, obj.object_id, len(obj.payload))

async def serve():
    server = moqt.listen(4433, "cert.pem", "key.pem")
    async for session in server:
        asyncio.create_task(handle(session))

async def handle(session: moqt.Session):
    async for event in session:
        match event:
            case moqt.SubscribeRequest():
                writer = await event.accept()
                await writer.start_group()
                await writer.write(b"hello")
                await writer.finish()
            case moqt.PublishRequest():
                reader = await event.accept()
                async for obj in reader:
                    ...
            case moqt.PublishNamespaceRequest() | moqt.SubscribeNamespaceRequest():
                await event.accept()
            case moqt.Disconnected() | moqt.ProtocolViolation():
                break
```

The `async` methods return `asyncio.Future` objects rather than coroutines, so
schedule them with `asyncio.ensure_future(...)` instead of
`asyncio.create_task(...)` when you need a handle before awaiting.

Every `TrackWriter` group is one subgroup on its own QUIC stream, closed by
`start_group()` / `finish()` with an `EndOfGroup` status. The first group id
defaults to the wall clock in microseconds so a republished track never
reuses a location.
