use std::{collections::HashMap, sync::Arc};

use bytes::{buf, BytesMut};
use tokio::sync::{mpsc, oneshot, Mutex};

pub(crate) async fn buffer_manager(rx: &mut mpsc::Receiver<BufferCommand>) {
    tracing::info!("buffer_manager start");

    let mut buffers = HashMap::<usize, HashMap<u64, BufferType>>::new();

    use BufferCommand::*;
    while let Some(cmd) = rx.recv().await {
        tracing::info!("command received");
        match cmd {
            Get {
                session_id,
                stream_id,
                resp,
            } => {
                let mut session_hash = buffers.get_mut(&session_id);
                if let None = session_hash {
                    buffers.insert(session_id, HashMap::<u64, Arc<Mutex<BytesMut>>>::new());
                    session_hash = buffers.get_mut(&session_id);
                }
                let session_hash = session_hash.unwrap();

                let mut stream_hash = session_hash.get(&stream_id);
                if let None = stream_hash {
                    session_hash.insert(stream_id, Arc::new(Mutex::new(BytesMut::new())));
                    stream_hash = session_hash.get(&stream_id);
                }
                let stream_hash = stream_hash.unwrap();

                let buf = stream_hash.clone();

                let _ = resp.send(buf);
            }
            ReleaseStream {
                session_id,
                stream_id,
            } => {
                if buffers.contains_key(&session_id) {
                    buffers.get_mut(&session_id).unwrap().remove(&stream_id);
                }
            }
            ReleaseSession { session_id } => {
                buffers.remove(&session_id);
            }
        }
    }

    tracing::info!("buffer_manager end");
}

#[derive(Debug)]
pub(crate) enum BufferCommand {
    Get {
        session_id: usize,
        stream_id: u64,
        resp: oneshot::Sender<BufferType>,
    },
    ReleaseStream {
        session_id: usize,
        stream_id: u64,
    },
    ReleaseSession {
        session_id: usize,
    },
}

type BufferType = Arc<Mutex<BytesMut>>;

pub(crate) async fn request_buffer(
    tx: mpsc::Sender<BufferCommand>,
    session_id: usize,
    stream_id: u64,
) -> BufferType {
    tracing::info!("request_buffer start");

    let (resp_tx, resp_rx) = oneshot::channel::<BufferType>();

    let cmd = BufferCommand::Get {
        session_id,
        stream_id,
        resp: resp_tx,
    };
    tx.send(cmd).await.unwrap();

    let buf = resp_rx.await.unwrap();

    tracing::info!("request_buffer end");

    buf
}

pub(crate) struct PayloadBuffer {
    buffer: BufferType,
    session_id: usize,
    stream_id: u64,
    tx: mpsc::Sender<BufferCommand>,
}

impl PayloadBuffer {
    async fn new(tx: mpsc::Sender<BufferCommand>, session_id: usize, stream_id: u64) -> Self {
        let (resp_tx, resp_rx) = oneshot::channel::<BufferType>();

        let cmd = BufferCommand::Get {
            session_id,
            stream_id,
            resp: resp_tx,
        };
        tx.send(cmd).await.unwrap();

        let buf = resp_rx.await.unwrap();

        PayloadBuffer {
            buffer: buf,
            session_id,
            stream_id,
            tx,
        }
    }

    pub(crate) fn buffer(&self) -> BufferType {
        self.buffer.clone()
    }
}
