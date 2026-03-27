use crate::modules::core::{
    data_object::DataObject, data_receiver::receiver::DataReceiver, subscriber::Subscriber,
    subscription::Subscription,
};

pub(crate) enum ReceivedData {
    Data {
        track_key: u128,
        group_id: u64,
        data_object: DataObject,
    },
    EndOfStream {
        track_key: u128,
        group_id: u64,
    },
}

pub(crate) struct ReceiverMonitor {
    join_handle: tokio::task::JoinHandle<()>,
}

impl ReceiverMonitor {
    pub(crate) async fn run(
        subscriber: Box<dyn Subscriber>,
        subscription: Box<dyn Subscription>,
        track_key: u128,
        sender: tokio::sync::mpsc::Sender<DataReceiver>,
    ) -> Self {
        let join_handle = tokio::spawn(async move {
            let data_receiver = subscriber.create_data_receiver(&subscription).await?;
            match data_receiver {
                DataReceiver::Stream(receiver) => {}
                DataReceiver::Datagram(receiver) => {}
            }
        });
        ReceiverMonitor { join_handle }
    }
}
