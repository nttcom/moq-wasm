# [S] Drop the incoming rendition's groups before the splice start once a seek settles the switch

Next: in `crates/wasm/js/src/media/player/livePlayer.ts:758`, skip objects whose `groupId` is below `pending.splice.start` before calling `handleLocObject`.

## Goal

A seek right after an LOC rendition handover logs no `decoder error error=Decoding error.` from `moq-wasm::media::video_decoder`. Today `seek` → `abandonVideoSwitch` (`livePlayer.ts:805`) clears `videoSwitch`, so the settled branch of `handleIncomingVideo` passes every incoming object on, including the mid-group deltas of the group before the start.
Reproduced locally (RTMP, `--transcode`): seeking just after the ABR step up's `catalog codec changed` warning sent `video_480p` objects 2.. of group start−1 to the live decoder, which failed at once; without the seek no decoder error appears.

## Plan

Impact is small while the live picture hides behind the review; with `keepLiveWhileReviewing` the live picture would freeze until the next keyframe (not verified). `RenditionSplice.acceptIncoming` (`renditionSplice.ts:46`) already drops these while the switch is pending; reuse it rather than a second check.
