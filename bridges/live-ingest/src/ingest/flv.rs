use std::path::PathBuf;

use anyhow::{Context, Result};
use tokio::{
    fs,
    io::AsyncWriteExt,
    process::{Child, ChildStdin, Command},
};

const RECORDINGS_DIR: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/recordings");

pub struct FlvRecorder {
    child: Child,
    stdin: ChildStdin,
    header_written: bool,
}

impl FlvRecorder {
    pub async fn spawn(app: &str, stream: &str) -> Result<Self> {
        let output_path = Self::build_path(app, stream);
        if let Some(parent) = output_path.parent() {
            fs::create_dir_all(parent)
                .await
                .with_context(|| format!("create dir {:?}", parent))?;
        }

        // ffmpeg: stdin で FLV を受け取り、そのまま FLV を書き出す
        let mut cmd = Command::new("ffmpeg");
        cmd.args([
            "-hide_banner",
            "-loglevel",
            "error",
            "-y",
            "-f",
            "flv",
            "-i",
            "pipe:0",
            "-c",
            "copy",
            "-f",
            "flv",
        ])
        .arg(&output_path)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::inherit());

        let mut child = cmd.spawn().context("spawn ffmpeg for FLV output")?;
        let stdin = child.stdin.take().context("take ffmpeg stdin (piped)")?;

        println!("ffmpeg started: writing FLV to {:?}", output_path);

        Ok(Self {
            child,
            stdin,
            header_written: false,
        })
    }

    pub async fn write_audio(&mut self, timestamp_ms: u32, payload: &[u8]) -> Result<()> {
        self.write_tag(8, timestamp_ms, payload).await
    }

    pub async fn write_video(&mut self, timestamp_ms: u32, payload: &[u8]) -> Result<()> {
        self.write_tag(9, timestamp_ms, payload).await
    }

    async fn write_tag(&mut self, tag_type: u8, timestamp_ms: u32, payload: &[u8]) -> Result<()> {
        self.ensure_header().await?;

        let data_size = payload.len() as u32;

        let mut header = [0u8; 11];
        header[0] = tag_type;
        header[1] = ((data_size >> 16) & 0xFF) as u8;
        header[2] = ((data_size >> 8) & 0xFF) as u8;
        header[3] = (data_size & 0xFF) as u8;
        header[4] = ((timestamp_ms >> 16) & 0xFF) as u8;
        header[5] = ((timestamp_ms >> 8) & 0xFF) as u8;
        header[6] = (timestamp_ms & 0xFF) as u8;
        header[7] = ((timestamp_ms >> 24) & 0xFF) as u8;
        // stream_id (always 0)

        let prev_tag_size = 11 + data_size;

        self.stdin.write_all(&header).await?;
        self.stdin.write_all(payload).await?;
        self.stdin
            .write_all(&prev_tag_size.to_be_bytes())
            .await
            .context("write FLV prev tag size")?;
        self.stdin.flush().await?;

        Ok(())
    }

    async fn ensure_header(&mut self) -> Result<()> {
        if self.header_written {
            return Ok(());
        }

        // 常に audio+video フラグを立てたヘッダを出す (0b00000101)
        const HEADER: [u8; 9] = [b'F', b'L', b'V', 1, 0b0000_0101, 0, 0, 0, 9];
        self.stdin
            .write_all(&HEADER)
            .await
            .context("write FLV header")?;
        self.stdin
            .write_all(&0u32.to_be_bytes())
            .await
            .context("write FLV initial prev tag")?;
        self.header_written = true;
        Ok(())
    }

    fn build_path(app: &str, stream: &str) -> PathBuf {
        let filename = format!("{app}_{stream}.flv");
        PathBuf::from(RECORDINGS_DIR).join(filename)
    }
}

impl Drop for FlvRecorder {
    fn drop(&mut self) {
        // best-effort: terminate ffmpeg if still running.
        let _ = self.child.start_kill();
    }
}
