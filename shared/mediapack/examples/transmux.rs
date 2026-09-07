use std::{env, fs::File, io::BufWriter};

use anyhow::{Context, Result, bail};
use mediapack::{InputFormat, OutputFormat, transmux};

fn main() -> Result<()> {
    let args: Vec<String> = env::args().skip(1).collect();
    let [input_format, output_format, input_path, output_path] = args.as_slice() else {
        bail!("usage: transmux <mpegts|flv> <flv|fmp4> <input> <output>");
    };
    let input = match input_format.as_str() {
        "mpegts" => InputFormat::MpegTs,
        "flv" => InputFormat::Flv,
        other => bail!("unsupported input format {other}"),
    };
    let output = match output_format.as_str() {
        "flv" => OutputFormat::Flv,
        "fmp4" => OutputFormat::Fmp4,
        other => bail!("unsupported output format {other}"),
    };
    let mut reader = File::open(input_path).with_context(|| format!("open {input_path}"))?;
    let mut writer =
        BufWriter::new(File::create(output_path).with_context(|| format!("create {output_path}"))?);
    transmux(input, output, &mut reader, &mut writer)
}
