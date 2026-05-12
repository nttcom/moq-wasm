use crate::app_config::Target;
use crate::rtsp_frame::{EncodedAudioPacket, EncodedPacket, Frame, RtspPacket};
use anyhow::{bail, Result};
use base64::{engine::general_purpose, Engine as _};
use ffmpeg_next as ffmpeg;
use ffmpeg_next::codec::Id;
use ffmpeg_next::ffi::av_sdp_create;
use std::ffi::CStr;
use std::sync::mpsc::Sender;
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::sync::mpsc::Sender as TokioSender;

pub fn run(target: Target, tx: Sender<Frame>, err_tx: Sender<String>) {
    if let Err(err) = ffmpeg::init() {
        send_error(&err_tx, format!("ffmpeg init failed: {err}"));
        return;
    }
    let url = target.rtsp_url();
    let mut opts = ffmpeg::Dictionary::new();
    apply_rtsp_low_latency_options(&mut opts);
    let mut input = match ffmpeg::format::input_with_dictionary(&url, opts) {
        Ok(input) => input,
        Err(err) => {
            send_error(&err_tx, err.to_string());
            return;
        }
    };
    log_sdp(&input);
    let stream = match input.streams().best(ffmpeg::media::Type::Video) {
        Some(stream) => stream,
        None => {
            send_error(&err_tx, "no video stream found".to_string());
            return;
        }
    };
    let stream_index = stream.index();
    let context = match ffmpeg::codec::context::Context::from_parameters(stream.parameters()) {
        Ok(context) => context,
        Err(err) => {
            send_error(&err_tx, err.to_string());
            return;
        }
    };
    let mut decoder = match context.decoder().video() {
        Ok(decoder) => decoder,
        Err(err) => {
            send_error(&err_tx, err.to_string());
            return;
        }
    };
    let mut scaler = match ffmpeg::software::scaling::Context::get(
        decoder.format(),
        decoder.width(),
        decoder.height(),
        ffmpeg::format::Pixel::RGBA,
        decoder.width(),
        decoder.height(),
        ffmpeg::software::scaling::Flags::BILINEAR,
    ) {
        Ok(scaler) => scaler,
        Err(err) => {
            send_error(&err_tx, err.to_string());
            return;
        }
    };
    let mut decoded = ffmpeg::util::frame::Video::empty();
    let mut rgba = ffmpeg::util::frame::Video::empty();
    for (stream, packet) in input.packets() {
        if stream.index() != stream_index {
            continue;
        }
        if decoder.send_packet(&packet).is_err() {
            continue;
        }
        while decoder.receive_frame(&mut decoded).is_ok() {
            if scaler.run(&decoded, &mut rgba).is_err() {
                continue;
            }
            if let Some(frame) = copy_frame(&rgba, decoder.width(), decoder.height()) {
                let _ = tx.send(frame);
            }
        }
    }
}

pub fn run_encoded(
    target: Target,
    codec_label: String,
    payload_format: PayloadFormat,
    tx: TokioSender<RtspPacket>,
    err_tx: Sender<String>,
) {
    run_encoded_url(target.rtsp_url(), codec_label, payload_format, tx, err_tx);
}

