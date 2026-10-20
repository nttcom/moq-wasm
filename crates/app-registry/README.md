# app-registry

The registry of apps whose secrets sign client JWTs. It is one JSON object in
Cloud Storage: an array of rows in the shape the VTS apps file already uses,
with optional fields added.

```json
[
  { "appId": "ac8adbc8-...", "name": "camera site 1", "secret": "...", "isRelay": false,
    "sub": "accounts.google.com:1234...", "createdAt": 1760900000 },
  { "appId": "11111111-...", "secret": "...", "isRelay": true }
]
```

`RegistryStore::load` reads the apps and the object's generation,
`RegistryStore::generation` reads only the generation, and
`RegistryStore::update` changes the apps with a conditional upload, so writers
on any number of machines never overwrite each other.

Set `STORAGE_EMULATOR_HOST` (for example `http://localhost:4443`) to use an
emulator instead of Cloud Storage; requests are then sent without credentials.

## Design

[Architecture document](../../docs/architecture/app-registry/architecture.md).
