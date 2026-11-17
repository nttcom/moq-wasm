# [L] Find how unsent streams of a lower Subscriber Priority stop the relay from opening streams for a higher one

Next: reproduce it and read the stream state: on a downlink limited to 2 Mbps, subscribe a small track without a DELIVERY TIMEOUT at the Subscriber Priority of video the link cannot carry, and watch the DEBUG lines of `open_stream` (`services/relay/src/modules/data_plane/egress/group_sender.rs:611`): `no stream credit` for the track without a timeout, `passed its delivery timeout unopened` for the video.

## Goal

A track of a lower Subscriber Priority that the relay cannot deliver delays only itself. Today, after about 20 s, `open_stream` fails for every track of the session, a video of a higher Subscriber Priority included: its groups pass their DELIVERY TIMEOUT unopened.
Measured with four cameras, each a 1.7 Mbps video and a media timeline: one video at Subscriber Priority 64, the other three videos and the four media timelines at 128. With the media timelines at a priority above every video, no open failed in 4 minutes.

## Plan

Not confirmed: that the unsent media timeline streams are what holds the subscriber's stream credit (inferred: only they have no timeout), the stream limit the browser grants, and when quinn gets stream credit back.
Confirm these before proposing a change.
