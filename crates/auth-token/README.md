# auth-token

HS256 JWT claims, signing, verification, namespace authorization, VTS verification client, and token-minting CLI for MoQT `CLIENT_SETUP`, bot-launch, SIP call, and relay-to-relay tokens.

Used by workspace services (`vts`, `console`, `relay`, `recorder`, `bot-dispatcher`, `sip-gateway`, `observability`, `temporary-apps`), `sdk/rust`, and operators minting tokens outside `anon/**`. Native-only.

## Quickstart: Mint a token from the CLI

Build with the `cli` feature and pass the app secret via `AUTH_TOKEN_SECRET` (never a CLI flag). From a fresh checkout using the development registry (`services/vts/apps.example.json`):

```shell
APP_ID="ac8adbc8-a2ff-4c41-9f5e-fdaed5e1e65e"
AUTH_TOKEN_SECRET="$(jq -r --arg id "$APP_ID" '.[] | select(.appId == $id) | .secret' services/vts/apps.example.json)" \
  cargo run -p auth-token --features cli -- \
    --app-id "$APP_ID" --publish site1/cam1 --subscribe site1 --ttl 12h
```

- `--ttl` accepts whole numbers with `s`, `m`, `h`, or `d` (default `12h`). Relays reject non-relay tokens over 24 h by default.
- Pass `""` for `--publish`, `--subscribe`, `--bots`, or `--calls` to grant access to the entire app; omit a flag to deny that operation.
- For relay-to-relay tokens (`AUTH_RELAY_TOKEN`), also pass `--relay-id <RELAY_ID>`.

## Token payload

```json
{
  "appId": "ac8adbc8-a2ff-4c41-9f5e-fdaed5e1e65e",
  "publish": "site1/cam1",
  "subscribe": "site1",
  "iat": 1759591600,
  "exp": 1759678000
}
```

| Claim | Meaning |
| --- | --- |
| `appId` | Required application ID (`temp-*` for temporary apps) |
| `publish`, `subscribe`, `bots`, `calls` | Optional relative namespace prefix under `appId` (`""` covers the whole app; omit to deny) |
| `iat`, `exp` | Issued-at and expiry timestamps in Unix seconds |
| `relayId` | Relay identity for relay-to-relay tokens (`isRelay: true` apps only) |

## Library usage

### Sign & verify via VTS (recommended)

Enable the `vts-client` feature:

```toml
[dependencies]
auth-token = { git = "https://github.com/nttcom/moq-wasm", rev = "<commit>", features = ["vts-client"] }
```

Inside an `async fn` returning `anyhow::Result<()>` (given `secret: &str`):

```rust
use std::time::{Duration, SystemTime};
use auth_token::{Access, ClaimPolicy, Claims, VtsClient, sign_token, unix_now};

// 1. Sign a token
let claims = Claims::issued(
    "APP".into(),
    Some("site1/cam1".into()),
    Some("site1".into()),
    unix_now(),
    Duration::from_secs(3600),
);
let jwt = sign_token(&claims, secret)?;

// 2. Verify via VTS and enforce lifetime + namespace authorization
let vts = VtsClient::new("http://127.0.0.1:8081/verify".into());
let signed = vts.verify(&jwt).await?;
signed.check(ClaimPolicy::default(), SystemTime::now())?;
assert!(signed.covers(Access::Publish, "APP/site1/cam1"));
```

### Verify locally without VTS

```rust
use std::time::SystemTime;
use auth_token::{Access, ClaimPolicy, Claims, SignedToken, decode_unverified_payload, verify_signature};

let payload = decode_unverified_payload(&jwt).ok_or_else(|| anyhow::anyhow!("malformed token"))?;
verify_signature(&jwt, &app.secret)?;
let claims: Claims = serde_json::from_value(serde_json::Value::Object(payload))?;
let signed = SignedToken { app_id: app.app_id, is_relay: app.is_relay, claims };
signed.check(ClaimPolicy::default(), SystemTime::now())?;
assert!(signed.covers(Access::Publish, "APP/site1/cam1"));
```

## Cargo features

| Feature | Adds |
| --- | --- |
| *(default)* | `Claims`, `SignedToken`, `ClaimPolicy`, `Access`, `sign_token`, `verify_signature`, `decode_unverified_payload`, `temporary_app_id`, `unix_now` |
| `cli` | `auth-token` binary for minting tokens |
| `vts-client` | `VtsClient`, `VtsVerifyError`, and `AuthorizeError` for calling VTS `POST /verify` (`verify`) and checking a claim against a namespace prefix (`authorize`) |

## Security & validation invariants

- **Signature ≠ authorization**: Both `VtsClient::verify` and `verify_signature` check only the HS256 signature. Always call `signed.check(...)` and `signed.covers(...)`, or `VtsClient::authorize`, which runs both and refuses relay tokens.
- **Unverified inspection**: `decode_unverified_payload` and `temporary_app_id` never verify signatures—use them only to look up `appId` before verification.
- **Policy defaults (`ClaimPolicy::default()`)**: Requires `iat` and `exp`, allows `60 s` clock leeway, caps non-relay TTL at `24 h` (`is_relay: true` is exempt), and rejects empty namespace segments (`"/a"`, `"a/"`, `"a//b"`).
- **Boundary matching**: `covers()` matches on `/` boundaries (`"site1"` matches `"APP/site1"` and `"APP/site1/cam1"`, never `"APP/site10"`).
- **VTS client**: Uses a 3 s HTTP timeout, propagates W3C trace context from the active `tracing::Span`, and maps `401` to `VtsVerifyError::Unauthorized` vs. transport/5xx errors to `VtsVerifyError::Unavailable`.

## Further reading

- Architecture: [docs/architecture/auth-token/architecture.md](../../docs/architecture/auth-token/architecture.md)
- Verification service: [services/vts/README.md](../../services/vts/README.md)
