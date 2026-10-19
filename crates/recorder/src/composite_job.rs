use std::path::{Path as LocalPath, PathBuf};

use anyhow::{Context, Result};
use object_store::{ObjectStoreExt, buffered::BufReader, path::Path};
use tokio::io::AsyncWriteExt;

use crate::{
    CompositeArgs,
    composite::{MemberFiles, composite_room},
    manifest::parse_manifest,
    recording::RecordedFile,
    room_state::CompositeState,
    store::RecordingStore,
    upload_task::{Upload, UploadTask},
};

pub(crate) const COMPOSITE_FILE_NAME: &str = "composite.mp4";

/// Composites recording `args.id` from the files its manifest lists into
/// `<id>/composite.mp4` in the store, reporting its progress in
/// `state/<id>.composite.json`. The files are downloaded into `--work-dir`,
/// which is removed afterwards.
pub(crate) async fn run(args: CompositeArgs) -> Result<()> {
    let store = RecordingStore::open(&args.store)?;
    store
        .write_composite(&args.id, &CompositeState::Composing)
        .await?;
    let work_dir = args.work_dir.join(&args.id);
    let result = composite(&store, &args.id, &work_dir).await;
    if let Err(error) = tokio::fs::remove_dir_all(&work_dir).await {
        tracing::warn!(path = %work_dir.display(), %error, "cannot remove the work directory");
    }
    let state = match &result {
        Ok(()) => CompositeState::Done,
        Err(error) => {
            tracing::error!(error = format!("{error:#}"), "compositing failed");
            CompositeState::Failed(format!("{error:#}"))
        }
    };
    store.write_composite(&args.id, &state).await?;
    result
}

async fn composite(store: &RecordingStore, id: &str, work_dir: &LocalPath) -> Result<()> {
    let manifest = store
        .read_manifest(id)
        .await?
        .with_context(|| format!("recording {id} has no manifest"))?;
    tokio::fs::create_dir_all(work_dir)
        .await
        .with_context(|| format!("cannot create {}", work_dir.display()))?;
    let mut members = Vec::new();
    for member in parse_manifest(&manifest)? {
        let mut files = Vec::new();
        for (key, capture_origin) in member.files {
            let path = work_dir.join(key.as_ref().replace('/', "_"));
            match download(store, &key, &path).await {
                Ok(()) => files.push(RecordedFile {
                    path,
                    key: Some(key),
                    capture_origin: Some(capture_origin),
                }),
                Err(error) => {
                    tracing::warn!(%key, error = format!("{error:#}"), "leaving out a file that cannot be read");
                }
            }
        }
        members.push(MemberFiles {
            namespace: member.namespace,
            files,
        });
    }
    let output = work_dir.join(COMPOSITE_FILE_NAME);
    tracing::info!(id, "compositing the room");
    composite_room(&members, &output).await?;
    let task = UploadTask::run(store.object_store());
    task.queue().upload(Upload {
        path: output,
        key: store.recording_prefix(id).join(COMPOSITE_FILE_NAME),
    });
    anyhow::ensure!(
        task.finish().await.is_empty(),
        "cannot upload the composite"
    );
    tracing::info!(id, "room composited");
    Ok(())
}

async fn download(store: &RecordingStore, key: &Path, path: &PathBuf) -> Result<()> {
    let object_store = store.object_store();
    let meta = object_store.head(key).await?;
    let mut reader = BufReader::new(object_store, &meta);
    let mut file = tokio::fs::File::create(path)
        .await
        .with_context(|| format!("cannot create {}", path.display()))?;
    tokio::io::copy(&mut reader, &mut file).await?;
    file.flush().await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use object_store::PutPayload;
    use serde_json::json;
    use url::Url;

    use super::*;
    use crate::test_support::{scratch_dir, write_fixture_fmp4};

    #[tokio::test]
    async fn composites_the_files_of_the_manifest_into_the_store() {
        // Arrange: one participant's file in a directory store, plus a listed file that is gone
        let dir = scratch_dir();
        let store_dir = dir.join("store");
        let source = dir.join("a.mp4");
        write_fixture_fmp4(&source);
        let store = RecordingStore::open(&Url::from_directory_path(&store_dir).unwrap()).unwrap();
        let key = store.recording_prefix("1").join("a.mp4");
        store
            .object_store()
            .put(&key, PutPayload::from(std::fs::read(&source).unwrap()))
            .await
            .unwrap();
        let manifest = json!({ "members": [{
            "namespace": "anon/room/a",
            "files": [
                { "key": key.as_ref(), "capture_origin_us": 1_700_000_000_000_000_u64 },
                { "key": "1/gone.mp4", "capture_origin_us": 1_700_000_001_000_000_u64 },
            ],
        }] });
        std::fs::write(store_dir.join("1/manifest.json"), manifest.to_string()).unwrap();
        let args = CompositeArgs {
            id: "1".to_string(),
            store: Url::from_directory_path(&store_dir).unwrap(),
            work_dir: dir.join("work"),
        };

        // Act
        run(args).await.unwrap();

        // Assert
        let composite = store_dir.join("1/composite.mp4").exists();
        let state = std::fs::read_to_string(store_dir.join("state/1.composite.json")).unwrap();
        let work_left = dir.join("work/1").exists();
        std::fs::remove_dir_all(&dir).unwrap();
        assert!(composite);
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&state).unwrap(),
            json!({ "state": "done", "error": null })
        );
        assert!(!work_left);
    }
}
