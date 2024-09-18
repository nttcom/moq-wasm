use std::{collections::HashMap, sync::Arc};

use anyhow::Result;
use async_trait::async_trait;
use moqt_core::{
    messages::moqt_payload::MOQTPayload, stream_type::StreamType, SendStreamDispatcherRepository,
};
use tokio::sync::{mpsc, oneshot};

type SenderToSendStreamThread = mpsc::Sender<Arc<Box<dyn MOQTPayload>>>;

use SendStreamDispatchCommand::*;
// Called as a separate thread
pub(crate) async fn send_stream_dispatcher(rx: &mut mpsc::Receiver<SendStreamDispatchCommand>) {
    tracing::trace!("send_stream_dispatcher start");
    // {
    //   "${session_id}" : {
    //     "unidirectional_stream" : tx,
    //     "bidirectional_stream" : tx,
    //   }
    // }
    let mut dispatcher = HashMap::<usize, HashMap<String, SenderToSendStreamThread>>::new();

    while let Some(cmd) = rx.recv().await {
        tracing::debug!("command received: {:#?}", cmd);
        match cmd {
            Set {
                session_id,
                stream_type,
                sender,
            } => {
                let inner_map = dispatcher.entry(session_id).or_default();
                inner_map.insert(stream_type.to_string(), sender);
                tracing::debug!("set: {:?} of {:?}", stream_type, session_id);
            }
            List {
                stream_type,
                exclude_session_id,
                resp,
            } => {
                let mut senders = Vec::new();
                for (session_id, inner_map) in &dispatcher {
                    if let Some(exclude_session_id) = exclude_session_id {
                        if *session_id == exclude_session_id {
                            continue;
                        }
                    }
                    if let Some(sender) = inner_map.get(&stream_type) {
                        senders.push(sender.clone());
                    }
                }
                let _ = resp.send(senders);
            }
            Get {
                session_id,
                stream_type,
                resp,
            } => {
                let sender = dispatcher
                    .get(&session_id)
                    .and_then(|inner_map| inner_map.get(&stream_type))
                    .cloned();
                tracing::debug!("get: {:?}", sender);
                let _ = resp.send(sender);
            }
            Delete { session_id } => {
                dispatcher.remove(&session_id);
                tracing::debug!("delete: {:?}", session_id);
            }
        }
    }

    tracing::trace!("send_stream_dispatcher end");
}

#[derive(Debug)]
pub(crate) enum SendStreamDispatchCommand {
    Set {
        session_id: usize,
        stream_type: String,
        sender: SenderToSendStreamThread,
    },
    List {
        stream_type: String,
        exclude_session_id: Option<usize>, // Currently, exclude_session_id is only used in broadcast for List
        resp: oneshot::Sender<Vec<SenderToSendStreamThread>>,
    },
    Get {
        session_id: usize,
        stream_type: String,
        resp: oneshot::Sender<Option<SenderToSendStreamThread>>,
    },
    Delete {
        session_id: usize,
    },
}

pub(crate) struct SendStreamDispatcher {
    tx: mpsc::Sender<SendStreamDispatchCommand>,
}

impl SendStreamDispatcher {
    pub fn new(tx: mpsc::Sender<SendStreamDispatchCommand>) -> Self {
        Self { tx }
    }
}

#[async_trait]
impl SendStreamDispatcherRepository for SendStreamDispatcher {
    async fn broadcast_message_to_send_stream_threads(
        &self,
        session_id: Option<usize>,
        message: Box<dyn MOQTPayload>,
    ) -> Result<()> {
        let (resp_tx, resp_rx) = oneshot::channel::<Vec<SenderToSendStreamThread>>();
        let cmd = SendStreamDispatchCommand::List {
            stream_type: "bidirectional_stream".to_string(),
            exclude_session_id: session_id,
            resp: resp_tx,
        };
        self.tx.send(cmd).await.unwrap();

        let senders = resp_rx.await?;
        let message_arc = Arc::new(message);
        for sender in senders {
            let message_arc_clone = Arc::clone(&message_arc);
            let _ = sender.send(message_arc_clone).await;
        }
        Ok(())
    }
    async fn send_message_to_send_stream_thread(
        &self,
        session_id: usize,
        message: Box<dyn MOQTPayload>,
        stream_type: StreamType,
    ) -> Result<()> {
        let (resp_tx, resp_rx) = oneshot::channel::<Option<SenderToSendStreamThread>>();

        let stream_type_str = match stream_type {
            StreamType::Uni => "unidirectional_stream",
            StreamType::Bi => "bidirectional_stream",
        };
        let cmd = SendStreamDispatchCommand::Get {
            session_id,
            stream_type: stream_type_str.to_string(),
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
