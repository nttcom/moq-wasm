//! The subset of SDP (RFC 8866) that describes a plain RTP session: one
//! `m=` section per stream with its `c=`, `a=rtpmap` and `a=fmtp` lines. The
//! codec parameters follow RFC 6184 §8.1 (H.264), RFC 3640 §4.1 (AAC),
//! RFC 3551 §4.5.14 (G.711 A-law) and RFC 7587 §7 (Opus).

use std::{fmt, net::IpAddr};

use anyhow::{Context, Result, bail, ensure};
use base64::{Engine, engine::general_purpose::STANDARD as BASE64};
use bytes::Bytes;

use crate::{
    aac::AudioSpecificConfig, h264::AvcDecoderConfigurationRecord, rtp::aac::AuHeaderConfig,
};

const H264_ENCODING: &str = "H264";
const AAC_ENCODING: &str = "MPEG4-GENERIC";
const AAC_HBR_MODE: &str = "AAC-hbr";
const PCMA_ENCODING: &str = "PCMA";
const OPUS_ENCODING: &str = "opus";
/// RFC 3551 §6 assigns G.711 A-law the static payload type 8 at 8 kHz.
const PCMA_STATIC_PAYLOAD_TYPE: u8 = 8;
const PCMA_STATIC_CLOCK_RATE: u32 = 8_000;
/// RFC 7587 §7 fixes the Opus clock at 48 kHz and its rtpmap at two channels,
/// whatever the encoder sends.
pub const OPUS_CLOCK_RATE: u32 = 48_000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionDescription {
    /// The session-level `c=` address; a media-level one overrides it.
    pub destination: Option<IpAddr>,
    pub media: Vec<MediaDescription>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MediaDescription {
    pub kind: MediaKind,
    pub port: u16,
    pub destination: Option<IpAddr>,
    /// The first format of the `m=` line, the one the sender prefers.
    pub payload_type: u8,
    pub clock_rate: u32,
    pub codec: Codec,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MediaKind {
    Video,
    Audio,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Codec {
    /// `parameter_sets` is `sprop-parameter-sets` when the sender announces
    /// SPS and PPS out of band.
    H264 {
        parameter_sets: Option<AvcDecoderConfigurationRecord>,
    },
    Aac {
        config: AudioSpecificConfig,
        au_headers: AuHeaderConfig,
    },
    Pcma {
        channels: u8,
    },
    /// `channels` is 2 when `sprop-stereo=1` says the sender sends stereo.
    Opus {
        channels: u8,
    },
    /// A format this crate does not carry, named by its encoding, or
    /// `static payload type <n>` when the section has no `a=rtpmap`.
    Unsupported(String),
}

impl SessionDescription {
    /// Fails on a line without `=`, an `m=` line without a port or payload
    /// type, an interleaved H.264 packetization mode, or H.264 / AAC-hbr
    /// parameters that do not decode.
    pub fn parse(text: &str) -> Result<Self> {
        let mut description = Self {
            destination: None,
            media: Vec::new(),
        };
        let mut section: Option<Section> = None;
        for line in text.lines().map(str::trim_end) {
            if line.is_empty() {
                continue;
            }
            let (kind, value) = line
                .split_once('=')
                .with_context(|| format!("SDP line without '=': {line:?}"))?;
            if kind == "m" {
                if let Some(section) = section.take() {
                    description.media.extend(section.into_media()?);
                }
                section = Some(Section::new(value)?);
                continue;
            }
            match &mut section {
                Some(section) => section.push(kind, value)?,
                None if kind == "c" => description.destination = Some(parse_connection(value)?),
                None => {}
            }
        }
        if let Some(section) = section {
            description.media.extend(section.into_media()?);
        }
        Ok(description)
    }

    pub fn destination_of(&self, media: &MediaDescription) -> Option<IpAddr> {
        media.destination.or(self.destination)
    }
}

struct Section {
    kind: Option<MediaKind>,
    port: u16,
    payload_type: u8,
    destination: Option<IpAddr>,
    rtpmap: Option<String>,
    fmtp: Option<String>,
}

impl Section {
    fn new(media_line: &str) -> Result<Self> {
        let mut fields = media_line.split_whitespace();
        let kind = match fields.next() {
            Some("video") => Some(MediaKind::Video),
            Some("audio") => Some(MediaKind::Audio),
            _ => None,
        };
        let port = fields
            .next()
            .and_then(|port| port.split('/').next())
            .and_then(|port| port.parse().ok())
            .with_context(|| format!("m= line without a port: {media_line:?}"))?;
        let payload_type = fields
            .nth(1)
            .and_then(|format| format.parse().ok())
            .with_context(|| format!("m= line without a payload type: {media_line:?}"))?;
        Ok(Self {
            kind,
            port,
            payload_type,
            destination: None,
            rtpmap: None,
            fmtp: None,
        })
    }

    fn push(&mut self, kind: &str, value: &str) -> Result<()> {
        match kind {
            "c" => self.destination = Some(parse_connection(value)?),
            "a" => {
                let Some((name, rest)) = value.split_once(':') else {
                    return Ok(());
                };
                let Some((payload_type, parameters)) = rest.split_once(' ') else {
                    return Ok(());
                };
                if payload_type.parse::<u8>().ok() != Some(self.payload_type) {
                    return Ok(());
                }
                match name {
                    "rtpmap" => self.rtpmap = Some(parameters.to_owned()),
                    "fmtp" => self.fmtp = Some(parameters.to_owned()),
                    _ => {}
                }
            }
            _ => {}
        }
        Ok(())
    }

    fn into_media(self) -> Result<Option<MediaDescription>> {
        let Some(kind) = self.kind else {
            return Ok(None);
        };
        let Some(rtpmap) = &self.rtpmap else {
            let (clock_rate, codec) = match self.payload_type {
                PCMA_STATIC_PAYLOAD_TYPE if kind == MediaKind::Audio => {
                    (PCMA_STATIC_CLOCK_RATE, Codec::Pcma { channels: 1 })
                }
                other => (
                    0,
                    Codec::Unsupported(format!("static payload type {other}")),
                ),
            };
            return Ok(Some(self.media(kind, clock_rate, codec)));
        };
        let mut fields = rtpmap.split('/');
        let encoding = fields.next().unwrap_or_default().to_owned();
        let clock_rate = fields
            .next()
            .and_then(|rate| rate.parse().ok())
            .with_context(|| format!("rtpmap without a clock rate: {rtpmap:?}"))?;
        let channels = fields
            .next()
            .and_then(|count| count.parse().ok())
            .unwrap_or(1);
        let parameters = self.fmtp.as_deref().map(parse_fmtp).unwrap_or_default();
        let codec = match encoding.to_ascii_uppercase().as_str() {
            H264_ENCODING if kind == MediaKind::Video => h264_codec(&parameters)?,
            AAC_ENCODING if kind == MediaKind::Audio => aac_codec(&parameters)?,
            PCMA_ENCODING if kind == MediaKind::Audio => Codec::Pcma { channels },
            "OPUS" if kind == MediaKind::Audio => {
                let stereo = parameter(&parameters, "sprop-stereo") == Some("1");
                Codec::Opus {
                    channels: if stereo { 2 } else { 1 },
                }
            }
            _ => Codec::Unsupported(encoding),
        };
        Ok(Some(self.media(kind, clock_rate, codec)))
    }

    fn media(&self, kind: MediaKind, clock_rate: u32, codec: Codec) -> MediaDescription {
        MediaDescription {
            kind,
            port: self.port,
            destination: self.destination,
            payload_type: self.payload_type,
            clock_rate,
            codec,
        }
    }
}

fn parse_connection(value: &str) -> Result<IpAddr> {
    let address = value
        .split_whitespace()
        .nth(2)
        .with_context(|| format!("c= line without an address: {value:?}"))?;
    // A multicast address may carry a TTL and an address count: `224.2.1.1/127/3`.
    let address = address.split('/').next().unwrap_or(address);
    address
        .parse()
        .with_context(|| format!("invalid c= address {address:?}"))
}

/// `fmtp` parameters as lower-cased names; RFC 3640 §4.1 makes them case-insensitive.
fn parse_fmtp(parameters: &str) -> Vec<(String, String)> {
    parameters
        .split(';')
        .filter_map(|parameter| {
            let (name, value) = parameter.trim().split_once('=')?;
            Some((name.trim().to_ascii_lowercase(), value.trim().to_owned()))
        })
        .collect()
}

fn parameter<'a>(parameters: &'a [(String, String)], name: &str) -> Option<&'a str> {
    parameters
        .iter()
        .find(|(key, _)| key == name)
        .map(|(_, value)| value.as_str())
}

fn h264_codec(parameters: &[(String, String)]) -> Result<Codec> {
    let mode = parameter(parameters, "packetization-mode").unwrap_or("0");
    ensure!(
        matches!(mode, "0" | "1"),
        "H.264 packetization-mode {mode} is interleaved"
    );
    let Some(sets) = parameter(parameters, "sprop-parameter-sets") else {
        return Ok(Codec::H264 {
            parameter_sets: None,
        });
    };
    let mut sps = Vec::new();
    let mut pps = Vec::new();
    for set in sets.split(',') {
        let nal = Bytes::from(
            BASE64
                .decode(set)
                .with_context(|| format!("invalid sprop-parameter-sets entry {set:?}"))?,
        );
        match nal.first().map(|header| header & 0x1F) {
            Some(7) => sps.push(nal),
            Some(8) => pps.push(nal),
            _ => bail!("sprop-parameter-sets entry is neither SPS nor PPS"),
        }
    }
    ensure!(!pps.is_empty(), "sprop-parameter-sets has no PPS");
    Ok(Codec::H264 {
        parameter_sets: Some(AvcDecoderConfigurationRecord::from_parameter_sets(
            sps, pps,
        )?),
    })
}

fn aac_codec(parameters: &[(String, String)]) -> Result<Codec> {
    let mode = parameter(parameters, "mode").unwrap_or_default();
    if !mode.eq_ignore_ascii_case(AAC_HBR_MODE) {
        return Ok(Codec::Unsupported(format!("{AAC_ENCODING} mode={mode}")));
    }
    let config = parameter(parameters, "config").context("AAC-hbr fmtp without config")?;
    let config = AudioSpecificConfig::parse(&decode_hex(config)?)?;
    let width = |name: &str, default: u8| -> Result<u8> {
        parameter(parameters, name).map_or(Ok(default), |value| {
            value
                .parse()
                .with_context(|| format!("invalid {name} {value:?}"))
        })
    };
    let hbr = AuHeaderConfig::AAC_HBR;
    Ok(Codec::Aac {
        config,
        au_headers: AuHeaderConfig {
            size_length: width("sizelength", hbr.size_length)?,
            index_length: width("indexlength", hbr.index_length)?,
            index_delta_length: width("indexdeltalength", hbr.index_delta_length)?,
        },
    })
}

fn decode_hex(text: &str) -> Result<Vec<u8>> {
    ensure!(
        text.len().is_multiple_of(2),
        "odd-length hex string {text:?}"
    );
    (0..text.len())
        .step_by(2)
        .map(|index| {
            u8::from_str_radix(&text[index..index + 2], 16)
                .with_context(|| format!("invalid hex string {text:?}"))
        })
        .collect()
}

fn encode_hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn address_type(address: IpAddr) -> &'static str {
    match address {
        IpAddr::V4(_) => "IP4",
        IpAddr::V6(_) => "IP6",
    }
}

