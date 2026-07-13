use anyhow::{Result, bail};
use bytes::{Bytes, BytesMut};
use std::time::Instant;

use super::{Frame, Timestamp};

const MAX_BUFFERED_BYTES: usize = 16 * 1024 * 1024;

/// Assembles an Annex-B H.264 byte stream into a sequence of [`Frame`]s.
pub struct AnnexBFramer {
    codec_base: String,
    tail: BytesMut,      // bytes after the last start code (a possibly incomplete NAL)
    current: AccessUnit, // access unit being assembled
    sps: Vec<Bytes>,     // most recently seen SPS, for injection into bare keyframes
    pps: Vec<Bytes>,     // most recently seen PPS
    zero: Option<Instant>, // wall-clock origin for synthesized timestamps
    pending: Vec<Frame>, // frames finished during the current feed/flush
}

/// An H.264 access unit under construction.
#[derive(Default)]
struct AccessUnit {
    chunks: BytesMut,     // NAL units accumulated so far (with start codes)
    contains_idr: bool,   // holds an IDR slice -> keyframe
    contains_slice: bool, // holds a coded slice (rejects param/SEI-only units)
    sps_seen: Vec<Bytes>, // SPS carried inline in this unit (skip re-injection)
    pps_seen: Vec<Bytes>, // PPS carried inline in this unit
}

impl AnnexBFramer {
    pub fn new(codec_base: String) -> Self {
        Self {
            codec_base,
            tail: BytesMut::new(),
            current: AccessUnit::default(),
            sps: Vec::new(),
            pps: Vec::new(),
            zero: None,
            pending: Vec::new(),
        }
    }

    pub fn feed(&mut self, data: &[u8]) -> Result<Vec<Frame>> {
        self.tail.extend_from_slice(data);
        let codes = start_code_positions(&self.tail);

        let complete: Vec<Bytes> = (0..codes.len().saturating_sub(1))
            .map(|i| Bytes::copy_from_slice(&self.tail[codes[i].1..codes[i + 1].0]))
            .collect();
        let drain_to = codes.last().map(|(code_start, _)| *code_start).unwrap_or(0);
        let _ = self.tail.split_to(drain_to);
        if self.tail.len() > MAX_BUFFERED_BYTES {
            bail!("no NAL start code within {MAX_BUFFERED_BYTES} bytes; input is not annex-b?");
        }

        for nal in &complete {
            self.classify_nal(nal)?;
        }
        Ok(std::mem::take(&mut self.pending))
    }

    pub fn flush(&mut self) -> Result<Vec<Frame>> {
        if let Some((_, body_start)) = start_code_positions(&self.tail).first().copied() {
            let nal = Bytes::copy_from_slice(&self.tail[body_start..]);
            self.classify_nal(&nal)?;
        }
        self.tail.clear();
        self.maybe_finish_frame()?;
        Ok(std::mem::take(&mut self.pending))
    }

    /// The full codec string (e.g. `avc3.640028`), known once an SPS is seen.
    pub fn codec_string(&self) -> Option<String> {
        let sps = self.sps.first()?;
        build_codec_string(&self.codec_base, sps).ok()
    }

    fn pts(&mut self) -> Timestamp {
        let now = Instant::now();
        let zero = *self.zero.get_or_insert(now);
        Timestamp(now.saturating_duration_since(zero).as_micros() as i64)
    }

    fn classify_nal(&mut self, nal: &[u8]) -> Result<()> {
        if self.current.contains_slice && starts_access_unit(nal) {
            self.maybe_finish_frame()?;
        }
        self.current.chunks.extend_from_slice(&[0, 0, 0, 1]);
        self.current.chunks.extend_from_slice(nal);

        let nal_type = nal_type(nal);
        if is_vcl(nal_type) {
            self.current.contains_slice = true;
        }
        match nal_type {
            // TODO: open-GOP/GDR (broadcast, MPEG-TS) has no IDR so late-join breaks. Handle recovery-point SEI then.
            5 => self.current.contains_idr = true,
            7 => {
                let sps = Bytes::copy_from_slice(nal);
                self.current.sps_seen.push(sps.clone());
                self.sps = vec![sps];
            }
            8 => {
                let pps = Bytes::copy_from_slice(nal);
                self.current.pps_seen.push(pps.clone());
                self.pps = vec![pps];
            }
            _ => {}
        }
        Ok(())
    }

