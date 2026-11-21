use std::future::Future;

use broadcast::MediaSourceError;
use tokio::sync::watch;

/// Whether a `LocalBroadcast` has ended, for its media sources and
/// renditions: they stop sending frames to the subscriptions the namespace
/// had, and their waits end.
#[derive(Clone)]
pub(super) struct BroadcastEnd {
    ended_receiver: watch::Receiver<bool>,
}

impl BroadcastEnd {
    pub(super) fn new(ended_sender: &watch::Sender<bool>) -> Self {
        Self {
            ended_receiver: ended_sender.subscribe(),
        }
    }

    pub(super) fn check(&self) -> Result<(), MediaSourceError> {
        if *self.ended_receiver.borrow() {
            return Err(MediaSourceError::Stopped(anyhow::anyhow!(
                "the broadcast was closed"
            )));
        }
        Ok(())
    }

    pub(super) async fn until<F: Future>(&self, future: F) -> anyhow::Result<F::Output> {
        let mut ended_receiver = self.ended_receiver.clone();
        tokio::select! {
            output = future => Ok(output),
            _ = ended_receiver.wait_for(|ended| *ended) => anyhow::bail!("the broadcast was closed"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn the_end_refuses_pushes_and_stops_a_wait() {
        // Arrange
        let ended_sender = watch::Sender::new(false);
        let end = BroadcastEnd::new(&ended_sender);
        assert!(end.check().is_ok());

        // Act
        ended_sender.send_replace(true);
        let waited = end.until(std::future::pending::<()>()).await;

        // Assert
        assert!(matches!(end.check(), Err(MediaSourceError::Stopped(_))));
        assert!(waited.is_err());
    }
}
