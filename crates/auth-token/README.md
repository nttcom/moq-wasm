# auth-token

HS256 JWT claims, signing, verification, and authorization checks for MoQT `CLIENT_SETUP`, bot-launch, and relay tokens (used by VTS, console, relay, `bot-dispatcher`, and `recorder`).

## Token payload

```json
{ "appId": "ac8adbc8-...", "publish": "site1/cam1", "subscribe": "site1", "iat": 1759591600, "exp": 1759678000 }
```

| Claim | Meaning |
| --- | --- |
| `appId` | Application ID (required; `temp-*` for temporary apps) |
| `publish`, `subscribe`, `bots` | Optional relative path prefix under `appId` (`""` covers the whole app; omit to deny) |
| `iat`, `exp` | Issued-at and expiry in Unix seconds |
| `relayId` | Relay identity for relay-to-relay tokens (`isRelay: true` apps only) |

## CLI: Mint a token

Enable the `cli` feature and pass the app's secret in `AUTH_TOKEN_SECRET`:

```shell
AUTH_TOKEN_SECRET="$(jq -r '.[] | select(.appId == "ac8adbc8-...") | .secret' apps.json)" \
  cargo run -p auth-token --features cli -- \
    --app-id ac8adbc8-... --publish site1/cam1 --subscribe site1 --ttl 12h
```

`--ttl` accepts `s`, `m`, `h`, or `d` (e.g. `15s`, `30m`, `12h`, `365d`; relays reject non-relay tokens over 24 h by default). Omit `--publish`, `--subscribe`, or `--bots` to deny that operation. For relay-to-relay tokens (`AUTH_RELAY_TOKEN`), also pass `--relay-id <RELAY_ID>`.

## Library usage

1. **Sign**: Build `Claims::issued(...)` (or `Claims`) and call `sign_token(&claims, secret)`.
2. **Inspect**: `decode_unverified_payload(token)` returns `Option<serde_json::Map<String, Value>>` without checking the signature (use only to look up `appId` before verification).
3. **Authorize**: Never grant access from signature verification alone — obtain a verified `SignedToken`, then enforce lifetime/format with `signed.check(ClaimPolicy::default(), now)` and namespace scope with `signed.covers(Access::..., namespace)`:
   - **Via VTS** (recommended; enable `vts-client`): `let signed = vts_client.verify(token).await?;`
   - **Locally**: look up the app in your registry, call `verify_signature(token, &app.secret)?`, deserialise `Claims` from the payload, and build `SignedToken { app_id: app.app_id, is_relay: app.is_relay, claims }`.

| Feature | Adds |
| --- | --- |
| `cli` | `auth-token` binary for minting tokens from the command line |
| `vts-client` | `VtsClient` HTTP client for calling VTS `POST /verify` |

Design: [architecture.md](../../docs/architecture/auth-token/architecture.md).
