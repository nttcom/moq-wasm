# [S] Keep recorder status `starting` until the room is joined

Next: initialise `RoomStatus.state` to `RoomState::Starting` in `RecordingJob::record` (`services/recorder/src/bot_job.rs:158-161`) and switch it to `RoomState::Recording` in `follow_members` (`services/recorder/src/room_recording.rs:172-180`) only after both `subscribe_namespace` and `publish_namespace` succeed.

## Goal

A recording's status stays `starting` until the recorder has connected to the relay, subscribed to the room's namespace, and announced its presence, so a caller waiting for `recording` (such as `start()` in `examples/browser/tests/recorder-e2e-arrange.ts`) cannot publish media before the recorder is in the room.
Today `RecordingJob::record` (`services/recorder/src/bot_job.rs:158-169`) sets `RoomState::Recording` before calling `record` (`services/recorder/src/room_recording.rs:99`), and `StateSyncTask::run` (`services/recorder/src/state_sync_task.rs:47-86`) ticks immediately, writing `recording` to the store and sending a `running` heartbeat before the join completes.

## Plan

`bot_state` (`services/recorder/src/state_sync_task.rs:124-126`) maps both `RoomState::Starting` and `RoomState::Recording` to `BotState::Running` (`services/recorder/src/bot_dispatcher_client.rs:13-18` has no `Starting` variant), so keeping the bot record at `starting` too requires deciding whether to report `starting` heartbeats (which `bot-dispatcher` accepts) or keep `running` once the process is up.
Also update `stopping` (`services/recorder/src/room_recording.rs:415-420`) so a stop arriving while `RoomState::Starting` still transitions to `RoomState::Stopping` and is not overwritten to `Recording` when the join finishes.