pub fn run_encoded_url(
    url: String,
    codec_label: String,
    payload_format: PayloadFormat,
    tx: TokioSender<RtspPacket>,
    err_tx: Sender<String>,
) {
    if let Err(err) = ffmpeg::init() {
        send_error(&err_tx, format!("ffmpeg init failed: {err}"));
        return;
    }
    let mut opts = ffmpeg::Dictionary::new();
    apply_rtsp_low_latency_options(&mut opts);
    let mut input = match ffmpeg::format::input_with_dictionary(&url, opts) {
        Ok(input) => input,
        Err(err) => {
            send_error(&err_tx, err.to_string());
            return;
        }
    };
    log_sdp(&input);
    let video_stream = match input.streams().best(ffmpeg::media::Type::Video) {
        Some(stream) => stream,
        None => {
            send_error(&err_tx, "no video stream found".to_string());
            return;
        }
    };
    let annexb = if payload_format == PayloadFormat::AnnexB {
        match AnnexBConverter::from_stream(&video_stream) {
            Ok(converter) => converter,
            Err(err) => {
                send_error(
                    &err_tx,
                    format!("annexb converter init failed: {err} (assume AnnexB)"),
                );
                None
            }
        }
    } else {
        None
    };
    let video_stream_index = video_stream.index();
    let video_time_base = video_stream.time_base();
    let default_interval_us = default_frame_interval_us(&video_stream);
    let mut video_timestamp_corrector = TimestampCorrector::new(default_interval_us);
    let fallback_codec_label = (!codec_label.is_empty()).then_some(codec_label);
    let mut video_codec_label = None;
    let mut avcc_state = if payload_format == PayloadFormat::Avcc {
        match AnnexBConverter::from_stream(&video_stream) {
            Ok(Some(converter)) => Some(AvccConfig::from(&converter)),
            Ok(None) => None,
            Err(err) => {
                send_error(
                    &err_tx,
                    format!("avcc config init failed: {err} (payload-format=avcc)"),
                );
                None
            }
        }
    } else {
        annexb.as_ref().map(AvccConfig::from)
    };
    let audio_info = match input.streams().best(ffmpeg::media::Type::Audio) {
        Some(stream) => match AudioStreamInfo::from_stream(&stream) {
            Ok(Some(info)) => {
                log::info!(
                    "RTSP audio stream detected: codec={} sample_rate={:?} channels={:?}",
                    info.codec_label,
                    info.sample_rate,
                    info.channels
                );
                Some(info)
            }
            Ok(None) => None,
            Err(err) => {
                log::warn!("failed to initialize RTSP audio stream: {err}");
                None
            }
        },
        None => {
            log::info!("RTSP audio stream not found");
            None
        }
    };
    let mut format_logged = false;
    let mut video_last_timestamp_source: Option<TimestampSource> = None;
    let mut video_timestamp_presence = TimestampPresence::default();
    let mut audio_last_timestamp_source: Option<TimestampSource> = None;
    let mut audio_timestamp_presence = TimestampPresence::default();
    let mut audio_timestamp_corrector = audio_info
        .as_ref()
        .map(|info| TimestampCorrector::new(info.default_interval_us));
    for (stream, packet) in input.packets() {
        if stream.index() == video_stream_index {
            let ingest_wallclock_micros = now_micros();
            let is_keyframe = packet.is_key();
            let payload = match build_payload(&packet, is_keyframe, payload_format, annexb.as_ref())
            {
                Ok(Some(payload)) => payload,
                Ok(None) => continue,
                Err(err) => {
                    send_error(&err_tx, format!("annexb conversion failed: {err}"));
                    return;
                }
            };
            if !format_logged {
                log::info!("RTSP payload format in use: {}", payload_format.label());
                format_logged = true;
            }
            if payload_format == PayloadFormat::AnnexB && is_annexb(&payload) {
                let nal_info = collect_nal_types(&payload);
                if nal_info.has_idr != is_keyframe {
                    log::warn!(
                        "RTSP keyframe mismatch: packet.is_key={} has_idr={} nal_types={}",
                        is_keyframe,
                        nal_info.has_idr,
                        nal_info.summary
                    );
                }
            }
            if let Some(slice_type) = detect_slice_type(
                &payload,
                payload_format,
                avcc_state
                    .as_ref()
                    .map(|state| state.nal_length_size)
                    .unwrap_or(DEFAULT_NAL_LENGTH_SIZE),
            ) {
                let label = slice_type_label(slice_type);
                let timestamp_us = packet_timestamp_us(&packet, video_time_base);
                log::debug!(
                    "RTSP slice_type={} raw={} ts_us={} keyframe={} format={}",
                    label,
                    slice_type,
                    timestamp_us,
                    is_keyframe,
                    payload_format.label()
                );
            }
            if payload_format == PayloadFormat::AnnexB && !is_annexb(&payload) {
                log::warn!(
                    "RTSP payload is not AnnexB: size={} keyframe={}",
                    payload.len(),
                    is_keyframe,
                );
            }
            let pts = packet.pts();
            let dts = packet.dts();
            if !video_timestamp_presence.logged {
                log::debug!(
                    "RTSP video timestamps initial: pts_raw={:?} dts_raw={:?} time_base={}/{}",
                    pts,
                    dts,
                    video_time_base.numerator(),
                    video_time_base.denominator()
                );
                video_timestamp_presence.logged = true;
            }
            if pts.is_some() && !video_timestamp_presence.seen_pts {
                log::debug!(
                    "RTSP video timestamp available: PTS (us={:?})",
                    pts.and_then(|value| to_microseconds(value, video_time_base))
                );
                video_timestamp_presence.seen_pts = true;
            }
            if dts.is_some() && !video_timestamp_presence.seen_dts {
                log::debug!(
                    "RTSP video timestamp available: DTS (us={:?})",
                    dts.and_then(|value| to_microseconds(value, video_time_base))
                );
                video_timestamp_presence.seen_dts = true;
            }
            if payload_format == PayloadFormat::AnnexB {
                if let Some(state) = avcc_state.as_mut() {
                    let update = state.update_from_annexb(&payload);
                    if update.sps_updated || update.pps_updated {
                        log_sps_pps(state);
                        log_avcc_bytes(state);
                    }
                } else if let Some(state) = AvccConfig::from_annexb(&payload) {
                    log_sps_pps(&state);
                    log_avcc_bytes(&state);
                    avcc_state = Some(state);
                }
            } else if payload_format == PayloadFormat::Avcc {
                if let Some(state) = avcc_state.as_mut() {
                    let update = state.update_from_avcc(&payload);
                    if update.sps_updated || update.pps_updated {
                        log_sps_pps(state);
                        log_avcc_bytes(state);
                    }
                } else if let Some(state) = AvccConfig::from_avcc_payload(&payload) {
                    log_sps_pps(&state);
                    log_avcc_bytes(&state);
                    avcc_state = Some(state);
                }
            }
            if let Some(state) = avcc_state.as_ref() {
                if let Some(codec) = state.codec_string() {
                    video_codec_label = Some(codec);
                }
            }
            let codec_for_packet = is_keyframe
                .then(|| {
                    video_codec_label
                        .clone()
                        .or_else(|| fallback_codec_label.clone())
                })
                .flatten();
            let description_base64 = is_keyframe
                .then(|| avcc_state.as_ref().and_then(AvccConfig::to_base64))
                .flatten();
            let avc_format = payload_format.label().to_string();
            let timestamp_source = if packet.pts().is_some() {
                TimestampSource::Pts
            } else if packet.dts().is_some() {
                TimestampSource::Dts
            } else {
                TimestampSource::FallbackZero
            };
            if video_last_timestamp_source != Some(timestamp_source) {
                if timestamp_source == TimestampSource::Pts {
                    log::debug!("RTSP video timestamp source: PTS");
                } else {
                    log::info!(
                        "RTSP video timestamp source fallback: {:?}",
                        timestamp_source
                    );
                }
                video_last_timestamp_source = Some(timestamp_source);
            }
            let raw_timestamp_us = packet_timestamp_us(&packet, video_time_base);
            let duration_us = if packet.duration() > 0 {
                to_microseconds(packet.duration(), video_time_base)
            } else {
                None
            };
            let timestamp_us = video_timestamp_corrector.correct(raw_timestamp_us, duration_us);
            let encoded = build_encoded_packet(EncodedPacketArgs {
                payload,
                packet: &packet,
                is_keyframe,
                time_base: video_time_base,
                codec_label: codec_for_packet,
                description_base64,
                avc_format: Some(avc_format),
                timestamp_source,
                timestamp_us,
                ingest_wallclock_micros,
                duration_us,
            });
            if tx.blocking_send(RtspPacket::Video(encoded)).is_err() {
                return;
            }
            continue;
        }

        let Some(audio_info) = audio_info.as_ref() else {
            continue;
        };
        if stream.index() != audio_info.stream_index {
            continue;
        }
        let Some(payload) = packet.data().map(ToOwned::to_owned) else {
            continue;
        };
        let pts = packet.pts();
        let dts = packet.dts();
        if !audio_timestamp_presence.logged {
            log::debug!(
                "RTSP audio timestamps initial: pts_raw={:?} dts_raw={:?} time_base={}/{}",
                pts,
                dts,
                audio_info.time_base.numerator(),
                audio_info.time_base.denominator()
            );
            audio_timestamp_presence.logged = true;
        }
        if pts.is_some() && !audio_timestamp_presence.seen_pts {
            log::debug!(
                "RTSP audio timestamp available: PTS (us={:?})",
                pts.and_then(|value| to_microseconds(value, audio_info.time_base))
            );
            audio_timestamp_presence.seen_pts = true;
        }
        if dts.is_some() && !audio_timestamp_presence.seen_dts {
            log::debug!(
                "RTSP audio timestamp available: DTS (us={:?})",
                dts.and_then(|value| to_microseconds(value, audio_info.time_base))
            );
            audio_timestamp_presence.seen_dts = true;
        }
        let timestamp_source = if pts.is_some() {
            TimestampSource::Pts
        } else if dts.is_some() {
            TimestampSource::Dts
        } else {
            TimestampSource::FallbackZero
        };
        if audio_last_timestamp_source != Some(timestamp_source) {
            if timestamp_source == TimestampSource::Pts {
                log::debug!("RTSP audio timestamp source: PTS");
            } else {
                log::info!(
                    "RTSP audio timestamp source fallback: {:?}",
                    timestamp_source
                );
            }
            audio_last_timestamp_source = Some(timestamp_source);
        }
        let raw_timestamp_us = packet_timestamp_us(&packet, audio_info.time_base);
        let duration_us = if packet.duration() > 0 {
            to_microseconds(packet.duration(), audio_info.time_base)
        } else {
            None
        };
        let corrected_duration_us = duration_us
            .or_else(|| frame_size_to_duration_us(audio_info.frame_size, audio_info.sample_rate));
        let timestamp_us = audio_timestamp_corrector
            .as_mut()
            .map(|corrector| corrector.correct(raw_timestamp_us, corrected_duration_us))
            .unwrap_or(raw_timestamp_us);
        let encoded = build_audio_packet(
            &packet,
            payload,
            audio_info,
            timestamp_us,
            now_micros(),
            corrected_duration_us,
        );
        if tx.blocking_send(RtspPacket::Audio(encoded)).is_err() {
            return;
        }
    }
}

