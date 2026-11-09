# [S] Send a token refresh through one `moqt` call

Next: add `Subscriber::refresh_authorization_token(app_id, token)` next to `track_status` in `crates/moqt/src/modules/moqt/domains/subscriber.rs`, then call it from `crates/python/src/session.rs:230`.

## Goal

The `update_auth_token` track name lives in one place. Today `crates/python/src/session.rs:19`, `services/recorder/src/token_refresh_task.rs:16`, `examples/rust/moq-cli/src/transport/auth_token_refresh_task.rs:11` and `tests/auth-e2e/src/main.rs:41` each define it and build the TRACK_STATUS with `AuthorizationToken::use_value_utf8` themselves.

## Plan

Open question: share the 10-second token-file polling task too; recorder's `TokenRefreshTask` and moq-cli's `AuthTokenRefreshTask` differ only in recorder's `TokenRenewal::Issuer` branch.
