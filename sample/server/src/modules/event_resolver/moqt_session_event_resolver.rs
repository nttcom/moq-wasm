use crate::modules::{
    enums::{FilterType, Location, MOQTMessageReceived},
    types::SessionId,
};

pub(crate) struct MOQTSessionEventResolver;

impl MOQTSessionEventResolver {
    pub(crate) fn resolve(session_id: SessionId, event: moqt::SessionEvent) -> MOQTMessageReceived {
        match event {
            moqt::SessionEvent::PublishNamespace(namespaces) => {
                MOQTMessageReceived::PublishNameSpace(session_id, namespaces)
            }
            moqt::SessionEvent::SubscribeNameSpace(namespaces) => {
                MOQTMessageReceived::SubscribeNameSpace(session_id, namespaces)
            }
            moqt::SessionEvent::Publish(
                namespaces,
                track_name,
                track_alias,
                group_order,
                is_content_exist,
                location,
                is_forward,
                delivery_timeout,
                max_cache_duration,
            ) => {
                let location = location.map(|location| Location {
                    group_id: location.group_id,
                    object_id: location.object_id,
                });
                MOQTMessageReceived::Publish(
                    session_id,
                    namespaces,
                    track_name,
                    track_alias,
                    group_order,
                    is_content_exist,
                    location,
                    is_forward,
                    delivery_timeout,
                    max_cache_duration,
                )
            }
            moqt::SessionEvent::Subscribe(
                namespaces,
                track_name,
                track_alias,
                subscriber_priority,
                group_order,
                is_content_exist,
                is_forward,
                filter_type,
                delivery_timeout,
            ) => {
                let filter_type = match filter_type {
                    moqt::FilterType::LatestGroup => FilterType::LatestGroup,
                    moqt::FilterType::LatestObject => FilterType::LatestObject,
                    moqt::FilterType::AbsoluteStart => FilterType::AbsoluteStart,
                    moqt::FilterType::AbsoluteRange => FilterType::AbsoluteRange,
                };
                MOQTMessageReceived::Subscribe(
                    session_id,
                    namespaces,
                    track_name,
                    track_alias,
                    subscriber_priority,
                    group_order,
                    is_content_exist,
                    is_forward,
                    filter_type,
                    delivery_timeout,
                )
            }
            moqt::SessionEvent::ProtocolViolation() => todo!(),
        }
    }
}
