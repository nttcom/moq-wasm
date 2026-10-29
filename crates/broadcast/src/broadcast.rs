use std::collections::BTreeMap;

use anyhow::Result;
use base64::{Engine, engine::general_purpose};
use mediapack::{AudioConfig, VideoConfig, loc::VideoCodec, mp4::Fmp4TrackMuxer};
use moqt::{executor, publish::OutgoingObject};
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

    /// Announces the namespace and accepts SUBSCRIBEs for `track_names` before
    /// the catalog lists them.
    pub async fn declare_tracks(&self, track_names: &[String]) -> Result<()> {
        let Some(relay) = self.connection.connected().await? else {
            return Ok(());
        };
        relay.declare_tracks(&self.namespace, track_names).await
    }

    /// Resolves once a SUBSCRIBE for any track of this broadcast, the catalog
    /// included, has been accepted, and at once without a target. Call after
    /// `announce` or `declare_tracks`, since no SUBSCRIBE is accepted before.
    /// Fails when the relay session ends.
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
            .update_catalog(&self.namespace, track_name, info, |metadata| {
                &mut metadata.video_tracks
            })
            .await
    }

    pub async fn update_audio_catalog(&self, track_name: &str, info: AudioTrackInfo) -> Result<()> {
        let Some(relay) = self.connection.connected().await? else {
            return Ok(());
        };
        relay
            .update_catalog(&self.namespace, track_name, info, |metadata| {
                &mut metadata.audio_tracks
            })
            .await
    }

    /// Sends the catalog without the video or audio track, then ends with
    /// PUBLISH_DONE every track that catalog no longer lists: the track, its
    /// CMAF sibling and, when it was the last default track, the `audio`
    /// placeholder, the media timeline and the chat. Stop sending the track's
    /// objects first. Updating its catalog again publishes it again. Fails
    /// when the relay session has ended.
    pub async fn remove_track(&self, track_name: &str) -> Result<()> {
        let Some(relay) = self.connection.connected().await? else {
            return Ok(());
        };
        relay.remove_track(&self.namespace, track_name).await
    }
}

/// The tracks one `BroadcastWriter` writes its source into. The media
/// timeline and the chat belong to the default `video` and `audio` tracks, so
/// a writer under other names publishes neither.
#[derive(Debug, Clone)]
pub struct SourceTracks {
    pub video: String,
    pub video_label: String,
    pub audio: String,
    pub audio_label: String,
    /// Lists a CMAF sibling next to each LOC track and sends it while a
    /// subscriber takes it.
    pub cmaf: bool,
    /// The video tracks of one switching set are alternates of each other
    /// (draft-ietf-moq-msf-01 §5.2.12), so sources of the same picture, such
    /// as the profiles of one camera, share it and a shared screen does not.
    pub switching_set: String,
}

