# [M] Play a source whose recorded rendition changed mid-recording

Next: in `services/recorder/src/vod.rs` (`listed_tracks`), see how the two renditions of one `altGroup` a recording switched between are listed.

## Goal

A screen share whose first catalog listed only `screenshare_480p` is recorded as one group of it, then `screenshare_720p`. The replay lists both in one `altGroup`, `LivePlayer` picks `screenshare_480p`, every later FETCH fails with INVALID_RANGE, and the player loses its range, so its `PlaybackGroup` never ends. Seen in the recording-player E2E's arrangement when the recording starts before the screen share.

## Plan

Open: one track per source in the replay (stitched), or a catalog that tells the player each rendition's range. The browser SDK plan lists this as out of scope (「録画を `LivePlayer` で再生する」, 範囲外), so settle it with the user first.
