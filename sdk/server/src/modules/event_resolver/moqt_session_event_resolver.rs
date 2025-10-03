use uuid::Uuid;

use crate::modules::enums::SessionEvent;

pub(crate) struct MOQTSessionEventResolver;

impl MOQTSessionEventResolver {
    pub(crate) fn resolve(uuid: Uuid, event: moqt::SessionEvent) -> SessionEvent {
        match event {
            moqt::SessionEvent::PublishNameSpace(request_id, namespaces, param) => {
                SessionEvent::PublishNameSpace(uuid, request_id, namespaces, param)
            }
            moqt::SessionEvent::SubscribeNameSpace(request_id, namespaces, param) => {
                SessionEvent::PublishNameSpace(uuid, request_id, namespaces, param)
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
        }
    }
}
