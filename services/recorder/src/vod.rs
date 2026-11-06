use std::collections::{BTreeMap, BTreeSet, HashMap};

use anyhow::{Context, Result};
use bytes::Bytes;
use moqt::object::{ExtensionHeaders, ForwardingPreference, Location, Object, ObjectStatus};
use msf::{
    CATALOG_TRACK_NAME, Catalog, InitData, KnownPackaging, KnownTrackRole, MediaTimelineRecord,
    Packaging, Track,
};
use serde_json::Value;

use crate::{
    archive_reader::{StoredRecording, StoredTrack},
    recorder::Content,
    room_recording::TRANSCRIPT_TRACK_NAME,
};

const TRANSCRIPT_EVENT_TYPE: &str = "com.skyway.transcript.v1";

const BUILT_LOCATION: Location = Location {
    group_id: 0,
    object_id: 0,
};

/// A track a replay serves instead of the stored one: one object at
/// [`BUILT_LOCATION`].
pub(crate) struct BuiltTrack {
    pub(crate) namespace: String,
    pub(crate) name: String,
    pub(crate) object: Object,
}

impl BuiltTrack {
    fn new(namespace: &str, name: &str, payload: Vec<u8>) -> Self {
        Self {
            namespace: namespace.to_string(),
            name: name.to_string(),
            object: Object {
                location: BUILT_LOCATION,
                // Subgroup 0, as draft-ietf-moq-msf-01 §5 requires of a catalog object.
                forwarding: ForwardingPreference::Subgroup { subgroup_id: 0 },
                publisher_priority: 0,
                status: ObjectStatus::Normal,
                extension_headers: ExtensionHeaders::default(),
                payload: Bytes::from(payload),
                ends_group: false,
            },
        }
    }
}

/// The VOD catalog and full-length media timelines each replay namespace
/// serves in place of the stored ones, and its speaker's part of the
/// transcriber's transcript. They are built from the stored objects alone, so every instance replaying the recording serves the
/// same bytes at the same location (draft-ietf-moq-transport-14 §2.1).
pub(crate) async fn build_tracks(recording: &StoredRecording) -> Result<Vec<BuiltTrack>> {
    let mut transcripts = transcripts(recording).await?;
    let mut built = Vec::new();
    for namespace in recording.namespaces() {
        let transcript = transcripts.remove(namespace).unwrap_or_default();
        built.extend(namespace_tracks(recording, namespace, transcript).await?);
    }
    Ok(built)
}