fn copy_frame(frame: &ffmpeg::util::frame::Video, width: u32, height: u32) -> Option<Frame> {
    let width = width as usize;
    let height = height as usize;
    if width == 0 || height == 0 {
        return None;
    }
    let stride = frame.stride(0);
    let data = frame.data(0);
    if data.len() < stride * height {
        return None;
    }
    let mut out = vec![0u8; width * height * 4];
    for row in 0..height {
        let src_start = row * stride;
        let src_end = src_start + width * 4;
        let dst_start = row * width * 4;
        let dst_end = dst_start + width * 4;
        out[dst_start..dst_end].copy_from_slice(&data[src_start..src_end]);
    }
    Some(Frame {
        width,
        height,
        data: out,
    })
}

fn extract_extradata(stream: &ffmpeg::Stream<'_>) -> Option<Vec<u8>> {
    unsafe {
        let params = stream.parameters();
        let ptr = (*params.as_ptr()).extradata;
        let size = (*params.as_ptr()).extradata_size;
        if ptr.is_null() || size <= 0 {
            return None;
        }
        let bytes = std::slice::from_raw_parts(ptr, size as usize);
        Some(bytes.to_vec())
    }
}

struct EncodedPacketArgs<'a> {
    payload: Vec<u8>,
    packet: &'a ffmpeg::Packet,
    is_keyframe: bool,
    time_base: ffmpeg::Rational,
    codec_label: Option<String>,
    description_base64: Option<String>,
    avc_format: Option<String>,
    timestamp_source: TimestampSource,
    timestamp_us: u64,
    ingest_wallclock_micros: u64,
    duration_us: Option<u64>,
}

struct AudioStreamInfo {
    stream_index: usize,
    time_base: ffmpeg::Rational,
    codec_label: String,
    description_base64: Option<String>,
    sample_rate: Option<u32>,
    channels: Option<u8>,
    frame_size: u32,
    default_interval_us: u64,
}

