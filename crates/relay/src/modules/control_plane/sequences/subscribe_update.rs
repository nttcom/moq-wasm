use tracing::Span;

use crate::modules::domain::{
    pub_sub_directory::InMemoryLocalPubSubDirectory, session_id::SessionId,
};

pub(crate) struct SubscribeUpdate;

impl SubscribeUpdate {
    /// Only the Forward State is applied; Start Location, End Group and
    /// Subscriber Priority keep the values of the SUBSCRIBE.
    #[tracing::instrument(
        level = "info",
        name = "relay.sequence.subscribe_update",
        skip_all,
        parent = session_span,
        fields(session_id = %session_id, subscription_request_id, forward)
    )]
    pub(crate) fn handle(
        &self,
        session_id: SessionId,
        session_span: &Span,
        table: &InMemoryLocalPubSubDirectory,
        subscription_request_id: u64,
        forward: bool,
    ) {
        if table.update_downstream_forward(session_id, subscription_request_id, forward) {
            tracing::info!("downstream subscription forward state updated");
        } else {
            tracing::warn!("SUBSCRIBE_UPDATE for no active downstream subscription");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::modules::{
        domain::{pub_sub_directory::entry::UpstreamSubscriptionOrigin, session_peer::SessionPeer},
        test_support::directory_fixtures::table_with_upstream,
    };

    #[test]
    fn forward_off_reaches_the_egress_runner_of_the_subscription() {
        // Arrange
        let (table, upstream_key) = table_with_upstream(UpstreamSubscriptionOrigin::Subscribe);
        let signals = table
            .register_downstream_subscription(2, 100, SessionPeer::Client, upstream_key, None)
            .unwrap();

        // Act
        SubscribeUpdate.handle(2, &Span::none(), &table, 100, false);

        // Assert
        assert!(!*signals.forward_receiver.borrow());
    }

    #[test]
    fn update_from_another_session_leaves_the_subscription_forwarding() {
        // Arrange
        let (table, upstream_key) = table_with_upstream(UpstreamSubscriptionOrigin::Subscribe);
        let signals = table
            .register_downstream_subscription(2, 100, SessionPeer::Client, upstream_key, None)
            .unwrap();

        // Act
        SubscribeUpdate.handle(3, &Span::none(), &table, 100, false);

        // Assert
        assert!(*signals.forward_receiver.borrow());
    }
}
