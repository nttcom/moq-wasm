# proptest for property-based codec tests in moqt

## Status

Accepted

## Date

2026-11-19

## What

Add `proptest` as a dev-dependency of the `moqt` crate, with default features
off and only `std` on. The tests use it to state properties of the wire
codecs over generated messages and objects instead of a few hand-picked ones:

- decoding an encoded draft-14 control message gives back the same message
  and reads every byte of it;
- a variable-length integer of either draft reads back however its bytes are
  split across chunks;
- framed control messages decode to the same messages however the stream is
  cut into chunks;
- the objects written to a subgroup stream or a FETCH response of either
  draft read back the same however the stream is cut into chunks;
- received draft-18 Track Properties, in any encoding that holds each
  integer, go out byte-identical, and after any setters only the properties
  a setter changed are encoded afresh;
- a draft-18 SUBSCRIBE, SUBSCRIBE_NAMESPACE, PUBLISH, FETCH or PUBLISH_DONE
  decodes to the message encoded, up to what draft-18 cannot carry (FETCH
  reads the publisher's group order as Ascending, a reason phrase is cut
  at 1,024 bytes), and encoding fails only for a DELETE authorization
  token or a range ending before its start;
- a sequence of model SUBSCRIBE_UPDATEs sent as draft-18 REQUEST_UPDATEs
  and applied to the receiver's baseline leaves the model's state, and a
  received Subscription Filter holds the same groups once applied;
- during a session migration, `GroupCutover` takes each group from at most
  one subscription, every object of it or none, and from the current one
  only from its start while the previous one may still deliver it.

The strategies live in `modules/test_support/control_message_strategies.rs`
and `modules/test_support/object_strategies.rs`, shared by the tests of each
codec. They favour the values where an encoding changes: the edges of each
integer length, and object and group gaps of 0 to 3, where a codec omits or
shortens a delta. Uniform values almost never reach them.

## Context

Many of the `moqt` codec fixes were for inputs no example-based test had
picked: values at the edge of a varint length, names and reason phrases at
their draft limits, lengths above 32 bits on wasm32, non-UTF-8 fields. Each
example test checks the values its author thought of. A property over
generated messages also covers the combinations of fields and values nobody
listed, and when it fails it shrinks the input to a minimal case.

`wire_noise()` already feeds fixed byte strings to every decoder to check
that malformed input never panics or stalls. It says nothing about whether a
well-formed message decodes to what was encoded, which is what the
properties above check.

## Alternatives

- `quickcheck`: smaller, but generation is tied to one `Arbitrary` impl per
  type, so a draft constraint (1 to 32 namespace fields, a 4,096-byte Full
  Track Name) needs a newtype per constraint, each with a hand-written
  `shrink` that keeps the constraint.
- `bolero` or `cargo-fuzz`: coverage-guided fuzzing finds crashes in decoders
  better than random generation, but needs nightly and runs outside
  `cargo test`. It fits the "never panics on any bytes" property, not
  round-trip properties, and can be added for that later.
- More example tests: each one covers only the values written in it.

## Decision

Use `proptest`. Strategies compose from combinators, so a draft constraint is
written once in the strategy and shrinking keeps it. The tests run in
`cargo test` on stable like the rest of the suite (256 cases per property by
default, set by `PROPTEST_CASES`). With default features off, it adds four
crates to the lock (`proptest`, `rand_xorshift`, `unarray`, `core_detect`)
and, as a dev-dependency, none to crates that depend on `moqt`.

A failing case is written to `crates/moqt/proptest-regressions/` and replayed
first on the next run; commit that file with the fix of the failure it found.
