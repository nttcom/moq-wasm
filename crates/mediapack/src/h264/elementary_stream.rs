use anyhow::{Result, bail};
use bytes::{Bytes, BytesMut};

use crate::{
    h264::{
        ParameterSetTracker,
        annexb::{nal_units, with_start_codes},
    },
    sample::{MediaEvent, Timestamp},
};

/// A stream that shows no start code within this many bytes is not Annex-B.
const MAX_PENDING_BYTES: usize = 16 * 1024 * 1024;

/// Reads a raw H.264 Annex-B elementary stream in chunks of any size and
/// reports its access units as `MediaEvent`s. The stream carries no timestamps, so each access unit takes
/// the `arrival` of the chunk that completed it, moved 1 µs past the previous
/// access unit when one chunk completes several; a keyframe without its
/// SPS/PPS gets the last ones seen.
#[derive(Default)]
pub struct ElementaryStreamDemuxer {
    /// Bytes from the last start code on; the NAL unit there may continue in
    /// the next chunk.
    pending: BytesMut,
    access_unit: Vec<Bytes>,
    access_unit_has_slice: bool,
    parameter_sets: ParameterSetTracker,
    last_pts: Option<Timestamp>,
}

impl ElementaryStreamDemuxer {
    pub fn new() -> Self {
        Self::default()
    }

    /// Fails when 16 MiB arrive without a start code.
    pub fn push(&mut self, data: &[u8], arrival: Timestamp) -> Result<Vec<MediaEvent>> {
        self.pending.extend_from_slice(data);
        let complete = match last_start_code(&self.pending) {
            Some(position) => self.pending.split_to(position).freeze(),
            None => Bytes::new(),
        };
        if self.pending.len() > MAX_PENDING_BYTES {
            bail!("no Annex-B start code within {MAX_PENDING_BYTES} bytes");
        }
        let mut events = Vec::new();
        for nal in nal_units(&complete) {
            self.push_nal(complete.slice_ref(nal), arrival, &mut events)?;
        }
        // The header of the next NAL unit already tells whether the access
        // unit has ended, so it is reported without waiting for the rest.
        if let Some(next) = nal_units(&self.pending).next()
            && next.len() >= 2
            && self.access_unit_has_slice
            && starts_access_unit(next)
        {
            self.finish_access_unit(arrival, &mut events)?;
        }
        Ok(events)
    }

    /// Reports the last access unit, which no further start code completes.
    pub fn finish(&mut self, arrival: Timestamp) -> Result<Vec<MediaEvent>> {
        let rest = std::mem::take(&mut self.pending).freeze();
        let mut events = Vec::new();
        for nal in nal_units(&rest) {
            self.push_nal(rest.slice_ref(nal), arrival, &mut events)?;
        }
        self.finish_access_unit(arrival, &mut events)?;
        Ok(events)
    }

    fn push_nal(
        &mut self,
        nal: Bytes,
        arrival: Timestamp,
        events: &mut Vec<MediaEvent>,
    ) -> Result<()> {
        if self.access_unit_has_slice && starts_access_unit(&nal) {
            self.finish_access_unit(arrival, events)?;
        }
        self.access_unit_has_slice |= is_slice(&nal);
        self.access_unit.push(nal);
        Ok(())
    }

    /// Parameter sets and SEI that no slice follows are not an access unit
    /// and are dropped.
    fn finish_access_unit(
        &mut self,
        arrival: Timestamp,
        events: &mut Vec<MediaEvent>,
    ) -> Result<()> {
        let nals = std::mem::take(&mut self.access_unit);
        if !std::mem::take(&mut self.access_unit_has_slice) {
            return Ok(());
        }
        let annexb = with_start_codes(nals.iter().map(Bytes::as_ref));
        if let Some(unit) = self.parameter_sets.track(&annexb)? {
            let pts = self.last_pts.map_or(arrival, |last| {
                arrival.max(last.saturating_add(Timestamp::from_micros(1)))
            });
            self.last_pts = Some(pts);
            events.extend(unit.into_events(pts, pts));
        }
        Ok(())
    }
}

/// Where the last `00 00 01` starts, or the `00 00 00 01` around it.
fn last_start_code(data: &[u8]) -> Option<usize> {
    let position = data.windows(3).rposition(|window| window == [0, 0, 1])?;
    Some(if position > 0 && data[position - 1] == 0 {
        position - 1
    } else {
        position
    })
}

fn nal_type(nal: &[u8]) -> u8 {
    nal.first().map_or(0, |header| header & 0x1F)
}

/// A coded slice (H.264 Table 7-1, types 1 to 5).
fn is_slice(nal: &[u8]) -> bool {
    (1..=5).contains(&nal_type(nal))
}

