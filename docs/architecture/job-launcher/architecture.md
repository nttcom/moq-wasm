# job-launcher

`JobLauncher` starts a job and returns without waiting for it to end.
`Local` spawns a program as a child process, for development; `CloudRun`
runs an execution of a Cloud Run Job through the Admin API's `jobs:run`,
replacing the container's arguments with the given ones, and authenticates
as the workload's own service account (`google-cloud`).

A job's arguments are visible to anyone who can view its executions.

Used by `recorder`, which starts `recorder composite`, and by `bot-dispatcher`, which
starts each bot.
