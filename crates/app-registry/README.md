# app-registry

The registry of apps whose secrets sign client JWTs. It is one JSON document,
an array of rows in the shape the VTS apps file already uses with optional
fields added, kept in Cloud Storage or in a local file.

```json
[
  { "appId": "ac8adbc8-...", "name": "camera site 1", "secret": "...", "isRelay": false,
    "sub": "accounts.google.com:1234...", "createdAt": 1760900000 },
  { "appId": "11111111-...", "secret": "...", "isRelay": true }
]
```

`RegistryStore::new` takes a `Location`:

| Location | Generation | Write |
| --- | --- | --- |
| `CloudStorage { endpoint, bucket, object }` | The object's generation | Upload with `ifGenerationMatch`, safe for writers on any number of machines |
| `File(path)` | A hash of the contents | Compare the hash, write a temporary file and rename it over the registry; one writing process only |

`RegistryStore::load` reads the apps and the generation,
`RegistryStore::generation` reads only the generation, and
`RegistryStore::update` changes the apps, reloading and running the change
again when another writer stored first.

`Endpoint::from_env` uses the emulator at `STORAGE_EMULATOR_HOST` (for example
`http://localhost:4443`) instead of Cloud Storage; requests to it are sent
without credentials.

## Design

[Architecture document](../../docs/architecture/app-registry/architecture.md).
