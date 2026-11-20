use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};

use anyhow::Result;
use base64::{Engine, engine::general_purpose};
use mediapack::{AudioConfig, VideoConfig, loc::VideoCodec, mp4::Fmp4TrackMuxer};
use moqt::{
    executor,
    publish::{OutgoingObject, TrackDemand},
};
use msf::{
    Catalog, Dependency, Track,
    types::{KnownPackaging, KnownTrackRole, Packaging, TrackRole},
};

use tokio::sync::{Notify, mpsc};

use crate::relay_connection::RelayConnection;

pub const VIDEO_TRACK_NAME: &str = "video";
const AUDIO_TRACK_NAME: &str = "audio";
const CMAF_TRACK_SUFFIX: &str = "_cmaf";
const TIMELINE_TRACK_SUFFIX: &str = "_timeline";
const CHAT_TRACK_NAME: &str = "chat";
const CHAT_EVENT_TYPE: &str = "io.skyway.research.moqt.chat.v1";
/// The `framerate` listed for a video whose source does not cap its rate.
const NOMINAL_FRAMERATE: u32 = 30;
/// Upper estimates of the bitrate an encoder spends on one channel.
const AAC_BITRATE_PER_CHANNEL: u64 = 128_000;
const OPUS_BITRATE_PER_CHANNEL: u64 = 64_000;

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

    /// Registers the keyframe requests and the demand of a video track its
    /// source encodes and, with `cmaf`, of its CMAF sibling, whose demand is
    /// reported under the sibling's name. Call before the catalog update that
    /// publishes them.
    pub(crate) async fn prepare_encoded_video(
        &self,
        track_name: &str,
        cmaf: bool,
        keyframe_requests: Option<&Arc<Notify>>,
        demand_sender: Option<&mpsc::UnboundedSender<TrackDemand>>,
    ) -> Result<()> {
        let cmaf_name = cmaf.then(|| cmaf_track_name(track_name));
        for name in std::iter::once(track_name).chain(cmaf_name.as_deref()) {
            // Dynamic groups (draft-18 §12.6).
            if let Some(requested) = keyframe_requests
                && let Some(relay) = self.connection.connected().await?
            {
                relay
                    .accept_new_group_requests(&self.namespace, name, requested.clone())
                    .await?;
            }
            if let Some(demand_sender) = demand_sender {
                self.publish_on_demand(name, demand_sender.clone()).await?;
            }
        }
        Ok(())
    }

    /// Has the track published on demand (draft-18 §10.10): the relay has it
    /// sent only while a subscriber takes it, and `demand_sender` is sent
    /// whether one does, now and at every change. Call before the catalog
    /// update that publishes the track.
    pub(crate) async fn publish_on_demand(
        &self,
        track_name: &str,
        demand_sender: mpsc::UnboundedSender<TrackDemand>,
    ) -> Result<()> {
        let Some(relay) = self.connection.connected().await? else {
            return Ok(());
        };
        relay
            .publish_on_demand(&self.namespace, track_name, demand_sender)
            .await
    }

    pub(crate) async fn update_video_catalog(
        &self,
        track_name: &str,
        info: VideoTrackInfo,
    ) -> Result<()> {
        let Some(relay) = self.connection.connected().await? else {
            return Ok(());
        };
        relay
            .update_catalog(&self.namespace, track_name, info, |metadata| {
                &mut metadata.video_tracks
            })
            .await
    }

    pub(crate) async fn update_audio_catalog(
        &self,
        track_name: &str,
        info: AudioTrackInfo,
    ) -> Result<()> {
        let Some(relay) = self.connection.connected().await? else {
            return Ok(());
        };
        relay
            .update_catalog(&self.namespace, track_name, info, |metadata| {
                &mut metadata.audio_tracks
            })
            .await
    }

    /// Lists the chat track in the catalog while no default video or audio
    /// lists it, and sends the catalog. Fails when the relay session has
    /// ended.
    pub async fn offer_chat(&self) -> Result<()> {
        let Some(relay) = self.connection.connected().await? else {
            return Ok(());
        };
        relay
            .update_metadata(&self.namespace, |metadata| {
                !std::mem::replace(&mut metadata.chat_offered, true)
            })
            .await
    }

    /// Sends the catalog without the video or audio track, then ends with
    /// PUBLISH_DONE every track that catalog no longer lists: the track, its
    /// CMAF sibling, the media timeline of a source left without tracks and,
    /// when it was the last default track, the `audio` placeholder and the
    /// chat. Stop sending the track's objects first. Updating its catalog
    /// again publishes it again. Fails when the relay session has ended.
    pub async fn remove_track(&self, track_name: &str) -> Result<()> {
        let Some(relay) = self.connection.connected().await? else {
            return Ok(());
        };
        relay.remove_track(&self.namespace, track_name).await
    }
}

