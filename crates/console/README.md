# console

Backend of the internal MoQ console. It runs behind IAP on Cloud Run, takes
the employee's identity (`sub`) from the IAP assertion and serves their app
from the app registry.

## API

| Route                  | Body                                                         | Answer                                                |
| ---------------------- | ------------------------------------------------------------ | ----------------------------------------------------- |
| `GET /api/me`          |                                                              | `{ "app": { "appId", "name", "createdAt" } \| null }` |
| `POST /api/app`        | `{ "name": "camera site 1" }`                                | `201 { "app": … }`, `409` when the caller has an app  |
| `POST /api/tokens`     | `{ "publish": "cam1", "subscribe": "", "ttlSeconds": 3600 }` | `{ "token": "eyJ…", "expiresAt": 1760903600 }`        |
| `POST /api/app/secret` |                                                              | `{ "app": … }`; tokens issued before stop working     |
| `DELETE /api/app`      |                                                              | `204`                                                 |

Requests other than `GET` must carry `X-Console-Request`; a request whose
`Origin` is another site is refused.

## Running locally

Without IAP, give the console a fixed identity and a registry file:

```
CONSOLE_DEV_SUB=dev CONSOLE_REGISTRY_FILE=crates/vts/apps.example.json cargo run -p console
curl http://localhost:8080/api/me
```

`CONSOLE_DEV_SUB` is refused on Cloud Run. There, set `CONSOLE_IAP_AUDIENCE`
and `CONSOLE_REGISTRY_BUCKET` instead.

## Design

[Architecture document](../../docs/architecture/console/architecture.md).
