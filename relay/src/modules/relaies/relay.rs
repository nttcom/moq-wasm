use std::collections::VecDeque;

use crate::modules::{
    core::{datagram_receiver::DatagramReceiver, datagram_sender::DatagramSender},
    relaies::relay_properties::RelayProperties,
};

pub(crate) struct Relay {
    pub(crate) relay_properties: RelayProperties,
}

impl Relay {
    pub(crate) fn add_object_receiver(
        &mut self,
        mut datagram_receiver: Box<dyn DatagramReceiver>,
        object_datagram: moqt::ObjectDatagram,
    ) {
        let track_alias = datagram_receiver.track_alias();
        self.initialize_if_needed(track_alias);
        self.relay_properties
            .object_queue
            .get_mut(&track_alias)
            .unwrap()
            .push_back(object_datagram);
        let queue = self.relay_properties.object_queue.clone();
        self.relay_properties.joinset.spawn(async move {
            tracing::info!("add object receiver");
            while let Ok(datagram_object) = datagram_receiver.receive_object().await {
                tracing::info!("receive object");
                let queue = queue.get_mut(&datagram_object.track_alias);
                if queue.is_none() {
                    tracing::error!(
                        "Track alias {} not found in object queue",
                        datagram_object.track_alias
                    );
                    break;
                }
                queue.unwrap().push_back(datagram_object);
            }
        });
    }

    pub(crate) fn add_object_sender(
        &mut self,
        track_alias: u64,
        datagram_sender: Box<dyn DatagramSender>,
    ) {
        self.initialize_if_needed(track_alias);
        let mut receiver = self
            .relay_properties
            .sender_map
            .get(&track_alias)
            .unwrap()
            .subscribe();

        self.relay_properties.joinset.spawn(async move {
            tracing::info!("add object sender");
            while let Ok(datagram_object) = receiver.recv().await {
                tracing::info!("receive object: datagram sender");
                if let Err(e) = datagram_sender.send(datagram_object).await {
                    tracing::error!(
                        "Failed to send datagram object to subscriber for {}: {:?}",
                        track_alias,
                        e
                    );
                }
            }
        });
    }

    fn initialize_if_needed(&mut self, track_alias: u64) {
        let mut queue_check_flag = false;
        let mut sender_check_flag = false;

        self.relay_properties
            .object_queue
            .entry(track_alias)
            .or_insert_with({
                queue_check_flag = true;
                VecDeque::new
            });
        self.relay_properties
            .sender_map
            .entry(track_alias)
            .or_insert_with(|| {
                sender_check_flag = true;
                let (sender, _) = tokio::sync::broadcast::channel::<moqt::ObjectDatagram>(1024);
                sender
            });
        if queue_check_flag && sender_check_flag {
            let object_queue = self.relay_properties.object_queue.clone();
            let sender_map = self.relay_properties.sender_map.clone();
            self.relay_properties.joinset.spawn(async move {
                loop {
                    let queue = object_queue.get_mut(&track_alias);
                    if queue.is_none() {
                        tracing::error!("Track alias {} not found in object queue", track_alias);
                        break;
                    }
                    let mut queue = queue.unwrap();
                    let object = queue.pop_front();
                    if let Some(object) = object {
                        let sender = sender_map.get(&track_alias);
                        if let Some(sender) = sender {
                            if let Err(e) = sender.send(object.clone()) {
                                tracing::error!(
                                    "Failed to send datagram object to broadcast channel for {}: {:?}",
                                    track_alias,
                                    e
                                );
                                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                                continue;
                            }
                        } else {
                            tracing::error!("Track alias {} not found in sender map", track_alias);
                            break;
                        }
                    } else {
                        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                    }
                    // tokio::task::yield_now().await;
                }
            });
        }
    }
}