/// The tracks `build_tracks` builds for one namespace; none when it recorded
/// no catalog. `transcript` is left out when the catalog lists no audio track.
async fn namespace_tracks(
    recording: &StoredRecording,
    namespace: &str,
    transcript: Vec<Value>,
) -> Result<Vec<BuiltTrack>> {
    let stored: Vec<&StoredTrack> = recording
        .tracks
        .iter()
        .filter(|track| track.namespace == namespace)
        .map(AsRef::as_ref)
        .collect();
    let Some(catalog_track) = stored.iter().find(|track| track.name == CATALOG_TRACK_NAME) else {
        return Ok(Vec::new());
    };
    let catalogs: Vec<Catalog> = documents(recording, catalog_track, |payload| {
        serde_json::from_slice(payload)
    })
    .await?;
    let names: Vec<&str> = stored
        .iter()
        .map(|track| track.name.as_str())
        .filter(|name| *name != CATALOG_TRACK_NAME)
        .collect();
    let mut listed = listed_tracks(&catalogs, &names);
    let stored_track = |name: &str| stored.iter().find(|track| track.name == name);
    let mut built = Vec::new();
    let mut first_records_ms = Vec::new();
    for timeline in listed
        .tracks
        .iter()
        .filter(|track| track.packaging == Packaging::Known(KnownPackaging::MediaTimeline))
    {
        let mut groups = BTreeSet::new();
        for name in timeline.depends.iter().flatten() {
            if let Some(track) = stored_track(name) {
                groups.extend(track.groups()?);
            }
        }
        let track = stored_track(&timeline.name).context("listed track is not stored")?;
        let documents =
            documents(recording, track, |payload| serde_json::from_slice(payload)).await?;
        let records = full_timeline(documents, &groups);
        first_records_ms.extend(records.first().map(MediaTimelineRecord::encoded_at_ms));
        built.push(BuiltTrack::new(
            namespace,
            &timeline.name,
            serde_json::to_vec(&records)?,
        ));
    }
    let media = listed
        .tracks
        .iter()
        .filter(|track| track.packaging == Packaging::Known(KnownPackaging::Loc))
        .filter_map(|track| stored_track(&track.name)?.capture_range_us());
    let duration_ms = duration_ms(media, first_records_ms.into_iter().min());
    let audio: Vec<String> = listed
        .tracks
        .iter()
        .filter(|track| track.has_role(KnownTrackRole::Audio))
        .map(|track| track.name.clone())
        .collect();
    if !transcript.is_empty() && !audio.is_empty() {
        listed.tracks.push(transcript_track(audio));
        built.push(BuiltTrack::new(
            namespace,
            TRANSCRIPT_TRACK_NAME,
            serde_json::to_vec(&transcript)?,
        ));
    }
    for track in &mut listed.tracks {
        track.is_live = false;
        track.track_duration = duration_ms;
        // Without it each track inherits the replay namespace (§5.2.2).
        track.namespace = None;
        // Ignored once a track is not live (§5.2.8).
        track.target_latency = None;
    }
    let catalog = Catalog {
        // §5.1.2: omitted when isLive is false.
        generated_at: None,
        is_complete: Some(true),
        init_data_list: (!listed.init_data.is_empty()).then_some(listed.init_data),
        ..Catalog::new(listed.tracks, 0)
    };
    built.push(BuiltTrack::new(
        namespace,
        CATALOG_TRACK_NAME,
        serde_json::to_vec(&catalog)?,
    ));
    tracing::debug!(
        track_namespace = namespace,
        tracks = built.len(),
        "replay catalog, media timelines and transcript built"
    );
    Ok(built)
}

/// The transcriber's records, split by the camera replay namespace of the
/// speaker's first session whose captured media spans the record's `t`, each
/// list sorted by `t` with a record stored twice kept once. `t` and the LOC
/// capture timestamps are both Unix wallclock time. The transcriber names a
/// speaker after the one namespace element below the room, so a record is
/// left out when `<room>/<speaker>` is no member or `t` falls outside all of
/// that member's sessions.
async fn transcripts(recording: &StoredRecording) -> Result<HashMap<&str, Vec<Value>>> {
    let mut records = BTreeMap::new();
    for track in recording
        .transcriber
        .iter()
        .filter(|track| track.name == TRANSCRIPT_TRACK_NAME)
    {
        let documents: Vec<Vec<Value>> =
            documents(recording, track, |payload| serde_json::from_slice(payload)).await?;
        for record in documents.into_iter().flatten() {
            if let Some(t) = record["t"].as_u64() {
                records.insert((t, record.to_string()), record);
            }
        }
    }
    let sessions: Vec<(&str, &str, (u64, u64))> = recording
        .namespaces()
        .into_iter()
        .filter_map(|namespace| {
            let tracks: Vec<&StoredTrack> = recording
                .tracks
                .iter()
                .filter(|track| track.namespace == namespace)
                .map(AsRef::as_ref)
                .collect();
            let first = tracks
                .first()
                .filter(|track| track.content == Content::Camera)?;
            let captured = span(tracks.iter().filter_map(|track| track.capture_range_us()))?;
            Some((namespace, first.source_namespace.as_str(), captured))
        })
        .collect();
    let mut split: HashMap<&str, Vec<Value>> = HashMap::new();
    for ((t, _), record) in records {
        let Some(speaker) = record["data"]["speaker"].as_str() else {
            continue;
        };
        let member = format!("{}/{speaker}", recording.room);
        let t_us = t.saturating_mul(1000);
        if let Some((namespace, _, _)) = sessions
            .iter()
            .find(|(_, source, (start, end))| *source == member && (*start..=*end).contains(&t_us))
        {
            split.entry(namespace).or_default().push(record);
        }
    }
    Ok(split)
}

