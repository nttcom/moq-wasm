# console

Backend for the internal MoQ console. Runs behind IAP on Cloud Run, reads user identity (`sub`) from the IAP assertion, and manages apps in `app-registry`. Design: [architecture.md](../../docs/architecture/console/architecture.md).

## API

| Route | Body | Response |
| --- | --- | --- |
| `GET /api/me` | | `{ "email": …, "relayUrl": … }` |
| `GET /api/apps` | | `{ "apps": [{ "appId", "name", "createdAt", "rotatedAt" }] }` |
| `POST /api/apps` | `{ "name": "camera site 1" }` | `201 { "app": …, "secret": … }` (`400` above 20 apps per user) |
| `POST /api/apps/{appId}/tokens` | `{ "publish": "cam1", "subscribe": "", "ttlSeconds": 3600 }` | `{ "token": "eyJ…", "expiresAt": 1760903600 }` |
| `POST /api/apps/{appId}/secret` | | `{ "app": …, "secret": … }` (invalidates earlier tokens) |
| `DELETE /api/apps/{appId}` | | `204` |

Non-`GET` requests require `X-Console-Request`. The app `secret` is returned only on creation and rotation.

## Signing tokens on your own server

Sign a compact HS256 JWS (`{"alg":"HS256"}`) keyed by the raw UTF-8 bytes of the app secret (do not base64-decode it):

| Claim | Value |
| --- | --- |
| `appId` | App's `appId` |
| `publish` | Allowed publish prefix under `appId` (`""` for whole app; omit to deny) |
| `subscribe` | Allowed subscribe prefix under `appId` (`""` for whole app; omit to deny) |
| `iat`, `exp` | Unix seconds (`exp - iat` must be ≤ 24 h) |

```python
import base64, hashlib, hmac, json, time

def b64url(data: bytes) -> str:
    return base64.urlsafe_b64encode(data).rstrip(b"=").decode()

def sign(app_id: str, secret: str, publish: str, ttl: int) -> str:
    now = int(time.time())
    claims = {"appId": app_id, "publish": publish, "iat": now, "exp": now + ttl}
    signing_input = b64url(b'{"alg":"HS256"}') + "." + b64url(json.dumps(claims).encode())
    signature = hmac.new(secret.encode(), signing_input.encode(), hashlib.sha256).digest()
    return signing_input + "." + b64url(signature)
```

For starting bots, sign a launch token the same way and call `bot-dispatcher` ([README](../bot-dispatcher/README.md)).

## Running locally

Set a dev `sub` and local registry file, then run the Vite dev server (which proxies `/api`):

```shell
CONSOLE_DEV_SUB=dev CONSOLE_REGISTRY_FILE=services/vts/apps.example.json cargo run -p console
npm --prefix services/console/web install
npm --prefix services/console/web run dev
```

On Cloud Run, set `CONSOLE_IAP_AUDIENCE` and `CONSOLE_REGISTRY_BUCKET` instead (`CONSOLE_DEV_SUB` is rejected). The container serves built frontend assets from `CONSOLE_STATIC_DIR`:

```shell
docker build -f services/console/Dockerfile -t moqt-console .
```
