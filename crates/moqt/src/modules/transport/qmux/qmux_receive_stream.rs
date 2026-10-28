use std::{
    collections::VecDeque,
    sync::{Arc, Mutex},
    task::{Context, Poll},
};

use bytes::{Buf, Bytes};
use futures::task::AtomicWaker;
use tokio::io::ReadBuf;

use super::{
    frame::{Frame, ProtocolError, TransportErrorCode},
    session_state::{Command, SessionState},
    stream_id::StreamKind,
};
use crate::modules::transport::{
    read_error::ReadError, transport_receive_stream::TransportReceiveStream,
};

/// The receiving state of one stream, shared by the receive task, which
/// appends what the peer sends, and the handle, which reads it.
pub(crate) struct ReceiveShared {
    state: Mutex<ReceiveState>,
    waker: AtomicWaker,
}

struct ReceiveState {
    chunks: VecDeque<Bytes>,
    /// Bytes the peer has sent so far: the next STREAM frame's offset.
    received: u64,
    /// Bytes the application has read, or discarded after it dropped the handle.
    consumed: u64,
    /// The MAX_STREAM_DATA value the peer currently knows.
    advertised: u64,
    window: u64,
    final_size: Option<u64>,
    reset_code: Option<u64>,
    connection_lost: bool,
    /// The handle was dropped: data is discarded, credit is still returned.
    abandoned: bool,
}

/// What a STREAM frame or RESET_STREAM did to the stream.
pub(crate) struct Received {
    /// Bytes the peer newly sent, against the connection window.
    pub(crate) counted: u64,
    /// Bytes nobody will read, whose connection credit goes back at once.
    pub(crate) discarded: u64,
    /// The stream reached its final size: FIN received or the peer reset it.
    pub(crate) ended: bool,
    /// The stream ended after the application dropped its handle, so its
    /// MAX_STREAMS slot is freed now rather than in `Drop`.
    pub(crate) freed: bool,
}

impl ReceiveShared {
    pub(crate) fn new(window: u64) -> Self {
        Self {
            state: Mutex::new(ReceiveState {
                chunks: VecDeque::new(),
                received: 0,
                consumed: 0,
                advertised: window,
                window,
                final_size: None,
                reset_code: None,
                connection_lost: false,
                abandoned: false,
            }),
            waker: AtomicWaker::new(),
        }
    }

    pub(crate) fn push(
        &self,
        offset: u64,
        data: Bytes,
        fin: bool,
    ) -> Result<Received, ProtocolError> {
        let mut state = self.state.lock().expect("receive stream lock");
        if offset != state.received {
            return Err(ProtocolError::new(
                TransportErrorCode::ProtocolViolation,
                format!(
                    "STREAM data at offset {offset} does not follow the {} bytes received",
                    state.received
                ),
            ));
        }
        let end = offset + data.len() as u64;
        if let Some(final_size) = state.final_size
            && (end > final_size || (fin && end != final_size))
        {
            return Err(ProtocolError::new(
                TransportErrorCode::FinalSizeError,
                "STREAM data beyond the final size",
            ));
        }
        if end > state.advertised {
            return Err(ProtocolError::new(
                TransportErrorCode::FlowControlError,
                format!(
                    "STREAM data up to {end} exceeds the window {}",
                    state.advertised
                ),
            ));
        }
        let counted = data.len() as u64;
        state.received = end;
        if fin {
            state.final_size = Some(end);
        }
        let mut discarded = 0;
        if state.abandoned {
            state.consumed = end;
            discarded = counted;
        } else if !data.is_empty() {
            state.chunks.push_back(data);
        }
        let ended = state.final_size == Some(end);
        let freed = ended && state.abandoned;
        drop(state);
        self.waker.wake();
        Ok(Received {
            counted,
            discarded,
            ended,
            freed,
        })
    }

