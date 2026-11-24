# msf

Pure-Rust data types and validation for MSF catalogs (`draft-ietf-moq-msf-02`) and the CMAF profile (`draft-ietf-moq-cmsf-01`).

## Usage

Build a catalog and parse/validate a received catalog payload:

```rust
use msf::{Catalog, KnownPackaging, Track};

let catalog = Catalog::new(
    vec![Track::new("video", KnownPackaging::Loc, true)],
    1_759_924_160_383,
);
let bytes = serde_json::to_vec(&catalog)?;

let parsed = Catalog::parse(&bytes)?;
for track in parsed.loc_tracks() {
    let init_data = parsed.init_data(track);
    println!("{} (init: {:?})", track.name, init_data.is_some());
}
```

## Key types

| Item | Role |
| --- | --- |
| `Catalog`, `CATALOG_TRACK_NAME` | MSF catalog document (`"catalog"` track) with `Catalog::new`, `Catalog::parse`, `loc_tracks()`, and `init_data()` |
| `Track`, `Dependency` | Track entry, codec/dimension metadata, and inter-track dependencies |
| `channel_config`, `channel_count` | Helpers converting between audio channel counts and MSF `channelConfig` strings |
| `InitData` | Inline Base64 entry in `initDataList` referenced by a track's `initRef` |
| `Packaging`, `KnownPackaging`, `TrackRole`, `KnownTrackRole` | Packaging (`loc`, `cmaf`, `mediatimeline`, `eventtimeline`) and track role enums |
| `MediaTimelineRecord` | Media timeline entry mapping presentation time, MoQ location, and wallclock |
| `CatalogParseError` | Typed error distinguishing malformed JSON, unsupported version, delta updates, and invalid tracks |

## Validation rules

`Catalog::parse` enforces `draft-ietf-moq-msf-02` rules:

- Requires `version: "draft-02"` (returns `UnsupportedVersion` otherwise).
- Rejects delta updates (`DeltaUpdate`).
- Rejects malformed track names/namespaces, unresolved `depends` references, and tracks missing fields required by their packaging or role (`Invalid`).
- Does not reject an unresolved `initRef`; `catalog.init_data(track)` returns `None`, so consumers that require initialisation data should check the lookup result.
