use std::{future::Future, sync::Arc};

use tokio::sync::watch;

use crate::modules::extensions::mutex::lock;
use crate::{
    TrackProperties,
    modules::moqt::control_plane::control_messages::messages::{
        parameters::content_exists::ContentExists, publish_ok::PublishOk, subscribe_ok::SubscribeOk,
    },
    modules::moqt::control_plane::handler::publish_done_handler::PublishDoneHandler,
    modules::moqt::control_plane::options::SubscribeUpdateOption,
    object::GroupOrder,
    subscribe::{FilterType, SubscribeHandler, SubscribeOption},
};

#[derive(Clone, Debug)]
pub enum Subscription {
    PublisherInitiated(PublisherInitiatedSubscription),
    SubscriberInitiated(SubscriberInitiatedSubscription),
}

impl Subscription {
    pub fn request_id(&self) -> u64 {
        match self {
            Self::PublisherInitiated(subscription) => subscription.request_id,
            Self::SubscriberInitiated(subscription) => subscription.request_id,
        }
    }

    pub fn track_namespace(&self) -> &str {
        match self {
            Self::PublisherInitiated(subscription) => &subscription.track_namespace,
            Self::SubscriberInitiated(subscription) => &subscription.track_namespace,
        }
    }

    pub fn track_name(&self) -> &str {
        match self {
            Self::PublisherInitiated(subscription) => &subscription.track_name,
            Self::SubscriberInitiated(subscription) => &subscription.track_name,
        }
    }

    pub fn track_alias(&self) -> u64 {
        match self {
            Self::PublisherInitiated(subscription) => subscription.track_alias,
            Self::SubscriberInitiated(subscription) => subscription.track_alias,
        }
    }

    pub fn content_exists(&self) -> ContentExists {
        match self {
            Self::PublisherInitiated(subscription) => subscription.content_exists,
            Self::SubscriberInitiated(subscription) => subscription.content_exists,
        }
    }

    pub fn expires(&self) -> Option<u64> {
        match self {
            Self::PublisherInitiated(_) => None,
            Self::SubscriberInitiated(subscription) => Some(subscription.expires),
        }
    }

    pub fn group_order(&self) -> GroupOrder {
        match self {
            Self::PublisherInitiated(subscription) => subscription.group_order,
            Self::SubscriberInitiated(subscription) => subscription.group_order,
        }
    }

    pub fn filter_type(&self) -> FilterType {
        match self {
            Self::PublisherInitiated(subscription) => subscription.filter_type,
            Self::SubscriberInitiated(subscription) => subscription.filter_type,
        }
    }

    pub fn track_properties(&self) -> &TrackProperties {
        match self {
            Self::PublisherInitiated(subscription) => &subscription.track_properties,
            Self::SubscriberInitiated(subscription) => &subscription.track_properties,
        }
    }

    pub fn subscriber_priority(&self) -> u8 {
        match self {
            Self::PublisherInitiated(subscription) => subscription.subscriber_priority,
            Self::SubscriberInitiated(subscription) => subscription.subscriber_priority,
        }
    }

    /// The Forward State of the SUBSCRIBE or PUBLISH_OK. `last_update` holds
    /// the Forward State of a later update.
    pub(crate) fn forward(&self) -> bool {
        match self {
            Self::PublisherInitiated(subscription) => subscription.forward,
            Self::SubscriberInitiated(subscription) => subscription.forward,
        }
    }

    pub(crate) fn last_update(&self) -> &LastSubscribeUpdate {
        match self {
            Self::PublisherInitiated(subscription) => &subscription.last_update,
            Self::SubscriberInitiated(subscription) => &subscription.last_update,
        }
    }

    /// Resolves to the PUBLISH_DONE that ended the subscription, or to `None`
    /// once it ended without one (UNSUBSCRIBE, the session's end).
    pub(crate) fn done(&self) -> impl Future<Output = Option<PublishDoneHandler>> + 'static {
        match self {
            Self::PublisherInitiated(subscription) => subscription.end.done(),
            Self::SubscriberInitiated(subscription) => subscription.end.done(),
        }
    }
}

/// How the publisher ended a subscription this side receives, shared by its
/// clones. The session ends it from the moment the subscription is accepted,
/// so a PUBLISH_DONE read before the caller looks is kept. The default never
/// belonged to a session and has already ended without PUBLISH_DONE.
#[derive(Clone, Debug)]
pub struct SubscriptionEnd(watch::Receiver<Option<PublishDoneHandler>>);

impl Default for SubscriptionEnd {
    fn default() -> Self {
        Self(watch::channel(None).1)
    }
}

impl SubscriptionEnd {
    pub(crate) fn new(receiver: watch::Receiver<Option<PublishDoneHandler>>) -> Self {
        Self(receiver)
    }

