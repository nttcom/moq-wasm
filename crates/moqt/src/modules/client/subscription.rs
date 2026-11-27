use std::{future::Future, sync::Arc};

use anyhow::bail;
use bytes::Bytes;
use thiserror::Error;
use tokio::sync::Mutex;

use crate::{
    Session, Subscription as MoqtSubscription, TrackEvent, TrackObject, TrackReader,
    object::{
        DataReceiver, DataStreamResetCode, ExtensionHeaders, ForwardingPreference, Location,
        MalformedTrackError, Object, ObjectStatus, ReadError, StreamDataReceiver,
        StreamReceiveError, Subgroup, TrackData,
    },
    publish::PublishDoneCode,
};

/// An object of a subscription or a FETCH response.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReceivedObject {
    pub group_id: u64,
    /// 0 for an object that arrived in a datagram.
    pub subgroup_id: u64,
    pub object_id: u64,
    /// The Object Status (draft-14 §10.2.1.1) of an object without a payload;
    /// `None` for one with a payload.
    pub status: Option<ObjectStatus>,
    pub payload: Bytes,
    pub extension_headers: ExtensionHeaders,
}

impl From<TrackObject> for ReceivedObject {
    fn from(object: TrackObject) -> Self {
        Self {
            group_id: object.group_id,
            subgroup_id: subgroup_id(object.forwarding),
            object_id: object.object_id,
            status: None,
            payload: object.payload,
            extension_headers: object.extension_headers,
        }
    }
}

impl From<Object> for ReceivedObject {
    fn from(object: Object) -> Self {
        Self {
            group_id: object.location.group_id,
            subgroup_id: subgroup_id(object.forwarding),
            object_id: object.location.object_id,
            status: object.payload.is_empty().then_some(object.status),
            payload: object.payload,
            extension_headers: object.extension_headers,
        }
    }
}