    /// RESET_STREAM: `counted` is what the final size adds to the bytes the
    /// peer sent, `discarded` everything the application had not read.
    pub(crate) fn reset(
        &self,
        error_code: u64,
        final_size: u64,
    ) -> Result<Received, ProtocolError> {
        let mut state = self.state.lock().expect("receive stream lock");
        if final_size < state.received || state.final_size.is_some_and(|known| known != final_size)
        {
            return Err(ProtocolError::new(
                TransportErrorCode::FinalSizeError,
                "RESET_STREAM final size disagrees with the data received",
            ));
        }
        let counted = final_size - state.received;
        let discarded = final_size - state.consumed;
        let freed = state.abandoned;
        state.received = final_size;
        state.consumed = final_size;
        state.final_size = Some(final_size);
        if state.reset_code.is_none() {
            state.reset_code = Some(error_code);
        }
        state.chunks.clear();
        drop(state);
        self.waker.wake();
        Ok(Received {
            counted,
            discarded,
            ended: true,
            freed,
        })
    }

    pub(crate) fn connection_lost(&self) {
        self.state
            .lock()
            .expect("receive stream lock")
            .connection_lost = true;
        self.waker.wake();
    }
}

pub(crate) struct QmuxReceiveStream {
    stream_id: u64,
    session: Arc<SessionState>,
    shared: Arc<ReceiveShared>,
}

impl QmuxReceiveStream {
    pub(crate) fn new(
        stream_id: u64,
        session: Arc<SessionState>,
        shared: Arc<ReceiveShared>,
    ) -> Self {
        Self {
            stream_id,
            session,
            shared,
        }
    }
}

impl std::fmt::Debug for QmuxReceiveStream {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("QmuxReceiveStream")
            .field("stream_id", &self.stream_id)
            .finish()
    }
}

impl TransportReceiveStream for QmuxReceiveStream {
    fn poll_read(
        &mut self,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<Result<(), ReadError>> {
        self.shared.waker.register(cx.waker());
        let mut state = self.shared.state.lock().expect("receive stream lock");
        if state.reset_code.is_some() {
            return Poll::Ready(Err(ReadError::Reset));
        }
        let Some(chunk) = state.chunks.front_mut() else {
            if state.final_size == Some(state.received) {
                return Poll::Ready(Ok(()));
            }
            if state.connection_lost {
                return Poll::Ready(Err(ReadError::ConnectionLost));
            }
            return Poll::Pending;
        };
        let length = chunk.len().min(buf.remaining());
        buf.put_slice(&chunk[..length]);
        chunk.advance(length);
        if chunk.is_empty() {
            state.chunks.pop_front();
        }
        state.consumed += length as u64;
        let grant = replenish(&mut state);
        drop(state);
        if let Some(maximum) = grant {
            let _ = self
                .session
                .commands
                .send(Command::Frame(Frame::MaxStreamData {
                    stream_id: self.stream_id,
                    maximum,
                }));
        }
        self.session.consumed(length as u64);
        Poll::Ready(Ok(()))
    }
}

/// A new MAX_STREAM_DATA once half the window has been read.
fn replenish(state: &mut ReceiveState) -> Option<u64> {
    if state.final_size.is_some() || state.advertised - state.consumed > state.window / 2 {
        return None;
    }
    state.advertised = state.consumed + state.window;
    Some(state.advertised)
}

/// As in quinn, dropping the handle before the end of the stream asks the
/// peer to stop (STOP_SENDING 0); whatever still arrives is discarded. A
/// peer-opened stream's MAX_STREAMS slot goes back once both the handle is
/// gone and the peer's data ended, here or in the receive task, so a peer
/// that ignores STOP_SENDING cannot open streams beyond the limit.
impl Drop for QmuxReceiveStream {
    fn drop(&mut self) {
        let mut state = self.shared.state.lock().expect("receive stream lock");
        state.abandoned = true;
        let discarded = state
            .chunks
            .drain(..)
            .map(|chunk| chunk.len() as u64)
            .sum::<u64>();
        state.consumed += discarded;
        let ended = state.reset_code.is_some() || state.final_size == Some(state.received);
        drop(state);
        if discarded > 0 {
            self.session.consumed(discarded);
        }
        if !ended {
            let _ = self
                .session
                .commands
                .send(Command::Frame(Frame::StopSending {
                    stream_id: self.stream_id,
                    error_code: 0,
                }));
        } else if !self.session.role.opened_by_me(self.stream_id) {
            self.session
                .release_peer_stream(StreamKind::of(self.stream_id));
        }
    }
}
