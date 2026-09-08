use std::io::{Read, Write};

use anyhow::{Context, Result};
use bytes::{BufMut, Bytes, BytesMut};

use crate::{flv, mp4, mpegts, sample::MediaEvent};

const READ_CHUNK_SIZE: usize = 64 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputFormat {
    MpegTs,
    Flv,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutputFormat {
    Flv,
    Fmp4,
}

pub struct Transmuxer {
    demuxer: InputDemuxer,
    muxer: OutputMuxer,
}

enum InputDemuxer {
    MpegTs(mpegts::Demuxer),
    Flv(flv::Demuxer),
}

enum OutputMuxer {
    Flv(flv::Muxer),
    Fmp4(mp4::Fmp4Muxer),
}

impl Transmuxer {
    pub fn new(input: InputFormat, output: OutputFormat) -> Self {
        Self {
            demuxer: match input {
                InputFormat::MpegTs => InputDemuxer::MpegTs(mpegts::Demuxer::new()),
                InputFormat::Flv => InputDemuxer::Flv(flv::Demuxer::new()),
            },
            muxer: match output {
                OutputFormat::Flv => OutputMuxer::Flv(flv::Muxer::new()),
                OutputFormat::Fmp4 => OutputMuxer::Fmp4(mp4::Fmp4Muxer::new()),
            },
        }
    }

    pub fn push(&mut self, data: &[u8]) -> Result<Bytes> {
        let events = match &mut self.demuxer {
            InputDemuxer::MpegTs(demuxer) => demuxer.push(data)?,
            InputDemuxer::Flv(demuxer) => demuxer.push(data)?,
        };
        self.mux(&events)
    }

    pub fn finish(&mut self) -> Result<Bytes> {
        let events = match &mut self.demuxer {
            InputDemuxer::MpegTs(demuxer) => demuxer.finish()?,
            InputDemuxer::Flv(_) => Vec::new(),
        };
        let mut out = BytesMut::from(self.mux(&events)?.as_ref());
        if let OutputMuxer::Fmp4(muxer) = &mut self.muxer {
            out.put_slice(&muxer.finish()?);
        }
        Ok(out.freeze())
    }

    fn mux(&mut self, events: &[MediaEvent]) -> Result<Bytes> {
        let mut out = BytesMut::new();
        for event in events {
            let bytes = match &mut self.muxer {
                OutputMuxer::Flv(muxer) => muxer.push(event)?,
                OutputMuxer::Fmp4(muxer) => muxer.push(event)?,
            };
            out.put_slice(&bytes);
        }
        Ok(out.freeze())
    }
}

pub fn transmux(
    input: InputFormat,
    output: OutputFormat,
    reader: &mut impl Read,
    writer: &mut impl Write,
) -> Result<()> {
    let mut transmuxer = Transmuxer::new(input, output);
    let mut chunk = vec![0_u8; READ_CHUNK_SIZE];
    loop {
        let read = reader.read(&mut chunk).context("read transmux input")?;
        if read == 0 {
            break;
        }
        writer
            .write_all(&transmuxer.push(&chunk[..read])?)
            .context("write transmux output")?;
    }
    writer
        .write_all(&transmuxer.finish()?)
        .context("write transmux output")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use super::*;
    use crate::{
        mp4::muxer::tests::boxes,
        test_support::{FIXTURE_FLV, FIXTURE_TS, audio_samples, video_samples},
    };

    #[test]
    fn transmuxes_mpegts_to_flv_preserving_samples() {
        // Arrange
        let mut output = Vec::new();

        // Act
        transmux(
            InputFormat::MpegTs,
            OutputFormat::Flv,
            &mut Cursor::new(FIXTURE_TS),
            &mut output,
        )
        .unwrap();
        let events = flv::Demuxer::new().push(&output).unwrap();

        // Assert
        assert!(output.starts_with(b"FLV"));
        assert_eq!(video_samples(&events).len(), 9);
        assert_eq!(audio_samples(&events).len(), 30);
    }

    #[test]
    fn transmuxes_mpegts_to_fragmented_mp4() {
        // Arrange
        let mut transmuxer = Transmuxer::new(InputFormat::MpegTs, OutputFormat::Fmp4);
        let mut output = BytesMut::new();

        // Act
        for chunk in FIXTURE_TS.chunks(4_000) {
            output.put_slice(&transmuxer.push(chunk).unwrap());
        }
        output.put_slice(&transmuxer.finish().unwrap());

        // Assert
        let kinds: Vec<String> = boxes(&output).into_iter().map(|(kind, _)| kind).collect();
        assert_eq!(&kinds[..2], ["ftyp", "moov"]);
        assert_eq!(kinds.iter().filter(|kind| *kind == "moof").count(), 39);
        assert_eq!(kinds.iter().filter(|kind| *kind == "mdat").count(), 39);
    }

    #[test]
    fn transmuxes_flv_to_fragmented_mp4() {
        // Arrange
        let mut output = Vec::new();

        // Act
        transmux(
            InputFormat::Flv,
            OutputFormat::Fmp4,
            &mut Cursor::new(FIXTURE_FLV),
            &mut output,
        )
        .unwrap();

        // Assert
        let kinds: Vec<String> = boxes(&output).into_iter().map(|(kind, _)| kind).collect();
        assert_eq!(kinds.iter().filter(|kind| *kind == "moof").count(), 39);
    }
}
