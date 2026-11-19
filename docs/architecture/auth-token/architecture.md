# `auth-token` Architecture

## Status
Living document. Update this file in the same change whenever the design intent,
module boundaries, runtime flow, or invariants described here change.

## Scope
`auth-token` is the client JWT: the one payload shape used across the workspace,
its HS256 signing and verification, the checks a service applies to a token the
VTS verified, and a CLI that signs one.

| Module | Responsibility |
| --- | --- |
| `lib.rs` | `Claims`, `Claims::issued`, which sets `iat` and an `exp` a lifetime later, `unix_now`, the current time in the unit of `iat` and `exp`, and `TEMPORARY_APP_ID_PREFIX`, the `appId` prefix of a temporary app (see the VTS architecture). |
| `temporary_app.rs` | `temporary_app_id`: the `appId` of an unexpired temporary app token, read without checking the signature, failing with a `TemporaryAppTokenError`. `temporary-apps` checks the signature first; `live-translate` leaves it to the relay. |
| `jwt.rs` | Compact JWS: `sign_token` (HS256, header fixed to `{"alg":"HS256"}`), `decode_unverified_payload` (payload as a JSON object, no signature check) and `verify_signature`, which fails with a `SignatureError`. HMAC uses `aws-lc-rs`. |
| `signed_token.rs` | `SignedToken`, the VTS `/verify` answer (`appId`, `isRelay`, `claims`). `check` applies a `ClaimPolicy` (default: client tokens live at most 24 hours, clocks may differ by 60 seconds) and requires `iat`, `exp` and well-formed namespace paths, failing with a `ClaimError`; `covers` tells whether the `publish`, `subscribe` or `bots` claim covers a full namespace, element by element. |
| `bin/auth-token/` | The `auth-token` CLI, built only with the `cli` feature: `--app-id`, `--publish`, `--subscribe`, `--bots`, `--ttl` (`15s` / `30m` / `12h` / `365d`, parsed in `lifetime.rs`) and `--relay-id`, signed with the secret in `AUTH_TOKEN_SECRET`. |

The payload claims:

| Claim | Meaning |
| --- | --- |
| `appId` | The registered app whose secret signed the token; also the namespace root |
| `publish` / `subscribe` | Relative namespace path the token may publish / subscribe under; `""` is the whole app root; absent means not allowed |
| `bots` | Relative namespace path under which the token may start, stop and list bots on the bot service (docs/architecture/bots/architecture.md); `""` is the whole app root; absent on client tokens |
| `iat` / `exp` | Unix seconds; judged by `SignedToken::check` with the reader's `ClaimPolicy`, never by the VTS |
| `relayId` | The `RELAY_ID` of the relay presenting a relay app's token; the accepting relay records it as `peer_relay_id`. Absent on client tokens |

Producers and consumers:

- `services/vts` (`verify.rs`) reads the payload with `decode_unverified_payload` and checks
  it with `verify_signature`.
- `services/console` (`apps.rs`) signs `Claims::issued` with `sign_token`; so does the CLI.
- `services/relay` (`modules/auth/token_claims.rs`) and `services/recorder`
  (`api_auth.rs`) deserialise a `SignedToken` from the VTS `/verify` response
  and `check` it, the relay with the `ClaimPolicy` of its configuration; the
  recorder also asks whether a claim `covers` the room. They hold no secret
  and never sign or verify.

## Invariants

- `decode_unverified_payload` never checks the signature. It may not be used as an
  authorisation decision; only a successful `verify_signature` with the app's
  secret is.
- A header whose `alg` is anything other than `HS256` is `InvalidSignature`.
- The CLI takes the secret only from `AUTH_TOKEN_SECRET`, never from an argument,
  which shell history and the process list would keep, and reads no registry,
  so the crate depends on neither `app-registry` nor its storage clients.
- `clap` is an optional dependency behind `cli`; crates that use the library,
  the relay among them, never build it.
- Unknown claims are ignored on read and never produced on write, so the VTS
  can pass the signed payload through unchanged while its readers read only
  the fields above.
- Every reader of a VTS answer judges token times and namespace paths with
  `SignedToken::check`, so a token one service accepts is not refused by
  another for its times.
- The crate holds no secret. Callers pass the secret of the app that signs or
  verifies.
