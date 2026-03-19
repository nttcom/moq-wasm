mod ffmpeg;
mod moqt_sender;

use anyhow::{bail, Result};
use bytes::Bytes;
use moqt::{ExtensionHeaders, StreamDataSender, SubgroupId, SubgroupObject, QUIC};
use tokio::sync::mpsc;
use tracing::info;

pub struct Segment {
    pub init_segment: Vec<u8>,
    pub media_segment: Vec<u8>,
}

async fn read_segments(tx: mpsc::Sender<Segment>) -> Result<()> {
    let (_child, mut stdout) = ffmpeg::spawn_ffmpeg()?;
    info!("ffmpeg started");

    // 最初に moov box (init segment) を読み出す
    let init_box = ffmpeg::read_box(&mut stdout)
        .await?
        .expect("expected ftyp or moov box");

    let init_segment = if &init_box.box_type == b"ftyp" {
        // ftyp + moov をまとめて init segment にする
        let moov_box = ffmpeg::read_box(&mut stdout)
            .await?
            .expect("expected moov box after ftyp");
        assert_eq!(&moov_box.box_type, b"moov");
        let mut init = init_box.data;
        init.extend_from_slice(&moov_box.data);
        init
    } else {
        assert_eq!(&init_box.box_type, b"moov");
        init_box.data
    };

    info!(size = init_segment.len(), "init segment ready");

    // moof + mdat ペアを読み出してセグメントとして送信
    loop {
        let Some(moof) = ffmpeg::read_box(&mut stdout).await? else {
            info!("ffmpeg stdout closed");
            break;
        };

        if &moof.box_type != b"moof" {
            info!(box_type = moof.type_str(), "skipping unexpected box");
            continue;
        }

        let Some(mdat) = ffmpeg::read_box(&mut stdout).await? else {
            bail!("unexpected EOF after moof");
        };
        assert_eq!(&mdat.box_type, b"mdat");

        let mut media = moof.data;
        media.extend_from_slice(&mdat.data);

        info!(size = media.len(), "media segment ready");

        let segment = Segment {
            init_segment: init_segment.clone(),
            media_segment: media,
        };

        if tx.send(segment).await.is_err() {
            info!("receiver dropped, stopping");
            break;
        }
    }

    Ok(())
}

async fn send_segments(
    mut stream: StreamDataSender<QUIC>,
    mut rx: mpsc::Receiver<Segment>,
) -> Result<()> {
    let mut group_id: u64 = 0;

    while let Some(segment) = rx.recv().await {
        let ext = ExtensionHeaders {
            prior_group_id_gap: vec![],
            prior_object_id_gap: vec![],
            immutable_extensions: vec![],
        };

        let header = stream.create_header(group_id, SubgroupId::None, 128, false, false);

        // Object 0: init segment
        let obj0 = stream.create_object_field(
            &header,
            0,
            ext.clone(),
            SubgroupObject::new_payload(Bytes::from(segment.init_segment)),
        );
        stream.send(header.clone(), obj0).await?;

        // Object 1: media segment (moof + mdat)
        let obj1 = stream.create_object_field(
            &header,
            1,
            ext,
            SubgroupObject::new_payload(Bytes::from(segment.media_segment)),
        );
        stream.send(header, obj1).await?;

        info!(group_id, "group sent");
        group_id += 1;
    }

    Ok(())
}

fn generate_namespace() -> String {
    let now = chrono::Local::now();
    format!("live-{}", now.format("%H%M"))
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive(tracing::Level::INFO.into()),
        )
        .init();

    let namespace = generate_namespace();
    info!(namespace, "using namespace");

    // MoQT 接続 → subscriber 待ち
    let stream = moqt_sender::connect_and_wait_for_subscriber(&namespace).await?;

    // subscriber が来てから ffmpeg を起動
    let (tx, rx) = mpsc::channel::<Segment>(4);
    let ffmpeg_handle = tokio::spawn(async move {
        if let Err(e) = read_segments(tx).await {
            tracing::error!("ffmpeg reader error: {}", e);
        }
    });

    // 送信タスク
    let send_handle = tokio::spawn(async move {
        if let Err(e) = send_segments(stream, rx).await {
            tracing::error!("send error: {}", e);
        }
    });

    // Ctrl+C で終了
    tokio::signal::ctrl_c().await?;
    info!("shutting down");

    ffmpeg_handle.abort();
    send_handle.abort();

    Ok(())
}
