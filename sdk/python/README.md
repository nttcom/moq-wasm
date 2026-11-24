# moq-wasm for Python

Async Python SDK for Media over QUIC Transport (MoQT). Connect to a relay over QUIC or WebTransport to exchange raw data tracks, publish or decode media broadcasts, and build Pipecat AI bots interoperable with the [JavaScript](https://github.com/nttcom/moq-wasm/blob/master/sdk/js/js/README.md) and [Rust](https://github.com/nttcom/moq-wasm/blob/master/sdk/rust/README.md) SDKs.

## Choose what to install

| Package              | Module              | Use for                                                                                        |
| -------------------- | ------------------- | ---------------------------------------------------------------------------------------------- |
| `moq-wasm`           | `moq_wasm`, `.media` | Relay sessions, raw tracks, MSF broadcasts, and event timelines                                |
| `moq-wasm[codecs]`   | `moq_wasm.codecs`   | PyAV helpers: Opus encode/decode, video decode (H.264/H.265/VP8/VP9/AV1), video encode (H.264/VP8) |
| `moq-wasm[pipecat]`  | `moq_wasm.pipecat`  | `MoqWasmTransport` and `MoqWasmParticipantInput` for Pipecat bots                              |

## Core concepts

- **Session (`connect`)**: Connects via `moqt://` (QUIC) or `https://` (WebTransport), follows `GOAWAY`, and auto-reconnects for up to 60 s.
- **Namespace & track**: A `namespace` is a list of path segments (e.g. `["anon", "room-1"]`). Public demo namespaces start with `"anon"`; authenticated apps use a JWT whose `appId` is the first segment.
- **Broadcast (MSF)**: A namespace carrying a `catalog` track plus video, audio, or event timeline tracks (`create_broadcast` / `watch_broadcasts`).

## Install

Requires Python 3.11+ on Linux (x86_64/aarch64, glibc 2.28+) or macOS (Apple silicon):

```shell
python -m pip install "moq-wasm==0.0.1" \
  --find-links https://github.com/nttcom/moq-wasm/releases/expanded_assets/python-sdk-v0.0.1
```

Replace `"moq-wasm==0.0.1"` with `"moq-wasm[codecs]==0.0.1"` or `"moq-wasm[pipecat]==0.0.1"` to include an extra.

> **Release note:** This README tracks `master`. `0.0.1` runs the Quickstart below; later sections use unreleased APIs (`SessionEnded`, `MoqWasmParticipantInput`, `empty_timeout`, `on_room_empty`). To install from a repository checkout now (requires a Rust toolchain):
> `python -m pip install "./sdk/python[pipecat]"`

## Quickstart: Send and receive a message

Save as `hello.py` and run `python hello.py`. It connects two sessions to the public demo relay on a fresh namespace and prints `hello` (subscribe before writing because live subscriptions receive objects published afterwards):

```python
import asyncio
from uuid import uuid4

from moq_wasm import connect

RELAY = "https://relay-1.moqt.research.skyway.io:443"


async def main():
    namespace = ["anon", uuid4().hex]
    async with connect(RELAY) as publisher, connect(RELAY) as subscriber:
        writer = await publisher.publish_track(namespace, "messages")
        await publisher.announce(namespace)
        messages = await subscriber.subscribe(namespace, "messages")
        await writer.write(b"hello", new_group=True)
        print((await anext(messages)).payload.decode())


asyncio.run(main())
```

## Keep a service connected

A session automatically replaces a dropped connection for up to 60 s, restoring its subscriptions and published tracks. If reconnection fails or the token expires, the session ends: `session.run(...)` cancels its workload and raises a `moq_wasm.SessionEnded` subclass:

| Exception        | Cause                                                                                  |
| ---------------- | -------------------------------------------------------------------------------------- |
| `ConnectionLost` | Connection could not be restored (including token expiry without `refresh_token`); see `close_info` |
| `TokenExpired`   | `refresh_token` kept failing until the token expired                                   |
| `SessionClosed`  | `close()` was called                                                                   |

Rejected MoQT requests raise `moq_wasm.RequestError` (a `RuntimeError` with `message_type`, `code`, and `reason`); initial connection failures raise `RuntimeError`.

For a long-running service, wrap `connect(...)` in a reconnect loop and run concurrent workers inside an `asyncio.TaskGroup`. Catching `except* Exception` retries transient worker and token-endpoint failures (such as `TimeoutError`), while `KeyboardInterrupt` and cancellation — which are not `Exception` — still stop the service:

```python
import asyncio
import logging

import moq_wasm


async def publish(session: moq_wasm.Session) -> None:
    ...


async def follow(session: moq_wasm.Session) -> None:
    ...


async def serve(session: moq_wasm.Session) -> None:
    # TaskGroup ensures session.run waits for every worker's cleanup (unlike asyncio.gather).
    async with asyncio.TaskGroup() as workers:
        workers.create_task(publish(session))
        workers.create_task(follow(session))


async def fetch_token(current: str | None = None) -> str:
    ...  # fetch a JWT for your app (`current` is the expiring token)


async def main():
    moq_wasm.init(log_level="info")  # forward native logs to Python's `logging`
    while True:
        try:
            token = await asyncio.wait_for(fetch_token(), 30)
            async with moq_wasm.connect(RELAY, token=token, refresh_token=fetch_token) as session:
                await session.run(serve(session))
            return
        except* Exception as errors:
            logging.warning("relay session ended", exc_info=errors)
        await asyncio.sleep(2)
```

Token refresh runs 5 minutes before `exp` (or halfway through shorter lifetimes) and retries until expiry. `connect` reads `appId` and `exp` from the JWT without verifying the signature and rejects a refreshed token from a different `appId`.

## Build a Pipecat voice bot

With `moq-wasm[pipecat]`, `MoqWasmTransport` joins a room where every direct child namespace under `room` is a participant, and publishes the bot at `[*room, participant_id]` (playable from browsers via the JS SDK):

```python
import moq_wasm
from moq_wasm.pipecat import MoqWasmParams, MoqWasmTransport
from pipecat.audio.vad.silero import SileroVADAnalyzer
from pipecat.pipeline.pipeline import Pipeline
from pipecat.pipeline.worker import PipelineWorker, ProcessorUnusablePolicy
from pipecat.processors.aggregators.llm_context import LLMContext
from pipecat.processors.aggregators.llm_response_universal import LLMContextAggregatorPair, LLMUserAggregatorParams
from pipecat.workers.runner import WorkerRunner


def make_services():
    ...  # create fresh STT, LLM, and TTS services per session


async def talk(session: moq_wasm.Session, room: list[str]) -> None:
    stt, llm, tts = make_services()
    transport = MoqWasmTransport(
        session,
        MoqWasmParams(
            room=room,
            participant_id="bot",
            audio_in_enabled=True,
            audio_out_enabled=True,
            empty_timeout=30,
        ),
    )
    runner = WorkerRunner(handle_sigint=False)

    @transport.event_handler("on_room_empty")
    async def on_room_empty(transport):
        await runner.cancel()

    # Converts STT transcriptions into LLM context frames.
    context = LLMContextAggregatorPair(
        LLMContext(), user_params=LLMUserAggregatorParams(vad_analyzer=SileroVADAnalyzer())
    )
    pipeline = Pipeline(
        [transport.input(), stt, context.user(), llm, tts, transport.output(), context.assistant()]
    )
    worker = PipelineWorker(
        pipeline,
        # Exit via `on_room_empty` instead of Pipecat's 5-minute idle timeout.
        idle_timeout_secs=None,
        # Stop the pipeline if the relay rejects the room.
        processor_unusable_policy=ProcessorUnusablePolicy.CANCEL,
    )
    await runner.add_workers(worker)
    await runner.run()
```

Notes:

- Run `talk` inside the supervisor loop above as `session.run(talk(session, [app_id, "my-room"]))` (`app_id` is `"anon"` without a token).
- Build fresh services in `make_services()` on every session (ending a session cancels the pipeline). `SileroVADAnalyzer` requires `pipecat-ai[silero]`.
- Participant audio arrives as `UserAudioRawFrame` tagged with `user_id`. In multi-speaker rooms where STT cannot separate speakers, run one pipeline per participant with `MoqWasmParticipantInput`.

## Reference & examples

- **API docs**: [Media](https://nttcom.github.io/moq-wasm/docs/python/moq_wasm/media.html) · [Codecs](https://nttcom.github.io/moq-wasm/docs/python/moq_wasm/codecs.html) · [Pipecat](https://nttcom.github.io/moq-wasm/docs/python/moq_wasm/pipecat.html)
- **Full example**: [AI voice agent](https://github.com/nttcom/moq-wasm/tree/master/examples/python/moq-ai-voice-agent) (working pipeline + `uv` setup)
- **Internals & releases**: [Package design](https://github.com/nttcom/moq-wasm/blob/master/docs/architecture/python/sdk.md#package) · [Native stubs](https://github.com/nttcom/moq-wasm/blob/master/sdk/python/python/moq_wasm/_native.pyi) · [Changelog](https://github.com/nttcom/moq-wasm/blob/master/sdk/python/CHANGELOG.md)
