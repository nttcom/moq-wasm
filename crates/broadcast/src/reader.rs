mod track_receive_task;

use std::sync::Arc;

use anyhow::{Context, Result};
use mediapack::{MediaEvent, StreamSet, Timestamp};
use moqt::{
    Session, Subscription,
    subscribe::{FilterType, SubscribeOption},
};
use tokio::sync::mpsc;

use crate::{
    CatalogReader,
    reader::track_receive_task::{StartPoint, TrackKind, TrackReceiveTask},
    selection::{Selection, TrackChoice, select_tracks},
};

const MEDIA_CHANNEL_CAPACITY: usize = 256;
/// Objects the relay has not sent this long after receiving them would
/// play late, so it drops them (draft-14 §9.2.1.2). Same as the browser player.
const DELIVERY_TIMEOUT_MS: u64 = 2_000;

/// Subscribes to an MSF broadcast and yields its media events: the LOC video
/// and audio tracks `TrackChoice` selects, demuxed from one shared capture
/// origin so the two stay aligned.
pub struct BroadcastReader {
    media_receiver: mpsc::Receiver<MediaEvent>,
    start: Arc<StartPoint>,
    _tasks: Vec<TrackReceiveTask>,
}

impl BroadcastReader {
    /// Reads the catalog of the broadcast at `namespace` until it lists a track
    /// `choice` selects, then subscribes to the selected tracks. Fails when the
    /// catalog cannot be read or parsed, ends before it lists such a track, lists
    /// a selected track `select_tracks` rejects, or a SUBSCRIBE fails.
    pub async fn run(session: &Session, namespace: &str, choice: TrackChoice<'_>) -> Result<Self> {
        let tracks = wait_for_selection(session, namespace, choice).await?;
        tracing::info!(
            namespace,
            video = tracks
                .video
                .as_ref()
                .map_or("-", |video| video.name.as_str()),
            audio = tracks
                .audio
                .as_ref()
                .map_or("-", |audio| audio.name.as_str()),
            "catalog resolved"
        );

        let (media_sender, media_receiver) = mpsc::channel(MEDIA_CHANNEL_CAPACITY);
        media_sender
            .send(MediaEvent::Streams(StreamSet {
                has_video: tracks.video.is_some(),
                has_audio: tracks.audio.is_some(),
            }))
            .await
            .expect("the receiver is held by this reader");
        let start = Arc::new(StartPoint::new(tracks.video.is_some()));
        let mut kinds = Vec::new();
        if let Some(video) = tracks.video {
            kinds.push((video.name, TrackKind::Video(video.codec)));
        }
        if let Some(audio) = tracks.audio {
            kinds.push((audio.name, TrackKind::Audio(audio.config)));
        }
        let mut tasks = Vec::new();
        for (name, kind) in kinds {
            // NextGroupStart begins the video at a keyframe. An audio object decodes
            // alone, and a publisher that sends its audio as one endless group never
            // starts another, so the audio continues after the newest object
            // (draft-ietf-moq-transport-14 §9.7).
            let filter_type = match kind {
                TrackKind::Video(_) => FilterType::NextGroupStart,
                TrackKind::Audio(_) => FilterType::LargestObject,
            };
            let subscription = subscribe(session, namespace, &name, filter_type).await?;
            let reader = session.subscriber().track_reader(&subscription);
            tasks.push(TrackReceiveTask::run(
                name,
                reader,
                kind,
                start.clone(),
                media_sender.clone(),
            ));
        }
        Ok(Self {
            media_receiver,
            start,
            _tasks: tasks,
        })
    }

    /// `None` once every track has ended.
    pub async fn next_event(&mut self) -> Option<MediaEvent> {
        self.media_receiver.recv().await
    }

    /// The LOC capture timestamp that presentation time zero stands for on
    /// every track, or `Timestamp::ZERO` when the first admitted object
    /// carried none; `None` until a track admits an object.
    pub fn capture_origin(&self) -> Option<Timestamp> {
        self.start.capture_origin()
    }
}

async fn wait_for_selection(
    session: &Session,
    namespace: &str,
    choice: TrackChoice<'_>,
) -> Result<Selection> {
    let mut catalogs = CatalogReader::open(session, namespace).await?;
    while let Some(catalog) = catalogs.next().await? {
        if let Some(selection) = select_tracks(&catalog, choice)? {
            return Ok(selection);
        }
        tracing::info!(namespace, "waiting for the catalog to list a track to read");
    }
    anyhow::bail!("catalog track ended before it listed a track to read")
}

