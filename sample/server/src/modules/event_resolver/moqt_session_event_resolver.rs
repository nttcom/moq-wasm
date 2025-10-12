use crate::modules::{enums::SessionEvent, types::SessionId};

pub(crate) struct MOQTSessionEventResolver;

impl MOQTSessionEventResolver {
    pub(crate) fn resolve(session_id: SessionId, event: moqt::SessionEvent) -> SessionEvent {
        match event {
            moqt::SessionEvent::PublishNamespace(namespaces) => {
                SessionEvent::PublishNameSpace(session_id, namespaces)
            }
            moqt::SessionEvent::SubscribeNameSpace(namespaces) => {
                SessionEvent::SubscribeNameSpace(session_id, namespaces)
            }
            moqt::SessionEvent::Publish(
                request_id,
                namespaces,
                track_alias,
                group_order,
                is_content_exist,
                is_forward,
                authorization,
                delivery_timeout,
                max_cache_duration,
            ) => todo!(),
            moqt::SessionEvent::Subscribe(
                request_id,
                namespaces,
                subscriber_priority,
                group_order,
                is_content_exist,
                is_forward,
                filter_type,
                authorization,
                delivery_timeout,
            ) => todo!(),
            moqt::SessionEvent::FatalError() => todo!(),
        }
    }
}