    fn maybe_finish_frame(&mut self) -> Result<()> {
        if !self.current.contains_slice {
            self.current = AccessUnit::default();
            return Ok(());
        }
        let au = std::mem::take(&mut self.current);
        let payload = self.ensure_params(&au);
        let timestamp = self.pts();
        self.pending.push(Frame {
            keyframe: au.contains_idr,
            timestamp,
            payload,
        });
        Ok(())
    }

    /// Prepend cached SPS/PPS to a keyframe that omits them, so it is self-contained.
    fn ensure_params(&self, au: &AccessUnit) -> Bytes {
        let mut out = BytesMut::new();
        if au.contains_idr {
            if au.sps_seen.is_empty() {
                for sps in &self.sps {
                    out.extend_from_slice(&[0, 0, 0, 1]);
                    out.extend_from_slice(sps);
                }
            }
            if au.pps_seen.is_empty() {
                for pps in &self.pps {
                    out.extend_from_slice(&[0, 0, 0, 1]);
                    out.extend_from_slice(pps);
                }
            }
        }
        out.extend_from_slice(&au.chunks);
        out.freeze()
    }
}

/// Returns `(start_code_start, body_start)` for every Annex-B start code.
fn start_code_positions(data: &[u8]) -> Vec<(usize, usize)> {
    let mut positions = Vec::new();
    let mut i = 0;
    while i + 3 <= data.len() {
        if data[i] == 0 && data[i + 1] == 0 && data[i + 2] == 1 {
            let code_start = if i > 0 && data[i - 1] == 0 { i - 1 } else { i };
            positions.push((code_start, i + 3));
            i += 3;
        } else {
            i += 1;
        }
    }
    positions
}

fn build_codec_string(base: &str, sps: &[u8]) -> Result<String> {
    if nal_type(sps) != 7 {
        bail!("expected SPS NAL (type 7), got type {}", nal_type(sps));
    }
    if sps.len() < 4 {
        bail!("SPS NAL too short: {} bytes", sps.len());
    }
    Ok(format!("{base}.{:02x}{:02x}{:02x}", sps[1], sps[2], sps[3]))
}

fn nal_type(nal: &[u8]) -> u8 {
    nal.first().map(|byte| byte & 0x1F).unwrap_or(0)
}

fn is_vcl(nal_type: u8) -> bool {
    (1..=5).contains(&nal_type)
}

fn starts_access_unit(nal: &[u8]) -> bool {
    let nal_type = nal_type(nal);
    if is_vcl(nal_type) {
        // first_mb_in_slice == 0 (a new picture's first slice) is Exp-Golomb `1`,
        // i.e. the top bit of the slice header byte is set.
        return nal.get(1).is_some_and(|byte| byte & 0x80 != 0);
    }
    matches!(nal_type, 6..=9)
}

#[cfg(test)]
mod tests {
    use super::*;

    const STREAM: [u8; 33] = [
        0, 0, 0, 1, 0x67, 0x42, // SPS
        0, 0, 0, 1, 0x68, 0xEE, // PPS
        0, 0, 0, 1, 0x65, 0x88, 0x11, // IDR slice (first_mb=0)
        0, 0, 0, 1, 0x41, 0x88, 0x22, // P slice
        0, 0, 0, 1, 0x41, 0x88, 0x33, // P slice
    ];

    fn framer() -> AnnexBFramer {
        AnnexBFramer::new("avc3".to_string())
    }

    #[test]
    fn framer_splits_stream_into_frames() {
        let mut framer = framer();

        let mut frames = framer.feed(&STREAM).unwrap();
        frames.extend(framer.flush().unwrap());

        assert_eq!(frames.len(), 3);
        assert!(frames[0].keyframe);
        assert!(!frames[1].keyframe);
        assert_eq!(
            frames[0].payload.as_ref(),
            &[
                0, 0, 0, 1, 0x67, 0x42, 0, 0, 0, 1, 0x68, 0xEE, 0, 0, 0, 1, 0x65, 0x88, 0x11
            ]
        );
        assert_eq!(frames[1].payload.as_ref(), &[0, 0, 0, 1, 0x41, 0x88, 0x22]);
    }

    #[test]
    fn framer_is_independent_of_chunk_boundaries() {
        let mut framer = framer();
        let mut frames = Vec::new();

        for byte in STREAM.iter() {
            frames.extend(framer.feed(&[*byte]).unwrap());
        }
        frames.extend(framer.flush().unwrap());

        assert_eq!(frames.len(), 3);
        assert!(frames[0].keyframe);
        assert!(!frames[2].keyframe);
    }

