use std::{path::PathBuf, sync::Arc};

use anyhow::{Context, Result};
use object_store::{ObjectStore, buffered::BufWriter, path::Path};
use tokio::{io::AsyncWriteExt, sync::mpsc, task::JoinHandle};

/// A closed recording file and the object it becomes. Objects carry no
/// attributes, since a `file://` store cannot hold them.
pub(crate) struct Upload {
    pub(crate) path: PathBuf,
    pub(crate) key: Path,
}

/// Uploads closed files one at a time, in the order they were queued, and
/// deletes each local file once its object is complete. A file whose upload
/// fails after the store's retries stays on disk.
pub(crate) struct UploadTask {
    handle: JoinHandle<Vec<PathBuf>>,
    /// An `Option` so that `finish` can drop it to close the queue, which
    /// the `Drop` impl keeps it from moving out.
    upload_sender: Option<mpsc::UnboundedSender<Upload>>,
}

impl UploadTask {
    pub(crate) fn run(store: Arc<dyn ObjectStore>) -> Self {
        let (upload_sender, mut upload_receiver) = mpsc::unbounded_channel::<Upload>();
        let handle = tokio::spawn(async move {
            let mut failed = Vec::new();
            while let Some(upload) = upload_receiver.recv().await {
                match upload_file(&store, &upload).await {
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

async fn upload_file(store: &Arc<dyn ObjectStore>, upload: &Upload) -> Result<()> {
    let mut file = tokio::fs::File::open(&upload.path)
        .await
        .with_context(|| format!("cannot open {}", upload.path.display()))?;
    let mut writer = BufWriter::new(Arc::clone(store), upload.key.clone());
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
    use object_store::{ObjectStoreExt, local::LocalFileSystem, memory::InMemory};

    use super::*;
    use crate::test_support::scratch_dir;

    #[tokio::test]
    async fn uploads_a_closed_file_and_deletes_it() {
        // Arrange
        let dir = scratch_dir();
        let path = dir.join("rec-1.mp4");
        tokio::fs::write(&path, b"fragmented mp4").await.unwrap();
        let store = Arc::new(InMemory::new());
        let task = UploadTask::run(store.clone());
        let key = Path::from("recordings/anon/live/1700000000000-1.mp4");

        // Act
        task.queue().upload(Upload {
            path: path.clone(),
            key: key.clone(),
        });
        let failed = task.finish().await;

        // Assert
        assert!(failed.is_empty());
        let object = store.get(&key).await.unwrap();
        assert_eq!(&object.bytes().await.unwrap()[..], b"fragmented mp4");
        assert!(!path.exists());
        tokio::fs::remove_dir_all(&dir).await.unwrap();
    }

    #[tokio::test]
    async fn uploads_into_a_directory_store() {
        // Arrange
        let dir = scratch_dir();
        let path = dir.join("rec-1.mp4");
        tokio::fs::write(&path, b"fragmented mp4").await.unwrap();
        let store_dir = dir.join("store");
        tokio::fs::create_dir(&store_dir).await.unwrap();
        let store = Arc::new(LocalFileSystem::new_with_prefix(&store_dir).unwrap());
        let task = UploadTask::run(store);

        // Act
        task.queue().upload(Upload {
            path: path.clone(),
            key: Path::from("1/anon/live/1700000000000-1.mp4"),
        });
        let failed = task.finish().await;

        // Assert
        let uploaded = std::fs::read(store_dir.join("1/anon/live/1700000000000-1.mp4")).unwrap();
        tokio::fs::remove_dir_all(&dir).await.unwrap();
        assert!(failed.is_empty());
        assert_eq!(uploaded, b"fragmented mp4");
    }
}
