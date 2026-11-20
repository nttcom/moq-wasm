# [XS] Refuse a draft-14 Object ID delta that overflows u64

Next: `crates/moqt/src/modules/moqt/data_plane/object/subgroup.rs:263` adds `prev + 1 + object_id_delta` unchecked.

## Goal

A draft-14 subgroup stream whose Object ID deltas pass `u64::MAX` fails with a protocol violation, as the draft-18 decoder does (`version/draft18/objects.rs:433`), instead of panicking in a debug build and wrapping to a smaller Object ID in a release build.
