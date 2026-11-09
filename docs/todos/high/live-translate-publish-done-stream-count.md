# [S] Send the real Stream Count in live-translate's PUBLISH_DONE

Next: in `services/live-translate/src/broadcast_translation.rs:537`, pass the `TrackWriter`'s `opened_streams()` instead of `0`, as `crates/moqt/src/modules/moqt/domains/origin.rs:177` does.

## Goal

`end_language` ends tracks it wrote on subgroup streams (`TrackWriter` at `:565`) with Stream Count 0. Draft-14 §9.12 requires the exact count, or 2^62-1 when it is not known (MUST).
A subscriber that ends a subscription once its counted streams arrive (moqt does, for a Stream Count above 0) then cuts off streams still in flight.
