# reqwest for starting Cloud Run Jobs

## Status

Accepted

## Date

2026-10-19

## What

Add `reqwest` (`default-features = false`, `json` and `rustls`) to
`crates/recorder` as a direct dependency. `launcher::Launcher::CloudRun` uses it
to get an access token from the GCE metadata server and to start a Cloud Run
Job execution with `POST https://run.googleapis.com/v2/<job>:run`.

## Context

`recorder serve` no longer records in its own process: every room recording
and every composite runs as a job, so the API can scale to zero on Cloud Run
and a composite gets the CPU of a job of its own. On Cloud Run the API starts
those jobs through the Cloud Run Admin API, which needs an HTTPS client and an
OAuth token of the service's service account.

## Alternatives

- **`google-cloud-run` / `google-cloud-auth` SDK crates.** Typed requests and a
  token cache, but a large generated client for one REST call, plus its own
  error and runtime types.
- **hyper's client with `hyper-rustls`.** `hyper` is already a dependency for
  the API server, but its client needs `hyper-util`'s legacy client, a TLS
  connector and hand-written JSON bodies: more code for the same call.
- **`gcloud run jobs execute` from a shell.** No Rust code, but the image would
  need the Cloud SDK and a shell, and errors would come back as process output.

## Decision

Use `reqwest` with the same features as `crates/relay`. It is already in the
lock file through `object_store`'s `gcp` feature, so the addition brings no new
crate into the build. The token is requested per launch: launches are rare
(one per recording and per composite), so caching it is not worth the code.
