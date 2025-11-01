use std::sync::Arc;

use anyhow::bail;
use tracing_subscriber::filter;

use crate::{
    FilterType, GroupOrder, SubscribeOption, Subscription, TransportProtocol,
    modules::moqt::{
        enums::ResponseMessage,
        messages::{
            control_message_type::ControlMessageType,
            control_messages::{
                location::Location, namespace_ok::NamespaceOk, publish::Publish,
                publish_ok::PublishOk, request_error::RequestError, subscribe::Subscribe,
            },
        },
        sessions::session_context::SessionContext,
        utils,
    },
};

#[derive(Debug, Clone)]
pub struct PublishHandler<T: TransportProtocol> {
    session_context: Arc<SessionContext<T>>,
    request_id: u64,
    pub track_namespace: String,
    pub track_name: String,
    pub track_alias: u64,
    pub group_order: GroupOrder,
    pub content_exists: bool,
    pub largest_location: Option<Location>,
    pub forward: bool,
    pub authorization_token: Option<String>,
    pub max_cache_duration: Option<u64>,
    pub delivery_timeout: Option<u64>,
}

impl<T: TransportProtocol> PublishHandler<T> {
    pub(crate) fn new(session_context: Arc<SessionContext<T>>, publish_message: Publish) -> Self {
        Self {
            session_context,
            request_id: publish_message.request_id,
            track_namespace: publish_message.track_namespace_tuple.join("/"),
            track_name: publish_message.track_name,
            track_alias: publish_message.track_alias,
            group_order: publish_message.group_order,
            content_exists: publish_message.content_exists,
            largest_location: publish_message.largest_location,
            forward: publish_message.forward,
            authorization_token: None,
            max_cache_duration: None,
            delivery_timeout: None,
        }
    }

    pub async fn ok(&self, subscriber_priority: u8, filter_type: FilterType) -> anyhow::Result<()> {
        let publish_ok = PublishOk {
            request_id: self.request_id,
            forward: self.forward,
            subscriber_priority,
            group_order: self.group_order,
            filter_type,
            start_location: None,
            end_group: None,
            parameters: vec![],
        };
        let bytes = utils::create_full_message(ControlMessageType::PublishOk, publish_ok);
        self.session_context.send_stream.send(&bytes).await
    }

    pub async fn error(&self, error_code: u64, reason_phrase: String) -> anyhow::Result<()> {
        let err = RequestError {
            // TODO: assign correct request id.
            request_id: self.request_id,
            error_code,
            reason_phrase,
        };
        let bytes = utils::create_full_message(ControlMessageType::PublishError, err);
        self.session_context.send_stream.send(&bytes).await
    }

    pub async fn subscribe(
        &self,
        track_namespace: String,
        track_name: String,
        option: SubscribeOption,
    ) -> anyhow::Result<Subscription<T>> {
        let vec_namespace = track_namespace.split('/').map(|s| s.to_string()).collect();
        let (sender, receiver) = tokio::sync::oneshot::channel::<ResponseMessage>();
        let request_id = self.session_context.get_request_id();
        self.session_context
            .sender_map
            .lock()
            .await
            .insert(request_id, sender);
        let subscribe = Subscribe {
            request_id,
            track_namespace: vec_namespace,
            track_name,
            subscriber_priority: option.subscriber_priority,
            group_order: option.group_order,
            forward: option.forward,
            filter_type: option.filter_type,
            start_location: option.start_location,
            end_group: option.end_group,
            subscribe_parameters: vec![],
        };
        let bytes = utils::create_full_message(ControlMessageType::Subscribe, subscribe);
        self.session_context.send_stream.send(&bytes).await?;
        tracing::info!("Subscribe");
        let result = receiver.await;
        if let Err(e) = result {
            bail!("Failed to receive message: {}", e.to_string())
        }
        let response = result.unwrap();
        match response {
            ResponseMessage::SubscribeOk(message) => {
                if request_id != message.request_id {
                    bail!("Protocol violation")
                } else {
                    tracing::info!("Subscribe ok");
                    Ok(Subscription::new(self.session_context.clone(), message))
                }
            }
            ResponseMessage::SubscribeError(_, _, _) => {
                tracing::info!("Subscribe error");
                bail!("Subscribe error")
            }
            _ => bail!("Protocol violation"),
        }
    }
}
