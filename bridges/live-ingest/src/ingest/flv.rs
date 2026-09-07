use std::path::PathBuf;

use anyhow::{Context, Result};
use mediapack::flv::{Muxer, Tag};
use tokio::{
    fs::{self, File},
    io::AsyncWriteExt,
};

const RECORDINGS_DIR: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/recordings");

pub struct FlvRecorder {
    file: File,
    muxer: Muxer,
}

impl FlvRecorder {
    pub async fn open(app: &str, stream: &str) -> Result<Self> {
        let output_path = build_path(app, stream);
        fs::create_dir_all(RECORDINGS_DIR)
            .await
            .context("create recordings directory")?;
        let file = File::create(&output_path)
            .await
            .with_context(|| format!("create {}", output_path.display()))?;
        tracing::info!(path = %output_path.display(), "FLV recording started");
        Ok(Self {
            file,
            muxer: Muxer::new(),
        })
    }

    pub async fn write_tag(&mut self, tag: &Tag) -> Result<()> {
        self.file
            .write_all(&self.muxer.push_tag(tag))
            .await
            .context("write FLV tag")
    }
}

fn build_path(app: &str, stream: &str) -> PathBuf {
    PathBuf::from(RECORDINGS_DIR).join(format!("{app}_{stream}.flv"))
}
