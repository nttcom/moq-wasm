use std::{env, fs, path::Path};

use anyhow::{Context, Result, bail};
use bytes::BufMut;
use mediapack::{MediaEvent, mp4::Fmp4Muxer, mpegts};
use transcode::{Rendition, Transcoder, ladder_for};

#[tokio::main]
async fn main() -> Result<()> {
    let args: Vec<String> = env::args().skip(1).collect();
    let [input_path, output_prefix, rendition_args @ ..] = args.as_slice() else {
        bail!("usage: transcode <input.ts> <output-prefix> [<height>:<kbps> ...]");
    };
    let mut demuxer = mpegts::Demuxer::new();
    let mut events = demuxer.push(&fs::read(input_path).context("read input")?)?;
    events.extend(demuxer.finish()?);
    let source = events
        .iter()
        .find_map(|event| match event {
            MediaEvent::VideoConfig(config) => Some(config.sequence_parameter_set()),
            _ => None,
        })
        .context("input has no H.264 video")??;
    let renditions = if rendition_args.is_empty() {
        ladder_for(source.width, source.height)
    } else {
        rendition_args
            .iter()
            .map(|arg| parse_rendition(arg, source.width, source.height))
            .collect::<Result<Vec<_>>>()?
    };
    if renditions.is_empty() {
        bail!("no rendition below {}x{}", source.width, source.height);
    }

    let mut transcoder = Transcoder::new(&renditions)?;
    for event in &events {
        if let MediaEvent::Video(sample) = event {
            transcoder.push(sample)?;
        }
    }
    transcoder.finish()?;

    let mut muxers: Vec<Fmp4Muxer> = renditions.iter().map(|_| Fmp4Muxer::new()).collect();
    let mut outputs: Vec<bytes::BytesMut> = renditions.iter().map(|_| Default::default()).collect();
    while let Some(output) = transcoder.next().await {
        let output = output?;
        outputs[output.rendition].put_slice(&muxers[output.rendition].push(&output.event)?);
    }
    for (index, rendition) in renditions.iter().enumerate() {
        outputs[index].put_slice(&muxers[index].finish()?);
        let path = format!("{output_prefix}_{}.mp4", rendition.name);
        fs::write(Path::new(&path), &outputs[index]).with_context(|| format!("write {path}"))?;
        println!(
            "{path}: {}x{} {} kbps",
            rendition.width, rendition.height, rendition.bitrate_kbps
        );
    }
    Ok(())
}

fn parse_rendition(arg: &str, source_width: u32, source_height: u32) -> Result<Rendition> {
    let (height, bitrate) = arg
        .split_once(':')
        .with_context(|| format!("rendition {arg} must be <height>:<kbps>"))?;
    let height: u32 = height.parse().context("rendition height")?;
    Ok(Rendition {
        name: format!("{height}p"),
        width: (source_width * height / source_height).next_multiple_of(2),
        height,
        bitrate_kbps: bitrate.parse().context("rendition bitrate")?,
    })
}
