use std::sync::{Arc, Weak};

use crate::modules::moqt::domains::{connection::Connection, session_context::SessionContext};

/// Where the objects of one track this endpoint sends go.
#[derive(Clone)]
pub(crate) enum SendTargets {
    /// One connection, with its track alias: a track served for a SUBSCRIBE,
    /// or any track of a session that does not follow GOAWAY. Gone once the
    /// connection is.
    Connection {
        connection: Weak<Connection>,
        track_alias: u64,
    },
    /// A PUBLISH a session that follows GOAWAY carries: every connection it
    /// is published on, so objects reach both servers during a migration.
    Publication {
        session: Arc<SessionContext>,
        request_id: u64,
    },
}

impl SendTargets {
    /// The connections to send on now, each with its track alias.
    pub(crate) fn resolve(&self) -> Vec<(Arc<Connection>, u64)> {
        match self {
            Self::Connection {
                connection,
                track_alias,
            } => connection
                .upgrade()
                .map(|connection| (connection, *track_alias))
                .into_iter()
                .collect(),
            Self::Publication {
                session,
                request_id,
            } => session.publication_targets(*request_id),
        }
    }
}
