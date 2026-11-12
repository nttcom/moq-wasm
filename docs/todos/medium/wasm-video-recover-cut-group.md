# [M] Ask for a new group when a live video subgroup stream is reset

Next: in `crates/wasm/js/src/media/player/trackContext.ts:81`, where a reset group only logs `ended early`, tell an unexpected reset of the played video track apart from the expected ones.

## Goal

A viewer whose live video group loses its stream shows a picture again within about 1 s instead of at the next keyframe, up to 30 s later with the default keyframe interval.
A decoder that loses its group already asks for a new one (`LivePlayer.requestGroupAfterCut`), but it asks on a dropped delta, and no delta of a reset group arrives.

## Plan

The relay also resets streams the player expects to end (`group_sender.rs`: Forward 0 during review, `PastEnd` of an outgoing rendition), so asking on every reset would ask for groups nobody needs.
Cover the incoming rendition of a switch too: `TrackSubscriptions.requestNewGroup` asks once, so a group it asked for that is reset before its first object arrives leaves the switch waiting for the next keyframe.
