# msf

Types for the MSF catalog (draft-ietf-moq-msf-01) and its CMAF profile
(draft-ietf-moq-cmsf-01): `Catalog`, `Track`, `InitData` (an `initDataList`
entry), `Packaging`, `TrackRole` and the `MediaTimelineRecord` of the media
timeline track, plus what the drafts fix for every publisher: the `catalog`
track name, a track with only its required fields, a CMAF sibling of a track,
the inline init data a track's `initRef` names, and an independent catalog. Used
by the publishers, the relay E2E tests and, through `crates/wasm`, the browser
examples.
