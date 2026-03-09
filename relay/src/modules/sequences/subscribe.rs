use crate::modules::{
    core::handler::{publish::PublishHandler, subscribe::SubscribeHandler},
    enums::ContentExists,
    event_resolver::stream_binder::StreamBinder,
    sequences::{notifier::Notifier, tables::table::Table},
    types::SessionId,
};

pub(crate) struct Subscribe;

impl Subscribe {
    pub(crate) async fn handle(
        &self,
        session_id: SessionId,
        table: &dyn Table,
        notifier: &Notifier,
        stream_handler: &mut StreamBinder,
        handler: Box<dyn SubscribeHandler>,
    ) {
        tracing::info!("SequenceHandler::subscribe: {}", session_id);
        let track_namespace = handler.track_namespace();
        let track_name = handler.track_name();
        let full_track_namespace = format!("{}:{}", track_namespace, track_name);
        tracing::debug!("New track '{}' is subscribed.", full_track_namespace,);

        let pub_handler = table
            .find_publish_handler_with(handler.track_namespace(), handler.track_name())
            .await;
        if let Some((pub_session_id, pub_handler)) = pub_handler {
            self.subscribe_active_publish(
                session_id,
                pub_session_id,
                pub_handler.as_ref(),
                handler.as_ref(),
                stream_handler,
            )
            .await;
        } else if let Some(pub_session_id) = table.get_publish_namespace(track_namespace) {
            self.relay_subscribe(
                session_id,
                pub_session_id,
                track_namespace,
                track_name,
                notifier,
                stream_handler,
                handler.as_ref(),
            )
            .await;
        } else {
            self.response_error(handler.as_ref()).await;
        }
        tracing::info!("SequenceHandler::subscribe: {} DONE", session_id);
    }

    async fn subscribe_active_publish(
        &self,
        session_id: SessionId,
        pub_session_id: SessionId,
        pub_handler: &dyn PublishHandler,
        handler: &dyn SubscribeHandler,
        stream_handler: &mut StreamBinder,
    ) {
        tracing::info!(
            "publisher found. {}/{} (alias {})",
            pub_handler.track_namespace(),
            pub_handler.track_name(),
            pub_handler.track_alias()
        );
        let subscription = pub_handler.into_subscription(0);

        if handler
            .ok(
                subscription.track_alias(),
                subscription.expires(),
                subscription.content_exists(),
            )
            .await
            .is_ok()
        {
            tracing::info!("send `SUBSCRIBE_OK` ok");
            let track_alias = subscription.track_alias();
            let _ = stream_handler
                .bind_by_subscribe(
                    session_id,
                    subscription,
                    pub_session_id,
                    handler.into_publication(track_alias),
                )
                .await;
        } else {
            tracing::error!("Failed to send `SUBSCRIBE_OK`. Session close.");
        }
    }

    #[allow(warnings)]
    async fn relay_subscribe(
        &self,
        session_id: SessionId,
        pub_session_id: SessionId,
        track_namespace: &str,
        track_name: &str,
        notifier: &Notifier,
        stream_handler: &mut StreamBinder,
        handler: &dyn SubscribeHandler,
    ) {
        if let Ok(subscription) = notifier
            .subscribe(
                pub_session_id,
                track_namespace.to_string(),
                track_name.to_string(),
            )
            .await
        {
            tracing::info!("send `SUBSCRIBE_OK` ok");
            if handler
                .ok(
                    subscription.track_alias(),
                    subscription.expires(),
                    ContentExists::False,
                )
                .await
                .is_ok()
            {
                let pub_resource = handler.into_publication(subscription.track_alias());
                stream_handler
                    .bind_by_subscribe(session_id, subscription, pub_session_id, pub_resource)
                    .await;
            } else {
                tracing::error!("Failed to send `SUBSCRIBE_OK`. Session close.");
                // TODO: send_unsubscribe
                // TODO: close session
            }
        } else {
            tracing::warn!("Failed to send `SUBSCRIBE_OK`. Session close.");
        }
    }

    async fn response_error(&self, handler: &dyn SubscribeHandler) {
        let _ = handler
            .error(
                0,
                "Designated namespace and track name do not exist.".to_string(),
            )
            .await
            .inspect(|_| tracing::info!("send `SUBSCRIBE_ERROR` ok"))
            .inspect_err(|_| tracing::error!("Failed to send `SUBSCRIBE_ERROR`. Session close."));
    }
}
