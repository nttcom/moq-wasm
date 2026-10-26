mod track_receive_task;

use std::sync::Arc;

use anyhow::{Context, Result, bail, ensure};
use base64::{Engine, engine::general_purpose::STANDARD as BASE64};
use mediapack::{AudioConfig, MediaEvent, StreamSet, Timestamp, aac::AudioSpecificConfig};
use moqt::{Session, subscribe::FilterType};
use msf::{Catalog, KnownTrackRole, Track};
use tokio::sync::mpsc;

use crate::{
    broadcast_reader::track_receive_task::{StartPoint, TrackKind, TrackReceiveTask},
    relay_session::{read_catalog, subscribe, track_reader},
};

const MEDIA_CHANNEL_CAPACITY: usize = 256;
const PCMA_CODEC: &str = "pcma";
const OPUS_CODEC: &str = "opus";

/// Subscribes to an MSF broadcast and yields its media events: the first
/// LOC video track and the first LOC audio track of the catalog, demuxed
/// from one shared capture origin so the two stay aligned.
pub struct BroadcastReader {
    media_receiver: mpsc::Receiver<MediaEvent>,
    start: Arc<StartPoint>,
    _tasks: Vec<TrackReceiveTask>,
}

#[derive(Debug, PartialEq)]
pub struct SelectedTracks {
    pub video: Option<String>,
    pub audio: Option<(String, AudioConfig)>,
}

