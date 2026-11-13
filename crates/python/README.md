# python

`moq-wasm`, the Python SDK of this repository's MoQT implementation, imported
as `moq_wasm`. The PyO3 module `moq_wasm._native` (`src/`), built with
maturin, binds the `moqt` crate; the package around it is in `python/`. See
[the design](../../docs/architecture/python/sdk.md) and
[`_native.pyi`](python/moq_wasm/_native.pyi).

```python
import moq_wasm

async with moq_wasm.connect("moqt://127.0.0.1:4433", verify_certificate=False) as session:
    async for event in await session.watch_namespaces(["anon", "room"]):
        if event.active:
            catalog = await session.subscribe_current_group(event.namespace, "catalog")
            print((await anext(catalog)).payload)
```

The session dials the relay again when the connection ends, and restores its
tracks, namespaces and watches. `connect` offers draft-18, then draft-14;
`versions=["draft-14"]` names the drafts to offer instead.

## Use from another Python project

```toml
[project]
dependencies = ["moq-wasm"]

[tool.uv.sources]
moq-wasm = { path = "../../../crates/python" }
```

`moq-wasm[pipecat]` adds `moq_wasm.pipecat.MoqWasmTransport` and the PyAV it
encodes and decodes with; the wheel carries no codec of its own.

uv builds the module with maturin, which needs this repository's Rust
toolchain. After an uncommitted change to the `moqt` crate, or in a git
worktree, run `uv sync --reinstall-package moq-wasm`.

## Test

```shell
uv run pytest
```

The tests start a relay and a VTS from this workspace on free ports. The
GOAWAY test also needs Docker for Redis.
