# [S] Enforce draft-14's limits on Reason Phrase, Track Namespace and Full Track Name

Next: give draft-14 readers with these limits, as `crates/moqt/src/modules/moqt/version/draft18/codec.rs` has; `try_get_string` and `try_get_tuple` (`crates/moqt/src/modules/extensions/buf_get_ext.rs:28`, `:46`) have none.

## Goal

A draft-14 session closes with PROTOCOL_VIOLATION on a Reason Phrase over 1024 bytes (§1.4.3), a Track Namespace of 0 or more than 32 fields, or a Full Track Name over 4096 bytes (§2.4.1); all three are MUST.
Reproduced: a 1025-byte Reason Phrase decodes as SUBSCRIBE_ERROR, and namespaces of 0 and 33 fields and a 4097-byte Full Track Name decode as SUBSCRIBE.
