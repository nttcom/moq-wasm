# msf

Pure-Rust data types, serialization, and validation for **MOQ Streaming Format (MSF)** catalogs (`draft-ietf-moq-msf-02`) and the **CMAF profile** (`draft-ietf-moq-cmsf-01`).

No async runtime or system dependencies; compiles on native targets and `wasm32`.

> **Publishing or subscribing to a full MSF broadcast?** Use [`sdk/rust`](../../sdk/rust/README.md) (`moq-wasm`) for apps or [`crates/broadcast`](../broadcast/README.md) for custom pipelines. Use `msf` directly when you only need catalog JSON types, validation, or timeline records.

## Install

```toml
[dependencies]
msf = { git = "https://github.com/nttcom/moq-wasm", rev = "<commit>" }
serde_json = "1"
```

## Quickstart: Build and validate a catalog

Inside a function returning `Result<(), Box<dyn std::error::Error>>`:

```rust
use msf::{Catalog, KnownPackaging, KnownTrackRole, Track};

let mut audio = Track::new("audio", KnownPackaging::Loc, true);
audio.role = Some(KnownTrackRole::Audio.into());
audio.codec = Some("mp4a.40.2".into());
audio.bitrate = Some(128_000);
audio.sample_rate = Some(48_000);
audio.channel_config = Some(msf::channel_config(2)); // "stereo"
let init = audio.set_inline_init_data("ERA=".into());

let mut catalog = Catalog::new(vec![audio], 1_759_924_160_383);
catalog.init_data_list = Some(vec![init]);

let bytes = serde_json::to_vec(&catalog)?;
let parsed = Catalog::parse(&bytes)?;

for track in parsed.loc_tracks() {
    println!("{} init={:?}", track.name, parsed.init_data(track));
}
```

## Key types & helpers

### Catalog & tracks

| Item | Role |
| --- | --- |
| `Catalog`, `CATALOG_TRACK_NAME` | MSF `"catalog"` document (`new`, `parse`, `loc_tracks()`, `init_data()`) |
| `Track`, `InitData` | Track entry with `cmaf_sibling()`, `set_inline_init_data()`, `has_role()`, and `dependencies()` |
| `Dependency` | Inter-track reference (`TrackName(String)` or `RenderGroup(u64)`) with `Dependency::on_tracks()` |
| `Packaging`, `KnownPackaging`, `TrackRole`, `KnownTrackRole` | Packaging (`loc`, `cmaf`, `mediatimeline`, `eventtimeline`) and track role enums with `Other(String)` fallback |

### Timelines, helpers & errors

| Item | Role |
| --- | --- |
| `MediaTimelineRecord` | Media timeline tuple `[pts_ms, [group_id, object_id], encoded_at_ms]` |
| `channel_config`, `channel_count` | Converts between channel counts and MSF `channelConfig` strings (`"mono"`, `"stereo"`, `"6ch"`) |
| `CatalogParseError` | Typed error: `Malformed`, `UnsupportedVersion`, `DeltaUpdate`, `Invalid` |

## Validation rules (`Catalog::parse`)

`Catalog::parse` enforces `draft-ietf-moq-msf-02`:

- **Version & deltas**: Requires `version: "draft-02"` (`UnsupportedVersion` otherwise) and rejects delta updates (`DeltaUpdate`).
- **Names & namespaces**: Rejects empty track names, empty namespace segments, or namespaces with > 32 `/`-delimited segments.
- **Dependencies**: Each `TrackName` entry in `depends` must resolve to exactly one other target track in the catalog (no self-references or ambiguous target names); each `RenderGroup` must exist on at least one track.
- **Unresolved `initRef`**: Not rejected at parse time—`catalog.init_data(track)` returns `None`, so check the lookup result when initialization data is required.

Required track fields by packaging and role:

| Packaging | Role | Required fields |
| --- | --- | --- |
| `loc` | `video` | `codec`, `bitrate`, `framerate` |
| `loc` | `audio` | `codec`, `bitrate`, `samplerate`, `channelConfig` |
| `loc` | `audiodescription` | `codec`, `samplerate`, `channelConfig`, `lang` |
| `loc` | `signlanguage` | `codec`, `framerate` |
| `eventtimeline` | *(any)* | `eventType` |

## Further reading

- Specifications: [`draft-ietf-moq-msf-02.txt`](../../docs/spec/draft-ietf-moq-msf-02.txt), [`draft-ietf-moq-cmsf-01.txt`](../../docs/spec/draft-ietf-moq-cmsf-01.txt)
- Broadcast layer built on `msf`: [`crates/broadcast`](../broadcast/README.md)
