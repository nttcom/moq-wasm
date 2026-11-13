# console

Backend of the internal MoQ console. It runs behind IAP on Cloud Run, takes
the employee's identity (`sub`) from the IAP assertion and serves their apps
from the app registry.
Design: [architecture document](../../docs/architecture/console/architecture.md).

## API

| Route | Body | Answer |
| --- | --- | --- |
| `GET /api/me` | | `{ "email": …, "relayUrl": … }` |
| `GET /api/apps` | | `{ "apps": [{ "appId", "name", "createdAt", "rotatedAt" }] }` |
| `POST /api/apps` | `{ "name": "camera site 1" }` | `201 { "app": …, "secret": … }`, `400` beyond 20 apps per employee |
| `POST /api/apps/{appId}/tokens` | `{ "publish": "cam1", "subscribe": "", "ttlSeconds": 3600 }` | `{ "token": "eyJ…", "expiresAt": 1760903600 }` |
| `POST /api/apps/{appId}/secret` | | `{ "app": …, "secret": … }`; tokens issued before stop working |
| `DELETE /api/apps/{appId}` | | `204` |

Requests other than `GET` must carry `X-Console-Request`. Only creating an
app and rotating its secret return the `secret`, once.

## Signing tokens on your own server

A service that hands out tokens signs them itself with the app's secret: a
compact JWS with the header `{"alg":"HS256"}`, keyed by the UTF-8 bytes of the
secret string as shown (do not base64-decode it), and these claims:

| Claim | Value |
| --- | --- |
| `appId` | The app's `appId` |
| `publish` | Path under the app the token may publish to; `""` for the whole app; omitted for none |
| `subscribe` | The same for subscribing |
| `iat`, `exp` | Unix seconds; the relay refuses `exp - iat` over 24 hours |

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

## Running locally

Without IAP, give the console a fixed identity and a registry file, then open
the pages through the Vite dev server, which forwards `/api` to the console:

```
CONSOLE_DEV_SUB=dev CONSOLE_REGISTRY_FILE=services/vts/apps.example.json cargo run -p console
npm --prefix services/console/web install
npm --prefix services/console/web run dev
```

On Cloud Run, set `CONSOLE_IAP_AUDIENCE` and `CONSOLE_REGISTRY_BUCKET`
instead; `CONSOLE_DEV_SUB` is refused there. The image serves the built pages
itself (`CONSOLE_STATIC_DIR`):

```
docker build -f services/console/Dockerfile -t moqt-console .
```
