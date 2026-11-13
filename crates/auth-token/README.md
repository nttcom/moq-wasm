# auth-token

The JWT a MoQT client presents in `CLIENT_SETUP`: its claims, and their HS256
signing and verification. The VTS verifies tokens with it, the console signs
them, and the relay reads the claims the VTS returns.

```json
{ "appId": "ac8adbc8-...", "publish": "site1/cam1", "subscribe": "site1", "iat": 1759591600, "exp": 1759678000 }
```

`decode_unverified_payload` reads a payload without checking the signature.
Only a successful `verify_signature` with the app's secret is an authorisation
decision.

## Signing from the command line

The `cli` feature builds an `auth-token` binary that signs a token with the
secret in `AUTH_TOKEN_SECRET`:

```
AUTH_TOKEN_SECRET="$(jq -r '.[] | select(.appId == "ac8adbc8-...") | .secret' apps.json)" \
  cargo run -p auth-token --features cli -- --app-id ac8adbc8-... --publish site1/cam1 --subscribe site1 --ttl 12h
```

`--ttl` takes `15s`, `30m`, `12h` or `365d`. Omitting `--publish` or
`--subscribe` leaves that operation out of the token. A relay's own token
(`AUTH_RELAY_TOKEN`) also takes `--relay-id` with that relay's `RELAY_ID`.

Design: [architecture document](../../docs/architecture/auth-token/architecture.md).
