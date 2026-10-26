use std::rc::Rc;

use moqt::{
    Session, Subscription,
    fetch::{Fetch as FetchData, FetchHandle},
    object::{DataReceiver, StreamDataReceiver, Subgroup},
};
use wasm_bindgen_futures::spawn_local;

use super::{ClientShared, callbacks::emit};
use crate::messages::{
    FetchObjectMessage, FetchStreamEndMessage, ObjectDatagramMessage, ObjectDatagramStatusMessage,
    SubgroupHeaderMessage, SubgroupObjectMessage,
};

/// Forwards every object the peer sends on `subscription` to the JavaScript
/// callbacks until the subscription ends.
pub(crate) async fn read_track(
    shared: Rc<ClientShared>,
    session: Rc<Session>,
    subscription: Subscription,
) {
    let receiver = match session
        .subscriber()
        .accept_data_receiver(&subscription)
        .await
    {
        Ok(receiver) => receiver,
        Err(_) => return,
    };
    match receiver {
        DataReceiver::Stream(mut factory) => {
            while let Ok(stream) = factory.next().await {
                spawn_local(read_subgroup_stream(shared.clone(), stream));
            }
        }
        DataReceiver::Datagram(mut receiver) => {
            let track_alias = receiver.track_alias;
            while let Ok(object) = receiver.receive().await {
                if object.payload.is_empty() {
                    let message = ObjectDatagramStatusMessage::new(track_alias, object);
                    emit(
                        &shared.callbacks,
                        |c| &c.object_datagram_status,
                        &[message.into()],
                    );
                } else {
                    let message = ObjectDatagramMessage::new(track_alias, object);
                    emit(&shared.callbacks, |c| &c.object_datagram, &[message.into()]);
                }
            }
        }
    }
}

async fn read_subgroup_stream(shared: Rc<ClientShared>, mut stream: StreamDataReceiver) {
    let track_alias = stream.track_alias;
    loop {
        match stream.receive().await {
            Ok(Some(Subgroup::Header(start))) => {
                let message = SubgroupHeaderMessage::new(track_alias, &start);
                emit(&shared.callbacks, |c| &c.subgroup_header, &[message.into()]);
            }
            Ok(Some(Subgroup::Object(object))) => {
                let group_id = object.location.group_id;
                let args = [
                    js_sys::BigInt::from(track_alias).into(),
                    js_sys::BigInt::from(group_id).into(),
                    SubgroupObjectMessage::new(object).into(),
                ];
                emit(&shared.callbacks, |c| &c.subgroup_object, &args);
            }
            Ok(None) | Err(_) => break,
        }
    }
}

/// Forwards the objects of an accepted FETCH to the JavaScript callbacks and
/// reports how the stream ended. `request_id` is the id JavaScript chose for
/// the request, not the one on the wire.
pub(crate) async fn read_fetch(
    shared: Rc<ClientShared>,
    session: Rc<Session>,
    request_id: u64,
    fetch_handle: FetchHandle,
) {
    let end = match session
        .subscriber()
        .accept_fetch_receiver(&fetch_handle)
        .await
    {
        Ok(mut receiver) => loop {
            match receiver.receive().await {
                Ok(FetchData::Object(object)) => {
                    let message = FetchObjectMessage::new(request_id, &object);
                    emit(&shared.callbacks, |c| &c.fetch_object, &[message.into()]);
                }
                Ok(FetchData::End) => break FetchStreamEndMessage::finished(request_id),
                Err(_) => break FetchStreamEndMessage::reset(request_id, 0),
            }
        },
        Err(_) => FetchStreamEndMessage::reset(request_id, 0),
    };
    shared.fetch_requests.borrow_mut().remove(&request_id);
    emit(&shared.callbacks, |c| &c.fetch_stream_end, &[end.into()]);
}
