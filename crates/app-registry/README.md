# app-registry

Loads and atomically updates the JSON registry of apps whose secrets sign client JWTs, backed by Cloud Storage or a local file.

## Registry format

```json
[
  { "appId": "ac8adbc8-...", "name": "camera site 1", "secret": "...", "isRelay": false,
    "sub": "accounts.google.com:1234...", "createdAt": 1760900000, "rotatedAt": 1760990000 },
  { "appId": "11111111-...", "secret": "...", "isRelay": true }
]
```

Every entry requires `appId`, `secret`, and `isRelay`. Seed a brand-new registry file or object with `[]` before calling `load()` or `update()`.

## Usage

1. Choose a backend directly (`Location::CloudStorage` / `Location::File`) or resolve one from environment variable **names** via `Location::from_env(&LocationVariables { file: "APPS_FILE", bucket: "APPS_BUCKET", object: "APPS_OBJECT" })?` — returns `Ok(None)` when neither file nor bucket is set, fails if both are set, defaults `object` to `"registry.json"`, and uses `google_cloud::Endpoint::from_env` (which targets `STORAGE_EMULATOR_HOST` when set).
2. Unwrap the `Option<Location>` and construct `RegistryStore::new(location)?`.
3. Read with `store.load().await?` → `Apps`.
4. Mutate with `store.update(|apps: &mut Vec<App>| -> anyhow::Result<T> { ... }).await?` — loads the existing registry (failing if it does not exist), writes only if `apps` changed and passes `Apps::new` validation, and runs up to five attempts total if another writer stores first (keep the callback free of external side effects).

## Backends

| `Location` | Generation | Concurrency |
| --- | --- | --- |
| `CloudStorage { endpoint, bucket, object }` | GCS object generation (`ifGenerationMatch`) | Safe across multiple writers |
| `File(path)` | Content hash | Single writer process only |

Design: [architecture.md](../../docs/architecture/app-registry/architecture.md).
