use std::{
    path::PathBuf,
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result};
use moqt::{Session, Subscription, TrackObject, TrackReader};

use crate::{
    Args,
    file_writer::RecordingFile,
    pipeline::{Output, Pipeline},
    relay::{self, RelayConnection},
    token_refresh_task::{TokenRefreshTask, read_token_file},
    track_selection::{Selection, select_tracks},
};

pub(crate) async fn record(args: Args) -> Result<()> {
    let auth_token = match &args.auth_token_file {
        Some(path) => Some(read_token_file(path).await?),
        None => args.auth_token.clone(),
    };
    let connection = RelayConnection::connect(
        &args.relay,
        !args.insecure_skip_tls_verify,
        auth_token.clone(),
    )
    .await?;
    let token_refresh = args
        .auth_token_file
        .clone()
        .zip(auth_token)
        .map(|(path, token)| {
            TokenRefreshTask::run(
                connection.session.clone(),
                path,
                app_id(&args.namespace).to_string(),
                token,
            )
        });
    let result = record_broadcast(&connection.session, &args).await;
    drop(token_refresh);
    connection.close().await;
    result
}

/// An app's namespaces start with its app id, which the relay's token
/// refresh convention names as the TRACK_STATUS namespace.
fn app_id(namespace: &str) -> &str {
    namespace.split('/').next().unwrap_or(namespace)
}

async fn record_broadcast(session: &Session, args: &Args) -> Result<()> {
    let catalog_subscription =
        relay::subscribe(session, &args.namespace, msf::CATALOG_TRACK_NAME).await?;
    let selection = wait_for_selection(session, &catalog_subscription, args).await?;
    tracing::info!(
        video = %selection.video,
        audio = selection.audio.as_ref().map(|audio| audio.name.as_str()),
        "catalog resolved"
    );

    let video_subscription = relay::subscribe(session, &args.namespace, &selection.video).await?;
    let audio_subscription = match &selection.audio {
        Some(audio) => Some(relay::subscribe(session, &args.namespace, &audio.name).await?),
        None => None,
    };
    let mut video_reader = relay::track_reader(session, &video_subscription).await?;
    let mut audio_reader = match &audio_subscription {
        Some(subscription) => Some(relay::track_reader(session, subscription).await?),
        None => None,
    };

    let path = args
        .output
        .clone()
        .unwrap_or_else(|| default_output_path(&args.namespace, SystemTime::now()));
    let mut file = RecordingFile::create(path).await?;
    let mut pipeline = Pipeline::new(&selection)?;
    tracing::info!(path = %file.path().display(), "recording started");

    loop {
        let output = tokio::select! {
            biased;
            _ = tokio::signal::ctrl_c() => {
                tracing::info!("stopping on Ctrl-C");
                break;
            }
            close = session.closed() => {
                tracing::warn!(%close, "session closed");
                break;
            }
            object = video_reader.next_object() => match object {
                Ok(Some(object)) => pipeline.push_video(&object)?,
                Ok(None) => {
                    tracing::info!("video track ended");
                    break;
                }
                Err(error) => {
                    tracing::warn!(%error, "video read error; continuing with the next group");
                    continue;
                }
            },
            object = next_audio_object(audio_reader.as_mut()) => match object {
                Ok(Some(object)) => pipeline.push_audio(&object)?,
                Ok(None) => {
                    tracing::info!("audio track ended");
                    break;
                }
                Err(error) => {
                    tracing::warn!(%error, "audio read error; continuing with the next group");
                    continue;
                }
            },
        };
        write_output(&mut file, output).await?;
    }

    let tail = pipeline.finish()?;
    file.finish(&tail).await?;
    tracing::info!("recording finished");
    Ok(())
}

/// Reads the catalog's current object from the joining FETCH
/// (draft-ietf-moq-msf-01 §5), then later objects from the subscription, until
/// the catalog lists the tracks to record. The subscription reader opens only
/// after the FETCH: its first stream arrives with the next catalog object, up
/// to 30 s later.
async fn wait_for_selection(
    session: &Session,
    subscription: &Subscription,
    args: &Args,
) -> Result<Selection> {
    let mut current = match relay::fetch_joining_group(session, subscription).await {
        Ok(current) => current,
        Err(error) => {
            tracing::warn!(%error, "joining FETCH of the catalog failed");
            None
        }
    };
    let mut reader = None;
    loop {
        let payload = match current.take() {
            Some(payload) => payload,
            None => {
                let reader = match &mut reader {
                    Some(reader) => reader,
                    None => reader.insert(relay::track_reader(session, subscription).await?),
                };
                reader
                    .next_object()
                    .await?
                    .context("catalog track ended")?
                    .payload
            }
        };
        let catalog: msf::Catalog =
            serde_json::from_slice(&payload).context("catalog is not MSF JSON")?;
        if let Some(selection) = select_tracks(&catalog, &args.video_track, &args.audio_track)? {
            return Ok(selection);
        }
        tracing::info!(
            track = %args.video_track,
            "waiting for the catalog to list the video track"
        );
    }
}

async fn next_audio_object(reader: Option<&mut TrackReader>) -> Result<Option<TrackObject>> {
    match reader {
        Some(reader) => reader.next_object().await,
        None => std::future::pending().await,
    }
}

async fn write_output(file: &mut RecordingFile, output: Output) -> Result<()> {
    if let Some(tail) = output.previous_file_tail {
        file.rotate(&tail).await?;
        tracing::info!(path = %file.path().display(), "configuration changed; recording into a new file");
    }
    file.write(&output.data).await?;
    if output.group_complete {
        file.flush().await?;
    }
    Ok(())
}

fn default_output_path(namespace: &str, now: SystemTime) -> PathBuf {
    let seconds = now.duration_since(UNIX_EPOCH).unwrap_or_default().as_secs();
    PathBuf::from(format!("{}-{seconds}.mp4", namespace.replace('/', "_")))
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    #[test]
    fn default_output_path_names_the_namespace_and_the_unix_time() {
        // Arrange
        let now = UNIX_EPOCH + Duration::from_secs(1_700_000_000);

        // Act
        let path = default_output_path("anon/live/test", now);

        // Assert
        assert_eq!(path, PathBuf::from("anon_live_test-1700000000.mp4"));
    }
}
