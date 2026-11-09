# metadata-token

One type, `MetadataToken`: the OAuth access token of the service account a
Google Cloud workload (Cloud Run, Compute Engine) runs as, read from the
metadata server's `instance/service-accounts/default/token` and cached until a
minute before it expires. A caller sends it as `Authorization: Bearer` to a
Google API. Outside Google Cloud there is no metadata server and the call
fails, so a service that also runs locally pairs it with another source, such
as a token passed in the environment.

Used by `app-registry` (Cloud Storage) and `live-translate` (Agent Platform).
No key file is ever read: the identity is the workload's own.