/// The tracks one `BroadcastWriter` writes its source into. The chat belongs
/// to the default `video` and `audio` tracks, so a writer under other names
/// does not publish it.
#[derive(Debug, Clone)]
pub struct SourceTracks {
    /// Also names the source's media timeline, `<video>_timeline`, which a
    /// source without video publishes as well.
    pub video: String,
    pub video_label: String,
    pub audio: String,
    pub audio_label: String,
    /// Lists a CMAF sibling next to each LOC track and sends it while a
    /// subscriber takes it.
    pub cmaf: bool,
    /// The video tracks of one switching set are alternates of each other
    /// (draft-ietf-moq-msf-02 §5.2.13), so sources of the same picture, such
    /// as the profiles of one camera, share it and a shared screen does not.
    pub switching_set: String,
    /// The video's target bitrate in bits per second, known when the source
    /// encodes it.
    pub video_bitrate: Option<u64>,
    /// The most frames per second the source encodes the video at, known
    /// when the source caps it.
    pub video_framerate: Option<u32>,
    /// Set by a source that encodes a keyframe when asked: notified when a
    /// subscriber of the video or of one of its renditions asks for a new
    /// group (draft-18 §10.2.13), which the next keyframe starts.
    pub keyframe_requests: Option<Arc<Notify>>,
    /// Set by a source that stops encoding a track nobody takes: the video,
    /// its renditions and the audio are published on demand
    /// (`Broadcast::publish_on_demand`) and report their demand here.
    pub demand_sender: Option<mpsc::UnboundedSender<TrackDemand>>,
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
            video_bitrate: None,
            video_framerate: None,
            keyframe_requests: None,
            demand_sender: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AudioTrackInfo {
    pub(crate) label: String,
    pub(crate) config: AudioConfig,
    /// Lists a CMAF sibling, for AAC only.
    pub(crate) cmaf: bool,
    /// `SourceTracks::switching_set` of the track's source, whose render
    /// group the track joins.
    pub(crate) switching_set: String,
    /// `SourceTracks::video` of the track's source, whose media timeline
    /// lists the track's groups.
    pub(crate) source: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VideoTrackInfo {
    pub label: String,
    pub codec: String,
    pub width: u32,
    pub height: u32,
    /// The configuration the CMAF sibling's init segment is muxed from.
    pub config: VideoConfig,
    /// Target bitrate in bits per second, known when the publisher encodes
    /// the track itself; the catalog lists an estimate otherwise.
    pub bitrate: Option<u64>,
    /// Frames per second, known when the publisher caps the track's frame
    /// rate; the catalog lists `NOMINAL_FRAMERATE` otherwise.
    pub framerate: Option<u32>,
    pub cmaf: bool,
    /// `SourceTracks::switching_set` of the track's source.
    pub switching_set: String,
    /// `SourceTracks::video` of the track's source.
    pub source: String,
}

impl VideoTrackInfo {
    pub fn from_config(config: &VideoConfig, label: String) -> Result<Self> {
        let (width, height) = match config {
            VideoConfig::Avc(record) => {
                let sps = record.sequence_parameter_set()?;
                (sps.width, sps.height)
            }
            VideoConfig::Hevc(record) => {
                let sps = record.sequence_parameter_set()?;
                (sps.width, sps.height)
            }
            VideoConfig::Vp8(config) => (config.width, config.height),
            VideoConfig::Vp9(config) => (config.width, config.height),
            VideoConfig::Av1(config) => (config.max_width, config.max_height),
        };
        Ok(Self {
            label,
            codec: config.codec_string(),
            width,
            height,
            config: config.clone(),
            bitrate: None,
            framerate: None,
            cmaf: true,
            switching_set: VIDEO_TRACK_NAME.to_string(),
            source: VIDEO_TRACK_NAME.to_string(),
        })
    }
}

pub(crate) fn cmaf_track_name(track_name: &str) -> String {
    format!("{track_name}{CMAF_TRACK_SUFFIX}")
}

pub(crate) fn timeline_track_name(source: &str) -> String {
    format!("{source}{TIMELINE_TRACK_SUFFIX}")
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct CatalogMetadata {
    pub(crate) video_tracks: BTreeMap<String, VideoTrackInfo>,
    pub(crate) audio_tracks: BTreeMap<String, AudioTrackInfo>,
    /// The default `audio` was removed, so no placeholder stands in for it.
    default_audio_removed: bool,
    chat_offered: bool,
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
    let render_groups = render_groups(metadata);
    let reference_videos = reference_videos(metadata);

    let mut tracks = Vec::new();
    let mut init_data_list = Vec::new();
    // Per source, the tracks its media timeline depends on (draft-ietf-moq-msf-02 §7.2).
    let mut source_track_names = BTreeMap::<&str, Vec<String>>::new();
    for (name, info) in &metadata.video_tracks {
        let alt_group = alt_groups.get(info.switching_set.as_str()).copied();
        let cmaf_alt_group = alt_group.map(|group| group + 1);
        let mut loc = track(&namespace, name, KnownPackaging::Loc, video_role.clone());
        loc.label = Some(info.label.clone());
        loc.alt_group = alt_group;
        loc.render_group = render_groups.get(info.switching_set.as_str()).copied();
        loc.codec = Some(info.codec.clone());
        loc.mime_type = Some(video_mime_type(&info.codec).to_string());
        let framerate = info.framerate.unwrap_or(NOMINAL_FRAMERATE);
        loc.framerate = Some(f64::from(framerate));
        loc.width = Some(info.width);
        loc.height = Some(info.height);
        loc.bitrate = Some(video_bitrate(
            info,
            framerate,
            reference_videos.get(info.switching_set.as_str()).copied(),
        ));
        let cmaf = if info.cmaf {
            let init_segment = Fmp4TrackMuxer::video(info.config.clone()).init_segment()?;
            let mut cmaf = cmaf_sibling(&loc, "video/mp4", cmaf_alt_group);
            init_data_list
                .push(cmaf.set_inline_init_data(general_purpose::STANDARD.encode(init_segment)));
            Some(cmaf)
        } else {
            None
        };
        let names = source_track_names.entry(info.source.as_str()).or_default();
        names.push(loc.name.clone());
        names.extend(cmaf.as_ref().map(|cmaf| cmaf.name.clone()));
        tracks.push(loc);
        tracks.extend(cmaf);
    }

    for (name, info) in &metadata.audio_tracks {
        let mut audio = audio_track(&namespace, name, &info.label);
        audio.render_group = render_groups.get(info.switching_set.as_str()).copied();
        audio.codec = Some(info.config.codec_string());
        audio.mime_type = Some(audio_mime_type(&info.config).to_string());
        audio.sample_rate = Some(info.config.sample_rate());
        audio.channel_config = Some(msf::channel_config(info.config.channels()));
        audio.bitrate = Some(audio_bitrate(&info.config));
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
        let names = source_track_names.entry(info.source.as_str()).or_default();
        names.push(audio.name.clone());
        names.extend(audio_cmaf.as_ref().map(|cmaf| cmaf.name.clone()));
        tracks.push(audio);
        tracks.extend(audio_cmaf);
    }
    if let Some(placeholder) = audio_placeholder(
        &namespace,
        metadata,
        render_groups.get(VIDEO_TRACK_NAME).copied(),
    ) {
        source_track_names
            .entry(VIDEO_TRACK_NAME)
            .or_default()
            .push(placeholder.name.clone());
        tracks.push(placeholder);
    }
    let timelines: Vec<Track> = source_track_names
        .into_iter()
        .map(|(source, names)| {
            timeline_track(&namespace, source, Dependency::on_tracks(&tracks, names))
        })
        .collect();
    tracks.extend(timelines);
    if metadata.has_default_tracks() || metadata.chat_offered {
        tracks.push(chat_track(&namespace));
    }

    let mut catalog = Catalog::new(tracks, executor::system_time().as_millis() as u64);
    catalog.init_data_list = (!init_data_list.is_empty()).then_some(init_data_list);
    Ok(catalog)
}

/// Listed with the default `video` before the `audio` config arrives, so the
/// two are published together.
fn audio_placeholder(
    namespace: &Option<String>,
    metadata: &CatalogMetadata,
    render_group: Option<u64>,
) -> Option<Track> {
    if !metadata.has_default_tracks()
        || metadata.audio_tracks.contains_key(AUDIO_TRACK_NAME)
        || metadata.default_audio_removed
    {
        return None;
    }
    let mut audio = audio_track(namespace, AUDIO_TRACK_NAME, "Audio");
    audio.render_group = render_group;
    audio.codec = Some("mp4a.40.2".to_string());
    audio.mime_type = Some("audio/aac".to_string());
    audio.sample_rate = Some(48_000);
    audio.channel_config = Some(msf::channel_config(2));
    audio.bitrate = Some(AAC_BITRATE_PER_CHANNEL * 2);
    Some(audio)
}

/// A source's media timeline (draft-ietf-moq-msf-02 §7), listed while the
/// catalog lists any of the source's tracks.
fn timeline_track(namespace: &Option<String>, source: &str, depends: Vec<Dependency>) -> Track {
    let mut timeline = track(
        namespace,
        &timeline_track_name(source),
        KnownPackaging::MediaTimeline,
        TrackRole::Known(KnownTrackRole::MediaTimeline),
    );
    timeline.label = Some("Media timeline".to_string());
    timeline.depends = Some(depends);
    timeline.mime_type = Some("application/json".to_string());
    timeline
}

/// The chat indexes its records by wallclock time, so it depends on no track
/// (draft-ietf-moq-msf-02 §8.3).
fn chat_track(namespace: &Option<String>) -> Track {
    let mut chat = track(
        namespace,
        CHAT_TRACK_NAME,
        KnownPackaging::EventTimeline,
        TrackRole::Other(CHAT_TRACK_NAME.to_string()),
    );
    chat.event_type = Some(CHAT_EVENT_TYPE.to_string());
    chat.label = Some("Chat".to_string());
    chat.mime_type = Some("application/json".to_string());
    chat
}

fn video_mime_type(codec: &str) -> &'static str {
    match VideoCodec::from_codec_string(codec) {
        Some(VideoCodec::Vp8) => "video/vp8",
        Some(VideoCodec::Vp9) => "video/vp9",
        Some(VideoCodec::Av1) => "video/av1",
        Some(VideoCodec::H265) => "video/h265",
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

/// One `renderGroup` per switching set (draft-ietf-moq-msf-02 §5.2.12): the
/// video of a source, its renditions and its audio are rendered together. The
/// numbers mean nothing beyond that grouping.
fn render_groups(metadata: &CatalogMetadata) -> BTreeMap<&str, u64> {
    let switching_sets: BTreeSet<&str> = metadata
        .video_tracks
        .values()
        .map(|info| info.switching_set.as_str())
        .chain(
            metadata
                .audio_tracks
                .values()
                .map(|info| info.switching_set.as_str()),
        )
        .collect();
    switching_sets.into_iter().zip(1..).collect()
}

/// Per switching set, the largest video with a known bitrate, which
/// `video_bitrate` scales to the videos without one.
fn reference_videos(metadata: &CatalogMetadata) -> BTreeMap<&str, &VideoTrackInfo> {
    let mut references = BTreeMap::<&str, &VideoTrackInfo>::new();
    for info in metadata.video_tracks.values() {
        if info.bitrate.is_none() {
            continue;
        }
        let reference = references
            .entry(info.switching_set.as_str())
            .or_insert(info);
        if pixels(info) > pixels(reference) {
            *reference = info;
        }
    }
    references
}

/// A video's maximum bitrate: the encoder's target; for a video the
/// publisher relays without encoding it, the reference's bitrate scaled by
/// pixel count, or without a reference, an estimate from codec, size and
/// frame rate.
fn video_bitrate(info: &VideoTrackInfo, framerate: u32, reference: Option<&VideoTrackInfo>) -> u64 {
    if let Some(bitrate) = info.bitrate {
        return bitrate;
    }
    if let Some(reference) = reference
        && let Some(reference_bitrate) = reference.bitrate
        && pixels(reference) > 0
    {
        return reference_bitrate * pixels(info) / pixels(reference);
    }
    let bits_per_pixel = match VideoCodec::from_codec_string(&info.codec) {
        Some(VideoCodec::H265 | VideoCodec::Vp9 | VideoCodec::Av1) => 0.07,
        Some(VideoCodec::H264 | VideoCodec::Vp8) | None => 0.1,
    };
    (pixels(info) as f64 * f64::from(framerate) * bits_per_pixel) as u64
}

fn pixels(info: &VideoTrackInfo) -> u64 {
    u64::from(info.width) * u64::from(info.height)
}

/// An audio track's maximum bitrate, estimated as no source states it.
fn audio_bitrate(config: &AudioConfig) -> u64 {
    let per_channel = match config {
        AudioConfig::Pcma { sample_rate, .. } => u64::from(*sample_rate) * 8,
        AudioConfig::Opus { .. } => OPUS_BITRATE_PER_CHANNEL,
        AudioConfig::Aac(_) | AudioConfig::Mp3 { .. } => AAC_BITRATE_PER_CHANNEL,
    };
    per_channel * u64::from(config.channels())
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

#[cfg(test)]
mod tests {
    use bytes::Bytes;
    use mediapack::{
        aac::AudioSpecificConfig, h264::AvcDecoderConfigurationRecord, vp8::Vp8Config,
        vp9::Vp9Config,
    };

    use super::*;
    use crate::test_support::FakeRelay;

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
                    switching_set: VIDEO_TRACK_NAME.to_string(),
                    source: VIDEO_TRACK_NAME.to_string(),
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

    #[test]
    fn only_the_cmaf_tracks_of_the_catalog_await_subscribe() {
        // Arrange
        let catalog = build_catalog("live/test", &metadata_with_video_and_audio()).unwrap();

        // Act / Assert
        assert!(awaits_subscribe(&catalog, "video_cmaf"));
        assert!(awaits_subscribe(&catalog, "audio_cmaf"));
        assert!(!awaits_subscribe(&catalog, "video"));
        assert!(!awaits_subscribe(&catalog, "video_timeline"));
        assert!(!awaits_subscribe(&catalog, "video_720p_cmaf"));
    }

    #[test]
    fn an_offered_chat_is_listed_without_default_tracks() {
        // Arrange
        let metadata = CatalogMetadata {
            chat_offered: true,
            ..CatalogMetadata::default()
        };

        // Act
        let catalog = build_catalog("live/test", &metadata).unwrap();

        // Assert
        let names: Vec<&str> = catalog
            .tracks
            .iter()
            .flatten()
            .map(|track| track.name.as_str())
            .collect();
        assert_eq!(names, [CHAT_TRACK_NAME]);
        assert_eq!(track_named(&catalog, CHAT_TRACK_NAME).depends, None);
    }

    fn video(switching_set: &str) -> VideoTrackInfo {
        VideoTrackInfo {
            label: "Video".to_string(),
            codec: "avc1.42e01e".to_string(),
            width: 640,
            height: 360,
            config: VideoConfig::Vp8(Vp8Config {
                width: 640,
                height: 360,
            }),
            bitrate: None,
            framerate: None,
            cmaf: true,
            switching_set: switching_set.to_string(),
            source: switching_set.to_string(),
        }
    }

    fn aac_audio(switching_set: &str) -> AudioTrackInfo {
        AudioTrackInfo {
            label: "Audio".to_string(),
            config: AudioConfig::Aac(AudioSpecificConfig::new(2, 48_000, 1)),
            cmaf: true,
            switching_set: switching_set.to_string(),
            source: switching_set.to_string(),
        }
    }

    #[test]
    fn each_switching_set_takes_its_own_alt_groups() {
        // Arrange: the camera has a rendition, the shared screen has two tracks too
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
        assert_eq!(
            alt_group("video_cmaf"),
            alt_group("video").map(|group| group + 1)
        );
        assert_ne!(alt_group("video_cmaf"), alt_group("screen"));
    }

    #[test]
    fn a_switching_set_of_one_video_takes_no_alt_group() {
        // Arrange
        let metadata = CatalogMetadata {
            video_tracks: BTreeMap::from([("video".to_string(), video("video"))]),
            ..CatalogMetadata::default()
        };

        // Act
        let catalog = build_catalog("live/test", &metadata).unwrap();

        // Assert
        assert_eq!(track_named(&catalog, "video").alt_group, None);
        assert_eq!(track_named(&catalog, "video_cmaf").alt_group, None);
    }

    #[test]
    fn a_video_track_lists_the_frame_rate_its_publisher_caps_it_at_or_a_nominal_one() {
        // Arrange: the camera is capped at 30 fps, its rendition at 2, the screen not at all
        let metadata = CatalogMetadata {
            video_tracks: BTreeMap::from([
                (
                    "video".to_string(),
                    VideoTrackInfo {
                        framerate: Some(30),
                        ..video("video")
                    },
                ),
                (
                    "video_low".to_string(),
                    VideoTrackInfo {
                        framerate: Some(2),
                        ..video("video")
                    },
                ),
                ("screen".to_string(), video("screen")),
            ]),
            ..CatalogMetadata::default()
        };

        // Act
        let catalog = build_catalog("live/test", &metadata).unwrap();

        // Assert
        let framerate = |name: &str| track_named(&catalog, name).framerate;
        assert_eq!(framerate("video"), Some(30.0));
        assert_eq!(framerate("video_low"), Some(2.0));
        assert_eq!(framerate("screen"), Some(f64::from(NOMINAL_FRAMERATE)));
    }

    #[test]
    fn each_source_renders_its_video_renditions_and_audio_as_one_group() {
        // Arrange: a camera with a rendition and a shared screen, each with its audio
        let metadata = CatalogMetadata {
            video_tracks: BTreeMap::from([
                ("video".to_string(), video("video")),
                ("video_low".to_string(), video("video")),
                ("screen".to_string(), video("screen")),
            ]),
            audio_tracks: BTreeMap::from([
                ("audio".to_string(), aac_audio("video")),
                ("screen_audio".to_string(), aac_audio("screen")),
            ]),
            ..CatalogMetadata::default()
        };

        // Act
        let catalog = build_catalog("live/test", &metadata).unwrap();

        // Assert
        let render_group = |name: &str| track_named(&catalog, name).render_group;
        assert!(render_group("video").is_some());
        for name in ["video_low", "video_cmaf", "audio", "audio_cmaf"] {
            assert_eq!(render_group(name), render_group("video"), "{name}");
        }
        assert_eq!(render_group("screen_audio"), render_group("screen"));
        assert_ne!(render_group("screen"), render_group("video"));
    }

    #[test]
    fn each_source_has_a_media_timeline_that_depends_on_its_render_group() {
        // Arrange: a camera with a rendition, a shared screen and a source with audio only
        let metadata = CatalogMetadata {
            video_tracks: BTreeMap::from([
                ("video".to_string(), video("video")),
                ("video_low".to_string(), video("video")),
                ("screen".to_string(), video("screen")),
            ]),
            audio_tracks: BTreeMap::from([
                ("audio".to_string(), aac_audio("video")),
                ("mic_audio".to_string(), aac_audio("mic")),
            ]),
            ..CatalogMetadata::default()
        };

        // Act
        let catalog = build_catalog("live/test", &metadata).unwrap();

        // Assert
        let timelines: Vec<(&str, Vec<Dependency>)> = catalog
            .tracks
            .iter()
            .flatten()
            .filter(|track| track.packaging == Packaging::Known(KnownPackaging::MediaTimeline))
            .map(|track| {
                (
                    track.name.as_str(),
                    track.depends.clone().unwrap_or_default(),
                )
            })
            .collect();
        let render_group = |name: &str| {
            vec![Dependency::RenderGroup(
                track_named(&catalog, name).render_group.unwrap(),
            )]
        };
        assert_eq!(
            timelines,
            [
                ("mic_timeline", render_group("mic_audio")),
                ("screen_timeline", render_group("screen")),
                ("video_timeline", render_group("video")),
            ]
        );
    }

    #[test]
    fn the_media_timelines_of_sources_sharing_a_switching_set_depend_on_their_tracks() {
        // Arrange: two profiles of one camera
        let profile = |source: &str| VideoTrackInfo {
            source: source.to_string(),
            cmaf: false,
            ..video("camera")
        };
        let metadata = CatalogMetadata {
            video_tracks: BTreeMap::from([
                ("main".to_string(), profile("main")),
                ("sub".to_string(), profile("sub")),
            ]),
            ..CatalogMetadata::default()
        };

        // Act
        let catalog = build_catalog("live/test", &metadata).unwrap();

        // Assert
        assert_eq!(
            track_named(&catalog, "main_timeline").depends,
            Some(vec!["main".into()])
        );
        assert_eq!(
            track_named(&catalog, "sub_timeline").depends,
            Some(vec!["sub".into()])
        );
    }

    #[test]
    fn the_catalog_is_valid_under_the_draft() {
        // Arrange
        let mut metadata = metadata_with_video_and_audio();
        metadata
            .video_tracks
            .insert("screen".to_string(), video("screen"));
        metadata.chat_offered = true;
        let catalog = build_catalog("live/test", &metadata).unwrap();

        // Act
        let parsed = Catalog::parse(&serde_json::to_vec(&catalog).unwrap());

        // Assert
        assert_eq!(parsed.unwrap(), catalog);
    }

    #[test]
    fn the_audio_placeholder_joins_the_default_video_s_render_group() {
        // Arrange
        let metadata = CatalogMetadata {
            video_tracks: BTreeMap::from([("video".to_string(), video("video"))]),
            ..CatalogMetadata::default()
        };

        // Act
        let catalog = build_catalog("live/test", &metadata).unwrap();

        // Assert
        assert!(track_named(&catalog, "audio").render_group.is_some());
        assert_eq!(
            track_named(&catalog, "audio").render_group,
            track_named(&catalog, "video").render_group
        );
    }

    #[test]
    fn the_audio_placeholder_lists_the_fields_of_an_aac_stereo_track() {
        // Arrange
        let metadata = CatalogMetadata {
            video_tracks: BTreeMap::from([("video".to_string(), video("video"))]),
            ..CatalogMetadata::default()
        };

        // Act
        let catalog = build_catalog("live/test", &metadata).unwrap();

        // Assert
        let audio = track_named(&catalog, "audio");
        assert_eq!(audio.codec.as_deref(), Some("mp4a.40.2"));
        assert_eq!(audio.sample_rate, Some(48_000));
        assert_eq!(audio.channel_config.as_deref(), Some("stereo"));
        assert_eq!(audio.bitrate, Some(AAC_BITRATE_PER_CHANNEL * 2));
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
            .decode(catalog.init_data(cmaf).unwrap())
            .unwrap();
        assert!(init_segment.windows(4).any(|window| window == b"vp09"));
    }

    #[test]
    fn tracks_without_cmaf_list_no_cmaf_sibling() {
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
        assert!(
            catalog
                .init_data(track_named(&catalog, AUDIO_TRACK_NAME))
                .is_some()
        );
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
        let video_init_segment = general_purpose::STANDARD.encode(
            Fmp4TrackMuxer::video(metadata.video_tracks[VIDEO_TRACK_NAME].config.clone())
                .init_segment()
                .unwrap(),
        );
        assert_eq!(
            catalog.init_data(video_cmaf),
            Some(video_init_segment.as_str())
        );
        let AudioConfig::Aac(config) = metadata.audio_tracks[AUDIO_TRACK_NAME].config.clone()
        else {
            panic!("the fixture audio is AAC");
        };
        assert_eq!(
            catalog.init_data(track_named(&catalog, "audio")),
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
            catalog.init_data(audio_cmaf),
            Some(audio_init_segment.as_str())
        );
    }

    #[test]
    fn rendition_bitrate_reaches_both_of_its_catalog_tracks() {
        // Arrange: a rendition of a quarter of the source's pixels
        let mut metadata = metadata_with_video_and_audio();
        let mut rendition = metadata.video_tracks[VIDEO_TRACK_NAME].clone();
        rendition.width /= 2;
        rendition.height /= 2;
        rendition.bitrate = Some(1_200_000);
        metadata
            .video_tracks
            .insert("video_480p".to_string(), rendition);

        // Act
        let catalog = build_catalog("live/test", &metadata).unwrap();

        // Assert: the source's unknown bitrate scales the rendition's by pixel count
        assert_eq!(track_named(&catalog, "video_480p").bitrate, Some(1_200_000));
        assert_eq!(
            track_named(&catalog, "video_480p_cmaf").bitrate,
            Some(1_200_000)
        );
        assert_eq!(track_named(&catalog, "video").bitrate, Some(4_800_000));
    }

    #[test]
    fn a_video_without_a_bitrate_scales_the_largest_video_of_its_switching_set() {
        // Arrange: scaled from the 360p instead, the 1080p would list 4.5 Mbps
        let sized = |width, height, bitrate| VideoTrackInfo {
            width,
            height,
            bitrate,
            ..video("video")
        };
        let metadata = CatalogMetadata {
            video_tracks: BTreeMap::from([
                ("video".to_string(), sized(1920, 1080, None)),
                ("video_360p".to_string(), sized(640, 360, Some(500_000))),
                ("video_720p".to_string(), sized(1280, 720, Some(3_000_000))),
            ]),
            ..CatalogMetadata::default()
        };

        // Act
        let catalog = build_catalog("live/test", &metadata).unwrap();

        // Assert: the 720p's 3 Mbps scaled by the 2.25 times its pixels the 1080p has
        assert_eq!(track_named(&catalog, "video").bitrate, Some(6_750_000));
    }

    #[test]
    fn a_reference_video_without_pixels_is_not_scaled_from() {
        // Arrange
        let metadata = CatalogMetadata {
            video_tracks: BTreeMap::from([
                ("video".to_string(), video("video")),
                (
                    "video_empty".to_string(),
                    VideoTrackInfo {
                        width: 0,
                        height: 0,
                        bitrate: Some(1_000_000),
                        ..video("video")
                    },
                ),
            ]),
            ..CatalogMetadata::default()
        };

        // Act
        let catalog = build_catalog("live/test", &metadata).unwrap();

        // Assert: 0.1 bit per pixel of H.264 at the nominal 30 fps
        assert_eq!(track_named(&catalog, "video").bitrate, Some(691_200));
    }

    #[test]
    fn a_video_without_a_known_bitrate_in_its_switching_set_lists_an_estimate() {
        // Arrange
        let mut metadata = metadata_with_video_and_audio();
        let video = metadata.video_tracks.get_mut(VIDEO_TRACK_NAME).unwrap();
        video.width = 1280;
        video.height = 720;

        // Act
        let catalog = build_catalog("live/test", &metadata).unwrap();

        // Assert: 0.1 bit per pixel of H.264 at the nominal 30 fps
        assert_eq!(track_named(&catalog, "video").bitrate, Some(2_764_800));
    }

    #[test]
    fn an_audio_track_lists_a_bitrate_estimated_from_its_codec_and_channels() {
        // Arrange
        let mut metadata = metadata_with_video_and_audio();
        metadata.audio_tracks.insert(
            "screen_audio".to_string(),
            AudioTrackInfo {
                label: "Screen audio".to_string(),
                config: AudioConfig::Opus {
                    sample_rate: 48_000,
                    channels: 2,
                },
                cmaf: false,
                switching_set: "screen".to_string(),
                source: "screen".to_string(),
            },
        );

        // Act
        let catalog = build_catalog("live/test", &metadata).unwrap();

        // Assert
        assert_eq!(
            track_named(&catalog, "audio").bitrate,
            Some(AAC_BITRATE_PER_CHANNEL)
        );
        assert_eq!(
            track_named(&catalog, "screen_audio").bitrate,
            Some(OPUS_BITRATE_PER_CHANNEL * 2)
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
                switching_set: VIDEO_TRACK_NAME.to_string(),
                source: VIDEO_TRACK_NAME.to_string(),
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
        assert_eq!(audio.bitrate, Some(64_000));
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
    fn a_source_under_other_names_brings_its_own_timeline_but_not_the_default_audio() {
        // Arrange
        let mut metadata = metadata_with_video_and_audio();
        let source = "video/profile_1".to_string();
        let video = metadata.video_tracks.remove(VIDEO_TRACK_NAME).unwrap();
        let audio = metadata.audio_tracks.remove(AUDIO_TRACK_NAME).unwrap();
        metadata.video_tracks.insert(
            "video/profile_1".to_string(),
            VideoTrackInfo {
                source: source.clone(),
                ..video
            },
        );
        metadata.audio_tracks.insert(
            "audio/profile_1".to_string(),
            AudioTrackInfo { source, ..audio },
        );

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
                "audio/profile_1_cmaf",
                "video/profile_1_timeline"
            ]
        );
    }
    #[test]
    fn namespace_is_the_one_the_broadcast_was_made_with() {
        // Arrange
        let namespace = vec!["live".to_string(), "cam".to_string()];

        // Act
        let broadcast = Broadcast::new(RelayConnection::new(None), namespace.clone());

        // Assert
        assert_eq!(broadcast.namespace(), namespace);
    }

    #[tokio::test]
    async fn an_offered_chat_is_sent_in_a_catalog() {
        // Arrange
        let mut relay = FakeRelay::spawn("offer-chat");
        let broadcast = Broadcast::new(relay.connection(), vec!["live".to_string()]);

        // Act
        broadcast.offer_chat().await.unwrap();
        let catalog = relay.newest_catalog().await;

        // Assert
        assert!(
            catalog
                .tracks
                .iter()
                .flatten()
                .any(|track| track.name == CHAT_TRACK_NAME),
            "{catalog:?}"
        );
    }
    #[tokio::test]
    async fn an_updated_audio_track_is_sent_in_a_catalog() {
        // Arrange
        let mut relay = FakeRelay::spawn("audio-catalog");
        let broadcast = Broadcast::new(relay.connection(), vec!["live".to_string()]);

        // Act
        broadcast
            .update_audio_catalog("audio_en", aac_audio(VIDEO_TRACK_NAME))
            .await
            .unwrap();
        let catalog = relay.newest_catalog().await;

        // Assert
        assert!(
            catalog
                .tracks
                .iter()
                .flatten()
                .any(|track| track.name == "audio_en"),
            "{catalog:?}"
        );
    }
}
