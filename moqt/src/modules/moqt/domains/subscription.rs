use crate::{
    FilterType, GroupOrder, SubscribeHandler, TransportProtocol,
    modules::moqt::control_plane::control_messages::messages::{
        parameters::content_exists::ContentExists, publish_ok::PublishOk, subscribe_ok::SubscribeOk,
    },
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
    pub filter_type: FilterType,
    pub delivery_timeout: Option<u64>,
}

impl SubscriberInitiatedSubscription {
    pub(crate) fn new(
        track_namespace: String,
        track_name: String,
        subscribe_ok: SubscribeOk,
        filter_type: FilterType,
    ) -> Self {
        Self {
            request_id: subscribe_ok.request_id,
            track_namespace,
            track_name,
            track_alias: subscribe_ok.track_alias,
            expires: subscribe_ok.expires,
            group_order: subscribe_ok.group_order,
            content_exists: subscribe_ok.content_exists,
            filter_type,
            delivery_timeout: None,
        }
    }

    pub(crate) fn from_subscribe_handler<T: TransportProtocol>(
        track_alias: u64,
        handler: &SubscribeHandler<T>,
    ) -> Self {
        Self {
            request_id: handler.request_id(),
            track_namespace: handler.track_namespace.clone(),
            track_name: handler.track_name.clone(),
            track_alias,
            expires: 0,
            group_order: handler.group_order,
            content_exists: ContentExists::False,
            filter_type: handler.filter_type,
            delivery_timeout: handler.delivery_timeout,
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
}

impl PublisherInitiatedSubscription {
    pub(crate) fn new(
        track_namespace: String,
        track_name: String,
        track_alias: u64,
        publish_ok: PublishOk,
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
            delivery_timeout: None,
        }
    }

    pub(crate) fn with_content_exists(mut self, content_exists: ContentExists) -> Self {
        self.content_exists = content_exists;
        self
    }
}
