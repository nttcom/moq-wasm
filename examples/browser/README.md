# browser examples

Browser examples that use `bindings/wasm`.

```shell
npm --prefix examples/browser install
npm --prefix examples/browser run wasm
make browser
```

To launch Chrome with certificate pinning for WebTransport:

```shell
make chrome
```

## Connecting to an authenticated relay

Relays always authenticate. A session that presents no token is scoped to
`anon/**`, so every example connects without a token and defaults to
namespaces such as `anon/live/test`; keep that prefix when you type your own.

The call example also accepts an app-scoped JWT via `?jwt=<token>` (minted
with `services/vts/bin/mint.mjs`). Its namespace root then becomes that
token's appId, and a rejected token is reported with the relay's close code
and reason instead of a generic connection error.