impl AudioStreamInfo {
    fn from_stream(stream: &ffmpeg::Stream<'_>) -> Result<Option<Self>> {
        let codec_id = stream.parameters().id();
        let codec_label = match codec_id {
            Id::AAC => "mp4a.40.2".to_string(),
            Id::OPUS => "opus".to_string(),
            Id::PCM_ALAW => "pcma".to_string(),
            other => {
                log::warn!("RTSP audio codec unsupported for MoQ bridge: {:?}", other);
                return Ok(None);
            }
        };
        let context = ffmpeg::codec::context::Context::from_parameters(stream.parameters())?;
        let decoder = context.decoder().audio()?;
        let sample_rate = Some(decoder.rate()).filter(|value| *value > 0);
        let channels = u8::try_from(decoder.channels())
            .ok()
            .filter(|value| *value > 0);
        let frame_size = decoder.frame_size();
        let default_interval_us =
            frame_size_to_duration_us(frame_size, sample_rate).unwrap_or(DEFAULT_AUDIO_INTERVAL_US);
        let description_base64 = if codec_id == Id::AAC {
            extract_extradata(stream)
                .filter(|bytes| !bytes.is_empty())
                .map(|bytes| general_purpose::STANDARD.encode(bytes))
        } else {
            None
        };
        Ok(Some(Self {
            stream_index: stream.index(),
            time_base: stream.time_base(),
            codec_label,
            description_base64,
            sample_rate,
            channels,
            frame_size,
            default_interval_us,
        }))
    }
}

fn build_encoded_packet(args: EncodedPacketArgs<'_>) -> EncodedPacket {
    let pts_us = args
        .packet
        .pts()
        .and_then(|value| to_microseconds(value, args.time_base));
    let dts_us = args
        .packet
        .dts()
        .and_then(|value| to_microseconds(value, args.time_base));
    log_timestamp_choice(
        args.packet,
        args.time_base,
        args.timestamp_us,
        args.timestamp_source,
        args.is_keyframe,
    );
    EncodedPacket {
        data: args.payload,
        is_keyframe: args.is_keyframe,
        timestamp_us: args.timestamp_us,
        ingest_wallclock_micros: args.ingest_wallclock_micros,
        pts_us,
        dts_us,
        duration_us: args.duration_us,
        codec: args.codec_label,
        description_base64: args.description_base64,
        avc_format: args.avc_format,
    }
}

fn build_audio_packet(
    packet: &ffmpeg::Packet,
    payload: Vec<u8>,
    info: &AudioStreamInfo,
    timestamp_us: u64,
    ingest_wallclock_micros: u64,
    duration_us: Option<u64>,
) -> EncodedAudioPacket {
    let pts_us = packet
        .pts()
        .and_then(|value| to_microseconds(value, info.time_base));
    let dts_us = packet
        .dts()
        .and_then(|value| to_microseconds(value, info.time_base));
    EncodedAudioPacket {
        data: payload,
        timestamp_us,
        ingest_wallclock_micros,
        pts_us,
        dts_us,
        duration_us,
        codec: info.codec_label.clone(),
        description_base64: info.description_base64.clone(),
        sample_rate: info.sample_rate,
        channels: info.channels,
    }
}

fn to_microseconds(value: i64, time_base: ffmpeg::Rational) -> Option<u64> {
    if value < 0 {
        return None;
    }
    let den = time_base.denominator() as i128;
    if den == 0 {
        return None;
    }
    let num = time_base.numerator() as i128;
    let scaled = value as i128 * num * 1_000_000i128 / den;
    if scaled < 0 {
        return None;
    }
    Some(scaled as u64)
}

fn packet_timestamp_us(packet: &ffmpeg::Packet, time_base: ffmpeg::Rational) -> u64 {
    packet
        .pts()
        .or_else(|| packet.dts())
        .and_then(|pts| to_microseconds(pts, time_base))
        .unwrap_or(0)
}

