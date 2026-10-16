use std::sync::Arc;

use tokio::sync::watch;

/// Counts the subgroup streams of one subscription that a connection handed
/// to the application and that have not ended.
#[derive(Clone)]
pub(crate) struct OpenSubgroups(Arc<watch::Sender<usize>>);

/// Keeps a stream counted in its `OpenSubgroups` until dropped.
#[derive(Debug)]
pub(crate) struct OpenSubgroup(Arc<watch::Sender<usize>>);

impl OpenSubgroups {
    pub(crate) fn new() -> Self {
        Self(Arc::new(watch::Sender::new(0)))
    }

    pub(crate) fn open(&self) -> OpenSubgroup {
        self.0.send_modify(|open| *open += 1);
        OpenSubgroup(self.0.clone())
    }

    pub(crate) fn is_empty(&self) -> bool {
        *self.0.borrow() == 0
    }

    pub(crate) async fn all_ended(&self) {
        let mut receiver = self.0.subscribe();
        // `self` holds the sender, so the channel cannot close.
        let _ = receiver.wait_for(|open| *open == 0).await;
    }
}

impl Drop for OpenSubgroup {
    fn drop(&mut self) {
        self.0.send_modify(|open| *open -= 1);
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    #[tokio::test]
    async fn all_ended_waits_for_the_last_open_stream() {
        // Arrange
        let open_subgroups = OpenSubgroups::new();
        let first = open_subgroups.open();
        let second = open_subgroups.open();

        // Act
        drop(first);
        let while_one_is_open =
            tokio::time::timeout(Duration::from_millis(50), open_subgroups.all_ended()).await;
        drop(second);
        let after_the_last =
            tokio::time::timeout(Duration::from_millis(50), open_subgroups.all_ended()).await;

        // Assert
        assert!(while_one_is_open.is_err());
        assert!(after_the_last.is_ok());
        assert!(open_subgroups.is_empty());
    }
}
