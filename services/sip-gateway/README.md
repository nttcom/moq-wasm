# sip-gateway

Places and answers telephone calls over a SIP trunk (SIP/TLS + SDES-SRTP G.711) and bridges each call's audio bidirectionally to a MoQT room as an Opus MSF broadcast (`<room>/phone/<call_id>`). Design: [architecture.md](../../docs/architecture/sip-gateway/architecture.md).

## Run

Run against a local Asterisk test trunk, relay, VTS, and the browser Phone example:

```shell
./tests/sip-gateway-e2e/run.sh
```

Or run the binary directly (requires GStreamer `mulawdec`/`alawdec`/`opusenc`/`opusdec` plugins and TLS cert/key + trunk CA files):

```shell
SIP_GATEWAY_TLS_CERT_FILE=cert.pem \
SIP_GATEWAY_TLS_KEY_FILE=key.pem \
SIP_GATEWAY_TRUNK_HOST=trunk.example.com:5061 \
SIP_GATEWAY_TRUNK_CA_FILE=ca.pem \
SIP_GATEWAY_RELAY_URL=moqt://127.0.0.1:4433 \
SIP_GATEWAY_VTS_URL=http://127.0.0.1:8081/verify \
SIP_GATEWAY_REGISTRY_FILE=apps.json \
cargo run -p sip-gateway
```

Pair with [`examples/browser/examples/phone/`](../../examples/browser/examples/phone/README.md) (`PhoneCall` in `moq-wasm/media`) and an app server that mints `calls`-scoped JWTs and handles inbound webhooks.

## API & Tracks

- `POST /calls` (`Authorization: Bearer <JWT>` with `calls` claim, lifetime <= 5 min): body `{"to": "+81...", "namespace_prefix": "<app>/<room>", "peer": "<app>/<room>/..."}` → `201 {"call_id", "namespace"}`.
- Inbound `INVITE`: verifies source IP against `SIP_GATEWAY_TRUNK_SIGNALLING_ADDRESSES`, looks up the DID in `app-registry`, and `POST`s `{"call_id", "from", "to"}` to the app's `callWebhook` to accept (`{"namespace_prefix", "peer"}`) or reject (`{"reject": "busy" | "decline"}`).
- `GET /healthz`: health check.
- MoQT tracks: gateway publishes `<room>/phone/<call_id>` (`catalog`, Opus `audio`, and JSON `call` state track: `dialing` / `ringing` / `answered` / `ended`); page publishes `<peer>` (Opus audio and JSON `control` track: `hangup` or RFC 4733 `dtmf`).

## Configuration

| Variable | Default | Meaning |
| --- | --- | --- |
| `PORT` | `8099` | HTTP API listen port |
| `SIP_GATEWAY_SIP_ADDRESS` | `0.0.0.0:5061` | Local SIP/TLS bind address |
| `SIP_GATEWAY_SIP_PUBLIC_ADDRESS` | — | Public `host:port` advertised in SIP Via/Contact |
| `SIP_GATEWAY_TLS_CERT_FILE` / `SIP_GATEWAY_TLS_KEY_FILE` | — | PEM certificate chain and private key (required) |
| `SIP_GATEWAY_TRUNK_HOST` / `SIP_GATEWAY_TRUNK_CA_FILE` | — | Trunk TLS `host[:port]` and trusted CA PEM (required) |
| `SIP_GATEWAY_TRUNK_USERNAME` / `SIP_GATEWAY_TRUNK_PASSWORD` | — | SIP digest credentials (401/407) |
| `SIP_GATEWAY_TRUNK_SIGNALLING_ADDRESSES` | empty (refuses all) | Comma-separated IPs/CIDRs allowed to send inbound `INVITE` |
| `SIP_GATEWAY_MEDIA_ADDRESS` | — | Public media IP advertised in SDP |
| `SIP_GATEWAY_RELAY_URL` | — | Target MoQT relay URL (required) |
| `SIP_GATEWAY_RELAY_INSECURE_SKIP_TLS_VERIFY` | `false` | Set `true` to skip relay TLS verification (local dev only) |
| `SIP_GATEWAY_VTS_URL` | — | VTS `/verify` URL (required unless insecure skip auth) |
| `SIP_GATEWAY_REGISTRY_FILE` or `SIP_GATEWAY_REGISTRY_BUCKET` / `SIP_GATEWAY_REGISTRY_OBJECT` | — | App registry location for DIDs, webhooks, and destination rules |
| `SIP_GATEWAY_INSECURE_SKIP_AUTH` | `false` | Set `true` to skip auth and join without token (disables inbound calls) |
| `SIP_GATEWAY_MAX_CALLS_PER_APP` / `SIP_GATEWAY_MAX_CALL_SECONDS` | `2` / `3600` | Per-app concurrent call limit and max call duration |