fn log_timestamp_choice(
    packet: &ffmpeg::Packet,
    time_base: ffmpeg::Rational,
    timestamp_us: u64,
    source: TimestampSource,
    is_keyframe: bool,
) {
    // Temporarily disabled: use send_video_packet logs to align with group/object IDs.
    let _ = packet;
    let _ = time_base;
    let _ = timestamp_us;
    let _ = source;
    let _ = is_keyframe;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TimestampSource {
    Pts,
    Dts,
    FallbackZero,
}

#[derive(Default)]
struct TimestampPresence {
    logged: bool,
    seen_pts: bool,
    seen_dts: bool,
}

struct TimestampCorrector {
    last_raw: Option<u64>,
    last_corrected: Option<u64>,
    interval_us: u64,
    offset_us: i64,
    correction_log_count: u64,
}

impl TimestampCorrector {
    fn new(interval_us: u64) -> Self {
        let interval_us = if interval_us == 0 {
            DEFAULT_FRAME_INTERVAL_US
        } else {
            interval_us
        };
        Self {
            last_raw: None,
            last_corrected: None,
            interval_us,
            offset_us: 0,
            correction_log_count: 0,
        }
    }

    fn correct(&mut self, raw_us: u64, duration_us: Option<u64>) -> u64 {
        if let Some(duration_us) = duration_us {
            if (MIN_INTERVAL_US..=MAX_INTERVAL_US).contains(&duration_us) {
                self.interval_us = duration_us;
            }
        } else if let Some(last_raw) = self.last_raw {
            if raw_us > last_raw {
                let diff = raw_us - last_raw;
                if (MIN_INTERVAL_US..=MAX_INTERVAL_US).contains(&diff) {
                    self.interval_us = (self.interval_us * 3 + diff) / 4;
                }
            }
        }

        let interval = self.interval_us.max(1);
        let min_step = (interval / 2).max(1);
        let max_step = interval.saturating_mul(MAX_STEP_MULTIPLIER);

        let Some(last_corrected) = self.last_corrected else {
            self.last_raw = Some(raw_us);
            self.last_corrected = Some(raw_us);
            return raw_us;
        };

        let candidate = raw_us as i64 + self.offset_us;
        let min_allowed = last_corrected.saturating_add(min_step);
        let max_allowed = last_corrected.saturating_add(max_step);

        let corrected = if candidate < min_allowed as i64 || candidate > max_allowed as i64 {
            let expected = last_corrected.saturating_add(interval);
            self.offset_us = expected as i64 - raw_us as i64;
            self.correction_log_count = self.correction_log_count.saturating_add(1);
            if self.correction_log_count <= 5 || self.correction_log_count.is_multiple_of(100) {
                log::debug!(
                    "RTSP timestamp corrected: count={} raw_us={} corrected_us={} interval_us={} last_corrected_us={} offset_us={}",
                    self.correction_log_count,
                    raw_us,
                    expected,
                    interval,
                    last_corrected,
                    self.offset_us
                );
            }
            expected
        } else {
            candidate as u64
        };

        self.last_raw = Some(raw_us);
        self.last_corrected = Some(corrected);
        corrected
    }
}

const DEFAULT_FRAME_INTERVAL_US: u64 = 66_666;
const DEFAULT_AUDIO_INTERVAL_US: u64 = 20_000;
const MIN_INTERVAL_US: u64 = 5_000;
const MAX_INTERVAL_US: u64 = 200_000;
const MAX_STEP_MULTIPLIER: u64 = 5;

fn default_frame_interval_us(stream: &ffmpeg::Stream<'_>) -> u64 {
    let avg = stream.avg_frame_rate();
    if let Some(interval) = interval_from_rate(avg) {
        return interval;
    }
    let rate = stream.rate();
    if let Some(interval) = interval_from_rate(rate) {
        return interval;
    }
    DEFAULT_FRAME_INTERVAL_US
}

fn interval_from_rate(rate: ffmpeg::Rational) -> Option<u64> {
    let num = rate.numerator() as i128;
    let den = rate.denominator() as i128;
    if num <= 0 || den <= 0 {
        return None;
    }
    let interval = 1_000_000i128 * den / num;
    if interval <= 0 {
        return None;
    }
    Some(interval as u64)
}

fn frame_size_to_duration_us(frame_size: u32, sample_rate: Option<u32>) -> Option<u64> {
    let frame_size = u64::from(frame_size);
    let sample_rate = u64::from(sample_rate?);
    if frame_size == 0 || sample_rate == 0 {
        return None;
    }
    Some((frame_size * 1_000_000 + sample_rate / 2) / sample_rate)
}

fn send_error(err_tx: &Sender<String>, message: String) {
    let _ = err_tx.send(message);
}

fn apply_rtsp_low_latency_options(opts: &mut ffmpeg::Dictionary<'_>) {
    opts.set("rtsp_transport", "tcp");
    opts.set("fflags", "nobuffer");
    opts.set("flags", "low_delay");
    opts.set("max_delay", "0");
    opts.set("reorder_queue_size", "0");
}

fn now_micros() -> u64 {
    let duration = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    duration.as_micros() as u64
}

fn log_sdp(input: &ffmpeg::format::context::Input) {
    if let Some(sdp) = input.metadata().get("sdp") {
        log::info!("RTSP SDP:\n{sdp}");
        return;
    }
    if let Some(sdp) = create_sdp_from_context(input) {
        log::info!("RTSP SDP:\n{sdp}");
        return;
    }
    log::info!("RTSP SDP not available");
}

fn create_sdp_from_context(input: &ffmpeg::format::context::Input) -> Option<String> {
    let mut buffer = vec![0u8; 8192];
    let result = unsafe {
        let mut contexts = [input.as_ptr() as *mut ffmpeg::ffi::AVFormatContext];
        av_sdp_create(
            contexts.as_mut_ptr(),
            contexts.len() as i32,
            buffer.as_mut_ptr() as *mut _,
            buffer.len() as i32,
        )
    };
    if result < 0 {
        return None;
    }
    let cstr = unsafe { CStr::from_ptr(buffer.as_ptr() as *const _) };
    let sdp = cstr.to_string_lossy().to_string();
    if sdp.is_empty() {
        None
    } else {
        Some(sdp)
    }
}

fn build_payload(
    packet: &ffmpeg::Packet,
    is_keyframe: bool,
    payload_format: PayloadFormat,
    converter: Option<&AnnexBConverter>,
) -> Result<Option<Vec<u8>>> {
    let Some(data) = packet.data() else {
        return Ok(None);
    };
    if payload_format == PayloadFormat::Avcc {
        if has_start_code(data) {
            return annexb_to_avcc(data, DEFAULT_NAL_LENGTH_SIZE).map(Some);
        }
        return Ok(Some(data.to_vec()));
    }
    if is_annexb(data) {
        return Ok(Some(data.to_vec()));
    }
    let Some(converter) = converter else {
        return Ok(Some(data.to_vec()));
    };
    converter.to_annexb(data, is_keyframe).map(Some)
}

struct AnnexBConverter {
    nal_length_size: usize,
    sps: Vec<Vec<u8>>,
    pps: Vec<Vec<u8>>,
}

impl AnnexBConverter {
    fn from_stream(stream: &ffmpeg::Stream<'_>) -> Result<Option<Self>> {
        if stream.parameters().id() != Id::H264 {
            return Ok(None);
        }
        let Some(extradata) = extract_extradata(stream) else {
            return Ok(None);
        };
        let converter = AnnexBConverter::from_avcc(&extradata)?;
        Ok(Some(converter))
    }

    fn from_avcc(data: &[u8]) -> Result<Self> {
        if data.len() < 7 {
            bail!("AVCC data too short");
        }
        if data[0] != 1 {
            bail!("unsupported AVCC version: {}", data[0]);
        }
        let nal_length_size = ((data[4] & 0x03) + 1) as usize;
        if !(1..=4).contains(&nal_length_size) {
            bail!("invalid NAL length size: {nal_length_size}");
        }
        let num_sps = (data[5] & 0x1f) as usize;
        let mut offset = 6;
        let mut sps = Vec::with_capacity(num_sps);
        for _ in 0..num_sps {
            let nal = read_avcc_unit(data, &mut offset)?;
            sps.push(nal);
        }
        if offset >= data.len() {
            bail!("missing PPS count in AVCC");
        }
        let num_pps = data[offset] as usize;
        offset += 1;
        let mut pps = Vec::with_capacity(num_pps);
        for _ in 0..num_pps {
            let nal = read_avcc_unit(data, &mut offset)?;
            pps.push(nal);
        }
        Ok(Self {
            nal_length_size,
            sps,
            pps,
        })
    }

    fn to_annexb(&self, data: &[u8], is_keyframe: bool) -> Result<Vec<u8>> {
        let mut out = Vec::with_capacity(data.len() + 128);
        if is_keyframe {
            for sps in &self.sps {
                push_start_code(&mut out);
                out.extend_from_slice(sps);
            }
            for pps in &self.pps {
                push_start_code(&mut out);
                out.extend_from_slice(pps);
            }
        }
        append_annexb_nals(data, self.nal_length_size, &mut out)?;
        Ok(out)
    }
}

fn append_annexb_nals(data: &[u8], nal_length_size: usize, out: &mut Vec<u8>) -> Result<()> {
    let mut offset = 0;
    while offset + nal_length_size <= data.len() {
        let nal_len = read_nal_length(&data[offset..offset + nal_length_size]);
        offset += nal_length_size;
        if nal_len == 0 {
            continue;
        }
        let end = offset + nal_len;
        if end > data.len() {
            bail!("NAL length out of bounds");
        }
        push_start_code(out);
        out.extend_from_slice(&data[offset..end]);
        offset = end;
    }
    if offset != data.len() {
        bail!("trailing bytes in AVCC payload");
    }
    Ok(())
}

const DEFAULT_NAL_LENGTH_SIZE: usize = 4;

struct AvccConfig {
    nal_length_size: usize,
    sps: Vec<Vec<u8>>,
    pps: Vec<Vec<u8>>,
}

struct AvccUpdate {
    sps_updated: bool,
    pps_updated: bool,
}

impl AvccConfig {
    fn from_annexb(data: &[u8]) -> Option<Self> {
        let mut config = Self {
            nal_length_size: DEFAULT_NAL_LENGTH_SIZE,
            sps: Vec::new(),
            pps: Vec::new(),
        };
        let update = config.update_from_annexb(data);
        (update.sps_updated || update.pps_updated).then_some(config)
    }

    fn update_from_annexb(&mut self, data: &[u8]) -> AvccUpdate {
        let mut update = AvccUpdate {
            sps_updated: false,
            pps_updated: false,
        };
        let mut sps = Vec::new();
        let mut pps = Vec::new();
        for nal in annexb_units(data) {
            let nal_type = nal.first().map(|b| b & 0x1f);
            match nal_type {
                Some(7) => sps.push(nal.to_vec()),
                Some(8) => pps.push(nal.to_vec()),
                _ => {}
            }
        }
        if !sps.is_empty() && sps != self.sps {
            self.sps = sps;
            update.sps_updated = true;
        }
        if !pps.is_empty() && pps != self.pps {
            self.pps = pps;
            update.pps_updated = true;
        }
        update
    }

    fn update_from_avcc(&mut self, data: &[u8]) -> AvccUpdate {
        let mut update = AvccUpdate {
            sps_updated: false,
            pps_updated: false,
        };
        let (sps, pps) = collect_avcc_parameter_sets(data, self.nal_length_size);
        if !sps.is_empty() && sps != self.sps {
            self.sps = sps;
            update.sps_updated = true;
        }
        if !pps.is_empty() && pps != self.pps {
            self.pps = pps;
            update.pps_updated = true;
        }
        update
    }

    fn from_avcc_payload(data: &[u8]) -> Option<Self> {
        let mut config = Self {
            nal_length_size: DEFAULT_NAL_LENGTH_SIZE,
            sps: Vec::new(),
            pps: Vec::new(),
        };
        let update = config.update_from_avcc(data);
        (update.sps_updated || update.pps_updated).then_some(config)
    }

    fn to_base64(&self) -> Option<String> {
        let avcc = self.to_avcc_bytes()?;
        Some(general_purpose::STANDARD.encode(avcc))
    }

    fn to_avcc_bytes(&self) -> Option<Vec<u8>> {
        let sps = self.sps.first()?;
        if sps.len() < 4 {
            return None;
        }
        let mut out = Vec::with_capacity(11 + sps.len());
        out.push(1);
        out.push(sps[1]);
        out.push(sps[2]);
        out.push(sps[3]);
        let length_size_minus_one = (self.nal_length_size.saturating_sub(1) as u8) & 0x03;
        out.push(0b1111_1100 | length_size_minus_one);
        let num_sps = self.sps.len().min(31);
        out.push(0b1110_0000 | num_sps as u8);
        for sps in self.sps.iter().take(num_sps) {
            out.extend_from_slice(&(sps.len() as u16).to_be_bytes());
            out.extend_from_slice(sps);
        }
        let num_pps = self.pps.len().min(255);
        out.push(num_pps as u8);
        for pps in self.pps.iter().take(num_pps) {
            out.extend_from_slice(&(pps.len() as u16).to_be_bytes());
            out.extend_from_slice(pps);
        }
        Some(out)
    }

    fn codec_string(&self) -> Option<String> {
        let sps = self.sps.first()?;
        if sps.len() < 4 {
            return None;
        }
        let profile_idc = sps[1];
        let constraints = sps[2];
        let level_idc = sps[3];
        Some(format!(
            "avc1.{profile_idc:02X}{constraints:02X}{level_idc:02X}"
        ))
    }
}

impl From<&AnnexBConverter> for AvccConfig {
    fn from(value: &AnnexBConverter) -> Self {
        Self {
            nal_length_size: value.nal_length_size,
            sps: value.sps.clone(),
            pps: value.pps.clone(),
        }
    }
}

fn read_nal_length(bytes: &[u8]) -> usize {
    bytes.iter().fold(0usize, |acc, &b| (acc << 8) | b as usize)
}

fn collect_avcc_parameter_sets(
    data: &[u8],
    nal_length_size: usize,
) -> (Vec<Vec<u8>>, Vec<Vec<u8>>) {
    let mut offset = 0;
    let mut sps = Vec::new();
    let mut pps = Vec::new();
    while offset + nal_length_size <= data.len() {
        let nal_len = read_nal_length(&data[offset..offset + nal_length_size]);
        offset += nal_length_size;
        if nal_len == 0 {
            continue;
        }
        let end = offset + nal_len;
        if end > data.len() {
            break;
        }
        let nal = &data[offset..end];
        if let Some(&first) = nal.first() {
            match first & 0x1f {
                7 => sps.push(nal.to_vec()),
                8 => pps.push(nal.to_vec()),
                _ => {}
            }
        }
        offset = end;
    }
    (sps, pps)
}

fn read_avcc_unit(data: &[u8], offset: &mut usize) -> Result<Vec<u8>> {
    if *offset + 2 > data.len() {
        bail!("AVCC unit length missing");
    }
    let len = u16::from_be_bytes([data[*offset], data[*offset + 1]]) as usize;
    *offset += 2;
    let end = *offset + len;
    if end > data.len() {
        bail!("AVCC unit length out of bounds");
    }
    let unit = data[*offset..end].to_vec();
    *offset = end;
    Ok(unit)
}

fn push_start_code(out: &mut Vec<u8>) {
    out.extend_from_slice(&[0, 0, 0, 1]);
}

fn annexb_units(data: &[u8]) -> Vec<&[u8]> {
    let mut units = Vec::new();
    let Some((start, start_len)) = find_start_code(data, 0) else {
        return units;
    };
    let mut pos = start + start_len;
    let mut next = pos;
    while let Some((index, len)) = find_start_code(data, next) {
        if index > pos {
            units.push(&data[pos..index]);
        }
        pos = index + len;
        next = pos;
    }
    if pos < data.len() {
        units.push(&data[pos..]);
    }
    units
}

fn detect_slice_type(
    payload: &[u8],
    payload_format: PayloadFormat,
    nal_length_size: usize,
) -> Option<u32> {
    match payload_format {
        PayloadFormat::AnnexB => find_slice_type_in_annexb(payload),
        PayloadFormat::Avcc => find_slice_type_in_avcc(payload, nal_length_size),
    }
}

fn find_slice_type_in_annexb(data: &[u8]) -> Option<u32> {
    for nal in annexb_units(data) {
        if let Some(slice_type) = slice_type_from_nal(nal) {
            return Some(slice_type);
        }
    }
    None
}

fn find_slice_type_in_avcc(data: &[u8], nal_length_size: usize) -> Option<u32> {
    let mut offset = 0;
    while offset + nal_length_size <= data.len() {
        let nal_len = read_nal_length(&data[offset..offset + nal_length_size]);
        offset += nal_length_size;
        if nal_len == 0 {
            continue;
        }
        let end = offset + nal_len;
        if end > data.len() {
            break;
        }
        let nal = &data[offset..end];
        if let Some(slice_type) = slice_type_from_nal(nal) {
            return Some(slice_type);
        }
        offset = end;
    }
    None
}

fn slice_type_from_nal(nal: &[u8]) -> Option<u32> {
    let nal_type = nal.first().map(|b| b & 0x1f)?;
    if nal_type == 1 || nal_type == 5 {
        return parse_slice_type(nal);
    }
    None
}

fn parse_slice_type(nal: &[u8]) -> Option<u32> {
    if nal.len() < 2 {
        return None;
    }
    let rbsp = remove_emulation_prevention(&nal[1..]);
    let mut reader = BitReader::new(&rbsp);
    let _first_mb = reader.read_ue()?;
    let slice_type = reader.read_ue()?;
    Some(slice_type)
}

fn slice_type_label(slice_type: u32) -> &'static str {
    match slice_type % 5 {
        0 => "P",
        1 => "B",
        2 => "I",
        3 => "SP",
        4 => "SI",
        _ => "unknown",
    }
}

