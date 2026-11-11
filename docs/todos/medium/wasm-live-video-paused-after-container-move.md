# [S] Keep live LOC video playing after the page moves the player's container

Next: in `crates/wasm/js/src/media/player/pictureStage.ts:10`, play `liveVideo` again on a `pause` event the player did not ask for (`LivePlayer.paused` is false).

## Goal

After a toggle of the meeting's stage view, a member's picture keeps updating. Today `PictureFrame` (`examples/browser/examples/meeting/src/components/MediaStreamElements.tsx:53`) removes the container in one task and appends it in the next, so Chrome pauses the `<video>`; a later rendition splice keeps the element, and its `videoWidth` stays at the old rendition.
Reproduced locally in `Auto follows the size of a member tile when the view changes` (with bob's mic muted 4 s after his camera): 9 of 32 runs failed at the 320 px assertion with `paused=true`, while the player's own frame size was 320x180. A full resubscribe hides it: the new `srcObject` autoplays.

## Plan

Fix it in the SDK, not the page: any page may re-parent the container, and only the player knows whether a pause was asked for.
