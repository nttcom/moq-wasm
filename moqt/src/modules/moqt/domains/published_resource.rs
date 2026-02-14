use crate::{
    FilterType, GroupOrder, SubscribeHandler, TransportProtocol,
    modules::moqt::control_plane::messages::control_messages::publish_ok::PublishOk,
};

pub struct PublishedResource {
    pub track_namespace: String,
    pub track_name: String,
    pub track_alias: u64,
    pub group_order: GroupOrder,
    pub subscriber_priority: u8,
    pub forward: bool,
    pub filter_type: FilterType,
    pub delivery_timeout: Option<u64>,
}

impl PublishedResource {
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
            group_order: publish_ok.group_order,
            subscriber_priority: publish_ok.subscriber_priority,
            forward: publish_ok.forward,
            filter_type: publish_ok.filter_type,
            delivery_timeout: None,
        }
    }

    pub(crate) fn from_subscribe_handler<T: TransportProtocol>(
        track_alias: u64,
        handler: &SubscribeHandler<T>,
    ) -> Self {
        Self {
            track_namespace: handler.track_namespace.clone(),
            track_name: handler.track_name.clone(),
            track_alias,
            group_order: handler.group_order,
            subscriber_priority: handler.subscriber_priority,
            forward: handler.forward,
            filter_type: handler.filter_type,
            delivery_timeout: None,
        }
    }
}
