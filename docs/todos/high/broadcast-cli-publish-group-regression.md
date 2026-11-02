# [M] Keep group ids monotonic when moq-cli publishes an Annex-B stream

Next: read how `next_group_id` in `crates/broadcast/src/group_alignment.rs:21` meets a wall-clock group id, and reproduce with the command below.

## Goal

`moq-cli publish` keeps publishing. On 2026-11-02 it stopped after ~10 s against the deployed relay with `media publishing stopped: group 1793627721985535 precedes the next group 1793627721985536`, after several PUBLISH_OK.

Reproduce: `ffmpeg -f lavfi -i testsrc=size=320x240:rate=15 -c:v libx264 -preset ultrafast -tune zerolatency -g 15 -f h264 - | moq-cli publish --relay moqt://relay-1.moqt.research.skyway.io:443 --namespace anon/trace-check/cli`.

## Plan

The ids are wall-clock derived and went back by one, so a keyframe was placed into a group older than the one already opened. Open question: whether the fix belongs to the time-to-group mapping or to the stream's lack of timestamps (`moq-cli` stamps access units on arrival).