impl fmt::Display for SessionDescription {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let origin = self.destination.unwrap_or(IpAddr::from([0, 0, 0, 0]));
        writeln!(f, "v=0")?;
        writeln!(f, "o=- 0 0 IN {} {origin}", address_type(origin))?;
        writeln!(f, "s=-")?;
        if let Some(destination) = self.destination {
            writeln!(f, "c=IN {} {destination}", address_type(destination))?;
        }
        writeln!(f, "t=0 0")?;
        for media in &self.media {
            write!(f, "{media}")?;
        }
        Ok(())
    }
}

impl fmt::Display for MediaDescription {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let kind = match self.kind {
            MediaKind::Video => "video",
            MediaKind::Audio => "audio",
        };
        writeln!(f, "m={kind} {} RTP/AVP {}", self.port, self.payload_type)?;
        if let Some(destination) = self.destination {
            writeln!(f, "c=IN {} {destination}", address_type(destination))?;
        }
        let payload_type = self.payload_type;
        match &self.codec {
            Codec::H264 { parameter_sets } => {
                writeln!(
                    f,
                    "a=rtpmap:{payload_type} {H264_ENCODING}/{}",
                    self.clock_rate
                )?;
                write!(f, "a=fmtp:{payload_type} packetization-mode=1")?;
                if let Some(record) = parameter_sets {
                    let sets: Vec<String> = record
                        .sps
                        .iter()
                        .chain(&record.pps)
                        .map(|set| BASE64.encode(set))
                        .collect();
                    write!(
                        f,
                        ";sprop-parameter-sets={};profile-level-id={}",
                        sets.join(","),
                        encode_hex(&[
                            record.profile_idc,
                            record.profile_compatibility,
                            record.level_idc
                        ])
                    )?;
                }
                writeln!(f)
            }
            Codec::Aac { config, au_headers } => {
                writeln!(
                    f,
                    "a=rtpmap:{payload_type} {AAC_ENCODING}/{}/{}",
                    self.clock_rate,
                    config.channel_count()
                )?;
                writeln!(
                    f,
                    "a=fmtp:{payload_type} profile-level-id=1;mode={AAC_HBR_MODE};sizelength={};indexlength={};indexdeltalength={};config={}",
                    au_headers.size_length,
                    au_headers.index_length,
                    au_headers.index_delta_length,
                    encode_hex(&config.to_bytes())
                )
            }
            Codec::Pcma { channels } => {
                write!(
                    f,
                    "a=rtpmap:{payload_type} {PCMA_ENCODING}/{}",
                    self.clock_rate
                )?;
                if *channels != 1 {
                    write!(f, "/{channels}")?;
                }
                writeln!(f)
            }
            Codec::Opus { channels } => {
                writeln!(
                    f,
                    "a=rtpmap:{payload_type} {OPUS_ENCODING}/{OPUS_CLOCK_RATE}/2"
                )?;
                if *channels == 2 {
                    writeln!(f, "a=fmtp:{payload_type} sprop-stereo=1")?;
                }
                Ok(())
            }
            Codec::Unsupported(encoding) => {
                writeln!(f, "a=rtpmap:{payload_type} {encoding}/{}", self.clock_rate)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{fixture_record, mono_48k};

    const FFMPEG_SDP: &str = "v=0\r\n\
        o=- 0 0 IN IP4 127.0.0.1\r\n\
        s=No Name\r\n\
        c=IN IP4 127.0.0.1\r\n\
        t=0 0\r\n\
        a=tool:libavformat 61.7.100\r\n\
        m=video 5004 RTP/AVP 96\r\n\
        b=AS:2000\r\n\
        a=rtpmap:96 H264/90000\r\n\
        a=fmtp:96 packetization-mode=1; sprop-parameter-sets=Z0LQC9oKN+TARAAAAwAEAAADAHg8SJqA,aM48gA==; profile-level-id=42D00B\r\n\
        m=audio 5006 RTP/AVP 97\r\n\
        b=AS:128\r\n\
        a=rtpmap:97 MPEG4-GENERIC/48000/1\r\n\
        a=fmtp:97 profile-level-id=1;mode=AAC-hbr;sizelength=13;indexlength=3;indexdeltalength=3; config=1188\r\n";

    #[test]
    fn parses_an_ffmpeg_session_description() {
        // Act
        let description = SessionDescription::parse(FFMPEG_SDP).unwrap();

        // Assert
        assert_eq!(description.destination, Some(IpAddr::from([127, 0, 0, 1])));
        assert_eq!(
            description.media,
            [
                MediaDescription {
                    kind: MediaKind::Video,
                    port: 5004,
                    destination: None,
                    payload_type: 96,
                    clock_rate: 90_000,
                    codec: Codec::H264 {
                        parameter_sets: Some(fixture_record())
                    },
                },
                MediaDescription {
                    kind: MediaKind::Audio,
                    port: 5006,
                    destination: None,
                    payload_type: 97,
                    clock_rate: 48_000,
                    codec: Codec::Aac {
                        config: mono_48k(),
                        au_headers: AuHeaderConfig::AAC_HBR,
                    },
                },
            ]
        );
    }

    #[test]
    fn round_trips_through_display() {
        // Arrange
        let description = SessionDescription::parse(FFMPEG_SDP).unwrap();

        // Act
        let text = description.to_string();
        let reparsed = SessionDescription::parse(&text).unwrap();

        // Assert
        assert_eq!(reparsed, description);
        assert!(text.contains("a=fmtp:96 packetization-mode=1;sprop-parameter-sets=Z0LQC9oKN+TARAAAAwAEAAADAHg8SJqA,aM48gA==;profile-level-id=42d00b\n"));
        assert!(text.contains("a=rtpmap:97 MPEG4-GENERIC/48000/1\n"));
        assert!(text.contains("config=1188\n"));
    }

    #[test]
    fn keeps_media_level_connection_and_marks_unsupported_formats() {
        // Arrange
        let text = "v=0\n\
            m=video 5004 RTP/AVP 96\n\
            c=IN IP4 239.0.0.1/127\n\
            a=rtpmap:96 H264/90000\n\
            m=audio 5006 RTP/AVP 0\n\
            m=audio 5008 RTP/AVP 9\n\
            a=rtpmap:9 G722/8000\n\
            m=application 5010 RTP/AVP 98\n\
            a=rtpmap:98 metadata/90000\n";

        // Act
        let description = SessionDescription::parse(text).unwrap();

        // Assert
        assert_eq!(description.media.len(), 3);
        assert_eq!(
            description.destination_of(&description.media[0]),
            Some(IpAddr::from([239, 0, 0, 1]))
        );
        assert_eq!(
            description.media[0].codec,
            Codec::H264 {
                parameter_sets: None
            }
        );
        assert_eq!(
            description.media[1].codec,
            Codec::Unsupported("static payload type 0".to_owned())
        );
        assert_eq!(
            description.media[2].codec,
            Codec::Unsupported("G722".to_owned())
        );
    }

    #[test]
    fn reads_a_law_and_opus_sections() {
        // Arrange: A-law by its static payload type, Opus with stereo signalled
        let text = "v=0\n\
            m=audio 5006 RTP/AVP 8\n\
            m=audio 5008 RTP/AVP 111\n\
            a=rtpmap:111 opus/48000/2\n\
            a=fmtp:111 minptime=10;sprop-stereo=1\n\
            m=audio 5010 RTP/AVP 112\n\
            a=rtpmap:112 opus/48000/2\n\
            m=audio 5012 RTP/AVP 113\n\
            a=rtpmap:113 PCMA/16000/2\n";

        // Act
        let description = SessionDescription::parse(text).unwrap();
        let reparsed = SessionDescription::parse(&description.to_string()).unwrap();

        // Assert
        let codecs: Vec<(&Codec, u32)> = description
            .media
            .iter()
            .map(|media| (&media.codec, media.clock_rate))
            .collect();
        assert_eq!(
            codecs,
            [
                (&Codec::Pcma { channels: 1 }, 8_000),
                (&Codec::Opus { channels: 2 }, 48_000),
                (&Codec::Opus { channels: 1 }, 48_000),
                (&Codec::Pcma { channels: 2 }, 16_000),
            ]
        );
        assert_eq!(reparsed, description);
    }

    #[test]
    fn rejects_malformed_descriptions() {
        // Arrange
        let no_port = "m=video RTP/AVP 96\n";
        let interleaved =
            "m=video 5004 RTP/AVP 96\na=rtpmap:96 H264/90000\na=fmtp:96 packetization-mode=2\n";
        let bad_sprop =
            "m=video 5004 RTP/AVP 96\na=rtpmap:96 H264/90000\na=fmtp:96 sprop-parameter-sets=!!\n";
        let no_config =
            "m=audio 5006 RTP/AVP 97\na=rtpmap:97 MPEG4-GENERIC/48000/1\na=fmtp:97 mode=AAC-hbr\n";
        let no_equals = "v=0\nhello\n";

        // Act / Assert
        for text in [no_port, interleaved, bad_sprop, no_config, no_equals] {
            assert!(SessionDescription::parse(text).is_err(), "{text:?}");
        }
    }
}
