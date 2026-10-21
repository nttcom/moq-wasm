# auth-token

The JWT a MoQT client presents in `CLIENT_SETUP`: its claims, shared by the
relay (which reads them from the VTS response), and their HS256 signing and
verification, used by the VTS (which verifies them, and signs them in
`vts-mint`).

```json
{ "appId": "ac8adbc8-...", "publish": "site1/cam1", "subscribe": "site1", "iat": 1759591600, "exp": 1759678000 }
```

`decode_unverified_payload` reads the payload of a compact JWS without verifying the
signature. Only a successful `verify_signature` with the app's secret is an
authorisation decision.

## Design

[Architecture document](../../docs/architecture/auth-token/architecture.md).
