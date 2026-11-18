# google-cloud

The Google Cloud calls the services make themselves, each a single HTTP
request over `reqwest`, as the service account the workload runs as. No key
file is ever read: the identity is the workload's own.

| Module | Responsibility |
| --- | --- |
| `metadata_token.rs` | `MetadataToken`: the OAuth access token of the service account a Google Cloud workload (Cloud Run, Compute Engine) runs as, read from the metadata server's `instance/service-accounts/default/token` and cached until a minute before it expires. A caller sends it as `Authorization: Bearer` to a Google API. Outside Google Cloud there is no metadata server and the call fails, so a service that also runs locally pairs it with another source, such as a token passed in the environment. `identity_token` reads the service account's Google ID token for an audience from `instance/service-accounts/default/identity`, which a caller sends to a service that verifies who it is (a recorder job to `bots`). |
| `cloud_storage.rs` | `CloudStorageBucket`: the Cloud Storage JSON API requests that need an object's generation: download with its generation, upload with `ifGenerationMatch`, delete with `ifGenerationMatch`. `Endpoint` picks Cloud Storage, whose requests carry the `MetadataToken`, or an emulator (`STORAGE_EMULATOR_HOST`), whose requests carry no token. |
| `testing.rs` | `FakeCloudStorage`, which serves those requests from memory, and `SilentServer`, which never answers; behind the `testing` feature. |

Used by `app-registry` (the registry object), `recorder` (the conditional
delete of a room's lock, which `object_store` lacks, and its ID token for
`bots`), `job-launcher` and `bots` (starting Cloud Run Jobs) and
`live-translate` (the Agent Platform).
