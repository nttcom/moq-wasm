use std::{path::PathBuf, sync::Arc};

use anyhow::{Context, Result};
use mediapack::Timestamp;
use object_store::{Attribute, Attributes, ObjectStore, buffered::BufWriter, path::Path};
use tokio::{io::AsyncWriteExt, sync::mpsc, task::JoinHandle};

/// The object metadata key holding the recording's capture origin, the Unix
/// time in microseconds that presentation time 0 of every file stands for.
const CAPTURE_ORIGIN_METADATA: &str = "capture-origin-us";

/// A closed recording file and the object it becomes.
pub(crate) struct Upload {
    pub(crate) path: PathBuf,
    pub(crate) key: Path,
    pub(crate) capture_origin: Option<Timestamp>,
}

/// Uploads closed files one at a time, in the order they were queued, and
/// deletes each local file once its object is complete unless `run` was
/// given `keep_local_files`. A file whose upload fails after the store's
/// retries stays on disk.
pub(crate) struct UploadTask {
    handle: JoinHandle<Vec<PathBuf>>,
    /// An `Option` so that `finish` can drop it to close the queue, which
    /// the `Drop` impl keeps it from moving out.
    upload_sender: Option<mpsc::UnboundedSender<Upload>>,
}

impl UploadTask {
    pub(crate) fn run(store: Arc<dyn ObjectStore>, keep_local_files: bool) -> Self {
        let (upload_sender, mut upload_receiver) = mpsc::unbounded_channel::<Upload>();
        let handle = tokio::spawn(async move {
            let mut failed = Vec::new();
            while let Some(upload) = upload_receiver.recv().await {
                match upload_file(&store, &upload, keep_local_files).await {
                    Ok(()) => tracing::info!(key = %upload.key, "uploaded"),
                    Err(error) => {
                        tracing::error!(
                            path = %upload.path.display(),
                            key = %upload.key,
                            error = format!("{error:#}"),
                            "upload failed; the file stays on disk"
                        );
                        failed.push(upload.path);
                    }
                }
            }
            failed
        });
        Self {
            handle,
            upload_sender: Some(upload_sender),
        }
    }

    pub(crate) fn queue(&self) -> UploadQueue {
        let Some(sender) = &self.upload_sender else {
            unreachable!("only `finish` clears the sender, and it consumes the task");
        };
        UploadQueue(sender.clone())
    }

    /// Waits until every queued file is uploaded or has failed, and returns
    /// the files that failed; it returns only once every `UploadQueue` is
    /// dropped.
    pub(crate) async fn finish(mut self) -> Vec<PathBuf> {
        self.upload_sender = None;
        (&mut self.handle).await.unwrap_or_else(|error| {
            tracing::error!(%error, "upload task failed");
            Vec::new()
        })
    }
}

#[derive(Clone)]
pub(crate) struct UploadQueue(mpsc::UnboundedSender<Upload>);

impl UploadQueue {
    pub(crate) fn upload(&self, upload: Upload) {
        if let Err(error) = self.0.send(upload) {
            tracing::error!(path = %error.0.path.display(), "upload task gone; the file stays on disk");
        }
    }
}

async fn upload_file(
    store: &Arc<dyn ObjectStore>,
    upload: &Upload,
    keep_local_file: bool,
) -> Result<()> {
    let mut file = tokio::fs::File::open(&upload.path)
        .await
        .with_context(|| format!("cannot open {}", upload.path.display()))?;
    let mut attributes = Attributes::new();
    attributes.insert(Attribute::ContentType, "video/mp4".into());
    if let Some(origin) = upload.capture_origin {
        attributes.insert(
            Attribute::Metadata(CAPTURE_ORIGIN_METADATA.into()),
            origin.micros().to_string().into(),
        );
    }
    let mut writer =
        BufWriter::new(Arc::clone(store), upload.key.clone()).with_attributes(attributes);
    let written = async {
        tokio::io::copy(&mut file, &mut writer).await?;
        writer.shutdown().await
    }
    .await;
    if let Err(error) = written {
        if let Err(abort) = writer.abort().await {
            tracing::warn!(key = %upload.key, %abort, "cannot abort the partial upload");
        }
        return Err(error).with_context(|| format!("cannot upload {}", upload.key));
    }
    if keep_local_file {
        return Ok(());
    }
    tokio::fs::remove_file(&upload.path)
        .await
        .with_context(|| format!("uploaded, but cannot delete {}", upload.path.display()))
}

impl Drop for UploadTask {
    fn drop(&mut self) {
        self.handle.abort();
    }
}

#[cfg(test)]
mod tests {
    use object_store::{ObjectStoreExt, memory::InMemory};

    use super::*;
    use crate::test_support::scratch_dir;

    #[tokio::test]
    async fn uploads_a_closed_file_with_its_capture_origin_and_deletes_it() {
        // Arrange
        let dir = scratch_dir();
        let path = dir.join("rec-1.mp4");
        tokio::fs::write(&path, b"fragmented mp4").await.unwrap();
        let store = Arc::new(InMemory::new());
        let task = UploadTask::run(store.clone(), false);
        let key = Path::from("recordings/anon/live/1700000000000-1.mp4");

        // Act
        task.queue().upload(Upload {
            path: path.clone(),
            key: key.clone(),
            capture_origin: Some(Timestamp::from_micros(1_700_000_000_123_456)),
        });
        let failed = task.finish().await;

        // Assert
        assert!(failed.is_empty());
        let object = store.get(&key).await.unwrap();
        let metadata = object
            .attributes
            .get(&Attribute::Metadata(CAPTURE_ORIGIN_METADATA.into()))
            .cloned();
        assert_eq!(&object.bytes().await.unwrap()[..], b"fragmented mp4");
        assert_eq!(metadata, Some("1700000000123456".into()));
        assert!(!path.exists());
        tokio::fs::remove_dir_all(&dir).await.unwrap();
    }
}
