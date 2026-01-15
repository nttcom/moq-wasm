use crate::config::Target;
use anyhow::{bail, Context, Result};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpStream;
use tokio::time::timeout;

#[derive(Debug)]
pub struct RtspProbeResult {
    pub endpoint: String,
    pub status_line: String,
    pub status_code: Option<u16>,
    pub www_authenticate: Option<String>,
}

pub async fn probe(target: &Target) -> Result<RtspProbeResult> {
    let addr = target.rtsp_socket_addr();
    let mut stream = timeout(target.timeout(), TcpStream::connect(&addr))
        .await
        .context("rtsp connect timeout")??;

    let request = format!(
        "OPTIONS {} RTSP/1.0\r\nCSeq: 1\r\nUser-Agent: moqt-client-onvif\r\n\r\n",
        target.rtsp_endpoint()
    );
    timeout(target.timeout(), stream.write_all(request.as_bytes()))
        .await
        .context("rtsp write timeout")??;

    let mut reader = BufReader::new(stream);
    let status_line = read_line(&mut reader, target.timeout())
        .await?
        .context("rtsp no response")?;
    let status_line = status_line.trim().to_string();
    let status_code = parse_status_code(&status_line);

    let mut www_authenticate = None;
    while let Some(line) = read_line(&mut reader, target.timeout()).await? {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            break;
        }
        if let Some((name, value)) = trimmed.split_once(':') {
            if name.eq_ignore_ascii_case("WWW-Authenticate") {
                www_authenticate = Some(value.trim().to_string());
            }
        }
    }

    if status_line.is_empty() {
        bail!("rtsp empty status line");
    }

    Ok(RtspProbeResult {
        endpoint: target.rtsp_display(),
        status_line,
        status_code,
        www_authenticate,
    })
}

fn parse_status_code(line: &str) -> Option<u16> {
    line.split_whitespace().nth(1)?.parse().ok()
}

async fn read_line(
    reader: &mut BufReader<TcpStream>,
    timeout_duration: std::time::Duration,
) -> Result<Option<String>> {
    let mut line = String::new();
    let bytes = timeout(timeout_duration, reader.read_line(&mut line))
        .await
        .context("rtsp read timeout")??;
    if bytes == 0 {
        return Ok(None);
    }
    Ok(Some(line))
}
