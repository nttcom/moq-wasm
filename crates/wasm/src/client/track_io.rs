use std::rc::Rc;

use moqt::{
    BROWSER, DataReceiver, Fetch as FetchData, FetchHandle, Session, StreamDataReceiver, Subgroup,
    Subscription, wire::SubgroupHeader,
};
use wasm_bindgen_futures::spawn_local;

use super::{
    ClientShared,
    callbacks::{
        emit_fetch_object, emit_fetch_stream_end, emit_object_datagram, emit_subgroup_header,
        emit_subgroup_object,
    },
};
use crate::messages::FetchStreamEndMessage;

/// Forwards every object the peer sends on `subscription` to the JavaScript
/// callbacks until the subscription ends.
pub(crate) async fn read_track(
    shared: Rc<ClientShared>,
    session: Rc<Session<BROWSER>>,
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
            while let Ok(datagram) = receiver.receive().await {
                emit_object_datagram(&shared.callbacks, datagram);
            }
        }
    }
}

async fn read_subgroup_stream(shared: Rc<ClientShared>, mut stream: StreamDataReceiver<BROWSER>) {
    let mut header: Option<SubgroupHeader> = None;
    let mut last_object_id = None;
    loop {
        match stream.receive().await {
            Ok(Some(Subgroup::Header(received))) => {
                emit_subgroup_header(&shared.callbacks, &received);
                header = Some(received);
            }
            Ok(Some(Subgroup::Object(field))) => {
                let Some(header) = &header else {
                    continue;
                };
                let object_id = field.resolve_object_id(last_object_id);
                last_object_id = Some(object_id);
                emit_subgroup_object(&shared.callbacks, header, field, object_id);
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
    session: Rc<Session<BROWSER>>,
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
                Ok(FetchData::Header(_)) => continue,
                Ok(FetchData::Object(field)) => {
                    emit_fetch_object(&shared.callbacks, request_id, &field);
                }
                Ok(FetchData::End) => break FetchStreamEndMessage::finished(request_id),
                Err(_) => break FetchStreamEndMessage::reset(request_id, 0),
            }
        },
        Err(_) => FetchStreamEndMessage::reset(request_id, 0),
    };
    shared.fetch_requests.borrow_mut().remove(&request_id);
    emit_fetch_stream_end(&shared.callbacks, end);
}
