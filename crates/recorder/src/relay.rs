use anyhow::{Context, Result};
use bytes::Bytes;
use moqt::{
    ClientConfig, DUAL, Endpoint, MigratingSession, MigratingTrack, TrackObject,
    fetch::{Fetch, FetchDataReceiver, FetchOption},
    object::{ContentExists, ForwardingPreference, GroupOrder, Location, Object, ObjectStatus},
    subscribe::{FilterType, SubscribeOption},
};

const SUBSCRIBER_PRIORITY: u8 = 128;

pub(crate) async fn connect(
    url: &str,
    verify_certificate: bool,
    authorization_token: Option<String>,
) -> Result<MigratingSession> {
    let endpoint = Endpoint::<DUAL>::create_client(&ClientConfig {
        port: 0,
        verify_certificate,
        authorization_token,
    })?;
    tracing::info!(relay = url, "connecting to relay");
    MigratingSession::connect(endpoint, url)
        .await
        .with_context(|| format!("cannot connect to {url}"))
}

/// `NextGroupStart` begins a live recording on a keyframe; `LargestObject`
/// continues right after what `fetch_cached` returns.
pub(crate) async fn subscribe(
    session: &MigratingSession,
    namespace: &str,
    name: &str,
    filter_type: FilterType,
) -> Result<MigratingTrack> {
    let option = SubscribeOption {
        subscriber_priority: SUBSCRIBER_PRIORITY,
        group_order: GroupOrder::Ascending,
        forward: true,
        filter_type,
    };
    session
        .subscribe(namespace.to_string(), name.to_string(), option)
        .await
        .with_context(|| format!("failed to subscribe {namespace}/{name}"))
}

/// The last object of the group `track` joined at: the joining FETCH
/// with offset 0 that draft-ietf-moq-msf-01 §5 asks for beside a catalog
/// SUBSCRIBE. `None` when the publisher has sent nothing yet.
pub(crate) async fn fetch_joining_group(track: &MigratingTrack) -> Result<Option<Bytes>> {
    let mut subscriber = track.session().subscriber();
    let handle = subscriber
        .fetch_relative_joining(track.subscription(), 0, FetchOption::default())
        .await?;
    let mut receiver = subscriber.accept_fetch_receiver(&handle).await?;
    let mut latest = None;
    loop {
        match receiver.receive().await? {
            Fetch::Object(object) if !object.payload.is_empty() => latest = Some(object.payload),
            Fetch::Object(_) => {}
            Fetch::End => return Ok(latest),
        }
    }
}

/// A standalone FETCH from the track's first location through the largest
/// object the track's subscription reported (draft-14 §9.16.1: End Location is the last
/// object plus one), so a `LargestObject` subscription continues where it
/// ends. The relay serves what it caches and asks the publisher for the
/// rest. `None` when the subscription reported no content.
pub(crate) async fn fetch_cached(track: &MigratingTrack) -> Result<Option<FetchDataReceiver>> {
    let subscription = track.subscription();
    let ContentExists::True { location: largest } = subscription.content_exists() else {
        return Ok(None);
    };
    let mut subscriber = track.session().subscriber();
    let handle = subscriber
        .fetch(
            subscription.track_namespace().to_string(),
            subscription.track_name().to_string(),
            Location {
                group_id: 0,
                object_id: 0,
            },
            Location {
                group_id: largest.group_id,
                object_id: largest.object_id + 1,
            },
            FetchOption::default(),
        )
        .await?;
    Ok(Some(subscriber.accept_fetch_receiver(&handle).await?))
}

/// `None` for status objects, which carry no media.
pub(crate) fn fetched_track_object(object: Object) -> Option<TrackObject> {
    if object.status != ObjectStatus::Normal || object.payload.is_empty() {
        return None;
    }
    Some(TrackObject {
        group_id: object.location.group_id,
        subgroup_id: match object.forwarding {
            ForwardingPreference::Subgroup { subgroup_id } => subgroup_id,
            ForwardingPreference::Datagram => 0,
        },
        object_id: object.location.object_id,
        extension_headers: object.extension_headers,
        payload: object.payload,
    })
}
