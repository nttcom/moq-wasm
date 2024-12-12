use bytes::BytesMut;
use std::{collections::HashMap, sync::Arc};
use tokio::sync::{mpsc, oneshot, Mutex};

// Called as a separate thread
pub(crate) async fn buffer_manager(rx: &mut mpsc::Receiver<BufferCommand>) {
    tracing::trace!("buffer_manager start");

    // {
    //   "${session_id}" : {
    //     "${stream_id}" : buffer
    //   }
    // }
    let mut buffers = HashMap::<usize, HashMap<u64, BufferType>>::new();

    while let Some(cmd) = rx.recv().await {
        tracing::debug!("command received: {:#?}", cmd);
        match cmd {
            BufferCommand::Get {
                session_id,
                stream_id,
                resp,
            } => {
                let mut session_hash = buffers.get_mut(&session_id);
                // Create empty hashmap if not exists
                if session_hash.is_none() {
                    buffers.insert(session_id, HashMap::<u64, Arc<Mutex<BytesMut>>>::new());
                    session_hash = buffers.get_mut(&session_id);
                }
                let session_hash = session_hash.unwrap();

                let mut stream_hash = session_hash.get(&stream_id);
                // Create empty buffer if not exists
                if stream_hash.is_none() {
                    session_hash.insert(stream_id, Arc::new(Mutex::new(BytesMut::new())));
                    stream_hash = session_hash.get(&stream_id);
                }
                let stream_hash = stream_hash.unwrap();

                let buf = stream_hash.clone();

                // response the buffer
                let _ = resp.send(buf);
            }
            BufferCommand::ReleaseStream {
                session_id,
                stream_id,
            } => {
                if buffers.contains_key(&session_id) {
                    buffers.get_mut(&session_id).unwrap().remove(&stream_id);
                }
            }
            BufferCommand::ReleaseSession { session_id } => {
                buffers.remove(&session_id);
            }
        }
    }

    tracing::trace!("buffer_manager end");
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

// Wrapper to encapsulate the channel handling for buffer requests
pub(crate) async fn request_buffer(
    tx: mpsc::Sender<BufferCommand>,
    session_id: usize,
    stream_id: u64,
) -> BufferType {
    tracing::trace!("request_buffer start");

    let (resp_tx, resp_rx) = oneshot::channel::<BufferType>();

    let cmd = BufferCommand::Get {
        session_id,
        stream_id,
        resp: resp_tx,
    };
    tx.send(cmd).await.unwrap();

    let buf = resp_rx.await.unwrap();

    tracing::trace!("request_buffer end");

    buf
}
