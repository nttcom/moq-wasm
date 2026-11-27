use std::{
    collections::VecDeque,
    future::Future,
    pin::pin,
    sync::{Arc, Mutex},
};

use moqt::{
    executor::{self, AbortOnDrop},
    publish::TrackDemand,
};
use tokio::sync::{Notify, mpsc};

/// The changes of whether a subscriber takes one of a `MediaSource`'s
/// tracks that nobody has read yet, at most one per track, so a reader that
/// falls behind, or never reads, holds one change per track rather than
/// every transition. Clones read the same changes, each change once.
#[derive(Clone)]
pub struct DemandChanges {
    board: Arc<DemandBoard>,
}

impl DemandChanges {
    pub(super) fn run(demand_receiver: mpsc::UnboundedReceiver<TrackDemand>) -> Self {
        Self {
            board: Arc::new(DemandBoard::run(demand_receiver)),
        }
    }

    /// The oldest unread change; a new change replaces its track's unread
    /// one. `None` once every track of the source is gone and every change
    /// was read.
    pub fn next(&self) -> impl Future<Output = Option<TrackDemand>> + Send + 'static {
        let unread = self.board.unread.clone();
        async move {
            loop {
                // Registered before the check, so a change made between the
                // check and the wait still wakes it.
                let mut changed = pin!(unread.changed.notified());
                changed.as_mut().enable();
                {
                    let mut state = unread.state.lock().unwrap();
                    if let Some(demand) = state.demands.pop_front() {
                        return Some(demand);
                    }
                    if state.ended {
                        return None;
                    }
                }
                changed.await;
            }
        }
    }
}

#[derive(Default)]
struct Unread {
    state: Mutex<UnreadState>,
    changed: Notify,
}

#[derive(Default)]
struct UnreadState {
    demands: VecDeque<TrackDemand>,
    /// Set once every sender of the changes is gone.
    ended: bool,
}

/// Moves the changes the broadcast sends into the unread ones, until every
/// `DemandChanges` clone is dropped.
struct DemandBoard {
    unread: Arc<Unread>,
    _join_handle: AbortOnDrop,
}

impl DemandBoard {
    fn run(mut demand_receiver: mpsc::UnboundedReceiver<TrackDemand>) -> Self {
        let unread = Arc::new(Unread::default());
        let board = unread.clone();
        let join_handle = executor::spawn("media-source-demand", async move {
            while let Some(demand) = demand_receiver.recv().await {
                {
                    let mut state = board.state.lock().unwrap();
                    match state
                        .demands
                        .iter_mut()
                        .find(|unread| unread.track_name == demand.track_name)
                    {
                        Some(unread) => *unread = demand,
                        None => state.demands.push_back(demand),
                    }
                }
                board.changed.notify_waiters();
            }
            board.state.lock().unwrap().ended = true;
            board.changed.notify_waiters();
        })
        .abort_on_drop();
        Self {
            unread,
            _join_handle: join_handle,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::TEST_TIMEOUT;

    fn demand(track_name: &str, wanted: bool) -> TrackDemand {
        TrackDemand {
            track_name: track_name.to_string(),
            wanted,
        }
    }

    async fn next(changes: &DemandChanges) -> Option<TrackDemand> {
        tokio::time::timeout(TEST_TIMEOUT, changes.next())
            .await
            .expect("a change or the end must arrive")
    }

    #[tokio::test]
    async fn unread_changes_of_one_track_leave_only_the_latest() {
        // Arrange
        let (demand_sender, demand_receiver) = mpsc::unbounded_channel();
        let changes = DemandChanges::run(demand_receiver);
        for wanted in [true, false, true, false, true] {
            demand_sender.send(demand("video", wanted)).unwrap();
        }
        drop(demand_sender);

        // Act
        let first = next(&changes).await;
        let second = next(&changes).await;

        // Assert
        assert_eq!(first, Some(demand("video", true)));
        assert_eq!(second, None);
    }

    #[tokio::test]
    async fn changes_of_different_tracks_keep_the_order_of_their_first_unread_change() {
        // Arrange
        let (demand_sender, demand_receiver) = mpsc::unbounded_channel();
        let changes = DemandChanges::run(demand_receiver);
        demand_sender.send(demand("video", true)).unwrap();
        demand_sender.send(demand("audio", true)).unwrap();
        demand_sender.send(demand("video", false)).unwrap();
        drop(demand_sender);

        // Act
        let read = [
            next(&changes).await,
            next(&changes).await,
            next(&changes).await,
        ];

        // Assert: the video's later change replaces its unread one in place
        assert_eq!(
            read,
            [
                Some(demand("video", false)),
                Some(demand("audio", true)),
                None
            ]
        );
    }

    #[tokio::test]
    async fn a_change_after_a_read_is_read_again() {
        // Arrange
        let (demand_sender, demand_receiver) = mpsc::unbounded_channel();
        let changes = DemandChanges::run(demand_receiver);
        demand_sender.send(demand("video", true)).unwrap();
        let first = next(&changes).await;

        // Act
        demand_sender.send(demand("video", false)).unwrap();
        let second = next(&changes).await;

        // Assert
        assert_eq!(first, Some(demand("video", true)));
        assert_eq!(second, Some(demand("video", false)));
    }

    #[tokio::test]
    async fn the_changes_end_once_every_sender_is_gone_and_every_change_was_read() {
        // Arrange
        let (demand_sender, demand_receiver) = mpsc::unbounded_channel();
        let changes = DemandChanges::run(demand_receiver);
        let reading = tokio::spawn({
            let changes = changes.clone();
            async move { changes.next().await }
        });
        tokio::task::yield_now().await;

        // Act
        drop(demand_sender);
        let read = tokio::time::timeout(TEST_TIMEOUT, reading).await;

        // Assert: a reader already waiting wakes with the end
        assert_eq!(read.expect("the end must arrive").unwrap(), None);
        assert_eq!(next(&changes).await, None);
    }

    #[tokio::test]
    async fn clones_share_the_unread_changes() {
        // Arrange
        let (demand_sender, demand_receiver) = mpsc::unbounded_channel();
        let changes = DemandChanges::run(demand_receiver);
        let clone = changes.clone();
        demand_sender.send(demand("video", true)).unwrap();
        demand_sender.send(demand("audio", true)).unwrap();

        // Act
        let from_clone = next(&clone).await;
        let from_original = next(&changes).await;

        // Assert: each change is read once, by whichever clone reads first
        assert_eq!(from_clone, Some(demand("video", true)));
        assert_eq!(from_original, Some(demand("audio", true)));
    }
}
