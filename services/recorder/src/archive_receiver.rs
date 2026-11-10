use std::task::{Context, Poll};

use anyhow::Result;
use moqt::{
    Session, Subscription,
    fetch::{Fetch, FetchOption},
    object::ContentExists,
    object::{MalformedTrackError, Object, Subgroup, SubgroupStart, TrackData},
    subscribe::{FilterType, SubscribeOption},
};
use tokio::{
    sync::mpsc,
    task::{JoinHandle, JoinSet},
};

pub(crate) enum Event {
    Header(SubgroupStart),
    Object(Object),
    End { group_id: u64, subgroup_id: u64 },
}

pub(crate) struct ArchiveTrack {
    pub(crate) subscription: Subscription,
    receiver: mpsc::Receiver<Result<Event>>,
    task: JoinHandle<()>,
}

impl ArchiveTrack {
    pub(crate) async fn subscribe(
        session: &Session,
        namespace: &str,
        name: &str,
        filter_type: FilterType,
        current_group: bool,
    ) -> Result<Self> {
        let mut subscriber = session.subscriber();
        let subscription = subscriber
            .subscribe(
                namespace.to_owned(),
                name.to_owned(),
                SubscribeOption {
                    filter_type,
                    // A track with long groups starts one when asked
                    // (draft-18 §5.1.3.1).
                    new_group_request: (filter_type == FilterType::NextGroupStart).then_some(0),
                    ..SubscribeOption::default()
                },
            )
            .await?;
        let (sender, receiver) = mpsc::channel(64);
        let task_subscription = subscription.clone();
        let task = tokio::spawn(async move {
            if current_group && task_subscription.content_exists() != ContentExists::False {
                let fetched = async {
                    let handle = subscriber
                        .fetch_relative_joining(&task_subscription, 0, FetchOption::default())
                        .await?;
                    let mut receiver = subscriber.accept_fetch_receiver(&handle).await?;
                    while let Fetch::Object(object) = receiver.receive().await? {
                        if sender.send(Ok(Event::Object(object))).await.is_err() {
                            break;
                        }
                    }
                    anyhow::Ok(())
                }
                .await;
                if let Err(error) = fetched {
                    let _ = sender.send(Err(error)).await;
                    return;
                }
            }
            let data = match subscriber.accept_data_receiver(&task_subscription).await {
                Ok(data) => data,
                Err(error) => {
                    let _ = sender.send(Err(error)).await;
                    return;
                }
            };
            let mut data = data;
            let mut streams = JoinSet::new();
            loop {
                tokio::select! {
                    received = data.next() => match received {
                        Ok(TrackData::Datagram(object)) => {
                            if sender.send(Ok(Event::Object(object))).await.is_err() {
                                break;
                            }
                        }
                        Ok(TrackData::Subgroup(mut stream)) => {
                            let sender = sender.clone();
                            streams.spawn(async move {
                                let mut started = None;
                                loop {
                                    match stream.receive().await {
                                        Ok(Some(Subgroup::Header(header))) => {
                                            started = Some((header.group_id, header.subgroup_id));
                                            if sender.send(Ok(Event::Header(header))).await.is_err() { break }
                                        },
                                        Ok(Some(Subgroup::Object(object))) => if sender.send(Ok(Event::Object(object))).await.is_err() { break },
                                        Ok(None) => {
                                            if let Some((group_id, subgroup_id)) = started {
                                                let _ = sender.send(Ok(Event::End { group_id, subgroup_id })).await;
                                            }
                                            break
                                        },
                                        // As in moqt's TrackReader, a failed subgroup stream ends only that stream.
                                        Err(error) => { tracing::debug!(%error, "subgroup stream failed; the track goes on"); break; }
                                    }
                                }
                            });
                        }
                        Err(error) => { if error.is::<MalformedTrackError>() { let _ = sender.send(Err(error)).await; } break; }
                    },
                    Some(result) = streams.join_next(), if !streams.is_empty() => if let Err(error) = result { let _ = sender.send(Err(error.into())).await; break; },
                }
            }
            while let Some(result) = streams.join_next().await {
                if let Err(error) = result {
                    let _ = sender.send(Err(error.into())).await;
                }
            }
        });
        Ok(Self {
            subscription,
            receiver,
            task,
        })
    }