fn remove_emulation_prevention(data: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(data.len());
    let mut zero_count = 0;
    for &byte in data {
        if zero_count >= 2 && byte == 0x03 {
            zero_count = 0;
            continue;
        }
        out.push(byte);
        if byte == 0 {
            zero_count += 1;
        } else {
            zero_count = 0;
        }
    }
    out
}

struct BitReader<'a> {
    data: &'a [u8],
    bit_pos: usize,
}

impl<'a> BitReader<'a> {
    fn new(data: &'a [u8]) -> Self {
        Self { data, bit_pos: 0 }
    }

    fn read_ue(&mut self) -> Option<u32> {
        let mut zeros = 0;
        while let Some(bit) = self.read_bit() {
            if bit == 0 {
                zeros += 1;
            } else {
                break;
            }
        }
        if zeros > 31 {
            return None;
        }
        let info = if zeros == 0 {
            0
        } else {
            self.read_bits(zeros)?
        };
        Some(((1u32 << zeros) - 1) + info)
    }

    fn read_bits(&mut self, count: usize) -> Option<u32> {
        let mut value = 0u32;
        for _ in 0..count {
            value = (value << 1) | self.read_bit()? as u32;
        }
        Some(value)
    }

    fn read_bit(&mut self) -> Option<u8> {
        if self.bit_pos >= self.data.len() * 8 {
            return None;
        }
        let byte = self.data[self.bit_pos / 8];
        let bit = (byte >> (7 - (self.bit_pos % 8))) & 0x01;
        self.bit_pos += 1;
        Some(bit)
    }
}

