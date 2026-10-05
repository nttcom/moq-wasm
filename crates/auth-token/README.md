# auth-token

The claims of the JWT a MoQT client presents in `CLIENT_SETUP`, shared by the
relay (which reads them from the VTS response) and the VTS (which signs them in
`vts-mint`).

```json
{ "appId": "ac8adbc8-...", "publish": "site1/cam1", "subscribe": "site1", "iat": 1759591600, "exp": 1759678000 }
```

`decode_claims` reads the payload of a compact JWS without verifying the
signature, for clients that want to act on their own token (for example to
refresh it before `exp`). Only the VTS can verify a token.

## Design

[Architecture document](../../docs/architecture/auth-token/architecture.md).