    pub(crate) fn done(&self) -> impl Future<Output = Option<PublishDoneHandler>> + 'static {
        let mut end = self.0.clone();
        async move {
            match end.wait_for(Option::is_some).await {
                Ok(done) => done.clone(),
                Err(_) => None,
            }
        }
    }
}

/// The last SUBSCRIBE_UPDATE sent for a subscription, shared by its clones,
/// so a later update repeats its range instead of widening it back to the
/// subscription's filter (draft-14 §9.10).
#[derive(Clone, Debug, Default)]
pub struct LastSubscribeUpdate(Arc<std::sync::Mutex<Option<SubscribeUpdateOption>>>);

impl LastSubscribeUpdate {
    pub(crate) fn get(&self) -> Option<SubscribeUpdateOption> {
        *lock(&self.0)
    }

    pub(crate) fn set(&self, option: SubscribeUpdateOption) {
        *lock(&self.0) = Some(option);
    }
}

#[derive(Clone, Debug)]
pub struct SubscriberInitiatedSubscription {
    pub request_id: u64,
    pub track_namespace: String,
    pub track_name: String,
    pub track_alias: u64,
    pub expires: u64,
    pub group_order: GroupOrder,
    pub content_exists: ContentExists,
    pub subscriber_priority: u8,
    pub forward: bool,
    pub filter_type: FilterType,
    /// What the SUBSCRIBE_OK said about the track.
    pub track_properties: TrackProperties,
    pub last_update: LastSubscribeUpdate,
    pub end: SubscriptionEnd,
}

impl SubscriberInitiatedSubscription {
    pub(crate) fn new(
        track_namespace: String,
        track_name: String,
        subscribe_ok: SubscribeOk,
        option: &SubscribeOption,
        end: SubscriptionEnd,
    ) -> Self {
        Self {
            request_id: subscribe_ok.request_id,
            track_namespace,
            track_name,
            track_alias: subscribe_ok.track_alias,
            expires: subscribe_ok.expires,
            group_order: subscribe_ok.group_order,
            content_exists: subscribe_ok.content_exists,
            subscriber_priority: option.subscriber_priority,
            forward: option.forward,
            filter_type: option.filter_type,
            track_properties: subscribe_ok.track_properties,
            last_update: LastSubscribeUpdate::default(),
            end,
        }
    }

    pub(crate) fn from_subscribe_handler(track_alias: u64, handler: &SubscribeHandler) -> Self {
        Self {
            request_id: handler.request_id(),
            track_namespace: handler.track_namespace.clone(),
            track_name: handler.track_name.clone(),
            track_alias,
            expires: 0,
            group_order: handler.group_order,
            content_exists: ContentExists::False,
            subscriber_priority: handler.subscriber_priority,
            forward: handler.forward,
            filter_type: handler.filter_type,
            track_properties: handler.track_properties.clone(),
            last_update: LastSubscribeUpdate::default(),
            end: SubscriptionEnd::default(),
        }
    }
}

#[derive(Clone, Debug)]
pub struct PublisherInitiatedSubscription {
    pub request_id: u64,
    pub track_namespace: String,
    pub track_name: String,
    pub track_alias: u64,
    pub group_order: GroupOrder,
    pub content_exists: ContentExists,
    pub subscriber_priority: u8,
    pub forward: bool,
    pub filter_type: FilterType,
    pub delivery_timeout: Option<u64>,
    /// Carried by the PUBLISH_OK (draft-18 §10.2.13).
    pub new_group_request: Option<u64>,
    /// What the PUBLISH said about the track.
    pub track_properties: TrackProperties,
    pub last_update: LastSubscribeUpdate,
    pub end: SubscriptionEnd,
}

impl PublisherInitiatedSubscription {
    pub(crate) fn new(
        track_namespace: String,
        track_name: String,
        track_alias: u64,
        publish_ok: PublishOk,
        track_properties: TrackProperties,
    ) -> Self {
        Self {
            track_namespace,
            track_name,
            track_alias,
            request_id: publish_ok.request_id,
            group_order: publish_ok.group_order,
            content_exists: ContentExists::False,
            subscriber_priority: publish_ok.subscriber_priority,
            forward: publish_ok.forward,
            filter_type: publish_ok.filter_type,
            delivery_timeout: publish_ok.delivery_timeout,
            new_group_request: publish_ok.new_group_request,
            track_properties,
            last_update: LastSubscribeUpdate::default(),
            end: SubscriptionEnd::default(),
        }
    }

    pub(crate) fn with_content_exists(mut self, content_exists: ContentExists) -> Self {
        self.content_exists = content_exists;
        self
    }
}
