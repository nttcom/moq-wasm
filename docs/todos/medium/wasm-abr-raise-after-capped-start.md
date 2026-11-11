# [S] Let Auto raise a rendition it started on because of the picture height

Next: in `crates/wasm/js/src/media/player/livePlayer.ts:915`, give `AdaptiveBitrate` the ceiling `startingTrack` capped the ladder to, so the next `decideUnderHeight` sees a taller picture as `raise`.

## Goal

A viewer whose video starts on `camera_low` in a 126 px tile steps up within 8 s when the tile grows to 285 px. Today `decideUnderHeight` (`adaptiveBitrate.ts:206`) detects a raise only against the ceiling of its previous call; when the tile grows before the first call after a fresh subscription, the old ceiling is the top one, so Auto waits for the 13 s calm spell.
Reproduced locally: 5 of 32 runs of `Auto follows the size of a member tile when the view changes` failed at the 640 px assertion this way, after the stage view's resubscription started on `camera_low` and the gallery click came 200 ms later.
