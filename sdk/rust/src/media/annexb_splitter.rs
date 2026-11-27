use anyhow::{Result, bail};
use bytes::Bytes;
use mediapack::{
    MediaEvent, Timestamp, elementary_stream::ElementaryStreamDemuxer,
    h264::AvcDecoderConfigurationRecord, hevc::HevcDecoderConfigurationRecord, loc::VideoCodec,
};

/// Splits an H.264 or H.265 Annex B stream, read in chunks of any size, into
/// the access units `MediaSource::push_video` takes. A keyframe without its
/// parameter sets gets the last ones seen.
pub struct AnnexBSplitter(Demuxer);

enum Demuxer {
    H264(ElementaryStreamDemuxer<AvcDecoderConfigurationRecord>),
    H265(ElementaryStreamDemuxer<HevcDecoderConfigurationRecord>),
}

impl AnnexBSplitter {
    /// `codec` is a WebCodecs codec string as `MediaSource::configure_video`
    /// takes: `avc1.*` or `avc3.*`, `hvc1.*` or `hev1.*`. Fails for any other
    /// codec.
    pub fn new(codec: &str) -> Result<Self> {
        let demuxer = match VideoCodec::from_codec_string(codec) {
            Some(VideoCodec::H264) => Demuxer::H264(ElementaryStreamDemuxer::new()),
            Some(VideoCodec::H265) => Demuxer::H265(ElementaryStreamDemuxer::new()),
            _ => bail!("not an Annex B codec: {codec}"),
        };
        Ok(Self(demuxer))
    }

    /// The access units `chunk` completes. Fails when 16 MiB arrive without a
    /// start code, or on a NAL unit of another codec.
    pub fn push(&mut self, chunk: &[u8]) -> Result<Vec<Bytes>> {
        // The stream has no timestamps; the caller times each access unit.
        let events = match &mut self.0 {
            Demuxer::H264(demuxer) => demuxer.push(chunk, Timestamp::ZERO)?,
            Demuxer::H265(demuxer) => demuxer.push(chunk, Timestamp::ZERO)?,
        };
        Ok(access_units(events))
    }

    /// The last access unit, which no further start code completes; call at
    /// the end of the stream.
    pub fn finish(&mut self) -> Result<Vec<Bytes>> {
        let events = match &mut self.0 {
            Demuxer::H264(demuxer) => demuxer.finish(Timestamp::ZERO)?,
            Demuxer::H265(demuxer) => demuxer.finish(Timestamp::ZERO)?,
        };
        Ok(access_units(events))
    }
}

fn access_units(events: Vec<MediaEvent>) -> Vec<Bytes> {
    events
        .into_iter()
        .filter_map(|event| match event {
            MediaEvent::Video(sample) => Some(sample.data),
            _ => None,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const SPS: &[u8] = &[
        0, 0, 0, 1, 0x67, 0x42, 0xd0, 0x0b, 0xda, 0x0a, 0x37, 0xe4, 0xc0, 0x44, 0x00, 0x00, 0x03,
        0x00, 0x04, 0x00, 0x00, 0x03, 0x00, 0x78, 0x3c, 0x48, 0x9a, 0x80,
    ];
    const PPS: &[u8] = &[0, 0, 0, 1, 0x68, 0xce, 0x3c, 0x80];
    const IDR: &[u8] = &[0, 0, 0, 1, 0x65, 0x88, 0x84];
    const P: &[u8] = &[0, 0, 0, 1, 0x41, 0x9a, 0x22];

    fn split_in_chunks(stream: &[u8], chunk_size: usize) -> Vec<Bytes> {
        let mut splitter = AnnexBSplitter::new("avc1.42d00b").unwrap();
        let mut units = Vec::new();
        for chunk in stream.chunks(chunk_size) {
            units.extend(splitter.push(chunk).unwrap());
        }
        units.extend(splitter.finish().unwrap());
        units
    }

    #[test]
    fn each_access_unit_comes_out_whole_however_the_stream_is_chunked() {
        // Arrange
        let stream = [SPS, PPS, IDR, P, P].concat();
        let expected = vec![[SPS, PPS, IDR].concat(), P.to_vec(), P.to_vec()];

        // Act
        let whole = split_in_chunks(&stream, stream.len());
        let bytewise = split_in_chunks(&stream, 1);

        // Assert
        assert_eq!(whole, expected);
        assert_eq!(bytewise, expected);
    }

    #[test]
    fn a_unit_is_held_until_the_next_one_starts() {
        // Arrange
        let mut splitter = AnnexBSplitter::new("avc1.42d00b").unwrap();

        // Act
        let first = splitter.push(&[SPS, PPS, IDR].concat()).unwrap();
        let second = splitter.push(P).unwrap();

        // Assert
        assert!(first.is_empty());
        assert_eq!(second, vec![[SPS, PPS, IDR].concat()]);
    }

    #[test]
    fn a_codec_without_annex_b_is_refused() {
        // Act / Assert
        assert!(AnnexBSplitter::new("vp8").is_err());
        assert!(AnnexBSplitter::new("hvc1.1.6.L93.B0").is_ok());
    }

    #[test]
    fn a_stream_of_another_codec_fails() {
        // Arrange: an H.264 SPS read as H.265 is a NAL unit type H.265 reserves
        let mut splitter = AnnexBSplitter::new("hvc1.1.6.L93.B0").unwrap();

        // Act
        let result = splitter
            .push(&[SPS, PPS, IDR, P].concat())
            .and_then(|_| splitter.finish());

        // Assert
        assert!(result.is_err());
    }
}
