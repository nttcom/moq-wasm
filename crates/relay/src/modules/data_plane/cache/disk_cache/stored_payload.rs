use std::{
    io::SeekFrom,
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

use bytes::Bytes;
use tokio::io::{AsyncReadExt, AsyncSeekExt};

#[derive(Debug)]
pub(crate) struct Segment {
    pub(super) path: PathBuf,
    deleted: AtomicBool,
}

impl Segment {
    pub(super) fn new(path: PathBuf) -> Self {
        Self {
            path,
            deleted: AtomicBool::new(false),
        }
    }

    /// Set before the file is removed, so the cache can drop the objects
    /// that point into it.
    pub(super) fn mark_deleted(&self) {
        self.deleted.store(true, Ordering::Release);
    }

    fn is_deleted(&self) -> bool {
        self.deleted.load(Ordering::Acquire)
    }
}

/// Where the disk cache wrote a cached object's payload.
#[derive(Clone, Debug)]
pub(crate) struct StoredPayload {
    pub(super) segment: Arc<Segment>,
    pub(super) offset: u64,
    pub(crate) len: usize,
}

impl StoredPayload {
    pub(crate) fn is_lost(&self) -> bool {
        self.segment.is_deleted()
    }

    pub(crate) async fn read(&self) -> anyhow::Result<Bytes> {
        if self.is_lost() {
            anyhow::bail!("segment {} was deleted", self.segment.path.display());
        }
        let mut file = tokio::fs::File::open(&self.segment.path).await?;
        file.seek(SeekFrom::Start(self.offset)).await?;
        let mut payload = vec![0; self.len];
        file.read_exact(&mut payload).await?;
        Ok(Bytes::from(payload))
    }
}
