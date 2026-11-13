# app-registry

The registry of apps whose secrets sign client JWTs: one JSON document, kept
in Cloud Storage or in a local file.

```json
[
  { "appId": "ac8adbc8-...", "name": "camera site 1", "secret": "...", "isRelay": false,
    "sub": "accounts.google.com:1234...", "createdAt": 1760900000, "rotatedAt": 1760990000 },
  { "appId": "11111111-...", "secret": "...", "isRelay": true }
]
```

`RegistryStore::load` reads the apps and their generation;
`RegistryStore::update` changes them, running the change again when another
writer stored first.

| `Location` | Generation | Safe for |
| --- | --- | --- |
| `CloudStorage { endpoint, bucket, object }` | The object's generation (`ifGenerationMatch`) | Writers on any number of machines |
| `File(path)` | A hash of the contents | One writing process |

`Endpoint::from_env` uses the emulator at `STORAGE_EMULATOR_HOST` instead of
Cloud Storage.

Design: [architecture document](../../docs/architecture/app-registry/architecture.md).
