# object_store for uploading recordings

## Status

Accepted

## Date

2026-10-16

## What

Add `object_store` (Apache Arrow project, `gcp` feature only) to `crates/recorder`
to upload closed recording files to Google Cloud Storage, and `url` to pass it the
upload location, which `object_store::parse_url_opts` takes as a `url::Url`.

## Context

Recordings have to land in a bucket instead of the recorder's local disk: GCS
first, S3 later. The upload has to authenticate the way the recorder is deployed
(a service account on a VM or Cloud Run, or the developer's ADC), refresh its
token, retry transient failures and stream files of tens of megabytes without
holding them in memory.

## Alternatives

- **An own `Uploader` trait with a GCS client on `reqwest`.** No new crate, but
  the recorder would own the OAuth token flow, the retry policy and the
  resumable or multipart upload protocol, and S3 would add SigV4 signing on
  top. That reimplements what `object_store` already maintains.
- **The vendor SDKs (`google-cloud-storage`, then `aws-sdk-s3`).** Each is a
  large dependency tree with its own client and error types, so S3 support
  would still need a trait of our own to hide the two of them.
- **Local files synced by gcsfuse or rclone.** No code, but one more process
  to run beside every recorder, and the recorder could not tell when a file
  reached the bucket.

## Decision

Use `object_store` with `default-features = false, features = ["gcp"]`. Its
`ObjectStore` trait is the seam: the recorder holds an `Arc<dyn ObjectStore>`
built from the `--upload` URL, so S3 needs the `aws` feature and an `s3://`
URL, and no recorder code of its own. Credentials are read from the
environment: a service account file, the ADC file, or the GCE metadata server
when neither is present. The crate uses `reqwest` 0.13 and `aws-lc-rs`, both
already in the workspace's lock file.
