use std::sync::Arc;

use tokio::sync::{mpsc, oneshot, watch};

use crate::{
    Session, Subscription, TrackObject, TrackReader,
    modules::{
        executor::{self, JoinHandle},
        moqt::domains::group_cutover::GroupCutover,
    },
    object::DataReceiver,
};

pub(super) struct TrackSource {
    pub(super) session: Arc<Session>,
    pub(super) subscription: Subscription,
    /// Dropped once the track takes no new group from the previous source.
    pub(super) cut_over: oneshot::Sender<()>,
}

/// One track subscribed through a `MigratingSession`. A GOAWAY makes the
/// session subscribe the track on the new session too; objects are then
/// taken from both subscriptions, with no group taken twice, until the
/// session closes the previous one.
pub struct MigratingTrack {
    session: Arc<Session>,
    subscription: Subscription,
    current: SourceReader,
    previous: Option<PreviousSource>,
    cutover: GroupCutover,
    source_receiver: mpsc::UnboundedReceiver<TrackSource>,
    migrating: watch::Receiver<bool>,
}

struct PreviousSource {
    reader: SourceReader,
    /// `None` after the cutover.
    cut_over: Option<oneshot::Sender<()>>,
}

impl MigratingTrack {
    pub(super) fn new(
        session: Arc<Session>,
        subscription: Subscription,
        source_receiver: mpsc::UnboundedReceiver<TrackSource>,
        migrating: watch::Receiver<bool>,
    ) -> Self {
        Self {
            current: SourceReader::accept(session.clone(), subscription.clone()),
            session,
            subscription,
            previous: None,
            cutover: GroupCutover::default(),
            source_receiver,
            migrating,
        }
    }

    /// The session the first SUBSCRIBE was accepted on; a FETCH that
    /// addresses `subscription` goes there.
    pub fn session(&self) -> &Arc<Session> {
        &self.session
    }

    /// The first subscription, as `SUBSCRIBE_OK` answered it.
    pub fn subscription(&self) -> &Subscription {
        &self.subscription
    }

    /// Like `TrackReader::next_object`. Returns `Ok(None)` once the track
    /// ended on every session it is subscribed on and no migration that could
    /// subscribe it again is under way. While two subscriptions are read, a
    /// failure of either one is only logged.
    pub async fn next_object(&mut self) -> anyhow::Result<Option<TrackObject>> {
        loop {
            tokio::select! {
                biased;
                Some(source) = self.source_receiver.recv() => self.begin_migration(source),
                object = next_previous_object(self.previous.as_mut()) => match object {
                    Some(Ok(object)) => {
                        if self.cutover.take_previous(object.group_id) {
                            return Ok(Some(object));
                        }
                    }
                    Some(Err(error)) => {
                        tracing::debug!(%error, "previous subscription failed");
                    }
                    None => self.release_previous(),
                },
                // `SourceReader::Ended` answers `None` at once, so this
                // branch only loops after `current_ended` replaced it.
                object = self.current.next_object() => match object {
                    Some(Ok(object)) => {
                        if self.cutover.take_current(object.group_id, object.object_id) {
                            self.note_cutover();
                            return Ok(Some(object));
                        }
                    }
                    Some(Err(error)) if self.previous.is_some() => {
                        tracing::debug!(%error, "new subscription failed");
                    }
                    Some(Err(error)) => return Err(error),
                    None => {
                        if !self.current_ended().await {
                            return Ok(None);
                        }
                    }
                },
            }
        }
    }

    fn begin_migration(&mut self, source: TrackSource) {
        let reader = SourceReader::accept(source.session, source.subscription);
        self.previous = Some(PreviousSource {
            reader: std::mem::replace(&mut self.current, reader),
            cut_over: Some(source.cut_over),
        });
        self.cutover.begin();
    }

    fn note_cutover(&mut self) {
        if let Some(previous) = &mut self.previous
            && self.cutover.is_cut_over()
        {
            previous.cut_over = None;
        }
    }

    fn release_previous(&mut self) {
        if self.previous.take().is_some() {
            self.cutover.previous_ended();
        }
    }

    /// Whether another subscription carries on: the previous one, or one a
    /// migration under way subscribes.
    async fn current_ended(&mut self) -> bool {
        if let Some(previous) = self.previous.take() {
            self.current = previous.reader;
            self.cutover.settle();
            return true;
        }
        let _ = self.migrating.wait_for(|migrating| !migrating).await;
        match self.source_receiver.try_recv() {
            Ok(source) => {
                self.begin_migration(source);
                true
            }
            Err(_) => false,
        }
    }
}

async fn next_previous_object(
    previous: Option<&mut PreviousSource>,
) -> Option<anyhow::Result<TrackObject>> {
    match previous {
        Some(previous) => previous.reader.next_object().await,
        None => std::future::pending().await,
    }
}

/// The objects of one subscription. The first data stream is awaited in a
/// task of its own because `Subscriber::accept_data_receiver` loses the
/// receiver when its future is dropped, and `next_object` runs inside
/// `select!`.
enum SourceReader {
    Accepting {
        reader_receiver: oneshot::Receiver<anyhow::Result<TrackReader>>,
        _accept_task: AcceptTask,
    },
    Reading(TrackReader),
    Ended,
}

impl SourceReader {
    fn accept(session: Arc<Session>, subscription: Subscription) -> Self {
        let (reader_sender, reader_receiver) = oneshot::channel();
        Self::Accepting {
            reader_receiver,
            _accept_task: AcceptTask::run(session, subscription, reader_sender),
        }
    }

    async fn next_object(&mut self) -> Option<anyhow::Result<TrackObject>> {
        loop {
            match self {
                Self::Accepting {
                    reader_receiver, ..
                } => match reader_receiver.await {
                    Ok(Ok(reader)) => *self = Self::Reading(reader),
                    Ok(Err(error)) => {
                        tracing::debug!(%error, "subscription delivered no data stream");
                        *self = Self::Ended;
                    }
                    Err(_) => *self = Self::Ended,
                },
                Self::Reading(reader) => {
                    return match reader.next_object().await {
                        Ok(Some(object)) => Some(Ok(object)),
                        Ok(None) => {
                            *self = Self::Ended;
                            None
                        }
                        Err(error) => Some(Err(error)),
                    };
                }
                Self::Ended => return None,
            }
        }
    }
}

struct AcceptTask {
    join_handle: JoinHandle,
}

impl AcceptTask {
    fn run(
        session: Arc<Session>,
        subscription: Subscription,
        reader_sender: oneshot::Sender<anyhow::Result<TrackReader>>,
    ) -> Self {
        let join_handle = executor::spawn("Migrating Track Accept", async move {
            let reader = match session
                .subscriber()
                .accept_data_receiver(&subscription)
                .await
            {
                Ok(DataReceiver::Stream(factory)) => Ok(TrackReader::new(factory)),
                Ok(DataReceiver::Datagram(_)) => {
                    Err(anyhow::anyhow!("the track is delivered in datagrams"))
                }
                Err(error) => Err(error),
            };
            let _ = reader_sender.send(reader);
        });
        Self { join_handle }
    }
}

impl Drop for AcceptTask {
    fn drop(&mut self) {
        self.join_handle.abort();
    }
}
