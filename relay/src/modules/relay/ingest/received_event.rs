use crate::modules::{core::data_object::DataObject, types::TrackKey};

#[derive(Debug)]
pub(crate) enum ReceivedEvent {
    StreamOpened {
        track_key: TrackKey,
        group_id: u64,
        object: DataObject,
    },
    DatagramOpened {
        track_key: TrackKey,
        group_id: u64,
    },
    Object {
        track_key: TrackKey,
        group_id: u64,
        object: DataObject,
    },
    EndOfGroup {
        track_key: TrackKey,
        group_id: u64,
    },
    DatagramClosed {
        track_key: TrackKey,
    },
}
