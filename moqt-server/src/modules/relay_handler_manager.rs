use std::{collections::HashMap, sync::Arc};

use anyhow::Result;
use async_trait::async_trait;
use moqt_core::message_handler::StreamType;
use moqt_core::messages::moqt_payload::MOQTPayload;
use moqt_core::RelayHandlerManagerRepository;
use tokio::sync::{mpsc, oneshot};

type MoqtRelayHandler = mpsc::Sender<Arc<Box<dyn MOQTPayload>>>;

use RelayHandlerCommand::*;
// Called as a separate thread
pub(crate) async fn relay_handler_manager(rx: &mut mpsc::Receiver<RelayHandlerCommand>) {
    tracing::info!("relay_handler_manager start");
    // {
    //   "${session_id}" : {
    //     "unidirectional_stream" : tx,
    //     "bidirectional_stream" : tx,
    //   }
    // }
    let mut relay_handlers = HashMap::<usize, HashMap<String, MoqtRelayHandler>>::new();

    while let Some(cmd) = rx.recv().await {
        tracing::info!("command received");
        match cmd {
            Set {
                session_id,
                stream_type,
                sender,
            } => {
                let inner_map = relay_handlers.entry(session_id).or_default();
                inner_map.insert(stream_type.to_string(), sender);
                tracing::info!("set: {:?}", relay_handlers);
            }
            List {
                stream_type,
                exclude_session_id,
                resp,
            } => {
                let mut senders = Vec::new();
                for (session_id, inner_map) in &relay_handlers {
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
                let sender = relay_handlers
                    .get(&session_id)
                    .and_then(|inner_map| inner_map.get(&stream_type))
                    .cloned();
                tracing::info!("get: {:?}", sender);
                let _ = resp.send(sender);
            }
        }
    }
}

#[derive(Debug)]
pub(crate) enum RelayHandlerCommand {
    Set {
        session_id: usize,
        stream_type: String,
        sender: MoqtRelayHandler,
    },
    List {
        stream_type: String,
        exclude_session_id: Option<usize>, // Currently, exclude_session_id is only used in broadcast for List
        resp: oneshot::Sender<Vec<MoqtRelayHandler>>,
    },
    Get {
        session_id: usize,
        stream_type: String,
        resp: oneshot::Sender<Option<MoqtRelayHandler>>,
    },
}

pub(crate) struct RelayHandlerManager {
    tx: mpsc::Sender<RelayHandlerCommand>,
}

impl RelayHandlerManager {
    pub fn new(tx: mpsc::Sender<RelayHandlerCommand>) -> Self {
        Self { tx }
    }
}

#[async_trait]
impl RelayHandlerManagerRepository for RelayHandlerManager {
    async fn broadcast_message_to_relay_handlers(
        &self,
        session_id: Option<usize>,
        message: Box<dyn MOQTPayload>,
    ) -> Result<()> {
        let (resp_tx, resp_rx) = oneshot::channel::<Vec<MoqtRelayHandler>>();
        let cmd = RelayHandlerCommand::List {
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
    async fn send_message_to_relay_handler(
        &self,
        session_id: usize,
        message: Box<dyn MOQTPayload>,
        stream_type: StreamType,
    ) -> Result<()> {
        let (resp_tx, resp_rx) = oneshot::channel::<Option<MoqtRelayHandler>>();

        let stream_type_str = match stream_type {
            StreamType::Uni => "unidirectional_stream",
            StreamType::Bi => "bidirectional_stream",
        };
        let cmd = RelayHandlerCommand::Get {
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
