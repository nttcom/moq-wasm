# [S] Stop carrying a subscription that PUBLISH_DONE ended

Next: in `cancel_subscription` (`crates/moqt/src/modules/moqt/domains/session_context.rs:521`, called at `runtime/tasks/publish_done_expiry_task.rs:79`), also remove the entry of `carried.subscriptions`, as `unsubscribe` does (`domains/subscriber.rs:790`).

## Goal

A subscription the publisher ended is neither subscribed again by a GOAWAY migration nor kept for the life of the session.
Reproduced on both drafts: the entry stays after PUBLISH_DONE, and a migration while the sink is alive (up to 10 s after PUBLISH_DONE) sends SUBSCRIBE for the ended track to the new server.
Otherwise the entry stays until `unsubscribe` or the next migration: one per ended subscription.
