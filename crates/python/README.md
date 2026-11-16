# python

`moq-wasm`, the Python SDK of this repository's MoQT implementation, imported
as `moq_wasm`. The PyO3 module `moq_wasm._native` (`src/`), built with
maturin, binds the `moqt` and `broadcast` crates; the package around it is in
`python/`. See [the design](../../docs/architecture/python/sdk.md) and
[`_native.pyi`](python/moq_wasm/_native.pyi).

It has three layers, each built on the one before:

| Module | Use it for |
| --- | --- |
| `moq_wasm` | A session to a relay and raw MoQT tracks, with the method names of the browser SDK's `MoqSession` |
| `moq_wasm.media` | MSF broadcasts: following participants' catalogs and tracks, publishing encoded media |
| `moq_wasm.pipecat` | `MoqWasmTransport`, in place of pipecat's `MOQTransport` |
| `moq_wasm.codecs` | PyAV's Opus, H.264 and VP8 encoders and decoders, which the transport uses, for a bot that runs its own pipeline |

## Session

```python
import moq_wasm

async with moq_wasm.connect(
    "https://relay.example.com",
    token=token,
    refresh_token=refresh,  # takes the current JWT, returns a later one of the same app
) as session:
    writer = await session.publish_track(["app", "chat"], "messages", retention_seconds=30)
    await writer.write(b"hello", new_group=True)

    async for message in await session.subscribe(["app", "chat"], "messages"):
        print(message.payload)
```

The native session, shared with the browser SDK, follows GOAWAY and replaces
a lost connection for up to 60 s, carrying its subscriptions and namespaces
over. When it cannot, the session ends. It refreshes the token five minutes
before it expires. `session.run(work)` awaits `work` and cancels it once the
session ends, raising what ended it: `ConnectionLost` when the connection
could not be replaced, `TokenExpired` when no refresh came in time,
`SessionClosed` after `close()`.

`connect` takes `moqt://` (raw QUIC) and `https://` (WebTransport) URLs and
offers draft-18, then draft-14; `versions=["draft-14"]` names the drafts to
offer instead.

## Media

Follow every broadcast one level under a room:

```python
from moq_wasm.media import watch_broadcasts

async def on_participant(remote):  # a task per broadcast, cancelled when it is withdrawn
    async for catalog in remote.catalogs():
        audio = catalog.select(role="audio", packaging="loc", codec="opus")
        if audio is not None:
            async for frame in remote.follow(audio.name):
                handle_opus(frame.payload)

await session.run(watch_broadcasts(session, ["app", "room"], on_participant))
```

Publish encoded video and audio with their MSF catalog, as the browser SDK
does:

```python
from moq_wasm.media import create_broadcast

async with create_broadcast(session, ["app", "room", "camera"]) as broadcast:
    media = broadcast.add_media_source(label="camera")  # before the first await
    media.configure_video("avc1.42e01f")  # H.264 Annex B
    media.configure_audio("opus", 48_000, 1)
    for packet in encoded_video():
        media.push_video(packet.data, packet.timestamp_micros)
```

`await media.keyframe_request()` returns when a viewer asks for a new group;
answer it with a keyframe. `broadcast.publish_track` and
`broadcast.event_timeline` add tracks of your own to the broadcast.

## pipecat

```python
from moq_wasm.pipecat import MoqWasmParams, MoqWasmTransport

transport = MoqWasmTransport(
    session,
    MoqWasmParams(
        room=["app", "room"],
        participant_id="bot",
        audio_in_enabled=True,
        audio_out_enabled=True,
        video_in_enabled=True,
        video_out_enabled=True,  # H.264 unless video_out_codec="VP8"
    ),
)
pipeline = Pipeline([transport.input(), stt, llm, tts, transport.output()])
await session.run(PipelineRunner().run(PipelineTask(pipeline)))
```

Every other broadcast one level under `room` is a participant: its audio comes
in as `UserAudioRawFrame` and its video as `UserImageRawFrame`, with the
participant id as `user_id`. The transport has the events of `MOQTransport`.

## Use from another Python project

```toml
[project]
dependencies = ["moq-wasm[pipecat]"]

[tool.uv.sources]
moq-wasm = { path = "../../../crates/python" }
```

The wheel carries no codec of its own. The `codecs` extra adds PyAV for
`moq_wasm.codecs`, and the `pipecat` extra adds pipecat with it; leave both
out when you only use `moq_wasm` and `moq_wasm.media`.

uv builds the module with maturin, which needs this repository's Rust
toolchain. After an uncommitted change to the `moqt` crate, or in a git
worktree, run `uv sync --reinstall-package moq-wasm`.

## Test

```shell
uv run pytest
```

The tests start a relay and a VTS from this workspace on free ports. The
GOAWAY test also needs Docker for Redis.
