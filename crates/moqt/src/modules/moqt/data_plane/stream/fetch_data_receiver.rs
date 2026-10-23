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
#[derive(Debug)]
struct FetchCarry {
    session: Arc<SessionContext>,
    /// The connection generation the stream being read came on.
    generation: u64,
    remainder: FetchRemainder,
    /// The FETCH_OK's End Location: the end plus one, an object of 0 meaning
    /// the whole group (draft-14 §9.17).
    end: Location,
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

#[cfg(test)]
mod tests {
    use bytes::Bytes;
    use tokio::time::timeout;

    use super::Fetch;
    use crate::{
        modules::{
            moqt::control_plane::error_codes::DataStreamResetCode,
            test_support::{
                HANDSHAKE_TIMEOUT, fetched_stream, location, object_with_extensions,
                read_fetch_objects,
            },
        },
        object::{ExtensionHeaders, Object, ObjectStatus},
    };

    #[tokio::test]
    async fn fetch_preserves_objects_and_reports_fin_after_the_last_object() {
        // Arrange
        let mut stream = fetched_stream("fetch-object-roundtrip", true, location(16_385, 0)).await;
        let expected = [
            object_with_extensions(),
            Object {
                location: location(16_384, 16_386),
                status: ObjectStatus::DoesNotExist,
                extension_headers: ExtensionHeaders::default(),
                payload: Bytes::new(),
                ..object_with_extensions()
            },
            Object {
                location: location(16_384, 16_387),
                payload: Bytes::new(),
                ..object_with_extensions()
            },
            Object {
                location: location(16_384, 16_388),
                status: ObjectStatus::EndOfGroup,
                payload: Bytes::new(),
                ..object_with_extensions()
            },
            Object {
                location: location(16_385, 0),
                status: ObjectStatus::EndOfTrack,
                payload: Bytes::new(),
                ..object_with_extensions()
            },
        ];

        // Act
        timeout(HANDSHAKE_TIMEOUT, async {
            for object in &expected {
                stream.sender.send_object(object).await.unwrap();
            }
            stream.sender.close().await.unwrap();
        })
        .await
        .unwrap();

        // Assert
        assert_eq!(
            timeout(HANDSHAKE_TIMEOUT, read_fetch_objects(&mut stream.receiver))
                .await
                .unwrap(),
            expected
        );
    }

    #[tokio::test]
    async fn fetch_reports_reset_as_an_error_after_a_delivered_object() {
        // Arrange
        let mut stream =
            fetched_stream("fetch-object-reset", false, location(16_384, 16_386)).await;
        let expected = object_with_extensions();
        timeout(HANDSHAKE_TIMEOUT, stream.sender.send_object(&expected))
            .await
            .unwrap()
            .unwrap();
        let Fetch::Object(_) = timeout(HANDSHAKE_TIMEOUT, stream.receiver.receive())
            .await
            .unwrap()
            .unwrap()
        else {
            panic!("expected an object before RESET");
        };

        // Act
        timeout(
            HANDSHAKE_TIMEOUT,
            stream.sender.reset(DataStreamResetCode::InternalError),
        )
        .await
        .unwrap()
        .unwrap();
        let result = timeout(HANDSHAKE_TIMEOUT, stream.receiver.receive())
            .await
            .unwrap();

        // Assert
        assert!(
            matches!(&result, Err(error) if error.to_string().contains("stream closed: stream reset by peer")),
            "{result:?}"
        );
    }
}
