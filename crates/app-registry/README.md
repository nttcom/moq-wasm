# app-registry

Loads and atomically updates the JSON registry of apps whose secrets sign client JWTs, backed by Google Cloud Storage or a local file.

> **Workspace crate**: Internal library shared by [`services/vts`](../../services/vts/README.md), [`services/console`](../../services/console/README.md), [`services/bot-dispatcher`](../../services/bot-dispatcher/README.md), and [`services/sip-gateway`](../../docs/architecture/sip-gateway/architecture.md). Native-only.

## Registry format

```json
[
  {
    "appId": "ac8adbc8-a2ff-4c41-9f5e-fdaed5e1e65e",
    "name": "camera site 1",
    "secret": "...",
    "isRelay": false,
    "sub": "accounts.google.com:1234...",
    "createdAt": 1760900000,
    "rotatedAt": 1760990000
  }
]
```

Every entry requires non-empty `appId` and `secret` plus `isRelay`. An optional `phone` block (`numbers`, `callWebhook`, `allowedDestinations`) configures SIP gateway routing. Seed a new file or GCS object with `[]` before calling `load()` or `update()`.

## Usage

Inside an `async fn` returning `anyhow::Result<()>`:

```rust
use app_registry::{App, Location, LocationVariables, RegistryStore};

let location = Location::from_env(&LocationVariables {
    file: "APPS_FILE",
    bucket: "APPS_BUCKET",
    object: "APPS_OBJECT",
})?
.expect("set APPS_FILE or APPS_BUCKET");

let store = RegistryStore::new(location)?;
let apps = store.load().await?;
let _app = apps.get("ac8adbc8-a2ff-4c41-9f5e-fdaed5e1e65e");

// Mutate and persist (keep the callback side-effect free; retries on conflict).
store
    .update(|rows: &mut Vec<App>| {
        if let Some(app) = rows.iter_mut().find(|a| a.app_id == "ac8adbc8-a2ff-4c41-9f5e-fdaed5e1e65e") {
            app.name = Some("camera site 1".into());
        }
        Ok(())
    })
    .await?;
```

## Key types & features

| Item | Role |
| --- | --- |
| `RegistryStore` | Async `load()` → `Apps` and optimistic-concurrency `update(\|&mut Vec<App>\| ...)` |
| `Location`, `LocationVariables` | Selects `Location::File(PathBuf)` or `Location::CloudStorage { endpoint, bucket, object }` (directly or via `Location::from_env`) |
| `Apps`, `App`, `Phone`, `is_e164` | Validated app collection and SIP phone configuration |
| `testing` feature | Exposes `testing::TemporaryRegistryFile` for tests (`[dev-dependencies]` only) |

## Backends & caveats

| Backend | Generation check | Concurrency |
| --- | --- | --- |
| `Location::CloudStorage` | GCS `ifGenerationMatch` (2 s timeout) | Safe across multiple writers/processes |
| `Location::File` | Content hash + `<path>.tmp` rename | Single writer process only |

- **Environment resolution**: `Location::from_env` returns `Ok(None)` when neither env var is set, fails if both are set, defaults `object` to `"registry.json"`, and honors `STORAGE_EMULATOR_HOST` via [`google_cloud::Endpoint::from_env`](../google-cloud/README.md).
- **Optimistic retries**: `update()` skips writing when unchanged, validates via `Apps::new`, and retries up to 5 attempts on conflict.
- **Validation**: `Apps::new` rejects empty `appId`/`secret`, duplicate `appId`, invalid E.164 numbers/prefixes, phone numbers without `callWebhook`, and phone numbers shared across two apps. Unknown JSON fields are dropped on write.

## Further reading

- Architecture: [docs/architecture/app-registry/architecture.md](../../docs/architecture/app-registry/architecture.md)
