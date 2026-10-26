use std::sync::Arc;

use crate::{
    Subscriber,
    fetch::FetchOption,
    modules::moqt::{
        data_plane::{
            object::canonical_object::Object, object_ledger::ObjectLedger,
            stream::stream_receiver::StreamReceiveError,
        },
        domains::{
            connection::generation_of, fetch_handle::FetchRemainder,
            session_context::SessionContext,
        },
        version::{FetchReader, draft18::FetchRequestEnd},
    },
    object::{GroupOrder, Location},
};

#[derive(Debug, Clone, PartialEq)]
pub enum Fetch {
    Object(Object),
    /// draft-18 §11.4.4.2 End of Unknown Range: the objects after the
    /// previous one (or the range start) through `last` have unknown status
    /// and may exist. draft-14 never yields it.
    UnknownRange {
        last: Location,
    },
    /// The publisher finished the stream (FIN): every object of the range
    /// the publisher knows was delivered; an object missing outside the
    /// unknown ranges does not exist.
    End,
}

#[derive(Debug)]
pub struct FetchDataReceiver {
    pub request_id: u64,
    reader: FetchReader,
    /// Set for a FETCH a session that follows GOAWAY can send again.
    carry: Option<FetchCarry>,
    /// `None` on draft-14, whose FETCH has no request stream.
    request_end: Option<FetchRequestEnd>,
    /// The fetched track's ledger once the stream is handed to the
    /// application.
    ledger: Option<Arc<ObjectLedger>>,
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
    async fn rest(&mut self) -> anyhow::Result<Option<FetchDataReceiver>> {
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
                    fill_timeout: self.remainder.fill_timeout_left(),
                },
            )
            .await?;
        let receiver = subscriber.accept_fetch_receiver(&handle).await?;
        self.generation = generation_of(handle.request_id);
        tracing::info!(start = ?self.remainder.start, "FETCH sent again on the new connection");
        Ok(Some(receiver))
    }
}

impl FetchDataReceiver {
    pub(crate) fn new(request_id: u64, reader: FetchReader) -> Self {
        Self {
            request_id,
            reader,
            carry: None,
            request_end: None,
            ledger: None,
        }
    }

    /// Checks every object read from here on against the track's `ledger`.
    pub(crate) fn check_with(&mut self, ledger: Arc<ObjectLedger>) {
        self.ledger = Some(ledger);
    }

    pub(crate) fn end_request_with(&mut self, request_end: Option<FetchRequestEnd>) {
        self.request_end = request_end;
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

    pub(crate) fn set_group_order(&mut self, group_order: GroupOrder) {
        self.reader.set_group_order(group_order);
    }

    /// Whether the objects carry their forwarding preference: draft-18 marks
    /// a datagram object (§11.4.4), while draft-14 sends it in the subgroup
    /// of its Object ID (§10.4.4).
    pub fn carries_forwarding_preference(&self) -> bool {
        self.reader.version().fetch_carries_forwarding_preference()
    }

    /// The next object or range of the response. An object that repeats one
    /// the session received before with other properties fails with a
    /// `MalformedTrackError` and is not returned.
    pub async fn receive(&mut self) -> anyhow::Result<Fetch> {
        loop {
            let error = match self.reader.next().await {
                Ok(fetched) => {
                    if let (Fetch::Object(object), Some(ledger)) = (&fetched, &self.ledger) {
                        ledger.check(object, !self.carries_forwarding_preference())?;
                    }
                    match (&fetched, &mut self.carry) {
                        (Fetch::Object(object), Some(carry)) => carry.delivered(object.location),
                        (Fetch::End, _) => self.response_ended(),
                        _ => {}
                    }
                    return Ok(fetched);
                }
                Err(error) => error,
            };
            self.response_ended();
            match &error {
                StreamReceiveError::Decode(_) | StreamReceiveError::ProtocolViolation(_) => {
                    tracing::error!(%error, "Failed to decode the fetch stream")
                }
                StreamReceiveError::MalformedTrack(_) => {
                    tracing::warn!(%error, "the fetch stream made its track malformed")
                }
                StreamReceiveError::Closed(_) => tracing::debug!(%error, "Fetch stream ended"),
            }
            if let Some(carry) = &mut self.carry
                && matches!(error, StreamReceiveError::Closed(_))
            {
                match carry.rest().await {
                    Ok(Some(rest)) => {
                        self.reader = rest.reader;
                        self.request_end = rest.request_end;
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

    fn response_ended(&mut self) {
        if let Some(request_end) = self.request_end.take() {
            request_end.response_ended();
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
        object::{ExtensionHeaders, Object, ObjectStatus, ReadError, StreamReceiveError},
    };

    #[tokio::test]
    async fn fetch_preserves_objects_and_reports_fin_after_the_last_object() {
        // Arrange
        let mut stream = fetched_stream("fetch-object-roundtrip").await;
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
        let mut stream = fetched_stream("fetch-object-reset").await;
        let expected = object_with_extensions();
        let reset_error = StreamReceiveError::Closed(ReadError::Reset).to_string();
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
            matches!(&result, Err(error) if error.to_string().contains(&reset_error)),
            "{result:?}"
        );
    }
}