fn annexb_to_avcc(data: &[u8], nal_length_size: usize) -> Result<Vec<u8>> {
    if nal_length_size == 0 || nal_length_size > 4 {
        bail!("invalid NAL length size: {nal_length_size}");
    }
    let units = annexb_units(data);
    if units.is_empty() {
        bail!("no AnnexB NAL units found");
    }
    let max_len = 1usize << (nal_length_size * 8);
    let mut out = Vec::with_capacity(data.len());
    for nal in units {
        let len = nal.len();
        if len >= max_len {
            bail!("NAL unit too large: {len}");
        }
        for shift in (0..nal_length_size).rev() {
            out.push(((len >> (shift * 8)) & 0xff) as u8);
        }
        out.extend_from_slice(nal);
    }
    Ok(out)
}

struct NalInfo {
    has_idr: bool,
    summary: String,
}

fn collect_nal_types(data: &[u8]) -> NalInfo {
    let mut types = Vec::new();
    let mut has_idr = false;
    for nal in annexb_units(data) {
        if let Some(&first) = nal.first() {
            let nal_type = first & 0x1f;
            if nal_type == 5 {
                has_idr = true;
            }
            types.push(nal_type);
        }
    }
    let summary = if types.is_empty() {
        "none".to_string()
    } else {
        types
            .iter()
            .map(|t| t.to_string())
            .collect::<Vec<String>>()
            .join(",")
    };
    NalInfo { has_idr, summary }
}

