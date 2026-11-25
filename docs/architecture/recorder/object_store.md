# `object_store` for Uploading Recordings (`recorder`)

**Status:** Accepted (2026-10-16)

## What

Add `object_store` (`default-features = false`, features: `fs`, `gcp`) and `url` to `services/recorder` to upload closed recording files to Google Cloud Storage or a local directory (`object_store::parse_url_opts` takes a `url::Url`).

## Context

Recordings must upload to object storage (GCS first, S3 later) rather than staying on local disk. Uploads must authenticate via VM/Cloud Run service accounts or developer ADC, refresh tokens, retry transient errors, and stream tens-of-megabyte files without buffering them in RAM.

## Alternatives

- **Custom `Uploader` trait + GCS client over `reqwest`:** Reimplements OAuth token refresh, retries, resumable/multipart uploads, and eventually S3 SigV4 signing.
- **Vendor SDKs (`google-cloud-storage`, `aws-sdk-s3`):** Large dependency trees with distinct client/error types that still need a custom abstraction layer.
- **Sidecar sync (`gcsfuse` / `rclone`):** Adds an extra process per recorder and prevents the recorder from knowing when uploads finish.

## Decision

Use `object_store` with `fs` and `gcp`:

- **`fs` (`LocalFileSystem`):** Supports `file://` URLs for local runs with identical object layout (adds `walkdir` and `nix` to the lock).
- **`Arc<dyn ObjectStore>` seam:** Built from `--upload`, so adding S3 later only requires enabling the `aws` feature and passing an `s3://` URL.
- **Auth & shared deps:** Reads service-account files, ADC, or the GCE metadata server automatically, and reuses `reqwest` 0.13 and `aws-lc-rs` already in the workspace lock.
