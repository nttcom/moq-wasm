# python

Python bindings for the `moqt` crate, built with PyO3 and maturin into the
module `moqt`. They cover what a bot that follows a room and publishes into it
needs: see [the design](../../docs/architecture/python/architecture.md) and
[`moqt.pyi`](moqt.pyi).

```python
import moqt

session = await moqt.connect("moqt://127.0.0.1:4433", verify_certificate=False)
async for event in await session.subscribe_namespace(["anon", "room"]):
    if event.active:
        catalog = await session.subscribe(event.namespace, "catalog", current_group=True)
        print((await anext(catalog)).payload)
```

`connect` offers draft-18, then draft-14; `versions=["draft-14"]` names the
drafts to offer instead.

## Use from another Python project

```toml
[project]
dependencies = ["moqt"]

[tool.uv.sources]
moqt = { path = "../../../crates/python" }
```

uv builds the module with maturin, which needs this repository's Rust
toolchain. After an uncommitted change to the `moqt` crate, or in a git
worktree, run `uv sync --reinstall-package moqt`.

## Test

```shell
uv run pytest
```

The tests start a relay and a VTS from this workspace on free ports. The
GOAWAY test also needs Docker for Redis.
