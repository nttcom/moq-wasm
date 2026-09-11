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

Relays started with `AUTH_VTS_URL` require a token in `CLIENT_SETUP`. The
examples obtain an `anon` token from the anon issuer when its URL is given as
a query parameter, e.g. `?anonIssuerUrl=http://127.0.0.1:8080/anon-token`
(the local compose stack started with `docker compose --profile auth up -d`).
Without the parameter no token is sent, which is what relays running with
`AUTH_DISABLED=true` expect. When the parameter is present and the issuer
cannot be reached, connecting fails instead of silently continuing without a
token.

`anon` tokens only grant namespaces whose first element is `anon`, so every
example defaults to namespaces such as `anon/live/test`; keep that prefix when
you type your own against an authenticated relay.
