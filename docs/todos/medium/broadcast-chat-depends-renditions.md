# [S] List every track of the default source in the chat's `depends`

Next: in `crates/broadcast/src/broadcast.rs:392` (`chat_track`), build `depends` from the default source's track names, the list `video_timeline` already depends on.

## Goal

A catalog that lists a camera rendition (`video_480p`) before `video`, with the microphone off, names the rendition in the chat's `depends`. The recorder's screen share recording then never takes the camera for another source (`services/recorder/src/track_selection.rs`, `SourceChoice::Main`): a recorder unit test with that catalog shows it.

## Plan

Open: whether draft-ietf-moq-msf-01 §8.2 ("all track names to which the event timeline track applies") covers the renditions and CMAF siblings; settle it with the user before changing `broadcast`.
