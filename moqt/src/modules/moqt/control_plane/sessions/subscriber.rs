use std::sync::Arc;

use anyhow::bail;

use crate::{
    SubscribeOption, Subscription,
    modules::moqt::control_plane::{
        enums::ResponseMessage,
        messages::{
            control_message_type::ControlMessageType,
            control_messages::{subscribe::Subscribe, subscribe_namespace::SubscribeNamespace},
        },
        sessions::session_context::SessionContext,
        utils,
    },
    modules::moqt::protocol::TransportProtocol,
};

pub struct Subscriber<T: TransportProtocol> {
    pub(crate) session: Arc<SessionContext<T>>,
}

impl<T: TransportProtocol> Subscriber<T> {
    pub async fn subscribe_namespace(&self, namespace: String) -> anyhow::Result<()> {
        let vec_namespace = namespace.split('/').map(|s| s.to_string()).collect();
        let (sender, receiver) = tokio::sync::oneshot::channel::<ResponseMessage>();
        let request_id = self.session.get_request_id();
        self.session
            .sender_map
            .lock()
            .await
            .insert(request_id, sender);
        let publish_namespace = SubscribeNamespace::new(request_id, vec_namespace, vec![]);
        let bytes = utils::create_full_message(
            ControlMessageType::SubscribeNamespace,
            publish_namespace.encode(),
        );
        self.session.send_stream.send(&bytes).await?;
        tracing::info!("Subscribe namespace");
        let result = receiver.await;
        if let Err(e) = result {
            bail!("Failed to receive message: {}", e.to_string())
        }
        let response = result.unwrap();
        match response {
            ResponseMessage::SubscribeNameSpaceOk(response_request_id) => {
                if request_id != response_request_id {
                    bail!("Protocol violation")
                } else {
                    tracing::info!("Subscribe namespace ok");
                    Ok(())
                }
            }
            ResponseMessage::SubscribeNameSpaceError(_, _, _) => {
                tracing::info!("Subscribe namespace error");
                bail!("Subscribe namespace error")
            }
            _ => bail!("Protocol violation"),
        }
    }

    pub async fn subscribe(
        &self,
        track_namespace: String,
        track_name: String,
        option: SubscribeOption,
    ) -> anyhow::Result<Subscription<T>> {
        let vec_namespace = track_namespace.split('/').map(|s| s.to_string()).collect();
        let (sender, receiver) = tokio::sync::oneshot::channel::<ResponseMessage>();
        let request_id = self.session.get_request_id();
        self.session
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
            subscribe_parameters: vec![],
        };
        let bytes = utils::create_full_message(ControlMessageType::Subscribe, subscribe.encode());
        self.session.send_stream.send(&bytes).await?;
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
                    Ok(Subscription::new(self.session.clone(), message))
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
