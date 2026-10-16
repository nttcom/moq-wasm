use std::sync::Arc;

use crate::{
    Subscriber,
    fetch::FetchOption,
    modules::moqt::{
        data_plane::{
            object::canonical_object::Object, stream::stream_receiver::StreamReceiveError,
        },
        domains::{
            connection::generation_of, fetch_handle::FetchRemainder,
            session_context::SessionContext,
        },
        version::FetchReader,
    },
    object::{GroupOrder, Location},
};

#[derive(Debug)]
pub enum Fetch {
    Object(Object),
    /// The publisher finished the stream (FIN): every object of the range
    /// was delivered.
    End,
}

#[derive(Debug)]
pub struct FetchDataReceiver {
    pub request_id: u64,
    reader: FetchReader,
    /// Set for a FETCH a session that follows GOAWAY can send again.
    carry: Option<FetchCarry>,
}

/// Lets a FETCH whose stream a migration cut off continue on the session's
/// current connection with the part not delivered yet (draft-14 §3.5).
struct FetchCarry {
    session: Arc<SessionContext>,
    /// The connection generation the stream being read came on.
    generation: u64,
    remainder: FetchRemainder,
    /// The FETCH_OK's End Location: the end plus one, an object of 0 meaning
    /// the whole group (draft-14 §9.17).
    end: Location,
}

impl std::fmt::Debug for FetchCarry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FetchCarry")
            .field("generation", &self.generation)
            .finish_non_exhaustive()
    }
}

impl FetchCarry {
    fn delivered(&mut self, location: Location) {
        self.remainder.start = Location {
            group_id: location.group_id,
            object_id: location.object_id + 1,
        };
    }

    fn rest_is_empty(&self) -> bool {
        let start = self.remainder.start;
        if self.end.object_id == 0 {
            start.group_id > self.end.group_id
        } else {
            (start.group_id, start.object_id) >= (self.end.group_id, self.end.object_id)
        }
    }

    /// The stream of the part not delivered yet, FETCHed on the current
    /// connection; `None` when the session has not moved since.
    async fn rest(&mut self) -> anyhow::Result<Option<FetchReader>> {
        if self.session.connection().generation == self.generation || self.rest_is_empty() {
            return Ok(None);
        }
        let mut subscriber = Subscriber {
            session: self.session.clone(),
        };
        let handle = subscriber
            .fetch(
                self.remainder.track_namespace.clone(),
                self.remainder.track_name.clone(),
                self.remainder.start,
                self.end,
                FetchOption {
                    subscriber_priority: self.remainder.subscriber_priority,
                    group_order: GroupOrder::Ascending,
                },
            )
            .await?;
        let receiver = subscriber.accept_fetch_receiver(&handle).await?;
        self.generation = generation_of(handle.request_id);
        tracing::info!(start = ?self.remainder.start, "FETCH sent again on the new connection");
        Ok(Some(receiver.reader))
    }
}

impl FetchDataReceiver {
    pub(crate) fn new(request_id: u64, reader: FetchReader) -> Self {
        Self {
            request_id,
            reader,
            carry: None,
        }
    }

    pub(crate) fn carry_with(
        &mut self,
        session: Arc<SessionContext>,
        remainder: FetchRemainder,
        end: Location,
        fetch_request_id: u64,
    ) {
        self.carry = Some(FetchCarry {
            session,
            generation: generation_of(fetch_request_id),
            remainder,
            end,
        });
    }

    pub async fn receive(&mut self) -> anyhow::Result<Fetch> {
        loop {
            let error = match self.reader.next_object().await {
                Ok(Some(object)) => {
                    if let Some(carry) = &mut self.carry {
                        carry.delivered(object.location);
                    }
                    return Ok(Fetch::Object(object));
                }
                Ok(None) => return Ok(Fetch::End),
                Err(error) => error,
            };
            match &error {
                StreamReceiveError::Decode(_) | StreamReceiveError::ProtocolViolation(_) => {
                    tracing::error!(%error, "Failed to decode the fetch stream")
                }
                StreamReceiveError::Closed(_) => tracing::debug!(%error, "Fetch stream ended"),
            }
            if let Some(carry) = &mut self.carry
                && matches!(error, StreamReceiveError::Closed(_))
            {
                match carry.rest().await {
                    Ok(Some(reader)) => {
                        self.reader = reader;
                        continue;
                    }
                    Ok(None) => {}
                    Err(rest_error) => {
                        tracing::warn!(
                            error = format!("{rest_error:#}"),
                            "the rest of the FETCH was not sent again"
                        );
                    }
                }
            }
            anyhow::bail!("Failed to receive data from fetch stream: {error}")
        }
    }
}
