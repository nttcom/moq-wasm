use std::{collections::HashMap, sync::Arc};

use anyhow::Result;
use async_trait::async_trait;
use moqt_core::messages::moqt_payload::MOQTPayload;
use moqt_core::StreamManagerRepository;
use tokio::sync::{mpsc, oneshot};

type MoqtMessageForwarder = mpsc::Sender<Arc<Box<dyn MOQTPayload>>>;

use StreamCommand::*;
// Called as a separate thread
pub(crate) async fn stream_manager(rx: &mut mpsc::Receiver<StreamCommand>) {
    tracing::info!("stream_manager start");
    // {
    //   "${session_id}" : {
    //     "unidirecional_stream" : tx,
    //     "bidirectional_stream" : tx,
    //   }
    // }
    let mut streams = HashMap::<usize, HashMap<String, MoqtMessageForwarder>>::new();

    while let Some(cmd) = rx.recv().await {
        tracing::info!("command received");
        match cmd {
            Set {
                session_id,
                stream_type,
                sender,
            } => {
                let inner_map = streams.entry(session_id).or_default();
                inner_map.insert(stream_type.to_string(), sender);
            }
            List {
                stream_type,
                exclude_session_id,
                resp,
            } => {
                let mut senders = Vec::new();
                for (session_id, inner_map) in &streams {
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
                let sender = streams
                    .get(&session_id)
                    .and_then(|inner_map| inner_map.get(&stream_type))
                    .cloned();
                let _ = resp.send(sender);
            }
        }
    }
}

#[derive(Debug)]
pub(crate) enum StreamCommand {
    Set {
        session_id: usize,
        stream_type: String,
        sender: MoqtMessageForwarder,
    },
    List {
        stream_type: String,
        exclude_session_id: Option<usize>, // 現在はListはbroadcastにしか利用されないため、exclude_session_idを指定する
        resp: oneshot::Sender<Vec<MoqtMessageForwarder>>,
    },
    Get {
        session_id: usize,
        stream_type: String,
        resp: oneshot::Sender<Option<MoqtMessageForwarder>>,
    },
}

pub(crate) struct StreamManager {
    tx: mpsc::Sender<StreamCommand>,
}

impl StreamManager {
    pub fn new(tx: mpsc::Sender<StreamCommand>) -> Self {
        Self { tx }
    }
}

#[async_trait]
impl StreamManagerRepository for StreamManager {
    // TODO: SUBSCRIBE・OBJECTメッセージの際に関数名を修正する
    async fn broadcast_message(
        &self,
        session_id: Option<usize>,
        message: Box<dyn MOQTPayload>,
    ) -> Result<()> {
        let (resp_tx, resp_rx) = oneshot::channel::<Vec<MoqtMessageForwarder>>();
        let cmd = StreamCommand::List {
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
    async fn relay_message(&self, session_id: usize, message: Box<dyn MOQTPayload>) -> Result<()> {
        let (resp_tx, resp_rx) = oneshot::channel::<Option<MoqtMessageForwarder>>();
        let cmd = StreamCommand::Get {
            session_id,
            stream_type: "bidirectional_stream".to_string(),
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