impl Default for SourceTracks {
    fn default() -> Self {
        Self {
            video: VIDEO_TRACK_NAME.to_string(),
            video_label: "Video".to_string(),
            audio: AUDIO_TRACK_NAME.to_string(),
            audio_label: "Audio".to_string(),
            cmaf: true,
            switching_set: VIDEO_TRACK_NAME.to_string(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AudioTrackInfo {
    pub label: String,
    pub config: AudioConfig,
    /// Lists a CMAF sibling, for AAC only.
    pub cmaf: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VideoTrackInfo {
    pub label: String,
    pub codec: String,
    pub width: u32,
    pub height: u32,
    pub init_segment: String,
    /// Target bitrate in bits per second, known when the publisher encodes
    /// the track itself.
    pub bitrate: Option<u64>,
    pub cmaf: bool,
    /// `SourceTracks::switching_set` of the track's source.
    pub switching_set: String,
}

impl VideoTrackInfo {
    pub fn from_config(config: &VideoConfig, label: String) -> Result<Self> {
        let (width, height) = match config {
            VideoConfig::Avc(record) => {
                let sps = record.sequence_parameter_set()?;
                (sps.width, sps.height)
            }
            VideoConfig::Vp8(config) => (config.width, config.height),
            VideoConfig::Vp9(config) => (config.width, config.height),
            VideoConfig::Av1(config) => (config.max_width, config.max_height),
        };
        let init_segment = Fmp4TrackMuxer::video(config.clone()).init_segment()?;
        Ok(Self {
            label,
            codec: config.codec_string(),
            width,
            height,
            init_segment: general_purpose::STANDARD.encode(init_segment),
            bitrate: None,
            cmaf: true,
            switching_set: VIDEO_TRACK_NAME.to_string(),
        })
    }
}

pub fn cmaf_track_name(track_name: &str) -> String {
    format!("{track_name}{CMAF_TRACK_SUFFIX}")
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct CatalogMetadata {
    pub(crate) video_tracks: BTreeMap<String, VideoTrackInfo>,
    pub(crate) audio_tracks: BTreeMap<String, AudioTrackInfo>,
    /// The default `audio` was removed, so no placeholder stands in for it.
    default_audio_removed: bool,
}

impl CatalogMetadata {
    /// Whether the catalog changed.
    pub(crate) fn remove_track(&mut self, track_name: &str) -> bool {
        if track_name == AUDIO_TRACK_NAME && !self.default_audio_removed {
            self.default_audio_removed = true;
            self.audio_tracks.remove(track_name);
            return true;
        }
        self.video_tracks.remove(track_name).is_some()
            || self.audio_tracks.remove(track_name).is_some()
    }

    fn has_default_tracks(&self) -> bool {
        self.video_tracks.contains_key(VIDEO_TRACK_NAME)
            || self.audio_tracks.contains_key(AUDIO_TRACK_NAME)
    }
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
    let alt_groups = alt_groups(metadata);

    let mut tracks = Vec::new();
    let mut init_data_list = Vec::new();
    for (name, info) in &metadata.video_tracks {
        let alt_group = alt_groups.get(info.switching_set.as_str()).copied();
        let cmaf_alt_group = alt_group.map(|group| group + 1);
        let mut loc = track(&namespace, name, KnownPackaging::Loc, video_role.clone());
        loc.label = Some(info.label.clone());
        loc.alt_group = alt_group;
        loc.codec = Some(info.codec.clone());
        loc.mime_type = Some(video_mime_type(&info.codec).to_string());
        loc.framerate = Some(30.0);
        loc.width = Some(info.width);
        loc.height = Some(info.height);
        loc.bitrate = info.bitrate;
        let cmaf = info.cmaf.then(|| {
            let mut cmaf = cmaf_sibling(&loc, "video/mp4", cmaf_alt_group);
            init_data_list.push(cmaf.set_inline_init_data(info.init_segment.clone()));
            cmaf
        });
        tracks.push(loc);
        tracks.extend(cmaf);
    }

    for (name, info) in &metadata.audio_tracks {
        let mut audio = audio_track(&namespace, name, &info.label);
        audio.codec = Some(info.config.codec_string());
        audio.mime_type = Some(audio_mime_type(&info.config).to_string());
        audio.sample_rate = Some(info.config.sample_rate());
        audio.channel_config = Some(channel_config_label(info.config.channels()));
        let mut audio_cmaf = None;
        if let AudioConfig::Aac(config) = &info.config {
            let audio_specific_config = general_purpose::STANDARD.encode(config.to_bytes());
            init_data_list.push(audio.set_inline_init_data(audio_specific_config));
        }
        if let (AudioConfig::Aac(_), true) = (&info.config, info.cmaf) {
            let mut cmaf = cmaf_sibling(&audio, "audio/mp4", None);
            let init_segment = Fmp4TrackMuxer::audio(info.config.clone()).init_segment()?;
            init_data_list
                .push(cmaf.set_inline_init_data(general_purpose::STANDARD.encode(init_segment)));
            audio_cmaf = Some(cmaf);
        }
        tracks.push(audio);
        tracks.extend(audio_cmaf);
    }
    if metadata.has_default_tracks() {
        tracks.extend(default_tracks(&namespace, metadata));
    }

    let mut catalog = Catalog::new(tracks, executor::system_time().as_millis() as u64);
    catalog.init_data_list = (!init_data_list.is_empty()).then_some(init_data_list);
    Ok(catalog)
}

/// The tracks that come with the default `video` and `audio`: the `audio`
/// placeholder listed before its config arrives, so it is published together
/// with `video`, the media timeline and the chat.
fn default_tracks(namespace: &Option<String>, metadata: &CatalogMetadata) -> Vec<Track> {
    let mut tracks = Vec::new();
    if !metadata.audio_tracks.contains_key(AUDIO_TRACK_NAME) && !metadata.default_audio_removed {
        let mut audio = audio_track(namespace, AUDIO_TRACK_NAME, "Audio");
        audio.codec = Some("mp4a.40.2".to_string());
        audio.mime_type = Some("audio/aac".to_string());
        tracks.push(audio);
    }

    let mut timeline = track(
        namespace,
        TIMELINE_TRACK_NAME,
        KnownPackaging::MediaTimeline,
        TrackRole::Known(KnownTrackRole::MediaTimeline),
    );
    timeline.label = Some("Media timeline".to_string());
    let mut depends = vec![VIDEO_TRACK_NAME.to_string()];
    if metadata
        .video_tracks
        .get(VIDEO_TRACK_NAME)
        .is_none_or(|video| video.cmaf)
    {
        depends.push(cmaf_track_name(VIDEO_TRACK_NAME));
    }
    timeline.depends = Some(depends);
    timeline.mime_type = Some("application/json".to_string());
    tracks.push(timeline);

    let mut chat = track(
        namespace,
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
    tracks
}

fn video_mime_type(codec: &str) -> &'static str {
    match VideoCodec::from_codec_string(codec) {
        Some(VideoCodec::Vp8) => "video/vp8",
        Some(VideoCodec::Vp9) => "video/vp9",
        Some(VideoCodec::Av1) => "video/av1",
        Some(VideoCodec::H264) | None => "video/h264",
    }
}

/// The LOC `altGroup` of each switching set with more than one video track;
/// its CMAF siblings take the next id, as each format forms its own set.
fn alt_groups(metadata: &CatalogMetadata) -> BTreeMap<&str, u64> {
    let mut sizes = BTreeMap::<&str, usize>::new();
    for info in metadata.video_tracks.values() {
        *sizes.entry(info.switching_set.as_str()).or_default() += 1;
    }
    sizes
        .into_iter()
        .filter(|(_, size)| *size > 1)
        .zip((1..).step_by(2))
        .map(|((switching_set, _), alt_group)| (switching_set, alt_group))
        .collect()
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

fn audio_track(namespace: &Option<String>, name: &str, label: &str) -> Track {
    let mut audio = track(
        namespace,
        name,
        KnownPackaging::Loc,
        TrackRole::Known(KnownTrackRole::Audio),
    );
    audio.label = Some(label.to_string());
    audio
}

fn audio_mime_type(config: &AudioConfig) -> &'static str {
    match config {
        AudioConfig::Aac(_) => "audio/aac",
        AudioConfig::Pcma { .. } => "audio/pcma",
        AudioConfig::Opus { .. } => "audio/opus",
        AudioConfig::Mp3 { .. } => "audio/mpeg",
    }
}

fn channel_config_label(channels: u8) -> String {
    match channels {
        1 => "mono".to_string(),
        2 => "stereo".to_string(),
        n => format!("{n}ch"),
    }
}

#[cfg(test)]
mod tests {
    use bytes::Bytes;
    use mediapack::{
        aac::AudioSpecificConfig, h264::AvcDecoderConfigurationRecord, vp9::Vp9Config,
    };

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
            VideoTrackInfo::from_config(&VideoConfig::Avc(record), "Video".to_string()).unwrap(),
        );
        CatalogMetadata {
            video_tracks,
            audio_tracks: BTreeMap::from([(
                AUDIO_TRACK_NAME.to_string(),
                AudioTrackInfo {
                    label: "Audio".to_string(),
                    config: AudioConfig::Aac(AudioSpecificConfig::new(2, 48_000, 1)),
                    cmaf: true,
                },
            )]),
            ..CatalogMetadata::default()
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
    fn each_switching_set_takes_its_own_alt_groups() {
        // Arrange: the camera has a rendition, the shared screen has two tracks too
        let video = |switching_set: &str| VideoTrackInfo {
            label: "Video".to_string(),
            codec: "avc1.42e01e".to_string(),
            width: 640,
            height: 360,
            init_segment: String::new(),
            bitrate: None,
            cmaf: true,
            switching_set: switching_set.to_string(),
        };
        let metadata = CatalogMetadata {
            video_tracks: BTreeMap::from([
                ("video".to_string(), video("video")),
                ("video_low".to_string(), video("video")),
                ("screen".to_string(), video("screen")),
                ("screen_low".to_string(), video("screen")),
            ]),
            ..CatalogMetadata::default()
        };

        // Act
        let catalog = build_catalog("live/test", &metadata).unwrap();

        // Assert
        let alt_group = |name: &str| track_named(&catalog, name).alt_group;
        assert_eq!(alt_group("video"), alt_group("video_low"));
        assert_eq!(alt_group("screen"), alt_group("screen_low"));
        assert_ne!(alt_group("video"), alt_group("screen"));
        assert_ne!(alt_group("video_cmaf"), alt_group("video"));
        assert_ne!(alt_group("video_cmaf"), alt_group("screen"));
    }

    #[test]
    fn a_vp9_video_is_listed_with_its_codec_and_a_vp09_cmaf_sibling() {
        // Arrange
        let config = VideoConfig::Vp9(Vp9Config {
            profile: 0,
            bit_depth: 8,
            subsampling_x: true,
            subsampling_y: true,
            full_range: false,
            width: 640,
            height: 360,
        });
        let metadata = CatalogMetadata {
            video_tracks: BTreeMap::from([(
                VIDEO_TRACK_NAME.to_string(),
                VideoTrackInfo::from_config(&config, "Video".to_string()).unwrap(),
            )]),
            ..CatalogMetadata::default()
        };

        // Act
        let catalog = build_catalog("live/test", &metadata).unwrap();

        // Assert
        let video = track_named(&catalog, VIDEO_TRACK_NAME);
        assert_eq!(video.codec.as_deref(), Some("vp09.00.21.08"));
        assert_eq!(video.mime_type.as_deref(), Some("video/vp9"));
        assert_eq!((video.width, video.height), (Some(640), Some(360)));
        let cmaf = track_named(&catalog, &cmaf_track_name(VIDEO_TRACK_NAME));
        let init_segment = general_purpose::STANDARD
            .decode(init_data_of(&catalog, cmaf).unwrap())
            .unwrap();
        assert!(init_segment.windows(4).any(|window| window == b"vp09"));
    }

    #[test]
    fn tracks_without_cmaf_list_no_cmaf_sibling_and_the_timeline_depends_on_loc_only() {
        // Arrange
        let mut metadata = metadata_with_video_and_audio();
        for video in metadata.video_tracks.values_mut() {
            video.cmaf = false;
        }
        for audio in metadata.audio_tracks.values_mut() {
            audio.cmaf = false;
        }

        // Act
        let catalog = build_catalog("live/test", &metadata).unwrap();

        // Assert
        let names: Vec<&str> = catalog
            .tracks
            .iter()
            .flatten()
            .map(|track| track.name.as_str())
            .collect();
        assert!(
            !names.iter().any(|name| name.ends_with("_cmaf")),
            "{names:?}"
        );
        assert_eq!(
            track_named(&catalog, TIMELINE_TRACK_NAME).depends,
            Some(vec![VIDEO_TRACK_NAME.to_string()])
        );
        assert!(init_data_of(&catalog, track_named(&catalog, AUDIO_TRACK_NAME)).is_some());
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
        let AudioConfig::Aac(config) = metadata.audio_tracks[AUDIO_TRACK_NAME].config.clone()
        else {
            panic!("the fixture audio is AAC");
        };
        assert_eq!(
            init_data_of(&catalog, track_named(&catalog, "audio")),
            Some(general_purpose::STANDARD.encode(config.to_bytes()).as_str())
        );
        let audio_cmaf = track_named(&catalog, "audio_cmaf");
        assert_eq!(audio_cmaf.packaging, Packaging::Known(KnownPackaging::Cmaf));
        assert_eq!(audio_cmaf.mime_type.as_deref(), Some("audio/mp4"));
        let audio_init_segment = general_purpose::STANDARD.encode(
            Fmp4TrackMuxer::audio(AudioConfig::Aac(config))
                .init_segment()
                .unwrap(),
        );
        assert_eq!(
            init_data_of(&catalog, audio_cmaf),
            Some(audio_init_segment.as_str())
        );
    }

    #[test]
    fn rendition_bitrate_reaches_both_of_its_catalog_tracks() {
        // Arrange
        let mut metadata = metadata_with_video_and_audio();
        let mut rendition = metadata.video_tracks[VIDEO_TRACK_NAME].clone();
        rendition.bitrate = Some(1_200_000);
        metadata
            .video_tracks
            .insert("video_480p".to_string(), rendition);

        // Act
        let catalog = build_catalog("live/test", &metadata).unwrap();

        // Assert: the source track has no known bitrate
        assert_eq!(track_named(&catalog, "video_480p").bitrate, Some(1_200_000));
        assert_eq!(
            track_named(&catalog, "video_480p_cmaf").bitrate,
            Some(1_200_000)
        );
        assert_eq!(track_named(&catalog, "video").bitrate, None);
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
        let mut metadata = metadata_with_video_and_audio();
        metadata.audio_tracks.insert(
            AUDIO_TRACK_NAME.to_string(),
            AudioTrackInfo {
                label: "Audio".to_string(),
                config: AudioConfig::Pcma {
                    sample_rate: 8_000,
                    channels: 1,
                },
                cmaf: true,
            },
        );

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

    #[test]
    fn a_source_under_other_names_brings_neither_the_timeline_nor_the_default_audio() {
        // Arrange
        let mut metadata = metadata_with_video_and_audio();
        let video = metadata.video_tracks.remove(VIDEO_TRACK_NAME).unwrap();
        let audio = metadata.audio_tracks.remove(AUDIO_TRACK_NAME).unwrap();
        metadata
            .video_tracks
            .insert("video/profile_1".to_string(), video);
        metadata
            .audio_tracks
            .insert("audio/profile_1".to_string(), audio);

        // Act
        let catalog = build_catalog("onvif/client", &metadata).unwrap();

        // Assert
        let names: Vec<&str> = catalog
            .tracks
            .iter()
            .flatten()
            .map(|track| track.name.as_str())
            .collect();
        assert_eq!(
            names,
            [
                "video/profile_1",
                "video/profile_1_cmaf",
                "audio/profile_1",
                "audio/profile_1_cmaf"
            ]
        );
    }
}