/// A participant's part of the transcript as an event timeline
/// (draft-ietf-moq-msf-01 §8.2). It is listed in their own catalog because
/// `depends`, which §8.2 requires, names tracks of the timeline's own
/// namespace (§5.2.14).
fn transcript_track(audio: Vec<String>) -> Track {
    let mut track = Track::new(TRANSCRIPT_TRACK_NAME, KnownPackaging::EventTimeline, false);
    track.event_type = Some(TRANSCRIPT_EVENT_TYPE.to_string());
    track.role = Some(KnownTrackRole::Subtitle.into());
    track.label = Some("Transcript".to_string());
    track.depends = Some(audio);
    track.mime_type = Some("application/json".to_string());
    track
}

/// Every stored object of `track` as `parse` reads it, in location order;
/// one that does not parse is left out.
async fn documents<T>(
    recording: &StoredRecording,
    track: &StoredTrack,
    parse: impl Fn(&[u8]) -> serde_json::Result<T>,
) -> Result<Vec<T>> {
    Ok(recording
        .payloads(track)
        .await?
        .iter()
        .filter_map(|payload| match parse(payload) {
            Ok(document) => Some(document),
            Err(error) => {
                tracing::warn!(
                    track_namespace = %track.namespace,
                    track_name = %track.name,
                    %error,
                    "stored object is not the JSON the track carries; left out"
                );
                None
            }
        })
        .collect())
}

/// The tracks of a VOD catalog and the `initDataList` entries they refer to.
struct Listed {
    tracks: Vec<Track>,
    init_data: Vec<InitData>,
}

/// Each of `names` that a stored catalog object describes, in that order, as
/// the last of them describes it, with:
/// - `depends` naming the listed tracks any of them named, in listed order;
/// - one `renderGroup` for the LOC tracks, the first one's: a replay namespace
///   holds one source, but `broadcast` numbers the sources anew in each
///   catalog, so tracks described by different objects may disagree.
fn listed_tracks(catalogs: &[Catalog], names: &[&str]) -> Listed {
    let described = |name: &str| {
        catalogs.iter().rev().find_map(|catalog| {
            let track = catalog
                .tracks
                .iter()
                .flatten()
                .find(|track| track.name == name)?;
            Some((catalog, track))
        })
    };
    let listed: Vec<(&Catalog, &Track)> = names.iter().filter_map(|name| described(name)).collect();
    let listed_names: Vec<&str> = listed
        .iter()
        .map(|(_, track)| track.name.as_str())
        .collect();
    let render_group = listed
        .iter()
        .find(|(_, track)| track.packaging == Packaging::Known(KnownPackaging::Loc))
        .and_then(|(_, track)| track.render_group);
    let mut init_data: Vec<InitData> = Vec::new();
    let tracks = listed
        .iter()
        .map(|(catalog, track)| {
            let mut track = (*track).clone();
            if track.depends.is_some() {
                let named: BTreeSet<&str> = catalogs
                    .iter()
                    .flat_map(|catalog| catalog.tracks.iter().flatten())
                    .filter(|other| other.name == track.name)
                    .flat_map(|other| other.depends.iter().flatten())
                    .map(String::as_str)
                    .collect();
                track.depends = Some(
                    listed_names
                        .iter()
                        .filter(|name| named.contains(*name))
                        .map(|name| name.to_string())
                        .collect(),
                );
            }
            if track.packaging == Packaging::Known(KnownPackaging::Loc) {
                track.render_group = render_group;
            }
            if let (Some(id), Some(data)) = (&track.init_ref, catalog.init_data(&track))
                && !init_data.iter().any(|entry| entry.id == *id)
            {
                init_data.push(InitData {
                    id: id.clone(),
                    r#type: "inline".to_string(),
                    data: data.to_string(),
                });
            }
            track
        })
        .collect();
    Listed { tracks, init_data }
}

