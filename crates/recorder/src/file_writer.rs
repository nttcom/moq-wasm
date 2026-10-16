use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use tokio::{
    fs::{File, OpenOptions},
    io::{AsyncWriteExt, BufWriter},
};

/// `index` counts from 1.
#[derive(Debug, PartialEq)]
pub(crate) struct ClosedFile {
    pub(crate) path: PathBuf,
    pub(crate) index: u32,
}

/// `rotate` continues in the next numbered file (`<stem>-2.mp4`,
/// `<stem>-3.mp4`, ...). An existing file is never overwritten.
pub(crate) struct RecordingFile {
    first_path: PathBuf,
    path: PathBuf,
    index: u32,
    writer: BufWriter<File>,
}

impl RecordingFile {
    pub(crate) async fn create(path: PathBuf) -> Result<Self> {
        let writer = open(&path).await?;
        Ok(Self {
            first_path: path.clone(),
            path,
            index: 1,
            writer,
        })
    }

    pub(crate) fn path(&self) -> &Path {
        &self.path
    }

    pub(crate) async fn write(&mut self, data: &[u8]) -> Result<()> {
        self.writer
            .write_all(data)
            .await
            .with_context(|| format!("cannot write {}", self.path.display()))?;
        Ok(())
    }

    pub(crate) async fn flush(&mut self) -> Result<()> {
        self.writer
            .flush()
            .await
            .with_context(|| format!("cannot flush {}", self.path.display()))
    }

    pub(crate) async fn rotate(&mut self, tail: &[u8]) -> Result<ClosedFile> {
        self.write(tail).await?;
        self.flush().await?;
        let closed = ClosedFile {
            path: self.path.clone(),
            index: self.index,
        };
        self.index += 1;
        self.path = numbered_path(&self.first_path, self.index);
        self.writer = open(&self.path).await?;
        Ok(closed)
    }

    pub(crate) async fn finish(mut self, tail: &[u8]) -> Result<ClosedFile> {
        self.write(tail).await?;
        self.flush().await?;
        Ok(ClosedFile {
            path: self.path,
            index: self.index,
        })
    }
}

async fn open(path: &Path) -> Result<BufWriter<File>> {
    let file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .await
        .with_context(|| format!("cannot create {}", path.display()))?;
    Ok(BufWriter::new(file))
}

fn numbered_path(first: &Path, index: u32) -> PathBuf {
    let stem = first
        .file_stem()
        .map(|stem| stem.to_string_lossy().into_owned())
        .unwrap_or_default();
    let name = match first.extension() {
        Some(extension) => format!("{stem}-{index}.{}", extension.to_string_lossy()),
        None => format!("{stem}-{index}"),
    };
    first.with_file_name(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numbers_the_next_file_before_the_extension() {
        // Arrange
        let first = Path::new("out/live-20261013T000000Z.mp4");

        // Act / Assert
        assert_eq!(
            numbered_path(first, 2),
            Path::new("out/live-20261013T000000Z-2.mp4")
        );
        assert_eq!(numbered_path(Path::new("live"), 3), Path::new("live-3"));
    }

    #[tokio::test]
    async fn rotate_closes_the_file_and_continues_in_the_next_one() {
        // Arrange
        let dir = std::env::temp_dir().join(format!("recorder-{}", std::process::id()));
        tokio::fs::create_dir_all(&dir).await.unwrap();
        let first = dir.join("rec.mp4");
        let mut file = RecordingFile::create(first.clone()).await.unwrap();

        // Act
        file.write(b"first").await.unwrap();
        let rotated = file.rotate(b"-tail").await.unwrap();
        file.write(b"second").await.unwrap();
        let finished = file.finish(b"!").await.unwrap();

        // Assert
        assert_eq!(
            rotated,
            ClosedFile {
                path: first.clone(),
                index: 1
            }
        );
        assert_eq!(
            finished,
            ClosedFile {
                path: dir.join("rec-2.mp4"),
                index: 2
            }
        );
        assert_eq!(tokio::fs::read(&first).await.unwrap(), b"first-tail");
        assert_eq!(
            tokio::fs::read(dir.join("rec-2.mp4")).await.unwrap(),
            b"second!"
        );
        tokio::fs::remove_dir_all(&dir).await.unwrap();
    }

    #[tokio::test]
    async fn never_overwrites_an_existing_file() {
        // Arrange
        let path =
            std::env::temp_dir().join(format!("recorder-existing-{}.mp4", std::process::id()));
        tokio::fs::write(&path, b"keep").await.unwrap();

        // Act
        let result = RecordingFile::create(path.clone()).await;

        // Assert
        assert!(result.is_err());
        assert_eq!(tokio::fs::read(&path).await.unwrap(), b"keep");
        tokio::fs::remove_file(&path).await.unwrap();
    }
}