fn subgroup_id(forwarding: ForwardingPreference) -> u64 {
    match forwarding {
        ForwardingPreference::Subgroup { subgroup_id } => subgroup_id,
        ForwardingPreference::Datagram => 0,
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SubscriptionEvent {
    Object(ReceivedObject),
    /// The publisher reset a subgroup stream of group `group_id` with `code`
    /// (draft-14 §13.1.8, draft-18 §15.10.4); its remaining objects are lost.
    Reset {
        group_id: u64,
        code: DataStreamResetCode,
    },
    /// The End of Group status object (draft-14 §10.2.1.1): no object of
    /// `group_id` comes after `object_id`.
    EndOfGroup {
        group_id: u64,
        /// 0 for one that arrived in a datagram.
        subgroup_id: u64,
        object_id: u64,
    },
}

/// The PUBLISH_DONE (draft-14 §9.12) that ended a subscription.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublishDone {
    pub code: PublishDoneCode,
    pub reason: String,
}

/// One SUBSCRIBE (draft-14 §9.7). It is read either by object (`next`) or by
/// subgroup stream (`next_group`), whichever is called first.
pub struct Subscription {
    session: Arc<Session>,
    subscription: MoqtSubscription,
    reading: Mutex<Reading>,
}

pub(super) enum Reading {
    Unstarted,
    Objects(TrackReader),
    Groups(DataReceiver),
}

impl Subscription {
    pub(super) fn new(
        session: Arc<Session>,
        subscription: MoqtSubscription,
        reading: Reading,
    ) -> Self {
        Self {
            session,
            subscription,
            reading: Mutex::new(reading),
        }
    }

    /// The largest location SUBSCRIBE_OK reported.
    pub fn largest(&self) -> Option<Location> {
        self.subscription.content_exists().largest()
    }

    /// Resolves once the publisher ends the subscription with PUBLISH_DONE,
    /// or to `None` once the session ends or `close` is called without one.
    pub fn done(&self) -> impl Future<Output = Option<PublishDone>> + 'static {
        let done = self.subscription.done();
        async move {
            done.await.map(|done| PublishDone {
                code: done.status_code(),
                reason: done.error_reason().to_string(),
            })
        }
    }

    /// The next object; `None` once the track has no more streams. Objects
    /// of one subgroup come in order; status objects are skipped. Fails once
    /// `next_group` read the subscription.
    pub async fn next(&self) -> anyhow::Result<Option<ReceivedObject>> {
        loop {
            match self.next_event().await? {
                Some(SubscriptionEvent::Object(object)) => return Ok(Some(object)),
                Some(SubscriptionEvent::Reset { .. } | SubscriptionEvent::EndOfGroup { .. }) => {
                    continue;
                }
                None => return Ok(None),
            }
        }
    }

    /// `next`, with the subgroup streams the publisher reset and the End of
    /// Group status objects, each in its place among the objects of its
    /// subgroup.
    pub async fn next_event(&self) -> anyhow::Result<Option<SubscriptionEvent>> {
        let mut reading = self.reading.lock().await;
        if let Reading::Unstarted = *reading {
            *reading = Reading::Objects(self.session.subscriber().track_reader(&self.subscription));
        }
        let Reading::Objects(reader) = &mut *reading else {
            bail!("the subscription is read by group");
        };
        Ok(reader.next_event().await?.map(|event| match event {
            TrackEvent::Object(object) => SubscriptionEvent::Object(object.into()),
            TrackEvent::Reset { group_id, code } => SubscriptionEvent::Reset { group_id, code },
            TrackEvent::EndOfGroup {
                location,
                forwarding,
            } => SubscriptionEvent::EndOfGroup {
                group_id: location.group_id,
                subgroup_id: subgroup_id(forwarding),
                object_id: location.object_id,
            },
        }))
    }

    /// The next subgroup stream; `None` once the track has no more streams.
    /// Fails for a track delivered in datagrams, and once `next` read the
    /// subscription.
    pub async fn next_group(&self) -> anyhow::Result<Option<Group>> {
        let mut reading = self.reading.lock().await;
        if let Reading::Unstarted = *reading {
            let mut subscriber = self.session.subscriber();
            match subscriber.accept_data_receiver(&self.subscription).await {
                Ok(receiver) => *reading = Reading::Groups(receiver),
                Err(_) => return Ok(None),
            }
        }
        let Reading::Groups(data) = &mut *reading else {
            bail!("the subscription is read by object");
        };
        let mut receiver = match data.next().await {
            Ok(TrackData::Subgroup(receiver)) => receiver,
            Ok(TrackData::Datagram(_)) => bail!("a datagram arrived on a track read by group"),
            Err(error) if error.is::<MalformedTrackError>() => return Err(error),
            Err(_) => return Ok(None),
        };
        match receiver.receive().await? {
            Some(Subgroup::Header(start)) => Ok(Some(Group {
                group_id: start.group_id,
                subgroup_id: start.subgroup_id,
                receiver: Mutex::new(receiver),
            })),
            Some(Subgroup::Object(_)) | None => {
                bail!("a subgroup stream did not start with its header")
            }
        }
    }

    /// SUBSCRIBE_UPDATE with the Forward flag (draft-14 §9.10).
    pub async fn set_forward(&self, forward: bool) -> anyhow::Result<()> {
        self.session
            .subscriber()
            .update_forward(&self.subscription, forward)
            .await
    }

    /// SUBSCRIBE_UPDATE with the Subscriber Priority (draft-14 §7.1), by
    /// which the relay orders what it has not sent yet.
    pub async fn set_priority(&self, priority: u8) -> anyhow::Result<()> {
        self.session
            .subscriber()
            .update_priority(&self.subscription, priority)
            .await
    }

    /// Asks the publisher to start a new group (NEW_GROUP_REQUEST, draft-18
    /// §10.2.13). `largest_group` is the largest group ID of the track the
    /// caller has seen. Returns `false`, without sending, for a track
    /// without dynamic groups.
    pub async fn request_new_group(&self, largest_group: Option<u64>) -> anyhow::Result<bool> {
        self.session
            .subscriber()
            .request_new_group(&self.subscription, largest_group)
            .await
    }

    /// SUBSCRIBE_UPDATE ending the subscription before `group_id` (draft-14
    /// §9.10); the publisher sends PUBLISH_DONE once it has sent the groups
    /// below it.
    pub async fn end_before_group(&self, group_id: u64) -> anyhow::Result<()> {
        self.session
            .subscriber()
            .end_before_group(&self.subscription, group_id)
            .await
    }

    /// UNSUBSCRIBE (draft-14 §9.11); `done` resolves to `None`.
    pub async fn close(&self) -> anyhow::Result<()> {
        self.session
            .subscriber()
            .unsubscribe(&self.subscription)
            .await
    }
}

/// One subgroup stream of a subscription (draft-14 §10.4).
pub struct Group {
    group_id: u64,
    subgroup_id: u64,
    receiver: Mutex<StreamDataReceiver>,
}

impl Group {
    pub fn group_id(&self) -> u64 {
        self.group_id
    }

    pub fn subgroup_id(&self) -> u64 {
        self.subgroup_id
    }

    /// The next object, a status object such as End of Group included;
    /// `None` once the publisher finished the stream.
    pub async fn next(&self) -> Result<Option<ReceivedObject>, GroupReadError> {
        let mut receiver = self.receiver.lock().await;
        loop {
            match receiver.receive().await {
                Ok(Some(Subgroup::Object(object))) => return Ok(Some(object.into())),
                Ok(Some(Subgroup::Header(_))) => continue,
                Ok(None) => return Ok(None),
                Err(StreamReceiveError::Closed(ReadError::Reset(code))) => {
                    let code = receiver.version().data_stream_reset_code_from_wire(code);
                    return Err(GroupReadError::Reset(code));
                }
                Err(error) => return Err(GroupReadError::Receive(error)),
            }
        }
    }
}

#[derive(Debug, Error)]
#[non_exhaustive]
pub enum GroupReadError {
    /// The publisher reset the stream with this code (draft-14 §13.1.8,
    /// draft-18 §15.10.4).
    #[error("subgroup stream reset with {}", .0.name())]
    Reset(DataStreamResetCode),
    #[error(transparent)]
    Receive(StreamReceiveError),
}
