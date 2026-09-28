use dashmap::DashMap;
use tokio::sync::broadcast;

use crate::modules::{relay::notifications::subgroup_opened::SubgroupOpened, types::TrackKey};

pub(crate) struct SubgroupOpenedNotifierMap {
    map: DashMap<TrackKey, broadcast::Sender<SubgroupOpened>>,
}

impl SubgroupOpenedNotifierMap {
    pub(crate) fn new() -> Self {
        Self {
            map: DashMap::new(),
        }
    }

    pub(crate) fn get_or_create(&self, track_key: &TrackKey) -> broadcast::Sender<SubgroupOpened> {
        self.map
            .entry(track_key.clone())
            .or_insert_with(|| broadcast::channel(256).0)
            .clone()
    }

    pub(crate) fn remove_unused(&self) {
        // Checked under the shard lock: a channel whose only sender is the map's own and that
        // has no receiver cannot gain a holder except through get_or_create, which takes the
        // same lock, so a recreated channel is the one every later reader and writer shares.
        self.map
            .retain(|_, sender| sender.strong_count() > 1 || sender.receiver_count() > 0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn track_key() -> TrackKey {
        TrackKey::new("ns", "track")
    }

    #[test]
    fn remove_unused_drops_a_channel_nobody_holds() {
        // Arrange
        let notifier_map = SubgroupOpenedNotifierMap::new();
        notifier_map.get_or_create(&track_key());

        // Act
        notifier_map.remove_unused();

        // Assert
        assert!(notifier_map.map.is_empty());
    }

    #[test]
    fn remove_unused_keeps_a_channel_a_writer_holds() {
        // Arrange
        let notifier_map = SubgroupOpenedNotifierMap::new();
        let _writer = notifier_map.get_or_create(&track_key());

        // Act
        notifier_map.remove_unused();

        // Assert
        assert!(notifier_map.map.contains_key(&track_key()));
    }

    #[test]
    fn remove_unused_keeps_a_channel_a_reader_subscribed_to() {
        // Arrange
        let notifier_map = SubgroupOpenedNotifierMap::new();
        let _reader = notifier_map.get_or_create(&track_key()).subscribe();

        // Act
        notifier_map.remove_unused();

        // Assert
        assert!(notifier_map.map.contains_key(&track_key()));
    }
}
