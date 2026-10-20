# console

Backend of the internal MoQ console. It runs behind IAP on Cloud Run, takes
the employee's identity (`sub`) from the IAP assertion and serves their app
from the app registry.

## API

```
GET /api/me
200 { "app": { "appId": "...", "name": "...", "createdAt": 1760900000 } }
200 { "app": null }
401 { "error": "unauthenticated" }
503 { "error": "registry_unavailable" }
```

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
