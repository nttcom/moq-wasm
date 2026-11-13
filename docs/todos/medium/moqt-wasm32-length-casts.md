# [XS] Read wire lengths without `as usize`

Next: replace the casts at `crates/moqt/src/modules/extensions/buf_get_ext.rs:34`, `:41` and `crates/moqt/src/modules/moqt/version/draft18/codec.rs:287`, `:316` with `usize::try_from` that fails as a length too long.

## Goal

On wasm32 a length above `u32::MAX` is refused, as on a 64-bit target.
Read, not run (no wasm32 test runner here): the cast keeps the low 32 bits, so a field a native endpoint closes the session for decodes as a short one, and draft-18's 4096-byte Full Track Name limit is checked on the truncated value.
`codec.rs:176` and `:254` compare the `u64` with their limit before the cast and need no change.
