use tracing::Span;

use crate::modules::domain::{
    delivery_control::{DeliveryControl, UpdateOutcome},
    pub_sub_directory::InMemoryLocalPubSubDirectory,
    session_id::SessionId,
};

pub(crate) struct SubscribeUpdate;

impl SubscribeUpdate {
    /// Applies the Forward State, Start Location and End Group; Subscriber
    /// Priority keeps the value of the SUBSCRIBE. An update that widens the
    /// range is not treated as the PROTOCOL_VIOLATION draft-14 §9.10 allows:
    /// its widening part is ignored.
    #[tracing::instrument(
        level = "info",
        name = "relay.sequence.subscribe_update",
        skip_all,
        parent = session_span,
        fields(session_id = %session_id, subscription_request_id, update = ?update)
    )]
    pub(crate) fn handle(
        &self,
        session_id: SessionId,
        session_span: &Span,
        table: &InMemoryLocalPubSubDirectory,
        subscription_request_id: u64,
        update: DeliveryControl,
    ) {
        match table.update_downstream_delivery(session_id, subscription_request_id, update) {
            Some(UpdateOutcome::Applied) => {
                tracing::info!("downstream subscription updated");
            }
            Some(UpdateOutcome::WideningIgnored) => {
                tracing::warn!(
                    "SUBSCRIBE_UPDATE widens the subscription; only its narrowing part is applied"
                );
            }
            None => tracing::warn!("SUBSCRIBE_UPDATE for no active downstream subscription"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::modules::{
        domain::{
            pub_sub_directory::{DownstreamRunnerSignals, entry::UpstreamSubscriptionOrigin},
            session_peer::SessionPeer,
        },
        test_support::{
            directory_fixtures::{delivery_control, table_with_upstream},
            relay_harness::fixtures::location,
        },
    };

    fn table_with_downstream() -> (InMemoryLocalPubSubDirectory, DownstreamRunnerSignals) {
        let (table, upstream_key) = table_with_upstream(UpstreamSubscriptionOrigin::Subscribe);
        let signals = table
            .register_downstream_subscription(
                2,
                100,
                SessionPeer::Client,
                upstream_key,
                None,
                delivery_control(true),
            )
            .unwrap();
        (table, signals)
    }

    #[test]
    fn forward_off_reaches_the_egress_runner_of_the_subscription() {
        // Arrange
        let (table, signals) = table_with_downstream();

        // Act
        SubscribeUpdate.handle(2, &Span::none(), &table, 100, delivery_control(false));

        // Assert
        assert!(!signals.delivery_control_receiver.borrow().forward);
    }

    #[test]
    fn end_group_reaches_the_egress_runner_of_the_subscription() {
        // Arrange
        let (table, signals) = table_with_downstream();
        let update = DeliveryControl::from_subscribe_update(true, location(0, 0), 4);

        // Act
        SubscribeUpdate.handle(2, &Span::none(), &table, 100, update);

        // Assert
        assert_eq!(
            signals.delivery_control_receiver.borrow().end_group,
            Some(4)
        );
    }

    #[test]
    fn widening_end_group_is_ignored() {
        // Arrange
        let (table, signals) = table_with_downstream();
        let narrowing = DeliveryControl::from_subscribe_update(true, location(0, 0), 4);
        SubscribeUpdate.handle(2, &Span::none(), &table, 100, narrowing);
        let open_ended = DeliveryControl::from_subscribe_update(true, location(0, 0), 0);

        // Act
        SubscribeUpdate.handle(2, &Span::none(), &table, 100, open_ended);

        // Assert
        assert_eq!(
            signals.delivery_control_receiver.borrow().end_group,
            Some(4)
        );
    }

    #[test]
    fn update_from_another_session_leaves_the_subscription_forwarding() {
        // Arrange
        let (table, signals) = table_with_downstream();

        // Act
        SubscribeUpdate.handle(3, &Span::none(), &table, 100, delivery_control(false));

        // Assert
        assert!(signals.delivery_control_receiver.borrow().forward);
    }
}
