# [S] Call `Connection::close_with_error` instead of `SessionContext::close_connection`

Next: replace each `session.close_connection(connection, code, reason)` (`grep -rn close_connection crates/moqt/src`) with `connection.close_with_error(code, reason)`, then delete the method at `crates/moqt/src/modules/moqt/domains/session_context.rs:592`.

## Goal

`SessionContext::close_connection` no longer exists: it only forwards to `Connection::close_with_error` and does not use `self`.
