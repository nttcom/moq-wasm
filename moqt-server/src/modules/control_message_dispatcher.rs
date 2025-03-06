use anyhow::Result;
use moqt_core::messages::moqt_payload::MOQTPayload;
use std::{collections::HashMap, sync::Arc};
use tokio::sync::{mpsc, oneshot};
type SenderToControlMessageSenderThread = mpsc::Sender<Arc<Box<dyn MOQTPayload>>>;

#[derive(Debug)]
pub(crate) enum ControlMessageDispatchCommand {
    Set {
        session_id: usize,
        sender: SenderToControlMessageSenderThread,
    },
    Get {
        session_id: usize,
        resp: oneshot::Sender<Option<SenderToControlMessageSenderThread>>,
    },
    Delete {
        session_id: usize,
    },
}

pub(crate) async fn control_message_dispatcher(
    rx: &mut mpsc::Receiver<ControlMessageDispatchCommand>,
) {
    tracing::trace!("control_message_dispatcher start");
    // {
    //   "${session_id}" : tx,
    //   }
    // }
    let mut dispatcher = HashMap::<usize, SenderToControlMessageSenderThread>::new();

    while let Some(cmd) = rx.recv().await {
        tracing::debug!("command received: {:#?}", cmd);
        match cmd {
            ControlMessageDispatchCommand::Set { session_id, sender } => {
                dispatcher.insert(session_id, sender);
                tracing::debug!("set: {:?}", session_id);
            }
            ControlMessageDispatchCommand::Get { session_id, resp } => {
                let sender = dispatcher.get(&session_id).cloned();
                tracing::debug!("get: {:?}", sender);
                let _ = resp.send(sender);
            }
            ControlMessageDispatchCommand::Delete { session_id } => {
                dispatcher.remove(&session_id);
                tracing::debug!("delete: {:?}", session_id);
            }
        }
    }

    tracing::trace!("control_message_dispatcher end");
}

#[derive(Clone)]
pub(crate) struct ControlMessageDispatcher {
    tx: mpsc::Sender<ControlMessageDispatchCommand>,
}

impl ControlMessageDispatcher {
    pub fn new(tx: mpsc::Sender<ControlMessageDispatchCommand>) -> Self {
        Self { tx }
    }

    // Used for testing in unsubscribe_handler
    #[allow(dead_code)]
    pub fn get_tx(&self) -> mpsc::Sender<ControlMessageDispatchCommand> {
        self.tx.clone()
    }
}

impl ControlMessageDispatcher {
    pub(crate) async fn transfer_message_to_control_message_sender_thread(
        &self,
        session_id: usize,
        message: Box<dyn MOQTPayload>,
    ) -> Result<()> {
        let (resp_tx, resp_rx) = oneshot::channel::<Option<SenderToControlMessageSenderThread>>();

        let cmd = ControlMessageDispatchCommand::Get {
            session_id,
            resp: resp_tx,
        };
        self.tx.send(cmd).await.unwrap();

        let sender = resp_rx
            .await?
            .ok_or_else(|| anyhow::anyhow!("sender not found"))?;
        let message_arc = Arc::new(message);
        let _ = sender.send(message_arc).await;
        Ok(())
    }
}
