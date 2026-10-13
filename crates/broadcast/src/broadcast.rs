use std::{
    collections::BTreeMap,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use anyhow::Result;
use base64::{Engine, engine::general_purpose};
use mediapack::{AudioConfig, h264::AvcDecoderConfigurationRecord, mp4::Fmp4TrackMuxer};
use moqt::publish::OutgoingObject;
use msf::{
    Catalog, Track,
    types::{KnownPackaging, KnownTrackRole, Packaging, TrackRole},
};

use crate::relay_connection::RelayConnection;

pub const VIDEO_TRACK_NAME: &str = "video";
const AUDIO_TRACK_NAME: &str = "audio";
pub(crate) const TIMELINE_TRACK_NAME: &str = "timeline";
const CMAF_TRACK_SUFFIX: &str = "_cmaf";
const CHAT_TRACK_NAME: &str = "chat";
const CHAT_EVENT_TYPE: &str = "com.skyway.chat.v1";

#[derive(Clone)]
pub struct Broadcast {
    connection: RelayConnection,
    namespace: Vec<String>,
}

impl Broadcast {
    pub fn new(connection: RelayConnection, namespace: Vec<String>) -> Self {
        Self {
            connection,
            namespace,
        }
    }

    pub fn connection(&self) -> &RelayConnection {
        &self.connection
    }

    pub fn namespace(&self) -> &[String] {
        &self.namespace
    }

    pub async fn announce(&self) -> Result<()> {
        let Some(relay) = self.connection.connected().await? else {
            return Ok(());
        };
        relay.announce(&self.namespace).await
    }

    /// Resolves once the relay has forwarded a SUBSCRIBE for a track of this
    /// broadcast, which the catalog accepts from `announce` on, so a source
    /// can start only when someone watches. Resolves at once without a
    /// target; fails when the relay session ends.
    pub async fn wait_for_subscriber(&self) -> Result<()> {
        let Some(relay) = self.connection.connected().await? else {
            return Ok(());
        };
        relay.wait_for_subscriber(&self.namespace).await
    }

    /// `false` when nothing was sent: there is no target, the peer stopped the
    /// stream, or the write ended in `WriteOutcome::NotSent`.
    pub async fn send_object(&self, track_name: &str, object: OutgoingObject) -> Result<bool> {
        let Some(relay) = self.connection.connected().await? else {
            return Ok(false);
        };
        relay.send_object(&self.namespace, track_name, object).await
    }

    pub async fn update_video_catalog(&self, track_name: &str, info: VideoTrackInfo) -> Result<()> {
        let Some(relay) = self.connection.connected().await? else {
            return Ok(());
        };
        relay
            .update_video_catalog(&self.namespace, track_name, info)
            .await
    }

    pub async fn update_audio_catalog(&self, config: AudioConfig) -> Result<()> {
        let Some(relay) = self.connection.connected().await? else {
            return Ok(());
        };
        relay.update_audio_catalog(&self.namespace, config).await
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VideoTrackInfo {
    pub label: String,
    pub codec: String,
    pub width: u32,
    pub height: u32,
    pub init_segment: String,
}

impl VideoTrackInfo {
    pub fn from_record(record: &AvcDecoderConfigurationRecord, label: String) -> Result<Self> {
        let sps = record.sequence_parameter_set()?;
        let init_segment = Fmp4TrackMuxer::video(record.clone()).init_segment()?;
        Ok(Self {
            label,
            codec: record.codec_string(),
            width: sps.width,
            height: sps.height,
            init_segment: general_purpose::STANDARD.encode(init_segment),
        })
    }
}

pub fn cmaf_track_name(track_name: &str) -> String {
    format!("{track_name}{CMAF_TRACK_SUFFIX}")
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct CatalogMetadata {
    pub(crate) video_tracks: BTreeMap<String, VideoTrackInfo>,
    pub(crate) audio_config: Option<AudioConfig>,
}

pub(crate) fn is_cmaf(track: &Track) -> bool {
    track.packaging == Packaging::Known(KnownPackaging::Cmaf)
}

pub(crate) fn awaits_subscribe(catalog: &Catalog, track_name: &str) -> bool {
    catalog
        .tracks
        .iter()
        .flatten()
        .any(|track| track.name == track_name && is_cmaf(track))
}

pub(crate) fn build_catalog(namespace_path: &str, metadata: &CatalogMetadata) -> Result<Catalog> {
    let namespace = Some(namespace_path.to_string());
    let video_role = TrackRole::Known(KnownTrackRole::Video);
    let alt_group = (metadata.video_tracks.len() > 1).then_some(1);
    let cmaf_alt_group = alt_group.map(|group| group + 1);

    let mut tracks = Vec::new();
    let mut init_data_list = Vec::new();
    for (name, info) in &metadata.video_tracks {
        let mut loc = track(&namespace, name, KnownPackaging::Loc, video_role.clone());
        loc.label = Some(info.label.clone());
        loc.alt_group = alt_group;
        loc.codec = Some(info.codec.clone());
        loc.mime_type = Some("video/h264".to_string());
        loc.framerate = Some(30.0);
        loc.width = Some(info.width);
        loc.height = Some(info.height);
        let mut cmaf = cmaf_sibling(&loc, "video/mp4", cmaf_alt_group);
        init_data_list.push(cmaf.set_inline_init_data(info.init_segment.clone()));
        tracks.push(loc);
        tracks.push(cmaf);
    }

    let audio_role = TrackRole::Known(KnownTrackRole::Audio);
    let mut audio = track(
        &namespace,
        AUDIO_TRACK_NAME,
        KnownPackaging::Loc,
        audio_role,
    );
    audio.label = Some("Audio".to_string());
    audio.codec = Some("mp4a.40.2".to_string());
    audio.mime_type = Some(audio_mime_type(metadata.audio_config.as_ref()).to_string());
    if let Some(config) = &metadata.audio_config {
        audio.codec = Some(config.codec_string());
        audio.sample_rate = Some(config.sample_rate());
        audio.channel_config = Some(channel_config_label(config.channels()));
    }
    // Only AAC has an fMP4 muxer, so other codecs travel as LOC alone.
    let mut audio_cmaf = None;
    if let Some(AudioConfig::Aac(config)) = &metadata.audio_config {
        let audio_specific_config = general_purpose::STANDARD.encode(config.to_bytes());
        init_data_list.push(audio.set_inline_init_data(audio_specific_config));
        let mut cmaf = cmaf_sibling(&audio, "audio/mp4", None);
        let init_segment = Fmp4TrackMuxer::audio(config.clone()).init_segment()?;
        init_data_list
            .push(cmaf.set_inline_init_data(general_purpose::STANDARD.encode(init_segment)));
        audio_cmaf = Some(cmaf);
    }
    tracks.push(audio);
    tracks.extend(audio_cmaf);

    let mut timeline = track(
        &namespace,
        TIMELINE_TRACK_NAME,
        KnownPackaging::MediaTimeline,
        TrackRole::Known(KnownTrackRole::MediaTimeline),
    );
    timeline.label = Some("Media timeline".to_string());
    timeline.depends = Some(vec![
        VIDEO_TRACK_NAME.to_string(),
        cmaf_track_name(VIDEO_TRACK_NAME),
    ]);
    timeline.mime_type = Some("application/json".to_string());
    tracks.push(timeline);

    let mut chat = track(
        &namespace,
        CHAT_TRACK_NAME,
        KnownPackaging::EventTimeline,
        TrackRole::Other(CHAT_TRACK_NAME.to_string()),
    );
    chat.event_type = Some(CHAT_EVENT_TYPE.to_string());
    chat.label = Some("Chat".to_string());
    chat.depends = Some(vec![
        VIDEO_TRACK_NAME.to_string(),
        AUDIO_TRACK_NAME.to_string(),
    ]);
    chat.mime_type = Some("application/json".to_string());
    tracks.push(chat);

    let mut catalog = Catalog::new(tracks, now_unix().as_millis() as u64);
    catalog.init_data_list = (!init_data_list.is_empty()).then_some(init_data_list);
    Ok(catalog)
}

fn cmaf_sibling(loc: &Track, mime_type: &str, alt_group: Option<u64>) -> Track {
    let mut cmaf = loc.cmaf_sibling(&cmaf_track_name(&loc.name), mime_type, alt_group);
    cmaf.label = loc.label.as_ref().map(|label| format!("{label} (CMAF)"));
    cmaf
}

fn track(
    namespace: &Option<String>,
    name: &str,
    packaging: KnownPackaging,
    role: TrackRole,
) -> Track {
    let mut track = Track::new(name, packaging, true);
    track.namespace = namespace.clone();
    track.role = Some(role);
    track
}

fn audio_mime_type(config: Option<&AudioConfig>) -> &'static str {
    match config {
        None | Some(AudioConfig::Aac(_)) => "audio/aac",
        Some(AudioConfig::Pcma { .. }) => "audio/pcma",
        Some(AudioConfig::Opus { .. }) => "audio/opus",
    }
}

fn channel_config_label(channels: u8) -> String {
    match channels {
        1 => "mono".to_string(),
        2 => "stereo".to_string(),
        n => format!("{n}ch"),
    }
}

pub(crate) fn now_unix() -> Duration {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use bytes::Bytes;
    use mediapack::aac::AudioSpecificConfig;

    use super::*;

    const FIXTURE_SPS: [u8; 24] = [
        0x67, 0x42, 0xd0, 0x0b, 0xda, 0x0a, 0x37, 0xe4, 0xc0, 0x44, 0x00, 0x00, 0x03, 0x00, 0x04,
        0x00, 0x00, 0x03, 0x00, 0x78, 0x3c, 0x48, 0x9a, 0x80,
    ];
    const FIXTURE_PPS: [u8; 4] = [0x68, 0xce, 0x3c, 0x80];

    fn metadata_with_video_and_audio() -> CatalogMetadata {
        let record = AvcDecoderConfigurationRecord::from_parameter_sets(
            vec![Bytes::from_static(&FIXTURE_SPS)],
            vec![Bytes::from_static(&FIXTURE_PPS)],
        )
        .unwrap();
        let mut video_tracks = BTreeMap::new();
        video_tracks.insert(
            VIDEO_TRACK_NAME.to_string(),
            VideoTrackInfo::from_record(&record, "Video".to_string()).unwrap(),
        );
        CatalogMetadata {
            video_tracks,
            audio_config: Some(AudioConfig::Aac(AudioSpecificConfig::new(2, 48_000, 1))),
        }
    }

    fn track_named<'a>(catalog: &'a Catalog, name: &str) -> &'a Track {
        catalog
            .tracks
            .as_ref()
            .unwrap()
            .iter()
            .find(|track| track.name == name)
            .unwrap_or_else(|| panic!("missing track {name}"))
    }

    fn init_data_of<'a>(catalog: &'a Catalog, track: &Track) -> Option<&'a str> {
        let init_ref = track.init_ref.as_ref()?;
        catalog
            .init_data_list
            .iter()
            .flatten()
            .find(|init_data| &init_data.id == init_ref)
            .map(|init_data| init_data.data.as_str())
    }

    #[test]
    fn only_the_cmaf_tracks_of_the_catalog_await_subscribe() {
        // Arrange
        let catalog = build_catalog("live/test", &metadata_with_video_and_audio()).unwrap();

        // Act / Assert
        assert!(awaits_subscribe(&catalog, "video_cmaf"));
        assert!(awaits_subscribe(&catalog, "audio_cmaf"));
        assert!(!awaits_subscribe(&catalog, "video"));
        assert!(!awaits_subscribe(&catalog, "timeline"));
        assert!(!awaits_subscribe(&catalog, "video_720p_cmaf"));
    }

    #[test]
    fn catalog_pairs_each_media_track_with_a_cmaf_track_carrying_its_init_segment() {
        // Arrange
        let metadata = metadata_with_video_and_audio();

        // Act
        let catalog = build_catalog("live/test", &metadata).unwrap();

        // Assert
        let video = track_named(&catalog, "video");
        assert_eq!(video.packaging, Packaging::Known(KnownPackaging::Loc));
        assert_eq!(video.init_ref, None);
        let video_cmaf = track_named(&catalog, "video_cmaf");
        assert_eq!(video_cmaf.packaging, Packaging::Known(KnownPackaging::Cmaf));
        assert_eq!(video_cmaf.mime_type.as_deref(), Some("video/mp4"));
        assert_eq!(video_cmaf.codec, video.codec);
        assert_eq!(video_cmaf.max_grp_sap_starting_type, Some(1));
        let video_init_segment = &metadata.video_tracks[VIDEO_TRACK_NAME].init_segment;
        assert_eq!(
            init_data_of(&catalog, video_cmaf),
            Some(video_init_segment.as_str())
        );
        let Some(AudioConfig::Aac(config)) = metadata.audio_config.clone() else {
            panic!("the fixture audio is AAC");
        };
        assert_eq!(
            init_data_of(&catalog, track_named(&catalog, "audio")),
            Some(general_purpose::STANDARD.encode(config.to_bytes()).as_str())
        );
        let audio_cmaf = track_named(&catalog, "audio_cmaf");
        assert_eq!(audio_cmaf.packaging, Packaging::Known(KnownPackaging::Cmaf));
        assert_eq!(audio_cmaf.mime_type.as_deref(), Some("audio/mp4"));
        let audio_init_segment =
            general_purpose::STANDARD.encode(Fmp4TrackMuxer::audio(config).init_segment().unwrap());
        assert_eq!(
            init_data_of(&catalog, audio_cmaf),
            Some(audio_init_segment.as_str())
        );
    }

    #[test]
    fn every_init_ref_names_an_inline_entry_of_its_own() {
        // Arrange: a second rendition adds a second video CMAF sibling
        let mut metadata = metadata_with_video_and_audio();
        let rendition = metadata.video_tracks[VIDEO_TRACK_NAME].clone();
        metadata
            .video_tracks
            .insert("video_480p".to_string(), rendition);

        // Act
        let catalog = build_catalog("live/test", &metadata).unwrap();

        // Assert
        let init_data_list = catalog.init_data_list.as_ref().unwrap();
        assert!(
            init_data_list
                .iter()
                .all(|init_data| init_data.r#type == "inline")
        );
        let mut ids: Vec<&str> = init_data_list
            .iter()
            .map(|init_data| init_data.id.as_str())
            .collect();
        ids.sort();
        assert_eq!(
            ids,
            ["audio", "audio_cmaf", "video_480p_cmaf", "video_cmaf"]
        );
        let mut init_refs: Vec<&str> = catalog
            .tracks
            .iter()
            .flatten()
            .filter_map(|track| track.init_ref.as_deref())
            .collect();
        init_refs.sort();
        assert_eq!(init_refs, ids);
    }

    #[test]
    fn catalog_carries_pcma_audio_as_loc_only() {
        // Arrange
        let metadata = CatalogMetadata {
            audio_config: Some(AudioConfig::Pcma {
                sample_rate: 8_000,
                channels: 1,
            }),
            ..metadata_with_video_and_audio()
        };

        // Act
        let catalog = build_catalog("live/test", &metadata).unwrap();

        // Assert
        let audio = track_named(&catalog, "audio");
        assert_eq!(audio.codec.as_deref(), Some("pcma"));
        assert_eq!(audio.mime_type.as_deref(), Some("audio/pcma"));
        assert_eq!(audio.sample_rate, Some(8_000));
        assert_eq!(audio.channel_config.as_deref(), Some("mono"));
        assert_eq!(audio.init_ref, None);
        assert!(
            catalog
                .tracks
                .iter()
                .flatten()
                .all(|track| track.name != "audio_cmaf")
        );
    }
}
