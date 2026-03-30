use dashmap::DashMap;
use tokio::{sync::mpsc, task::JoinHandle};

use crate::modules::{
    core::data_receiver::{
        datagram_receiver::DatagramReceiver, receiver::DataReceiver,
        stream_receiver::StreamReceiver,
    },
    relay::ingest::{
        datagram_reader::DatagramReader, received_event::ReceivedEvent,
        stream_reader::StreamReader,
    },
    types::TrackKey,
};

pub(crate) struct ReceiverRegistry {
    sender: mpsc::Sender<ReceivedEvent>,
    tasks: DashMap<TrackKey, Vec<JoinHandle<()>>>,
}

impl ReceiverRegistry {
    pub(crate) fn new(sender: mpsc::Sender<ReceivedEvent>) -> Self {
        Self {
            sender,
            tasks: DashMap::new(),
        }
    }

    pub(crate) fn register_data_receiver(&self, track_key: TrackKey, receiver: DataReceiver) {
        match receiver {
            DataReceiver::Stream(stream_receiver) => {
                self.register_stream(track_key, stream_receiver);
            }
            DataReceiver::Datagram(datagram_receiver) => {
                self.register_datagram(track_key, datagram_receiver, None);
            }
        }
    }

    pub(crate) fn register_stream(&self, track_key: TrackKey, receiver: Box<dyn StreamReceiver>) {
        let handle = StreamReader::spawn(track_key, receiver, self.sender.clone());
        self.push_handle(track_key, handle);
    }

    pub(crate) fn register_datagram(
        &self,
        track_key: TrackKey,
        receiver: Box<dyn DatagramReceiver>,
        initial_group_id: Option<u64>,
    ) {
        let handle = DatagramReader::spawn(track_key, receiver, self.sender.clone(), initial_group_id);
        self.push_handle(track_key, handle);
    }

    fn push_handle(&self, track_key: TrackKey, handle: JoinHandle<()>) {
        let mut handles = self.tasks.entry(track_key).or_default();
        handles.push(handle);
    }
}