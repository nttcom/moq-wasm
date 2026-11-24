# job-launcher

Fire-and-forget job launcher abstracting local child processes and Google Cloud Run Jobs behind one `JobLauncher` enum.

> **Workspace crate**: Internal library shared by [`services/recorder`](../../services/recorder/README.md) (launches `recorder composite`) and [`services/bot-dispatcher`](../../services/bot-dispatcher/README.md) (launches room bots). Native-only.

## Usage

Inside an `async fn` returning `anyhow::Result<()>`:

```rust
use std::{path::PathBuf, sync::Arc};
use google_cloud::MetadataToken;
use job_launcher::JobLauncher;

// Local development: spawns a child process
let local = JobLauncher::Local { program: PathBuf::from("./target/debug/recorder") };
let args = |store: &str| {
    ["composite", "--id", "rec-123", "--store", store].map(String::from).to_vec()
};
local.launch(args("file:///tmp/recordings"), vec![("RUST_LOG".into(), "info".into())]).await?;

// Production: triggers a Cloud Run Job execution
let client = reqwest::Client::new();
let cloud = JobLauncher::CloudRun {
    job: "projects/my-proj/locations/asia-northeast1/jobs/recorder".into(),
    client: client.clone(),
    metadata_token: Arc::new(MetadataToken::new(client)),
};
cloud.launch(args("gs://my-bucket/recordings"), vec![]).await?;
```

## API

| Item | Role |
| --- | --- |
| `JobLauncher::Local { program }` | Spawns `program` with `args` and `local_env`, then waits in a detached Tokio task (logs exit status) |
| `JobLauncher::CloudRun { job, client, metadata_token }` | Calls `POST https://run.googleapis.com/v2/{job}:run` with `containerOverrides[0].args` |
| `JobLauncher::launch(&self, args, local_env)` | Starts the job and returns once spawned or accepted by Cloud Run |

## Behavior & caveats

- **Fire-and-forget**: `launch()` returns as soon as the process spawns or the Cloud Run API accepts the execution—invalid job CLI arguments still return `Ok(())` and fail asynchronously inside the job.
- **`local_env` is local-only**: `local_env` applies only to `JobLauncher::Local` and is ignored by `JobLauncher::CloudRun`. Cloud Run execution overrides (`args` included) are visible to anyone with view permissions on executions, so never pass secrets in `args`.
- **Cloud Run IAM & timeout**: Requires `run.jobs.runWithOverrides` on `job`. API requests time out after 10 s and return an error with HTTP status and response body on non-2xx responses.

## Further reading

- Architecture: [docs/architecture/job-launcher/architecture.md](../../docs/architecture/job-launcher/architecture.md)
