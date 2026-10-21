# `auth-token` Architecture

## Status
Living document. Update this file in the same change whenever the design intent,
module boundaries, runtime flow, or invariants described here change.

## Scope
`auth-token` is the client JWT: the one payload shape used across the workspace,
and its HS256 signing and verification.

| Module | Responsibility |
| --- | --- |
| `lib.rs` | `Claims`. |
| `jwt.rs` | Compact JWS: `sign_token` (HS256, header fixed to `{"alg":"HS256"}`), `decode_unverified_payload` (payload as a JSON object, no signature check) and `verify_signature`, which fails with a `SignatureError`. HMAC uses `ring`. |

The payload claims:

| Claim | Meaning |
| --- | --- |
| `appId` | The registered app whose secret signed the token; also the namespace root |
| `publish` / `subscribe` | Relative namespace path the token may publish / subscribe under; `""` is the whole app root; absent means not allowed |
| `iat` / `exp` | Unix seconds; judged by the relay's `ClaimPolicy`, never by the VTS |

Producers and consumers:

- `crates/vts` (`verify.rs`) reads the payload with `decode_unverified_payload` and checks
  it with `verify_signature`; `mint.rs` signs `Claims` with `sign_token`.
- `crates/relay` (`modules/auth/token_claims.rs`) deserialises `Claims` from the
  VTS `/verify` response and applies its time and path policy. It holds no
  secret and never signs or verifies.

## Invariants

- `decode_unverified_payload` never checks the signature. It may not be used as an
  authorisation decision; only a successful `verify_signature` with the app's
  secret is.
- A header whose `alg` is anything other than `HS256` is `InvalidSignature`.
- Unknown claims are ignored on read and never produced on write, so the VTS
  can pass the signed payload through unchanged while the relay reads only
  the fields above.
- The crate holds no secret. Callers pass the secret of the app that signs or
  verifies.
