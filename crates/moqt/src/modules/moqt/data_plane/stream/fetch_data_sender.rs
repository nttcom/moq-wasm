use std::sync::{
    Arc, Weak,
    atomic::{AtomicBool, Ordering},
};

use crate::modules::executor;
use crate::modules::moqt::control_plane::error_codes::DataStreamResetCode;
use crate::modules::moqt::{
    data_plane::{object::canonical_object::Object, stream::stream_sender::StreamSender},
    domains::connection::Connection,
    version::{FetchEncoder, Version},
};
use crate::modules::transport::transport_connection::BoxedSendStream;

/// Dropped without `close` or `reset`, the response is incomplete, so it is
/// reset with INTERNAL_ERROR.
pub struct FetchDataSender {
    version: Version,
    stream_sender: Arc<StreamSender>,
    /// Held across the send, so objects reach the stream in the order they
    /// were encoded.
    encoder: tokio::sync::Mutex<FetchEncoder>,
    connection: Weak<Connection>,
    /// The FETCH's Request ID on `connection`.
    request_id: u64,
    /// Set by `close` and `reset`.
    ended: AtomicBool,
}

impl FetchDataSender {
    // On wasm32 the browser transport is !Send, but the sender is shared with
    // the native build, where the tasks run on a multi-threaded executor.
    #[cfg_attr(target_arch = "wasm32", allow(clippy::arc_with_non_send_sync))]
    pub(crate) async fn new(
        connection: &Arc<Connection>,
        send_stream: BoxedSendStream,
        request_id: u64,
    ) -> anyhow::Result<Self> {
        let version = connection.control.version();
        let stream_sender = Arc::new(StreamSender::new(send_stream));
        stream_sender
            .send(&version.encode_fetch_header(request_id))
            .await?;
        Ok(Self {
            version,
            stream_sender,
            encoder: tokio::sync::Mutex::new(FetchEncoder::new(version)),
            connection: Arc::downgrade(connection),
            request_id,
            ended: AtomicBool::new(false),
        })
    }

    /// Sends `object` on this FETCH response. draft-14 sends a datagram
    /// object with its Object ID as the Subgroup ID (§10.4.4) and a
    /// zero-length object as its status; draft-18 carries only Normal
    /// objects and skips the others.
    pub async fn send_object(&self, object: &Object) -> anyhow::Result<()> {
        let mut encoder = self.encoder.lock().await;
        match encoder.object(object)? {
            Some(bytes) => Ok(self.stream_sender.send(&bytes).await?),
            None => Ok(()),
        }
    }

    /// Ends the response with FIN: every object of the range this side
    /// knows was sent.
    pub async fn close(&self) -> anyhow::Result<()> {
        self.ended.store(true, Ordering::Relaxed);
        self.stream_sender.close().await?;
        response_ended(&self.connection, self.request_id).await
    }

    pub async fn reset(&self, code: DataStreamResetCode) -> anyhow::Result<()> {
        self.ended.store(true, Ordering::Relaxed);
        self.stream_sender
            .reset(self.version.data_stream_reset_code(code))
            .await?;
        response_ended(&self.connection, self.request_id).await
    }
}

impl Drop for FetchDataSender {
    fn drop(&mut self) {
        if self.ended.load(Ordering::Relaxed) {
            return;
        }
        let stream_sender = self.stream_sender.clone();
        let code = self
            .version
            .data_stream_reset_code(DataStreamResetCode::InternalError);
        let connection = self.connection.clone();
        let request_id = self.request_id;
        executor::try_spawn("reset an unfinished FETCH response", async move {
            if let Err(error) = stream_sender.reset(code).await {
                tracing::debug!(%error, "could not reset an unfinished FETCH response");
            }
            if let Err(error) = response_ended(&connection, request_id).await {
                tracing::debug!(%error, "could not end the request of a FETCH response");
            }
        });
    }
}

async fn response_ended(connection: &Weak<Connection>, request_id: u64) -> anyhow::Result<()> {
    if let Some(connection) = connection.upgrade() {
        connection.control.fetch_response_ended(request_id).await?;
    }
    Ok(())
}