impl BroadcastReader {
    pub async fn run(session: &Session, namespace: &str) -> Result<Self> {
        // A joining FETCH may only join a Largest Object subscription
        // (draft-ietf-moq-transport-14 §9.16.2).
        let catalog_subscription = subscribe(
            session,
            namespace,
            msf::CATALOG_TRACK_NAME,
            FilterType::LargestObject,
        )
        .await?;
        let catalog = read_catalog(session, &catalog_subscription).await?;
        let catalog: Catalog = serde_json::from_slice(&catalog).context("parse catalog json")?;
        let tracks = select_tracks(&catalog)?;
        tracing::info!(
            namespace,
            video = tracks.video.as_deref().unwrap_or("-"),
            audio = tracks.audio.as_ref().map_or("-", |(name, _)| name.as_str()),
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
        if let Some(name) = tracks.video {
            kinds.push((name, TrackKind::Video));
        }
        if let Some((name, config)) = tracks.audio {
            kinds.push((name, TrackKind::Audio(config)));
        }
        // Subscribe to every track before waiting on any: `track_reader` blocks
        // until the video's next group starts, and audio subscribed after that
        // wait would start later than the video.
        let mut subscriptions = Vec::new();
        for (name, kind) in kinds {
            // NextGroupStart begins the video at a keyframe. An audio object decodes
            // alone, and a publisher that sends its audio as one endless group never
            // starts another, so the audio continues after the newest object
            // (draft-ietf-moq-transport-14 §9.7).
            let filter_type = match kind {
                TrackKind::Video => FilterType::NextGroupStart,
                TrackKind::Audio(_) => FilterType::LargestObject,
            };
            let subscription = subscribe(session, namespace, &name, filter_type).await?;
            subscriptions.push((name, kind, subscription));
        }
        let mut tasks = Vec::new();
        for (name, kind, subscription) in subscriptions {
            let reader = track_reader(session, &subscription).await?;
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

/// The first LOC track of each role in catalog order. An AAC track whose
/// `initRef` (draft-ietf-moq-msf-01 §5.2.13) finds no AudioSpecificConfig in
/// `initDataList` (§5.1.7), or an A-law or Opus track without a readable
/// `samplerate` and `channelConfig`, is skipped; fails when neither role has a
/// track.
pub fn select_tracks(catalog: &Catalog) -> Result<SelectedTracks> {
    let video = catalog
        .loc_tracks()
        .find(|track| track.has_role(KnownTrackRole::Video))
        .map(|track| track.name.clone());
    let audio = match catalog
        .loc_tracks()
        .find(|track| track.has_role(KnownTrackRole::Audio))
    {
        Some(track) => match audio_config(catalog, track) {
            Ok(config) => Some((track.name.clone(), config)),
            Err(error) => {
                tracing::warn!(track = %track.name, %error, "audio track skipped");
                None
            }
        },
        None => None,
    };
    if video.is_none() && audio.is_none() {
        bail!("catalog lists no LOC video or audio track");
    }
    Ok(SelectedTracks { video, audio })
}

/// A-law and Opus tracks carry no initialization data: their catalog entry's
/// `codec`, `samplerate` and `channelConfig` (draft-ietf-moq-msf-01 §5.2.18,
/// §5.2.28, §5.2.29) are the whole configuration. Any other track is read as
/// AAC from its `initDataList` entry.
fn audio_config(catalog: &Catalog, track: &Track) -> Result<AudioConfig> {
    let rate_and_channels = || -> Result<(u32, u8)> {
        let sample_rate = track.sample_rate.context("audio track has no samplerate")?;
        let channels = track
            .channel_config
            .as_deref()
            .context("audio track has no channelConfig")?;
        Ok((sample_rate, channel_count(channels)?))
    };
    match track.codec.as_deref() {
        Some(PCMA_CODEC) => {
            let (sample_rate, channels) = rate_and_channels()?;
            Ok(AudioConfig::Pcma {
                sample_rate,
                channels,
            })
        }
        Some(OPUS_CODEC) => {
            let (sample_rate, channels) = rate_and_channels()?;
            Ok(AudioConfig::Opus {
                sample_rate,
                channels,
            })
        }
        _ => Ok(AudioConfig::Aac(aac_config(catalog, track)?)),
    }
}

/// `channelConfig` as `broadcast` writes it (`mono`, `stereo`, `<n>ch`) or as
/// the draft's examples do (`<n>`).
fn channel_count(label: &str) -> Result<u8> {
    let count = match label {
        "mono" => 1,
        "stereo" => 2,
        other => other
            .strip_suffix("ch")
            .unwrap_or(other)
            .parse()
            .with_context(|| format!("unknown channelConfig {other:?}"))?,
    };
    ensure!(count > 0, "channelConfig {label:?} names no channel");
    Ok(count)
}

fn aac_config(catalog: &Catalog, track: &Track) -> Result<AudioSpecificConfig> {
    let init_ref = track
        .init_ref
        .as_ref()
        .context("audio track has no initRef")?;
    let init_data = catalog
        .init_data(track)
        .with_context(|| format!("initDataList has no entry {init_ref}"))?;
    let bytes = BASE64.decode(init_data).context("decode initData base64")?;
    AudioSpecificConfig::parse(&bytes)
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
    use moqt::{
        ClientConfig,
        publish::{GroupBoundary, OutgoingObject, Retention},
    };

    use super::*;
    use crate::{relay_session::RelaySession, test_support::FakePublisher};
    use msf::KnownPackaging;

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

    #[test]
    fn selects_the_first_loc_track_of_each_role() {
        // Arrange
        let mut audio = loc_track("audio", KnownTrackRole::Audio);
        let init_data = audio.set_inline_init_data(BASE64.encode([0x11, 0x88]));
        let mut catalog = catalog_with(vec![
            loc_track("video", KnownTrackRole::Video),
            loc_track("video", KnownTrackRole::Video).cmaf_sibling("video_cmaf", "video/mp4", None),
            loc_track("video_480p", KnownTrackRole::Video),
            audio,
        ]);
        catalog.init_data_list = Some(vec![init_data]);

        // Act
        let tracks = select_tracks(&catalog).unwrap();

        // Assert
        assert_eq!(
            tracks,
            SelectedTracks {
                video: Some("video".to_owned()),
                audio: Some((
                    "audio".to_owned(),
                    AudioConfig::Aac(AudioSpecificConfig::new(2, 48_000, 1))
                )),
            }
        );
    }

    #[test]
    fn reads_a_law_and_opus_tracks_from_their_catalog_fields() {
        // Arrange
        let audio_catalog = |codec: &str, sample_rate: Option<u32>, channels: Option<&str>| {
            let mut audio = loc_track("audio", KnownTrackRole::Audio);
            audio.codec = Some(codec.to_owned());
            audio.sample_rate = sample_rate;
            audio.channel_config = channels.map(str::to_owned);
            catalog_with(vec![audio])
        };

        // Act
        let pcma = select_tracks(&audio_catalog("pcma", Some(8_000), Some("mono"))).unwrap();
        let opus = select_tracks(&audio_catalog("opus", Some(48_000), Some("2"))).unwrap();
        let rejected = [
            audio_catalog("opus", Some(48_000), Some("5.1")),
            audio_catalog("pcma", Some(8_000), Some("0")),
            audio_catalog("pcma", None, Some("mono")),
            audio_catalog("opus", Some(48_000), None),
        ];

        // Assert: a numeric channelConfig is read; a missing samplerate or channelConfig (§5.2.28, §5.2.29), `5.1` and `0` are rejected
        assert_eq!(
            pcma.audio.unwrap().1,
            AudioConfig::Pcma {
                sample_rate: 8_000,
                channels: 1
            }
        );
        assert_eq!(
            opus.audio.unwrap().1,
            AudioConfig::Opus {
                sample_rate: 48_000,
                channels: 2
            }
        );
        for catalog in &rejected {
            assert!(select_tracks(catalog).is_err());
        }
    }

    #[test]
    fn skips_audio_without_init_data_and_rejects_empty_catalogs() {
        // Arrange
        let without_init = catalog_with(vec![
            loc_track("video", KnownTrackRole::Video),
            loc_track("audio", KnownTrackRole::Audio),
        ]);
        let cmaf_only = catalog_with(vec![
            loc_track("video", KnownTrackRole::Video).cmaf_sibling("video_cmaf", "video/mp4", None),
        ]);

        // Act
        let tracks = select_tracks(&without_init).unwrap();

        // Assert
        assert_eq!(tracks.video.as_deref(), Some("video"));
        assert!(tracks.audio.is_none());
        assert!(select_tracks(&cmaf_only).is_err());
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
        audio.codec = Some(OPUS_CODEC.to_owned());
        audio.sample_rate = Some(48_000);
        audio.channel_config = Some("2".to_owned());
        let catalog = catalog_with(vec![loc_track("video", KnownTrackRole::Video), audio]);
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
        let relay = RelaySession::connect(
            &publisher.url,
            &ClientConfig {
                verify_certificate: false,
                ..ClientConfig::default()
            },
        )
        .await
        .unwrap();
        let session = relay.session.clone();
        let reading = tokio::spawn(async move { BroadcastReader::run(&session, NAMESPACE).await });
        assert_eq!(
            publisher.served.recv().await.as_deref(),
            Some(msf::CATALOG_TRACK_NAME)
        );
        write_catalog(GroupBoundary::Within).await.unwrap();
        for track in ["video", "audio"] {
            assert_eq!(publisher.served.recv().await.as_deref(), Some(track));
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
            }
        );
        drop(reader);
        relay.close().await;
    }
}
