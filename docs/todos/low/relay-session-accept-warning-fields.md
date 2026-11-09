# [XS] Add identifying fields to the relay's second "failed to establish session" warning

Next: add `session_id`, `session_peer` and `relay_hostname` to the warning after `handshake.accept(...)` in `services/relay/src/relay_server/session_handler.rs:84`.

## Goal

The warning can be found in exported logs without its span, as AGENTS.md §7 requires; today it carries only `%error`.
