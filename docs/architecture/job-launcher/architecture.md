# `job-launcher` Architecture

`JobLauncher` starts a background job and returns immediately without waiting for completion.

| Backend | Behavior |
| --- | --- |
| `Local` | Spawns a child process locally (development). |
| `CloudRun` | Triggers a Cloud Run Job execution via the Admin API (`jobs:run`), overrides container arguments, and authenticates with the workload's service account (`google-cloud`). |

## Security Note
Job arguments are visible to anyone with permission to view job executions.

## Callers
- `recorder` — starts `recorder composite`.
- `bot-dispatcher` — starts each bot job.
