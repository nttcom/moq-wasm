use std::{
    fmt,
    sync::atomic::{AtomicBool, AtomicU64, Ordering},
};

use crate::modules::{
    moqt::{control_plane::constants::TerminationErrorCode, version::ControlChannel},
    transport::transport_connection::BoxedConnection,
};

/// One transport connection of a session and the state draft-14 scopes to
/// it: the control stream and its request ids (§9.1), the track aliases this
/// side assigns, and whether the peer sent GOAWAY (§9.4).
pub(crate) struct Connection {
    pub(crate) transport: BoxedConnection,
    pub(crate) control: ControlChannel,
    is_server: bool,
    go_away_received: AtomicBool,
    track_alias: AtomicU64,
}

impl Connection {
    pub(crate) fn new(
        transport: BoxedConnection,
        control: ControlChannel,
        is_server: bool,
    ) -> Self {
        Self {
            transport,
            control,
            is_server,
            go_away_received: AtomicBool::new(false),
            track_alias: AtomicU64::new(0),
        }
    }

    /// draft-14 §9.4: a second GOAWAY, or a New Session URI sent to a
    /// server, is a protocol violation.
    pub(crate) fn note_go_away_received(&self, new_session_uri: &str) -> Result<(), &'static str> {
        if self.go_away_received.swap(true, Ordering::SeqCst) {
            return Err("received a second GOAWAY");
        }
        if self.is_server && !new_session_uri.is_empty() {
            return Err("GOAWAY sent to a server carries a New Session URI");
        }
        Ok(())
    }

    pub(crate) fn allocate_track_alias(&self) -> u64 {
        self.track_alias.fetch_add(1, Ordering::SeqCst)
    }

    pub(crate) fn close(&self, code: TerminationErrorCode, reason: &str) {
        self.transport
            .close(self.control.version().termination_code(code), reason);
    }
}

impl fmt::Debug for Connection {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Connection").finish_non_exhaustive()
    }
}
