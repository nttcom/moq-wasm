use anyhow::{bail, Context as _, Result};
use tokio::io::AsyncReadExt;
use tokio::process::{Child, ChildStdout};

pub struct Mp4Box {
    pub box_type: [u8; 4],
    pub data: Vec<u8>, // box 全体（ヘッダ含む）
}

impl Mp4Box {
    pub fn type_str(&self) -> &str {
        std::str::from_utf8(&self.box_type).unwrap_or("????")
    }
}

/// stdout から fMP4 の box を 1 つ読み出す。
/// EOF に達した場合は Ok(None) を返す。
pub async fn read_box(stdout: &mut ChildStdout) -> Result<Option<Mp4Box>> {
    // 先頭 8 バイト: size (u32 BE) + type (4 bytes)
    let mut header = [0u8; 8];
    match stdout.read_exact(&mut header).await {
        Ok(_) => {}
        Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(e) => return Err(e).context("failed to read box header"),
    }

    let size = u32::from_be_bytes([header[0], header[1], header[2], header[3]]) as usize;
    let box_type: [u8; 4] = [header[4], header[5], header[6], header[7]];

    if size < 8 {
        bail!(
            "invalid box size {} for type {:?}",
            size,
            std::str::from_utf8(&box_type).unwrap_or("????")
        );
    }

    let mut data = vec![0u8; size];
    data[..8].copy_from_slice(&header);
    stdout
        .read_exact(&mut data[8..])
        .await
        .context("failed to read box body")?;

    Ok(Some(Mp4Box { box_type, data }))
}

/// ffmpeg を子プロセスで起動し、(Child, ChildStdout) を返す。
pub fn spawn_ffmpeg() -> Result<(Child, ChildStdout)> {
    let mut child = tokio::process::Command::new("ffmpeg")
        .args([
            "-f",
            "avfoundation",
            "-framerate",
            "30",
            "-video_size",
            "640x480",
            "-i",
            "0:none",
            "-c:v",
            "libx264",
            "-pix_fmt",
            "yuv420p",
            "-preset",
            "ultrafast",
            "-tune",
            "zerolatency",
            "-g",
            "30",
            "-f",
            "mp4",
            "-movflags",
            "frag_keyframe+empty_moov+default_base_moof",
            "pipe:1",
        ])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .context("failed to spawn ffmpeg")?;

    let stdout = child.stdout.take().expect("stdout must be piped");
    Ok((child, stdout))
}
