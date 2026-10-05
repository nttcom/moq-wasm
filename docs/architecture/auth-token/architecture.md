# `auth-token` Architecture

## Status
Living document. Update this file in the same change whenever the design intent,
module boundaries, runtime flow, or invariants described here change.

## Scope
`auth-token` defines the one JWT payload shape used across the workspace:

| Claim | Meaning |
| --- | --- |
| `appId` | The registered app whose secret signed the token; also the namespace root |
| `publish` / `subscribe` | Relative namespace path the token may publish / subscribe under; `""` is the whole app root; absent means not allowed |
| `iat` / `exp` | Unix seconds; judged by the relay's `ClaimPolicy`, never by the VTS |

Producers and consumers:

- `crates/vts` (`mint.rs`) serialises `Claims` into the token it signs.
- `crates/relay` (`modules/auth/token_claims.rs`) deserialises `Claims` from the
  VTS `/verify` response and applies its time and path policy.
- `decode_claims` is for clients that need to look at their own token without
  verifying it. The browser counterpart is
  `examples/browser/examples/meeting/src/session/authToken.ts`.

## Invariants

- The crate holds no cryptography. Signing and verification stay in `vts`;
  `decode_claims` must never be used as an authorisation decision.
- Unknown claims are ignored on read and never produced on write, so the VTS
  can pass the signed payload through unchanged while the relay reads only
  the fields above.
- Dependencies are limited to `serde`, `serde_json`, `base64` and `anyhow` so
  the crate can be used from `moqt-client-wasm`.