    pub(crate) async fn next(&mut self) -> Result<Option<Event>> {
        self.receiver.recv().await.transpose()
    }

    pub(crate) fn poll_next(&mut self, cx: &mut Context<'_>) -> Poll<Result<Option<Event>>> {
        self.receiver.poll_recv(cx).map(Option::transpose)
    }
}

impl Drop for ArchiveTrack {
    fn drop(&mut self) {
        self.task.abort();
    }
}

#[cfg(test)]
mod tests {
    use bytes::Bytes;
    use moqt::object::{ExtensionHeaders, ForwardingPreference, Location, ObjectStatus};
    use std::time::Duration;

    use super::*;
    use crate::{
        relay::RelayConnection,
        test_support::{datagram_publisher, reset_stream_publisher},
    };

    #[tokio::test]
    async fn receives_datagrams_and_empty_status_objects_without_losing_fields() {
        // Arrange
        let mut headers = ExtensionHeaders::default();
        // Two well-formed Key-Value-Pairs: moqt refuses malformed Immutable Properties.
        headers.push_immutable_extension(Bytes::from_static(&[0x02, 0x05, 0x0D, 0x02, b'a', b'b']));
        let first = Object {
            location: Location {
                group_id: 7,
                object_id: 0,
            },
            forwarding: ForwardingPreference::Datagram,
            publisher_priority: 42,
            status: ObjectStatus::Normal,
            extension_headers: headers,
            payload: Bytes::from_static(b"payload"),
            ends_group: false,
        };
        let last = Object {
            location: Location {
                group_id: 7,
                object_id: 1,
            },
            forwarding: ForwardingPreference::Datagram,
            publisher_priority: 42,
            status: ObjectStatus::EndOfGroup,
            extension_headers: ExtensionHeaders::default(),
            payload: Bytes::new(),
            ends_group: false,
        };
        let url = datagram_publisher(vec![first.clone(), last.clone()]);
        let connection = RelayConnection::connect(&url, false, None).await.unwrap();

        // Act
        let mut track = ArchiveTrack::subscribe(
            &connection.session,
            "anon/test",
            "video",
            FilterType::LargestObject,
            false,
        )
        .await
        .unwrap();
        let received = tokio::time::timeout(Duration::from_secs(5), async {
            let mut objects = Vec::new();
            for _ in 0..2 {
                if let Some(Event::Object(object)) = track.next().await.unwrap() {
                    objects.push(object);
                }
            }
            objects
        })
        .await
        .unwrap();

        // Assert
        assert_eq!(received, [first, last]);
        connection.close().await;
    }

    fn subgroup_object(group_id: u64, payload: &'static [u8]) -> Object {
        Object {
            location: Location {
                group_id,
                object_id: 0,
            },
            forwarding: ForwardingPreference::Subgroup { subgroup_id: 0 },
            publisher_priority: 0,
            status: ObjectStatus::Normal,
            extension_headers: ExtensionHeaders::default(),
            payload: Bytes::from_static(payload),
            ends_group: false,
        }
    }

    #[tokio::test]
    async fn a_reset_subgroup_stream_is_skipped_and_the_next_group_is_received() {
        // Arrange
        let (url, proceed) =
            reset_stream_publisher(subgroup_object(0, b"g0"), subgroup_object(1, b"g1"));
        let connection = RelayConnection::connect(&url, false, None).await.unwrap();
        let mut track = ArchiveTrack::subscribe(
            &connection.session,
            "anon/test",
            "video",
            FilterType::LargestObject,
            false,
        )
        .await
        .unwrap();
        tokio::time::timeout(Duration::from_secs(5), async {
            while !matches!(track.next().await.unwrap(), Some(Event::Object(_))) {}
        })
        .await
        .unwrap();

        // Act
        proceed.send(()).unwrap();
        let ended = tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                if let Event::End { group_id, .. } = track.next().await.unwrap().unwrap() {
                    break group_id;
                }
            }
        })
        .await
        .unwrap();

        // Assert: the reset group 0 neither fails the track nor ends a subgroup
        assert_eq!(ended, 1);
        connection.close().await;
    }
}