/// Whether `nal` starts a new access unit (H.264 §7.4.1.2.3): an access
/// unit delimiter, SEI, SPS, PPS or types 14 to 18, or the first slice of
/// a picture. A first slice has first_mb_in_slice 0, coded as Exp-Golomb
/// `1`, so the top bit of the byte after the NAL header is set.
fn starts_access_unit(nal: &[u8]) -> bool {
    if is_slice(nal) {
        return nal.get(1).is_some_and(|byte| byte & 0x80 != 0);
    }
    matches!(nal_type(nal), 6..=9 | 14..=18)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        h264::annexb::with_start_codes,
        sample::{VideoConfig, VideoSample},
        test_support::{
            FIXTURE_PPS, FIXTURE_SPS, IDR_SLICE, NON_IDR_SLICE, delta_frame_annexb, fixture_record,
            keyframe_annexb,
        },
    };

    /// SPS, PPS and an IDR slice, then two P slices.
    fn stream() -> Bytes {
        with_start_codes([
            &FIXTURE_SPS[..],
            &FIXTURE_PPS,
            &IDR_SLICE,
            &NON_IDR_SLICE,
            &NON_IDR_SLICE,
        ])
    }

    fn samples(events: &[MediaEvent]) -> Vec<&VideoSample> {
        events
            .iter()
            .filter_map(|event| match event {
                MediaEvent::Video(sample) => Some(sample),
                _ => None,
            })
            .collect()
    }

    fn demux_in_chunks(stream: &[u8], chunk_size: usize) -> Vec<MediaEvent> {
        let mut demuxer = ElementaryStreamDemuxer::new();
        let mut events = Vec::new();
        for chunk in stream.chunks(chunk_size) {
            events.extend(demuxer.push(chunk, Timestamp::ZERO).unwrap());
        }
        events.extend(demuxer.finish(Timestamp::ZERO).unwrap());
        events
    }

    #[test]
    fn splits_a_stream_into_access_units_after_its_config() {
        // Arrange
        let mut demuxer = ElementaryStreamDemuxer::new();

        // Act
        let mut events = demuxer.push(&stream(), Timestamp::ZERO).unwrap();
        events.extend(demuxer.finish(Timestamp::ZERO).unwrap());

        // Assert
        assert_eq!(
            events[0],
            MediaEvent::VideoConfig(VideoConfig::Avc(fixture_record()))
        );
        let samples = samples(&events);
        assert_eq!(samples.len(), 3);
        assert_eq!(samples[0].data, keyframe_annexb());
        assert!(samples[0].is_keyframe);
        assert_eq!(samples[1].data, delta_frame_annexb());
        assert!(!samples[1].is_keyframe);
    }

    #[test]
    fn chunk_boundaries_do_not_change_the_access_units() {
        // Arrange
        let whole = demux_in_chunks(&stream(), usize::MAX);

        // Act
        let byte_by_byte = demux_in_chunks(&stream(), 1);

        // Assert
        assert_eq!(byte_by_byte, whole);
    }

    #[test]
    fn an_access_unit_takes_the_arrival_of_the_chunk_that_completes_it() {
        // Arrange
        let mut demuxer = ElementaryStreamDemuxer::new();
        demuxer
            .push(&keyframe_annexb(), Timestamp::from_millis(0))
            .unwrap();

        // Act
        let events = demuxer
            .push(&delta_frame_annexb(), Timestamp::from_millis(33))
            .unwrap();

        // Assert
        let samples = samples(&events);
        assert_eq!(samples.len(), 1);
        assert!(samples[0].is_keyframe);
        assert_eq!(samples[0].pts, Timestamp::from_millis(33));
    }

    #[test]
    fn access_units_completed_by_one_chunk_get_increasing_timestamps() {
        // Arrange: a producer faster than real time hands over two keyframes in one read
        let keyframes = [keyframe_annexb(), keyframe_annexb(), delta_frame_annexb()].concat();
        let mut demuxer = ElementaryStreamDemuxer::new();

        // Act
        let events = demuxer.push(&keyframes, Timestamp::from_millis(5)).unwrap();

        // Assert: a group is keyed by its keyframe's timestamp, so two keyframes must not share one
        let pts: Vec<_> = samples(&events).iter().map(|sample| sample.pts).collect();
        assert_eq!(
            pts,
            [Timestamp::from_millis(5), Timestamp::from_micros(5_001)]
        );
    }

    #[test]
    fn groups_the_slices_of_one_picture_into_one_access_unit() {
        // Arrange: the second IDR slice continues the picture (first_mb_in_slice != 0)
        const CONTINUING_IDR_SLICE: [u8; 3] = [0x65, 0x40, 0x84];
        let stream = with_start_codes([
            &FIXTURE_SPS[..],
            &FIXTURE_PPS,
            &IDR_SLICE,
            &CONTINUING_IDR_SLICE,
            &NON_IDR_SLICE,
        ]);

        // Act
        let events = demux_in_chunks(&stream, usize::MAX);

        // Assert
        let samples = samples(&events);
        assert_eq!(samples.len(), 2);
        assert_eq!(
            samples[0].data,
            with_start_codes([
                &FIXTURE_SPS[..],
                &FIXTURE_PPS,
                &IDR_SLICE,
                &CONTINUING_IDR_SLICE
            ])
        );
    }

    #[test]
    fn gives_a_bare_keyframe_the_parameter_sets_seen_before() {
        // Arrange
        let stream = with_start_codes([&FIXTURE_SPS[..], &FIXTURE_PPS, &IDR_SLICE, &IDR_SLICE]);

        // Act
        let events = demux_in_chunks(&stream, usize::MAX);

        // Assert
        let samples = samples(&events);
        assert_eq!(samples.len(), 2);
        assert_eq!(samples[1].data, keyframe_annexb());
    }

    #[test]
    fn rejects_data_without_a_start_code() {
        // Arrange
        let mut demuxer = ElementaryStreamDemuxer::new();
        demuxer.push(&keyframe_annexb(), Timestamp::ZERO).unwrap();
        let garbage = vec![0xFF; MAX_PENDING_BYTES + 1];

        // Act
        let result = demuxer.push(&garbage, Timestamp::ZERO);

        // Assert
        assert!(result.is_err());
    }
}
