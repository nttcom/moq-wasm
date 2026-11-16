# [S] Give the SDKs' PublishDone the code of the session's draft

Next: decide the code type `PublishDone.code` carries, at `crates/moqt/src/modules/client/subscription.rs` (`PublishDone`) and its users `crates/wasm/src/session.rs` and `crates/python/src/subscription.rs`.

## Goal

On a draft-18 session, the wasm and Python `PublishDone.code` match draft-18's numbers. Today both use `publish_done_code_to_wire` (draft-14), so EXPIRED and TOO_FAR_BEHIND come out swapped, and UPDATE_FAILED and EXCESSIVE_LOAD become 0 with a "PUBLISH_DONE code undefined in draft-14" WARN.

## Plan

Options: number by the session's draft, or expose a name (string/enum) instead of a number in both SDKs.