    #[test]
    fn framer_groups_multi_slice_picture_into_one_frame() {
        // A keyframe as two IDR slices, then a P frame as one slice.
        let stream = [
            0, 0, 0, 1, 0x67, 0x42, // SPS
            0, 0, 0, 1, 0x68, 0xEE, // PPS
            0, 0, 0, 1, 0x65, 0x88, 0x11, // IDR slice 0 (first_mb=0)
            0, 0, 0, 1, 0x65, 0x10, 0x22, // IDR slice 1 (continuation)
            0, 0, 0, 1, 0x41, 0x88, 0x33, // P slice 0 (first_mb=0)
        ];
        let mut framer = framer();

        let mut frames = framer.feed(&stream).unwrap();
        frames.extend(framer.flush().unwrap());

        assert_eq!(frames.len(), 2);
        assert!(frames[0].keyframe); // SPS+PPS+IDR0+IDR1 = one frame
        assert!(!frames[1].keyframe);
    }

    #[test]
    fn injects_cached_sps_pps_into_keyframe_that_omits_them() {
        let stream = [
            0, 0, 0, 1, 0x67, 0x42, // SPS
            0, 0, 0, 1, 0x68, 0xEE, // PPS
            0, 0, 0, 1, 0x65, 0x88, 0x11, // IDR slice (first_mb=0), GOP1 keyframe
            0, 0, 0, 1, 0x41, 0x88, 0x22, // P slice
            0, 0, 0, 1, 0x65, 0x90, 0x33, // IDR slice, GOP2 keyframe WITHOUT SPS/PPS
            0, 0, 0, 1, 0x41, 0x88, 0x44, // P slice
        ];
        let mut framer = framer();

        let mut frames = framer.feed(&stream).unwrap();
        frames.extend(framer.flush().unwrap());

        assert_eq!(frames.len(), 4);
        assert!(frames[0].keyframe);
        assert!(frames[2].keyframe);
        assert_eq!(
            frames[0].payload.as_ref(),
            &[
                0, 0, 0, 1, 0x67, 0x42, 0, 0, 0, 1, 0x68, 0xEE, 0, 0, 0, 1, 0x65, 0x88, 0x11
            ]
        );
        assert_eq!(
            frames[2].payload.as_ref(),
            &[
                0, 0, 0, 1, 0x67, 0x42, 0, 0, 0, 1, 0x68, 0xEE, 0, 0, 0, 1, 0x65, 0x90, 0x33
            ]
        );
    }

    #[test]
    fn timestamps_are_monotonic_from_zero() {
        let mut framer = framer();

        let mut frames = framer.feed(&STREAM).unwrap();
        frames.extend(framer.flush().unwrap());

        assert_eq!(frames[0].timestamp.0, 0);
        assert!(frames[1].timestamp.0 >= frames[0].timestamp.0);
        assert!(frames[2].timestamp.0 >= frames[1].timestamp.0);
    }

    #[test]
    fn codec_string_resolves_after_keyframe() {
        let stream = [
            0, 0, 0, 1, 0x67, 0x64, 0x00, 0x28, // SPS (type 7)
            0, 0, 0, 1, 0x68, 0xEE, // PPS
            0, 0, 0, 1, 0x65, 0x88, 0x11, // IDR slice
            0, 0, 0, 1, 0x41, 0x88, 0x22, // P slice, forces the keyframe to finish
        ];
        let mut framer = framer();

        assert!(framer.codec_string().is_none());
        framer.feed(&stream).unwrap();

        assert_eq!(framer.codec_string().as_deref(), Some("avc3.640028"));
    }

    #[test]
    fn build_codec_string_from_sps() {
        let sps = [0x67u8, 0x64, 0x00, 0x28, 0xFF, 0xFF];

        assert_eq!(build_codec_string("avc3", &sps).unwrap(), "avc3.640028");
    }

    #[test]
    fn build_codec_string_rejects_non_sps_nal() {
        let not_sps = [0x65u8, 0x64, 0x00, 0x28]; // type 5, not SPS

        assert!(build_codec_string("avc3", &not_sps).is_err());
    }

    #[test]
    fn splits_on_4byte_and_3byte_start_codes() {
        let data = [
            0, 0, 0, 1, 0x67, 0xAA, // 4-byte start code
            0, 0, 1, 0x65, 0xBB, 0xCC, // 3-byte start code
        ];

        let codes = start_code_positions(&data);

        assert_eq!(codes.len(), 2);
        assert_eq!(codes[0], (0, 4));
        assert_eq!(codes[1], (6, 9));
    }
}