fn log_sps_pps(config: &AvccConfig) {
    let sps_info = config.sps.first().map_or_else(
        || "none".to_string(),
        |sps| {
            if sps.len() >= 4 {
                format!(
                    "profile=0x{:02X} constraints=0x{:02X} level=0x{:02X} bytes={}",
                    sps[1],
                    sps[2],
                    sps[3],
                    sps.len()
                )
            } else {
                format!("bytes={}", sps.len())
            }
        },
    );
    let pps_info = config
        .pps
        .first()
        .map_or_else(|| "none".to_string(), |pps| format!("bytes={}", pps.len()));
    log::info!(
        "RTSP SPS/PPS updated: sps_count={} pps_count={} sps0={} pps0={}",
        config.sps.len(),
        config.pps.len(),
        sps_info,
        pps_info
    );
}

fn log_avcc_bytes(config: &AvccConfig) {
    let Some(avcc) = config.to_avcc_bytes() else {
        return;
    };
    let head = format_hex_prefix(&avcc, 8);
    log::info!("RTSP avcC bytes: len={} head={}", avcc.len(), head);
}

fn format_hex_prefix(bytes: &[u8], max_len: usize) -> String {
    bytes
        .iter()
        .take(max_len)
        .map(|b| format!("{:02X}", b))
        .collect::<Vec<String>>()
        .join(" ")
}

fn find_start_code(data: &[u8], mut offset: usize) -> Option<(usize, usize)> {
    while offset + 3 <= data.len() {
        if data[offset] == 0 && data[offset + 1] == 0 {
            if data[offset + 2] == 1 {
                return Some((offset, 3));
            }
            if offset + 3 < data.len() && data[offset + 2] == 0 && data[offset + 3] == 1 {
                return Some((offset, 4));
            }
        }
        offset += 1;
    }
    None
}

fn is_annexb(data: &[u8]) -> bool {
    matches!(data.get(0..4), Some([0, 0, 0, 1])) || matches!(data.get(0..3), Some([0, 0, 1]))
}

fn has_start_code(data: &[u8]) -> bool {
    find_start_code(data, 0).is_some()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PayloadFormat {
    AnnexB,
    Avcc,
}

impl PayloadFormat {
    pub fn parse(label: &str) -> Option<Self> {
        match label {
            "annexb" => Some(Self::AnnexB),
            "avcc" => Some(Self::Avcc),
            _ => None,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::AnnexB => "annexb",
            Self::Avcc => "avc",
        }
    }
}
