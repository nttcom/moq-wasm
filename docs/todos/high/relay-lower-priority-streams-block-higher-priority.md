# [L] Find how unsent streams of a lower Subscriber Priority stop the relay from opening streams for a higher one

Next: reproduce it and watch the DEBUG lines of `GroupSender::open_stream` (`services/relay/src/modules/data_plane/egress/group_sender.rs`): `no stream credit` for the track without a timeout, `passed its delivery timeout unopened` for the higher one.

## Goal

A track of a lower Subscriber Priority that the relay cannot deliver delays only itself. Today its unsent streams fill the subscriber's stream credit, and the groups of a higher-priority video pass their DELIVERY TIMEOUT unopened.
Measured on a 1.5 Mbps downlink, three 0.92 Mbps cameras: the stage at 64, one at 128 with a 2 s timeout, one at 200 without. After about 80 s the stage drops from 30 fps to 0–20 fps.

## Plan

Draft-14 §9.2.1.2: without a DELIVERY TIMEOUT every Object is delivered, so these streams cannot be dropped; the publisher MAY end the subscription with TOO_FAR_BEHIND.
Read, not run: moq.dev (`moq-dev/moq` at `e0df316e2`, `rs/moq-net/src/transport/open.rs`) opens a session's streams in send order and resets the lowest-ranked open stream an open outranks.
Not confirmed: the stream limit the browser grants (about 80, from when the stalls start).
