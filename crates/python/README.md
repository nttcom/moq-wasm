# moq-wasm for Python

Send a message through a MoQT relay and read it back in a second session.

## Install

In a Python 3.11+ virtual environment, install the wheel for Linux
(x86_64 or aarch64, glibc 2.28+) or macOS (Apple silicon):

```shell
python -m pip install "moq-wasm==0.0.1" \
  --find-links https://github.com/nttcom/moq-wasm/releases/expanded_assets/python-sdk-v0.0.1
```

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

## Media and bots

- [Media API](https://nttcom.github.io/moq-wasm/docs/python/moq_wasm/media.html) — follow room broadcasts or publish encoded audio and video.
- [Codec helpers](https://nttcom.github.io/moq-wasm/docs/python/moq_wasm/codecs.html) — install `moq-wasm[codecs]` for Opus, H.264 and VP8.
- [Pipecat transport](https://nttcom.github.io/moq-wasm/docs/python/moq_wasm/pipecat.html) — install `moq-wasm[pipecat]` to build a bot. The [voice agent example](https://github.com/nttcom/moq-wasm/tree/master/examples/python/moq-ai-voice-agent) includes a working pipeline and uv configuration.

For source builds and SDK internals, see the [package design](https://github.com/nttcom/moq-wasm/blob/master/docs/architecture/python/sdk.md#package)
and [native type signatures](https://github.com/nttcom/moq-wasm/blob/master/crates/python/python/moq_wasm/_native.pyi).
See the [changelog](https://github.com/nttcom/moq-wasm/blob/master/crates/python/CHANGELOG.md) for release notes.
