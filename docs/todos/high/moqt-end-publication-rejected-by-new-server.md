# [S] End a publication the new server rejected once the old connection closes

Next: in `forget_connection` (`crates/moqt/src/modules/moqt/domains/session_context.rs:268`), remove a publication whose last target it dropped and tell the application, as `leave_publication` (`:334`) does.

## Goal

After a GOAWAY migration in which the new server rejects a PUBLISH (`domains/session/migration_task.rs:498`), the application learns the track has no subscriber when the old connection closes.
Reproduced on both drafts: no `SessionEvent::Unsubscribe` arrives, the publication stays on 0 connections, and a write fails with "the track has no connection".
The publication is also sent again as PUBLISH on the next migration.
