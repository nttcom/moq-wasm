use std::sync::Arc;

use tokio::sync::{oneshot, watch};

use crate::modules::domain::{
    delivery_control::DeliveryControl, delivery_stats::DeliveryStats,
    publish_done_reason::PublishDoneReason,
};

/// The receiving ends of a consumer's signals, which its egress runner selects on.
pub(crate) struct DownstreamRunnerSignals {
    pub(crate) stop_receiver: oneshot::Receiver<PublishDoneReason>,
    pub(crate) delivery_control_receiver: watch::Receiver<DeliveryControl>,
    pub(crate) delivery_stats: Arc<DeliveryStats>,
}
