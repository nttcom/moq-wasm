use std::sync::{mpsc::Sender, Arc};

use anyhow::{bail, Context, Result};
use moqt::{
    object::{DataReceiver, GroupOrder},
    publish::PublishHandler,
    subscribe::{FilterType, SubscribeOption},
    ClientConfig, Endpoint, Session, SessionEvent, Subscriber, Subscription, TrackReader, DUAL,
};
use tokio::task::JoinHandle;

use crate::ptz_worker::Command;

const NO_EXPIRY: u64 = 0;

pub struct CommandTrack {
    pub namespace: String,
    pub track: String,
    pub subscriber_priority: u8,
}

/// Follows the viewer's command track on a session of its own, separate from
/// the media broadcast, and forwards each command to the PTZ worker. The
/// viewer announces the namespace or publishes the track; either way the
/// track is subscribed again whenever a new viewer takes over.
pub struct CommandSubscriber {
    _endpoint: Endpoint<DUAL>,
    task: JoinHandle<Result<()>>,
}

impl CommandSubscriber {
    pub async fn run(
        url: &str,
        client_config: &ClientConfig,
        command_track: CommandTrack,
        command_sender: Sender<Command>,
    ) -> Result<Self> {
        let endpoint = Endpoint::<DUAL>::create_client(client_config)?;
        let connecting = endpoint
            .connect(url)
            .await
            .context("connect command session")?;
        let session = Arc::new(connecting.await.context("establish command session")?);
        session
            .subscriber()
            .subscribe_namespace(command_track.namespace.clone())
            .await
            .context("subscribe command namespace")?;
        log::info!(
            "Subscribed command namespace for late join viewers: {}",
            command_track.namespace
        );
        let task = tokio::spawn(follow_commands(session, command_track, command_sender));
        Ok(Self {
            _endpoint: endpoint,
            task,
        })
    }

    /// Resolves only with an error: the command session disconnected or a
    /// control message failed.
    pub async fn finished(&mut self) -> Result<()> {
        (&mut self.task).await.context("command session task")?
    }
}

impl Drop for CommandSubscriber {
    fn drop(&mut self) {
        self.task.abort();
    }
}

async fn follow_commands(
    session: Arc<Session>,
    command_track: CommandTrack,
    command_sender: Sender<Command>,
) -> Result<()> {
    let mut command_receiver: Option<CommandReceiver> = None;
    loop {
        match session.receive_event().await? {
            SessionEvent::PublishNamespace(handler) => {
                let announced_namespace = handler.track_namespace.clone();
                handler.ok().await?;
                log::info!("Command namespace announced: {announced_namespace}");
                if announced_namespace != command_track.namespace
                    || command_receiver
                        .as_ref()
                        .is_some_and(CommandReceiver::is_running)
                {
                    continue;
                }
                command_receiver =
                    Some(subscribe_command_track(&session, &command_track, &command_sender).await?);
            }
            SessionEvent::Publish(handler) => {
                if let Some(receiver) = accept_command_publish(
                    handler,
                    &session,
                    &command_track,
                    &command_receiver,
                    &command_sender,
                )
                .await?
                {
                    command_receiver = Some(receiver);
                }
            }
            SessionEvent::PublishDone(handler) => {
                if command_receiver
                    .as_ref()
                    .is_some_and(|receiver| receiver.request_id == handler.request_id())
                {
                    log::info!(
                        "command publisher finished; waiting for the next command publisher"
                    );
                    command_receiver = None;
                }
            }
            SessionEvent::Subscribe(handler) => {
                let _ = handler
                    .error(0x4, "no tracks on the command session".to_string())
                    .await;
            }
            SessionEvent::Disconnected() => bail!("command session disconnected"),
            SessionEvent::ProtocolViolation() => bail!("command session protocol violation"),
            event => log::debug!("Unhandled command session message: {event:?}"),
        }
    }
}

