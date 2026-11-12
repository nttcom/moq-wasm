use std::{sync::Arc, time::Duration};

use tokio::sync::Notify;

use crate::modules::executor::{self, JoinHandle};

/// Notifies `requested` once: after `delay`, or when `end_delay` is called
/// before that.
pub(super) struct NewGroupDelayTask {
    handle: JoinHandle,
    delay_ended: Arc<Notify>,
}

impl NewGroupDelayTask {
    pub(super) fn run(requested: Arc<Notify>, delay: Duration) -> Self {
        let delay_ended = Arc::new(Notify::new());
        let handle = executor::spawn("Origin New Group Delay", {
            let delay_ended = delay_ended.clone();
            async move {
                tokio::select! {
                    () = executor::sleep(delay) => {}
                    () = delay_ended.notified() => {}
                }
                requested.notify_one();
            }
        });
        Self {
            handle,
            delay_ended,
        }
    }

    /// A no-op once the task notified.
    pub(super) fn end_delay(&self) {
        self.delay_ended.notify_one();
    }
}

impl Drop for NewGroupDelayTask {
    fn drop(&mut self) {
        self.handle.abort();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::modules::test_support::notified_within;

    const DELAY: Duration = Duration::from_secs(2);

    #[tokio::test(start_paused = true)]
    async fn notifies_when_the_delay_passed() {
        // Arrange
        let requested = Arc::new(Notify::new());

        // Act
        let _task = NewGroupDelayTask::run(requested.clone(), DELAY);
        let before = notified_within(&requested, DELAY - Duration::from_millis(1)).await;
        let after = notified_within(&requested, DELAY).await;

        // Assert
        assert!(!before);
        assert!(after);
    }

    #[tokio::test(start_paused = true)]
    async fn notifies_once_when_the_delay_is_ended_early() {
        // Arrange
        let requested = Arc::new(Notify::new());
        let task = NewGroupDelayTask::run(requested.clone(), DELAY);

        // Act
        task.end_delay();
        let early = notified_within(&requested, Duration::from_millis(1)).await;
        task.end_delay();
        let again = notified_within(&requested, DELAY).await;

        // Assert
        assert!(early);
        assert!(!again);
    }

    #[tokio::test(start_paused = true)]
    async fn a_dropped_task_does_not_notify() {
        // Arrange
        let requested = Arc::new(Notify::new());
        let task = NewGroupDelayTask::run(requested.clone(), DELAY);

        // Act
        drop(task);
        let notified = notified_within(&requested, DELAY * 2).await;

        // Assert
        assert!(!notified);
    }
}
