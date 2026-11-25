use std::future::Future;

use anyhow::{Context, Result};
use bytes::Bytes;
use tokio::sync::{mpsc, oneshot};
use tokio_util::sync::CancellationToken;

use super::{OutgoingObject, Placement};
use crate::{
    Publisher, Subscription, TrackWriter,
    modules::{
        executor::{self, JoinHandle},
        moqt::control_plane::error_codes::PublishDoneCode,
    },
    object::Location,
};

/// Sends what an outlet is given on its `TrackWriter`, one command at a time
/// in the order they were queued. `TrackWriter` numbers objects itself and
/// refuses a group before the last one, so the origin queues the commands
/// while it holds the lock it numbers objects under.
pub(super) struct OutletSendTask {
    command_sender: mpsc::UnboundedSender<OutletCommand>,
    /// Cancels without waiting for the queued commands: a subscriber that
    /// unsubscribed may never read the stream a send waits on.
    cancellation: CancellationToken,
    /// Not aborted on drop, unlike the origin's other tasks: aborting drops
    /// the writer in the middle of a send, which finishes its open group with
    /// a FIN. Once the command channel closes, the task resets the open group
    /// itself and exits.
    _join_handle: JoinHandle,
}

#[derive(Clone)]
pub(super) enum Payload {
    Object(OutgoingObject),
    /// One object sent as a finished group.
    Group(Bytes),
}

enum OutletCommand {
    Send {
        placement: Placement,
        payload: Payload,
        reply: oneshot::Sender<Result<bool>>,
    },
    /// Leaves the open group because one of its objects is not sent, as in
    /// Forward State 0 (draft-18 §5.1).
    LeaveGroup,
    /// Finishes the open group, sends PUBLISH_DONE and exits.
    End {
        code: PublishDoneCode,
        reason: &'static str,
        done: oneshot::Sender<Result<()>>,
    },
}

impl OutletSendTask {
    pub(super) fn run(
        writer: TrackWriter,
        publisher: Publisher,
        subscription: Subscription,
    ) -> Self {
        let (command_sender, command_receiver) = mpsc::unbounded_channel();
        let cancellation = CancellationToken::new();
        let join_handle = executor::spawn(
            "Origin Outlet Send",
            serve(
                writer,
                publisher,
                subscription,
                command_receiver,
                cancellation.clone(),
            ),
        );
        Self {
            command_sender,
            cancellation,
            _join_handle: join_handle,
        }
    }

    /// Resolves to whether the object was sent; to `false` when the task has
    /// already exited.
    pub(super) fn send(
        &self,
        placement: Placement,
        payload: Payload,
    ) -> impl Future<Output = Result<bool>> + use<> {
        let (reply, sent) = oneshot::channel();
        self.queue(OutletCommand::Send {
            placement,
            payload,
            reply,
        });
        async move { sent.await.unwrap_or(Ok(false)) }
    }

    pub(super) fn leave_group(&self) {
        self.queue(OutletCommand::LeaveGroup);
    }

    /// Stops the send in progress and drops the queued commands, whose
    /// sends resolve to `false`; the task then resets the open group with
    /// CANCELLED (draft-14 §10.4.3) and exits.
    pub(super) fn cancel(&self) {
        self.cancellation.cancel();
    }

    /// Resolves once PUBLISH_DONE is sent; the task takes no command after
    /// this one.
    pub(super) fn end(
        &self,
        code: PublishDoneCode,
        reason: &'static str,
    ) -> impl Future<Output = Result<()>> + use<> {
        let (done, ended) = oneshot::channel();
        self.queue(OutletCommand::End { code, reason, done });
        async move { ended.await.unwrap_or(Ok(())) }
    }

    fn queue(&self, command: OutletCommand) {
        let _ = self.command_sender.send(command);
    }
}

async fn serve(
    mut writer: TrackWriter,
    publisher: Publisher,
    subscription: Subscription,
    mut command_receiver: mpsc::UnboundedReceiver<OutletCommand>,
    cancellation: CancellationToken,
) {
    // A send stopped part way leaves its group open on the writer, so the
    // reset below covers it.
    while let Some(command) = cancellation
        .run_until_cancelled(command_receiver.recv())
        .await
        .flatten()
    {
        match command {
            OutletCommand::Send {
                placement,
                payload,
                reply,
            } => {
                let send = send(&mut writer, placement, payload);
                let Some(result) = cancellation.run_until_cancelled(send).await else {
                    break;
                };
                let _ = reply.send(result);
            }
            OutletCommand::LeaveGroup => writer.abandon_group(),
            OutletCommand::End { code, reason, done } => {
                let _ = done.send(end(writer, &publisher, &subscription, code, reason).await);
                return;
            }
        }
    }
    writer.cancel().await;
}

async fn send(writer: &mut TrackWriter, placement: Placement, payload: Payload) -> Result<bool> {
    match payload {
        Payload::Object(object) => send_placed(writer, placement, object).await,
        Payload::Group(payload) => {
            writer
                .write_group_at(placement.location.group_id, payload)
                .await
                .context("send group")?;
            Ok(true)
        }
    }
}

/// A writer that has sent nothing joins the group the object is in, and so
/// does one that left the group in Forward State 0; one whose group a peer
/// stopped or the transport reset resumes at the next group.
async fn send_placed(
    writer: &mut TrackWriter,
    placement: Placement,
    object: OutgoingObject,
) -> Result<bool> {
    let Location {
        group_id,
        object_id,
    } = placement.location;
    if placement.starts_group {
        writer
            .start_group_at(group_id)
            .await
            .context("start group")?;
    } else if writer.abandoned_group_id() == Some(group_id) {
        writer
            .rejoin_group_at(object_id)
            .await
            .context("rejoin group")?;
    } else if writer.current_group_id() != Some(group_id) {
        if writer.groups() > 0 {
            return Ok(false);
        }
        writer
            .join_group_at(group_id, object_id)
            .await
            .context("join group")?;
    }
    writer
        .write_with_extension_headers(object.payload, object.extension_headers)
        .await
        .context("send subgroup object")?;
    Ok(true)
}

/// Finishes the open group before PUBLISH_DONE, which draft-14 §9.12 forbids
/// until every stream of the subscription is closed.
async fn end(
    writer: TrackWriter,
    publisher: &Publisher,
    subscription: &Subscription,
    code: PublishDoneCode,
    reason: &'static str,
) -> Result<()> {
    let opened_streams = writer.opened_streams();
    if let Err(error) = writer.finish().await {
        tracing::debug!(?error, "open group could not be finished");
    }
    publisher
        .publish_done(subscription, code, opened_streams, reason.to_string())
        .await
}