async fn subscribe(
    session: &Session,
    namespace: &str,
    name: &str,
    filter_type: FilterType,
) -> Result<Subscription> {
    let option = SubscribeOption {
        filter_type,
        // A track with long groups starts one when asked (draft-18 §5.1.3.1).
        new_group_request: (filter_type == FilterType::NextGroupStart).then_some(0),
        delivery_timeout: Some(DELIVERY_TIMEOUT_MS),
        ..SubscribeOption::default()
    };
    session
        .subscriber()
        .subscribe(namespace.to_string(), name.to_string(), option)
        .await
        .with_context(|| format!("subscribe {namespace}/{name}"))
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use bytes::Bytes;
    use mediapack::{
        AudioSample,
        loc::{self, LocObject},
        mpegts,
    };
    use moqt::publish::{GroupBoundary, OutgoingObject, Retention};

    use super::*;
    use crate::test_support::{FakePublisher, connect};
    use msf::{Catalog, KnownPackaging, KnownTrackRole, Track};

    const NAMESPACE: &str = "anon/room/alice";
    const FIXTURE_TS: &[u8] = include_bytes!("../../mediapack/fixtures/testsrc.ts");
    /// TOC configuration 31, code 0: one 20 ms CELT frame.
    const OPUS_20_MS_FRAME: [u8; 4] = [0xFC, 0x01, 0x02, 0x03];

    fn loc_track(name: &str, role: KnownTrackRole) -> Track {
        let mut track = Track::new(name, KnownPackaging::Loc, true);
        track.role = Some(role.into());
        track
    }

    fn catalog_with(tracks: Vec<Track>) -> Catalog {
        Catalog::new(tracks, 0)
    }

    fn first_keyframe() -> LocObject {
        let mut demuxer = mpegts::Demuxer::new();
        let events = demuxer.push(FIXTURE_TS).unwrap();
        let muxer = loc::Muxer::new(Timestamp::from_micros(1_700_000_000_000_000));
        events
            .iter()
            .find_map(|event| match event {
                MediaEvent::Video(sample) if sample.is_keyframe => muxer.push(event),
                _ => {
                    muxer.push(event);
                    None
                }
            })
            .unwrap()
    }

    fn opus_object(capture_micros: u64) -> LocObject {
        LocObject {
            extensions: vec![loc::LocExtension::varint(loc::TIMESTAMP_ID, capture_micros)],
            payload: Bytes::from_static(&OPUS_20_MS_FRAME),
        }
    }

    fn meeting_catalog() -> Bytes {
        let mut audio = loc_track("audio", KnownTrackRole::Audio);
        audio.codec = Some("opus".to_owned());
        audio.sample_rate = Some(48_000);
        audio.channel_config = Some("2".to_owned());
        audio.bitrate = Some(64_000);
        let mut video = loc_track("video", KnownTrackRole::Video);
        video.codec = Some("avc1.42e01e".to_owned());
        video.bitrate = Some(1_000_000);
        video.framerate = Some(30.0);
        let catalog = catalog_with(vec![video, audio]);
        Bytes::from(serde_json::to_vec(&catalog).unwrap())
    }

    async fn next_matching<T>(
        reader: &mut BroadcastReader,
        mut matching: impl FnMut(MediaEvent) -> Option<T>,
    ) -> T {
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let event = reader.next_event().await.expect("the tracks ended");
                if let Some(found) = matching(event) {
                    return found;
                }
            }
        })
        .await
        .expect("no matching media event arrived")
    }

    #[tokio::test]
    async fn reads_audio_that_a_publisher_sends_as_one_endless_group() {
        // Arrange: like a meeting member, the audio stays in one group numbered below the video's
        let mut publisher = FakePublisher::spawn();
        let origin = publisher.origin.clone();
        let keyframe = first_keyframe();
        let video_capture = keyframe.capture_timestamp().unwrap().micros();
        for track in [msf::CATALOG_TRACK_NAME, "video", "audio"] {
            origin
                .ensure_track(NAMESPACE, track, 0, Retention::NewestOnly)
                .await;
        }
        let write = |track, group, object: LocObject| {
            origin.write(
                NAMESPACE,
                track,
                OutgoingObject {
                    group,
                    extension_headers: loc::to_extension_headers(&object.extensions),
                    payload: object.payload,
                },
            )
        };
        let write_catalog = |group| {
            origin.write(
                NAMESPACE,
                msf::CATALOG_TRACK_NAME,
                OutgoingObject::plain(group, meeting_catalog()),
            )
        };
        write_catalog(GroupBoundary::Next).await.unwrap();
        let audio_group = GroupBoundary::At(video_capture - 10_000_000);
        write("audio", audio_group, opus_object(video_capture - 40_000))
            .await
            .unwrap();
        let relay = connect(&publisher.url).await;
        let session = relay.clone();
        let reading = tokio::spawn(async move {
            BroadcastReader::run(&session, NAMESPACE, TrackChoice::default()).await
        });
        assert_eq!(publisher.next_served().await, msf::CATALOG_TRACK_NAME);
        write_catalog(GroupBoundary::Within).await.unwrap();
        for track in ["video", "audio"] {
            assert_eq!(publisher.next_served().await, track);
        }

        // Act: audio captured before the video's keyframe opens the audio stream and is held back
        write("video", GroupBoundary::At(video_capture), keyframe)
            .await
            .unwrap();
        write(
            "audio",
            GroupBoundary::Within,
            opus_object(video_capture - 20_000),
        )
        .await
        .unwrap();
        let mut reader = tokio::time::timeout(Duration::from_secs(5), reading)
            .await
            .expect("no audio stream opened")
            .unwrap()
            .unwrap();
        next_matching(&mut reader, |event| {
            matches!(event, MediaEvent::Video(_)).then_some(())
        })
        .await;
        write(
            "audio",
            GroupBoundary::Within,
            opus_object(video_capture + 20_000),
        )
        .await
        .unwrap();
        let audio = next_matching(&mut reader, |event| match event {
            MediaEvent::Audio(sample) => Some(sample),
            _ => None,
        })
        .await;

        // Assert: the audio is timed from the video's first capture timestamp
        assert_eq!(
            audio,
            AudioSample {
                data: Bytes::from_static(&OPUS_20_MS_FRAME),
                pts: Timestamp::from_micros(20_000),
                audio_level: None
            }
        );
        drop(reader);
        relay.close().await;
    }
}