/// The records of every stored timeline object merged by location, the first
/// stored one kept for a location, and only those of `groups`, in location
/// order. Each stored object lists the last 60 s; together they cover the
/// whole recording, and presentation time and wallclock stay the
/// publisher's (draft-ietf-moq-msf-01 §7.1.1).
fn full_timeline(
    documents: Vec<Vec<MediaTimelineRecord>>,
    groups: &BTreeSet<u64>,
) -> Vec<MediaTimelineRecord> {
    let mut records = BTreeMap::new();
    for record in documents.into_iter().flatten() {
        if groups.contains(&record.location().0) {
            records.entry(record.location()).or_insert(record);
        }
    }
    records.into_values().collect()
}

fn span(ranges: impl Iterator<Item = (u64, u64)>) -> Option<(u64, u64)> {
    ranges.reduce(|(first, last), (start, end)| (first.min(start), last.max(end)))
}

/// Milliseconds from the earlier of the first timeline record and the first
/// captured media object to the last captured one; `None` without media.
fn duration_ms(
    media_capture_us: impl Iterator<Item = (u64, u64)>,
    first_record_ms: Option<u64>,
) -> Option<u64> {
    let (first_us, last_us) = span(media_capture_us)?;
    let start_ms = first_record_ms.map_or(first_us / 1000, |record| record.min(first_us / 1000));
    Some((last_us / 1000).saturating_sub(start_ms))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{recorded_camera, recorded_meeting, transcript_record};
    use serde_json::json;

    fn track(name: &str, packaging: KnownPackaging) -> Track {
        Track::new(name, packaging, true)
    }

    fn with_depends(mut track: Track, depends: &[&str]) -> Track {
        track.depends = Some(depends.iter().map(|name| name.to_string()).collect());
        track
    }

    fn in_source(mut track: Track, render_group: u64) -> Track {
        track.render_group = Some(render_group);
        track
    }

    fn record(presentation_ms: u64, group_id: u64, encoded_at_ms: u64) -> MediaTimelineRecord {
        MediaTimelineRecord::new(presentation_ms, group_id, 0, encoded_at_ms)
    }

    #[test]
    fn lists_each_stored_track_as_the_last_catalog_describing_it() {
        // Arrange: the audio left the catalog before the end, and the camera's renderGroup changed with it
        let mut audio = in_source(track("audio", KnownPackaging::Loc), 1);
        audio.codec = Some("mp4a.40.2".to_string());
        let init_data = audio.set_inline_init_data("ERA=".to_string());
        let mut first = Catalog::new(
            vec![
                in_source(track("video", KnownPackaging::Loc), 1),
                audio,
                with_depends(
                    track("chat", KnownPackaging::EventTimeline),
                    &["video", "audio"],
                ),
            ],
            1,
        );
        first.init_data_list = Some(vec![init_data.clone()]);
        let mut video = in_source(track("video", KnownPackaging::Loc), 2);
        video.height = Some(720);
        let last = Catalog::new(
            vec![
                in_source(track("screen", KnownPackaging::Loc), 1),
                video,
                with_depends(track("chat", KnownPackaging::EventTimeline), &["video"]),
            ],
            2,
        );

        // Act
        let listed = listed_tracks(&[first, last], &["video", "audio", "chat", "transcript"]);

        // Assert: the stored transcript is described by no catalog
        let names: Vec<&str> = listed
            .tracks
            .iter()
            .map(|track| track.name.as_str())
            .collect();
        assert_eq!(names, ["video", "audio", "chat"]);
        assert_eq!(listed.tracks[0].height, Some(720));
        assert_eq!(listed.tracks[0].render_group, listed.tracks[1].render_group);
        assert_eq!(
            listed.tracks[2].depends.as_deref(),
            Some(&["video".to_string(), "audio".to_string()][..])
        );
        assert_eq!(listed.init_data, [init_data]);
    }

    #[test]
    fn a_full_timeline_merges_the_records_by_location_and_keeps_the_recorded_groups() {
        // Arrange: each object lists the records of its last three groups; groups 1 and 4 were not recorded
        let documents = vec![
            vec![
                record(0, 1, 100),
                record(1000, 2, 1100),
                record(2000, 3, 1200),
            ],
            vec![
                record(1000, 2, 1100),
                record(2000, 3, 1200),
                record(3000, 4, 1300),
            ],
            vec![
                record(2000, 3, 1200),
                record(3000, 4, 1300),
                record(4000, 5, 1400),
            ],
        ];
        let groups = BTreeSet::from([2, 3, 5]);

        // Act
        let timeline = full_timeline(documents, &groups);

        // Assert
        assert_eq!(
            timeline,
            [
                record(1000, 2, 1100),
                record(2000, 3, 1200),
                record(4000, 5, 1400)
            ]
        );
    }

    #[test]
    fn the_duration_starts_at_the_earlier_of_the_first_record_and_the_first_capture() {
        // Act
        let from_record = duration_ms([(2_500_000, 9_000_000)].into_iter(), Some(2_000));
        let from_capture = duration_ms([(2_500_000, 9_000_000)].into_iter(), Some(3_000));
        let without_media = duration_ms(std::iter::empty(), Some(2_000));

        // Assert
        assert_eq!(
            (from_record, from_capture, without_media),
            (Some(7_000), Some(6_500), None)
        );
    }

    #[tokio::test]
    async fn builds_a_complete_vod_catalog_and_a_full_timeline_from_the_stored_tracks() {
        // Arrange
        let recording = recorded_camera().await;

        // Act
        let built = build_tracks(&recording).await.unwrap();

        // Assert
        let names: Vec<&str> = built.iter().map(|track| track.name.as_str()).collect();
        assert_eq!(names, ["video_timeline", "catalog"]);
        let catalog: Catalog = serde_json::from_slice(&built[1].object.payload).unwrap();
        assert_eq!(
            (catalog.is_complete, catalog.generated_at),
            (Some(true), None)
        );
        let tracks = catalog.tracks.unwrap();
        for track in &tracks {
            assert!(!track.is_live, "{}", track.name);
            assert_eq!(track.track_duration, Some(2_000), "{}", track.name);
            assert_eq!(track.namespace, None, "{}", track.name);
            assert_eq!(track.target_latency, None, "{}", track.name);
        }
        assert_eq!(
            tracks[1].depends.as_deref(),
            Some(&["video".to_string()][..])
        );
        let timeline: Vec<MediaTimelineRecord> =
            serde_json::from_slice(&built[0].object.payload).unwrap();
        let groups: Vec<u64> = timeline.iter().map(|record| record.location().0).collect();
        assert_eq!(groups, [10, 11, 12]);
        assert!(
            built
                .iter()
                .all(|track| track.object.location == BUILT_LOCATION)
        );
    }

    #[tokio::test]
    async fn another_replay_of_the_recording_builds_the_same_bytes() {
        // Arrange
        let transcript = || {
            vec![
                json!([]),
                json!([transcript_record(1_700_000_001_500, "alice")]),
                json!([transcript_record(1_700_000_001_000, "alice")]),
            ]
        };
        let recordings = [
            (recorded_camera().await, recorded_camera().await),
            (
                recorded_meeting(transcript()).await,
                recorded_meeting(transcript()).await,
            ),
        ];

        for (first, second) in recordings {
            // Act
            let built = build_tracks(&first).await.unwrap();
            let again = build_tracks(&second).await.unwrap();

            // Assert
            let objects = |tracks: &[BuiltTrack]| -> Vec<(String, Object)> {
                tracks
                    .iter()
                    .map(|track| (track.namespace.clone(), track.object.clone()))
                    .collect()
            };
            assert_eq!(objects(&built), objects(&again));
        }
    }

    const ALICE_FIRST: &str = "anon/room/recordings/id/alice/camera/0";
    const ALICE_SECOND: &str = "anon/room/recordings/id/alice/camera/1";

    fn transcript_in(built: &[BuiltTrack], namespace: &str) -> Option<Vec<Value>> {
        let track = built
            .iter()
            .find(|track| track.namespace == namespace && track.name == "transcript")?;
        Some(serde_json::from_slice(&track.object.payload).unwrap())
    }

    #[tokio::test]
    async fn puts_each_record_in_the_speakers_session_whose_media_covers_it() {
        // Arrange: records out of order across objects, one stored twice
        let recording = recorded_meeting(vec![
            json!([]),
            json!([transcript_record(1_700_000_001_500, "alice")]),
            json!([transcript_record(1_700_000_011_000, "alice")]),
            json!([transcript_record(1_700_000_001_000, "alice")]),
            json!([transcript_record(1_700_000_001_000, "alice")]),
        ])
        .await;

        // Act
        let built = build_tracks(&recording).await.unwrap();

        // Assert
        assert_eq!(
            transcript_in(&built, ALICE_FIRST),
            Some(vec![
                transcript_record(1_700_000_001_000, "alice"),
                transcript_record(1_700_000_001_500, "alice"),
            ])
        );
        assert_eq!(
            transcript_in(&built, ALICE_SECOND),
            Some(vec![transcript_record(1_700_000_011_000, "alice")])
        );
        let transcript = built
            .iter()
            .find(|track| track.name == "transcript")
            .unwrap();
        assert_eq!(transcript.object.location, BUILT_LOCATION);
    }

    #[tokio::test]
    async fn leaves_out_records_of_no_participant_no_session_or_no_audio() {
        // Arrange: alice between her sessions, bob without a microphone,
        // carol never joined, the transcriber's own notice
        let recording = recorded_meeting(vec![json!([
            transcript_record(1_700_000_005_000, "alice"),
            transcript_record(1_700_000_001_000, "bob"),
            transcript_record(1_700_000_001_000, "carol"),
            transcript_record(1_700_000_001_000, "transcriber"),
        ])])
        .await;

        // Act
        let built = build_tracks(&recording).await.unwrap();

        // Assert
        assert!(built.iter().all(|track| track.name != "transcript"));
    }

    #[tokio::test]
    async fn lists_a_participants_transcript_as_a_subtitle_timeline_of_their_audio() {
        // Arrange
        let recording =
            recorded_meeting(vec![json!([transcript_record(1_700_000_001_000, "alice")])]).await;

        // Act
        let built = build_tracks(&recording).await.unwrap();

        // Assert: the transcript and every other track last as long as the session's media
        let catalog = built
            .iter()
            .find(|track| track.namespace == ALICE_FIRST && track.name == CATALOG_TRACK_NAME)
            .unwrap();
        let catalog: Catalog = serde_json::from_slice(&catalog.object.payload).unwrap();
        let tracks = catalog.tracks.unwrap();
        let mut transcript = Track::new("transcript", KnownPackaging::EventTimeline, false);
        transcript.event_type = Some("com.skyway.transcript.v1".to_string());
        transcript.role = Some(KnownTrackRole::Subtitle.into());
        transcript.label = Some("Transcript".to_string());
        transcript.depends = Some(vec!["audio".to_string()]);
        transcript.mime_type = Some("application/json".to_string());
        transcript.track_duration = Some(2_000);
        assert_eq!(tracks.last(), Some(&transcript));
        assert!(
            tracks
                .iter()
                .all(|track| track.track_duration == Some(2_000))
        );
    }
}
