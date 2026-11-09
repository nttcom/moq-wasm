# auth-token

The JWT a MoQT client presents in `CLIENT_SETUP`: its claims, shared by the
relay (which reads them from the VTS response), and their HS256 signing and
verification, used by the VTS (which verifies them) and the console (which
signs them).

```json
{ "appId": "ac8adbc8-...", "publish": "site1/cam1", "subscribe": "site1", "iat": 1759591600, "exp": 1759678000 }
```

`decode_unverified_payload` reads the payload of a compact JWS without verifying the
signature. Only a successful `verify_signature` with the app's secret is an
authorisation decision.

## Signing from the command line

The `cli` feature builds an `auth-token` binary that signs a token with the
secret in `AUTH_TOKEN_SECRET`, for operators, tests and services that hold an
app's secret. It reads no registry; take the secret from wherever the app's
secret is kept:

```
AUTH_TOKEN_SECRET="$(jq -r '.[] | select(.appId == "ac8adbc8-...") | .secret' apps.json)" \
  cargo run -p auth-token --features cli -- --app-id ac8adbc8-... --publish site1/cam1 --subscribe site1 --ttl 12h
```

`--ttl` takes `15s`, `30m`, `12h` or `365d`; omitting `--publish` or
`--subscribe` leaves that operation out of the token. A relay's own token
(`AUTH_RELAY_TOKEN`) also takes `--relay-id` with that relay's `RELAY_ID`.

## Design

[Architecture document](../../docs/architecture/auth-token/architecture.md).
