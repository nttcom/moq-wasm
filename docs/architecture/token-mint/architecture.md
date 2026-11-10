# `token-mint` Architecture

## Status
Living document. Update this file in the same change whenever the design intent,
module boundaries, runtime flow, or invariants described here change.

## Scope
`token-mint` hands anyone the token of a new temporary app, and a new token
for an app whose token has not expired. A temporary app has no registry entry:
its `appId` carries `auth_token::TEMPORARY_APP_ID_PREFIX`, and every token of
every temporary app is signed with one secret that the VTS verifies them with
(see the VTS architecture). The examples use these apps so that a demo runs
under a namespace root only the people its link reaches know, rather than the
public `anon` root.

There are no accounts. Who may join an app is whoever holds one of its
tokens; a token expires an hour after it is signed, which is the only way an
app is revoked.

## Modules

| Module | Responsibility |
| --- | --- |
| `issuer.rs` | `TokenMint`: `issue` signs `Claims::issued` for a new `temp-<32 hex>` app with `publish` and `subscribe` `""` and a one-hour lifetime; `refresh` verifies the token with the secret, requires a temporary `appId` and an `exp` after now, and signs a new hour for the same app. A failed refresh is a `RefreshError`. |
| `rate_limit.rs` | `RateLimit`: fixed windows per client address, counted in this process. |
| `server.rs` | `TokenMintServer`: serves HTTP/1 through `http_server::HttpServer`. Routes `POST /tokens` (rate limited), `POST /tokens/refresh`, `GET /healthz` and `OPTIONS` (CORS preflight); every response allows any origin. Finds the client address in `X-Forwarded-For` from the number of trusted proxies in front (`TOKEN_MINT_TRUSTED_PROXY_HOPS`). |
| `main.rs` | Reads `TOKEN_MINT_SECRET`, `TOKEN_MINT_TRUSTED_PROXY_HOPS` and `PORT`, and serves until terminated. |

## Invariants

- The service keeps no state besides the rate limit counts: an app exists only
  as its tokens, so any number of instances serve the same apps.
- An expired token cannot be refreshed, so a link holding it stops working an
  hour after it was signed even while the app lives on through refreshes made
  by the pages still using it.
- Only `POST /tokens` is rate limited. A refresh needs a valid token, and
  refreshing does not create an app.
- The client address is the `X-Forwarded-For` entry that the outermost of the
  trusted proxies (`TOKEN_MINT_TRUSTED_PROXY_HOPS`) appended; entries the client wrote before it are
  ignored, so a client cannot pose as another address. Without the header, or
  with `0` hops, it is the peer address.
- With several instances, each counts on its own, so a client may get up to
  the limit from every instance.

## Operational notes

- `temporary token issued` / `temporary token refreshed` are logged at INFO
  with the `app_id`; refusals at INFO with `error`. A signing failure is an
  ERROR. Tokens never reach logs or spans.
- Every request runs in a `token_mint.request` span that continues the
  caller's trace and records the `app_id` it answered for.
- The Docker image (`services/token-mint/Dockerfile`) is the same
  `cargo-chef` build onto `distroless/cc` as the VTS.
