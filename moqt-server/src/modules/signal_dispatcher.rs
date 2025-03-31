use anyhow::Result;
use moqt_core::messages::data_streams::object_status::ObjectStatus;
use std::collections::HashMap;
use tokio::sync::{mpsc, oneshot};
type SenderToDataStreamThread = mpsc::Sender<Box<DataStreamThreadSignal>>;

#[derive(Debug, Clone)]
pub(crate) enum DataStreamThreadSignal {
    Terminate(TerminateReason),
}

#[derive(Debug, Clone)]
#[allow(dead_code)]
pub(crate) enum TerminateReason {
    ObjectStatus(ObjectStatus),
    SessionClosed,
}

#[derive(Debug)]
pub(crate) enum SignalDispatchCommand {
    Set {
        session_id: usize,
        stream_id: u64,
        sender: SenderToDataStreamThread,
    },
    Get {
        session_id: usize,
        stream_id: u64,
        resp: oneshot::Sender<Option<SenderToDataStreamThread>>,
    },
    Delete {
        session_id: usize,
    },
}

pub(crate) async fn signal_dispatcher(rx: &mut mpsc::Receiver<SignalDispatchCommand>) {
    tracing::trace!("signal_dispatcher start");
    // {
    //   "${session_id}" : {
    //     "${stream_id}"
    //       tx
    //   }
    // }
    let mut dispatcher = HashMap::<usize, HashMap<u64, SenderToDataStreamThread>>::new();

    while let Some(cmd) = rx.recv().await {
        tracing::debug!("command received: {:#?}", cmd);
        match cmd {
            SignalDispatchCommand::Set {
                session_id,
                stream_id,
                sender,
            } => {
                let inner_map = dispatcher.entry(session_id).or_default();
                inner_map.insert(stream_id, sender);
                tracing::debug!("set: {:?}", session_id);
            }
            SignalDispatchCommand::Get {
                session_id,
                stream_id,
                resp,
            } => {
                let sender = dispatcher
                    .get(&session_id)
                    .and_then(|inner_map| inner_map.get(&stream_id).cloned());

                tracing::debug!("get: {:?}", sender);
                let _ = resp.send(sender);
            }
            SignalDispatchCommand::Delete { session_id } => {
                dispatcher.remove(&session_id);
                tracing::debug!("delete: {:?}", session_id);
            }
        }
    }

    tracing::trace!("signal_dispatcher end");
}

#[derive(Clone)]
pub(crate) struct SignalDispatcher {
    tx: mpsc::Sender<SignalDispatchCommand>,
}

impl SignalDispatcher {
    pub fn new(tx: mpsc::Sender<SignalDispatchCommand>) -> Self {
        Self { tx }
    }
}

impl SignalDispatcher {
    pub(crate) async fn transfer_signal_to_data_stream_thread(
        &self,
        session_id: usize,
        stream_id: u64,
        signal: Box<DataStreamThreadSignal>,
    ) -> Result<()> {
        let (resp_tx, resp_rx) = oneshot::channel::<Option<SenderToDataStreamThread>>();

        let cmd = SignalDispatchCommand::Get {
            session_id,
            stream_id,
            resp: resp_tx,
        };
        self.tx.send(cmd).await.unwrap();

        let sender = resp_rx
            .await?
            .ok_or_else(|| anyhow::anyhow!("sender not found"))?;
        let _ = sender.send(signal).await;
        Ok(())
    }
}
