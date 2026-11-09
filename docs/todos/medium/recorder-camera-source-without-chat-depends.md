# [M] Tell the camera from the screen share without the chat's `depends`

Next: in `services/recorder/src/track_selection.rs:234` (`main_source`), list what a meeting catalog holds, apart from the chat's `depends`, that marks the camera's renderGroup.

## Goal

A recorder unit test shows the camera recorded as the camera for a catalog whose chat has no `depends`, as draft-02 will allow (`low/msf-switch-to-draft-02.md`), and for one that lists a camera rendition (`video_480p`) before `video` with the microphone off, whose chat `depends` misses the rendition today.

## Plan

Settled 2026-11-09: do not list every default-source track in the chat's `depends` (`crates/broadcast/src/broadcast.rs:392`), since draft-02 drops it.
Open: MSF names no source kind, so the candidates are the track names `broadcast` gives the default source, or a kind the meeting app declares; settle it with the user.
