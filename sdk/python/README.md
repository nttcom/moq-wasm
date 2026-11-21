# moq-wasm for Python

Send a message through a MoQT relay and read it back in a second session.

## Install

In a Python 3.11+ virtual environment, install the wheel for Linux
(x86_64 or aarch64, glibc 2.28+) or macOS (Apple silicon):

```shell
python -m pip install "moq-wasm==0.0.1" \
  --find-links https://github.com/nttcom/moq-wasm/releases/expanded_assets/python-sdk-v0.0.1
```

This README describes the SDK on `master`. 0.0.1 runs "Send and receive a
message", but the sections after it use what the next release adds:
`moq_wasm.SessionEnded`, `MoqWasmParticipantInput`, and the transport's
`empty_timeout` and `on_room_empty`. Until that release, build the package
from a checkout of this repository, with a Rust toolchain:
`python -m pip install "./sdk/python[pipecat]"`.

## Send and receive a message

Save this as `hello.py`. It uses the public demo relay and picks a fresh
namespace on each run. Subscribe before writing because a live subscription
receives new messages.

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

Run `python hello.py`. It prints `hello` and closes both sessions.

## Keep a service connected

A session replaces a lost connection by itself for up to 60 s, carrying its
subscriptions and published tracks over. When it cannot, or when its token
expires, it ends, and the work running under `session.run` is cancelled. All
the ways a session ends subclass `moq_wasm.SessionEnded`:

| Exception | When |
| --- | --- |
| `ConnectionLost` | the relay connection could not be replaced, as when the token expired without `refresh_token`; `close_info` says how it closed |
| `TokenExpired` | `refresh_token` kept failing until the token expired |
| `SessionClosed` | `close()` was called |

A refused request raises `moq_wasm.RequestError` (a `RuntimeError`) with the
draft's `message_type`, `code` and `reason`; failing to connect raises
`RuntimeError`. A long-running service connects again itself. Its workers
run in an `asyncio.TaskGroup`, which raises a worker's failure inside an
`ExceptionGroup`, so the loop catches with `except*`. It catches every
`Exception`, as a supervisor cannot list every transient failure of its
token endpoint and workers (a `TimeoutError` among them), while
`KeyboardInterrupt` and cancellation, which are no `Exception`, still end it:

```python
import asyncio
import logging

import moq_wasm


async def publish(session: moq_wasm.Session) -> None:
    ...  # publish a broadcast


async def follow(session: moq_wasm.Session) -> None:
    ...  # subscribe, follow broadcasts


async def serve(session: moq_wasm.Session) -> None:
    # Several workers run in a TaskGroup, not asyncio.gather: `session.run` waits for
    # `serve` to clean up, and a TaskGroup waits for every worker's cleanup.
    async with asyncio.TaskGroup() as workers:
        workers.create_task(publish(session))
        workers.create_task(follow(session))


async def fetch_token(current: str | None = None) -> str:
    ...  # a token of your app from your app server; `current` is the one it replaces


async def main():
    moq_wasm.init(log_level="info")  # the native module's logs go to `logging`
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

`refresh_token` is called five minutes before the token's `exp` (halfway
through a shorter lifetime) and retried until the token expires; `connect`
reads `app_id` and `exp` from the token without verifying it, and refuses a
refreshed token of another app.

## A pipecat voice bot

With `moq-wasm[pipecat]`, `MoqWasmTransport` joins a room: every broadcast
one level under `room` is a participant, and the bot publishes its own
broadcast `[*room, participant_id]`, which browser pages play with the
JavaScript SDK.

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
    ...  # new stt, llm and tts pipecat services


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

    # The user aggregator turns the STT's transcriptions into the LLM's context, which an LLM
    # service answers; transcriptions alone it ignores.
    context = LLMContextAggregatorPair(
        LLMContext(), user_params=LLMUserAggregatorParams(vad_analyzer=SileroVADAnalyzer())
    )
    pipeline = Pipeline(
        [transport.input(), stt, context.user(), llm, tts, transport.output(), context.assistant()]
    )
    worker = PipelineWorker(
        pipeline,
        # `on_room_empty` ends the bot; pipecat's idle timeout would end it after 5 quiet minutes.
        idle_timeout_secs=None,
        # A room the relay refuses leaves the transport's processors unusable; pipecat's
        # default policy would keep the pipeline running without it.
        processor_unusable_policy=ProcessorUnusablePolicy.CANCEL,
    )
    await runner.add_workers(worker)
    await runner.run()
```

`make_services` builds the pipecat services of your choice on every call, as a
session that ends cancels them with the pipeline, and `SileroVADAnalyzer`
needs `pipecat-ai[silero]`. Run it as the `serve` above:
`session.run(talk(session, [app_id, "my-room"]))`, where `app_id` is your
token's app (`anon` without a token). Participants' audio arrives as
`UserAudioRawFrame` with the participant id as `user_id`; mix several
speakers into one STT only if it can tell them apart, or run a pipeline per
participant with `MoqWasmParticipantInput`.

## Media and bots

- [Media API](https://nttcom.github.io/moq-wasm/docs/python/moq_wasm/media.html) — follow room broadcasts or publish encoded audio and video.
- [Codec helpers](https://nttcom.github.io/moq-wasm/docs/python/moq_wasm/codecs.html) — install `moq-wasm[codecs]` for Opus, H.264 and VP8.
- [Pipecat transport](https://nttcom.github.io/moq-wasm/docs/python/moq_wasm/pipecat.html) — install `moq-wasm[pipecat]` to build a bot. The [voice agent example](https://github.com/nttcom/moq-wasm/tree/master/examples/python/moq-ai-voice-agent) includes a working pipeline and uv configuration.

For source builds and SDK internals, see the [package design](https://github.com/nttcom/moq-wasm/blob/master/docs/architecture/python/sdk.md#package)
and [native type signatures](https://github.com/nttcom/moq-wasm/blob/master/sdk/python/python/moq_wasm/_native.pyi).
See the [changelog](https://github.com/nttcom/moq-wasm/blob/master/sdk/python/CHANGELOG.md) for release notes.
