# [XS] Check a key-value pair's length as u64 before casting it to usize

Next: compare the `u64` length with `bytes.remaining()` before `length as usize` in `KeyValuePair::decode` (`crates/moqt/src/modules/moqt/control_plane/control_messages/key_value_pair.rs:31`).

## Goal

A control-message parameter with an odd key whose declared length exceeds the buffer is rejected on every target. On wasm32 `usize` is 32 bits, so a length of `2^32 + 1` truncates to `1`, passes the check and decodes the wrong bytes.