async fn subscribe_command_track(
    session: &Session,
    command_track: &CommandTrack,
    command_sender: &Sender<Command>,
) -> Result<CommandReceiver> {
    let mut subscriber = session.subscriber();
    let subscription = subscriber
        .subscribe(
            command_track.namespace.clone(),
            command_track.track.clone(),
            SubscribeOption {
                subscriber_priority: command_track.subscriber_priority,
                group_order: GroupOrder::Ascending,
                forward: true,
                filter_type: FilterType::NextGroupStart,
            },
        )
        .await
        .with_context(|| {
            format!(
                "subscribe command track after namespace announce: {}/{}",
                command_track.namespace, command_track.track
            )
        })?;
    log::info!(
        "Command track subscribed after namespace announce: ns={} track={} alias={}",
        command_track.namespace,
        command_track.track,
        subscription.track_alias()
    );
    Ok(CommandReceiver::run(
        subscriber,
        subscription,
        command_sender.clone(),
    ))
}

async fn accept_command_publish(
    handler: PublishHandler,
    session: &Session,
    command_track: &CommandTrack,
    command_receiver: &Option<CommandReceiver>,
    command_sender: &Sender<Command>,
) -> Result<Option<CommandReceiver>> {
    if handler.track_namespace != command_track.namespace
        || handler.track_name != command_track.track
    {
        log::warn!(
            "Ignoring unexpected command publish: ns={} track={}",
            handler.track_namespace,
            handler.track_name
        );
        let _ = handler
            .error(0x4, "unsupported publish track".to_string())
            .await;
        return Ok(None);
    }
    if command_receiver
        .as_ref()
        .is_some_and(CommandReceiver::is_running)
    {
        log::info!(
            "Command publish received after subscription already active: ns={} track={}",
            handler.track_namespace,
            handler.track_name
        );
        let _ = handler
            .error(0x4, "command track already active".to_string())
            .await;
        return Ok(None);
    }
    let subscription = handler
        .ok(
            command_track.subscriber_priority,
            FilterType::NextGroupStart,
            NO_EXPIRY,
        )
        .await
        .context("accept command publish")?;
    log::info!(
        "Accepted command publish via SUBSCRIBE_NAMESPACE discovery: ns={} track={} alias={}",
        command_track.namespace,
        command_track.track,
        subscription.track_alias()
    );
    Ok(Some(CommandReceiver::run(
        session.subscriber(),
        subscription,
        command_sender.clone(),
    )))
}

struct CommandReceiver {
    request_id: u64,
    join_handle: JoinHandle<()>,
}

impl CommandReceiver {
    fn run(
        mut subscriber: Subscriber,
        subscription: Subscription,
        command_sender: Sender<Command>,
    ) -> Self {
        let request_id = subscription.request_id();
        let join_handle = tokio::spawn(async move {
            let receiver = match subscriber.accept_data_receiver(&subscription).await {
                Ok(receiver) => receiver,
                Err(err) => {
                    log::warn!("command accept_data_receiver failed: {err}");
                    return;
                }
            };
            let DataReceiver::Stream(factory) = receiver else {
                log::warn!("command track is not delivered on subgroup streams");
                return;
            };
            let mut reader = TrackReader::new(factory);
            loop {
                match reader.next_object().await {
                    Ok(Some(object)) => {
                        if let Err(err) = handle_command_payload(&object.payload, &command_sender) {
                            log::warn!("command payload error: {err}");
                        }
                    }
                    Ok(None) => {
                        log::info!("command track ended; waiting for the next command publisher");
                        return;
                    }
                    Err(err) => log::warn!("command subgroup receive failed: {err}"),
                }
            }
        });
        Self {
            request_id,
            join_handle,
        }
    }

    fn is_running(&self) -> bool {
        !self.join_handle.is_finished()
    }
}

impl Drop for CommandReceiver {
    fn drop(&mut self) {
        self.join_handle.abort();
    }
}

fn handle_command_payload(payload: &[u8], command_sender: &Sender<Command>) -> Result<()> {
    let command: Command = serde_json::from_slice(payload).context("parse command json")?;
    log::info!("PTZ command received: {command:?}");
    let _ = command_sender.send(command);
    Ok(())
}
