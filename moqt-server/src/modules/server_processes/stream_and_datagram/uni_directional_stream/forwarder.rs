use crate::modules::{
    buffer_manager::BufferCommand,
    moqt_client::MOQTClient,
    object_cache_storage::{CacheHeader, CacheObject, ObjectCacheStorageWrapper},
    pubsub_relation_manager::wrapper::PubSubRelationManagerWrapper,
};
use anyhow::{bail, Result};
use bytes::BytesMut;
use moqt_core::{
    constants::TerminationErrorCode,
    data_stream_type::DataStreamType,
    messages::{
        control_messages::subscribe::FilterType,
        data_streams::{
            stream_header_subgroup::StreamHeaderSubgroup, stream_header_track::StreamHeaderTrack,
            DataStreams,
        },
    },
    models::tracks::ForwardingPreference,
    pubsub_relation_manager_repository::PubSubRelationManagerRepository,
    variable_integer::write_variable_integer,
};
use std::{sync::Arc, thread, time::Duration};
use tokio::sync::Mutex;
use tracing::{self};

use super::streams::UniSendStream;

pub(crate) async fn forward_object_stream(
    stream: &mut UniSendStream,
    client: Arc<Mutex<MOQTClient>>,
    data_stream_type: DataStreamType,
) -> Result<()> {
    let senders = client.lock().await.senders();
    let sleep_time = Duration::from_millis(10);

    let mut subgroup_header: Option<StreamHeaderSubgroup> = None; // For end group of AbsoluteRange

    let downstream_session_id = stream.stable_id();
    let downstream_stream_id = stream.stream_id();
    let downstream_subscribe_id = stream.subscribe_id();

    let pubsub_relation_manager =
        PubSubRelationManagerWrapper::new(senders.pubsub_relation_tx().clone());

    let mut object_cache_storage =
        ObjectCacheStorageWrapper::new(senders.object_cache_tx().clone());

    // Get the information of the original publisher who has the track being requested
    let (upstream_session_id, upstream_subscribe_id) = pubsub_relation_manager
        .get_related_publisher(downstream_session_id, downstream_subscribe_id)
        .await?;
    let downstream_subscription = pubsub_relation_manager
        .get_downstream_subscription_by_ids(downstream_session_id, downstream_subscribe_id)
        .await?
        .unwrap();
    let downstream_track_alias = downstream_subscription.get_track_alias();
    let filter_type = downstream_subscription.get_filter_type();
    let (start_group, start_object) = downstream_subscription.get_absolute_start();
    let (end_group, end_object) = downstream_subscription.get_absolute_end();

    // Validate the forwarding preference as Track
    match pubsub_relation_manager
        .get_upstream_forwarding_preference(upstream_session_id, upstream_subscribe_id)
        .await?
    {
        Some(ForwardingPreference::Track) => {
            if data_stream_type != DataStreamType::StreamHeaderTrack {
                let msg = std::format!(
                    "uni send stream's data stream type is wrong (expected Track, but got {:?})",
                    data_stream_type
                );
                senders
                    .close_session_tx()
                    .send((
                        u8::from(TerminationErrorCode::InternalError) as u64,
                        msg.clone(),
                    ))
                    .await?;
                bail!(msg)
            }
            pubsub_relation_manager
                .set_downstream_forwarding_preference(
                    downstream_session_id,
                    downstream_subscribe_id,
                    ForwardingPreference::Track,
                )
                .await?;
        }
        Some(ForwardingPreference::Subgroup) => {
            if data_stream_type != DataStreamType::StreamHeaderSubgroup {
                let msg = std::format!(
                    "uni send stream's data stream type is wrong (expected Subgroup, but got {:?})",
                    data_stream_type
                );
                senders
                    .close_session_tx()
                    .send((
                        u8::from(TerminationErrorCode::InternalError) as u64,
                        msg.clone(),
                    ))
                    .await?;
                bail!(msg)
            }
            pubsub_relation_manager
                .set_downstream_forwarding_preference(
                    downstream_session_id,
                    downstream_subscribe_id,
                    ForwardingPreference::Subgroup,
                )
                .await?;
        }
        _ => {
            let msg = "Invalid forwarding preference";
            senders
                .close_session_tx()
                .send((
                    u8::from(TerminationErrorCode::ProtocolViolation) as u64,
                    msg.to_string(),
                ))
                .await?;
            bail!(msg)
        }
    };

    // Get the header from the cache storage and send it to the client
    match object_cache_storage
        .get_header(upstream_session_id, upstream_subscribe_id)
        .await?
    {
        CacheHeader::Track(header) => {
            let mut buf = BytesMut::new();
            let header = StreamHeaderTrack::new(
                downstream_subscribe_id,
                downstream_track_alias,
                header.publisher_priority(),
            )
            .unwrap();

            header.packetize(&mut buf);

            let mut message_buf = BytesMut::with_capacity(buf.len() + 8);
            message_buf.extend(write_variable_integer(
                u8::from(DataStreamType::StreamHeaderTrack) as u64,
            ));
            message_buf.extend(buf);

            if let Err(e) = stream.send_stream.write_all(&message_buf).await {
                tracing::warn!("Failed to write to stream: {:?}", e);
                bail!(e);
            }
        }
        CacheHeader::Subgroup(header) => {
            let mut buf = BytesMut::new();
            let header = StreamHeaderSubgroup::new(
                downstream_subscribe_id,
                downstream_track_alias,
                header.group_id(),
                header.subgroup_id(),
                header.publisher_priority(),
            )
            .unwrap();

            subgroup_header = Some(header.clone());

            header.packetize(&mut buf);

            let mut message_buf = BytesMut::with_capacity(buf.len() + 8);
            message_buf.extend(write_variable_integer(
                u8::from(DataStreamType::StreamHeaderSubgroup) as u64,
            ));
            message_buf.extend(buf);

            if let Err(e) = stream.send_stream.write_all(&message_buf).await {
                tracing::warn!("Failed to write to stream: {:?}", e);
                bail!(e);
            }
        }
        _ => {
            let msg = "cache header not matched";
            senders
                .close_session_tx()
                .send((
                    u8::from(TerminationErrorCode::ProtocolViolation) as u64,
                    msg.to_string(),
                ))
                .await?;
            bail!(msg)
        }
    }

    let mut object_cache_id: Option<usize> = None;

    while object_cache_id.is_none() {
        // Get the first object from the cache storage
        let result = match filter_type {
            FilterType::LatestGroup => {
                object_cache_storage
                    .get_latest_group(upstream_session_id, upstream_subscribe_id)
                    .await
            }
            FilterType::LatestObject => {
                object_cache_storage
                    .get_latest_object(upstream_session_id, upstream_subscribe_id)
                    .await
            }
            FilterType::AbsoluteStart | FilterType::AbsoluteRange => {
                object_cache_storage
                    .get_absolute_object(
                        upstream_session_id,
                        upstream_subscribe_id,
                        start_group.unwrap(),
                        start_object.unwrap(),
                    )
                    .await
            }
        };

        object_cache_id = match result {
            // Send the object to the client if the first cache is exist as track
            Ok(Some((id, CacheObject::Track(object)))) => {
                let mut buf = BytesMut::new();
                object.packetize(&mut buf);

                let mut message_buf = BytesMut::with_capacity(buf.len());
                message_buf.extend(buf);

                if let Err(e) = stream.send_stream.write_all(&message_buf).await {
                    tracing::warn!("Failed to write to stream: {:?}", e);
                    bail!(e);
                }

                Some(id)
            }
            // Send the object to the client if the first cache is exist as subgroup
            Ok(Some((id, CacheObject::Subgroup(object)))) => {
                let mut buf = BytesMut::new();
                object.packetize(&mut buf);

                let mut message_buf = BytesMut::with_capacity(buf.len());
                message_buf.extend(buf);

                if let Err(e) = stream.send_stream.write_all(&message_buf).await {
                    tracing::warn!("Failed to write to stream: {:?}", e);
                    bail!(e);
                }

                Some(id)
            }
            // Will be retried if the first cache is not exist
            Ok(None) => {
                thread::sleep(sleep_time);
                object_cache_id
            }
            _ => {
                let msg = "cache is not exist";
                senders
                    .close_session_tx()
                    .send((
                        u8::from(TerminationErrorCode::InternalError) as u64,
                        msg.to_string(),
                    ))
                    .await?;
                break;
            }
        };
    }

    loop {
        // Get the next object from the cache storage
        object_cache_id = match object_cache_storage
            .get_next_object(
                upstream_session_id,
                upstream_subscribe_id,
                object_cache_id.unwrap(),
            )
            .await
        {
            // Send the object to the client if the next cache is exist as track
            Ok(Some((id, CacheObject::Track(object)))) => {
                let mut buf = BytesMut::new();
                object.packetize(&mut buf);

                let mut message_buf = BytesMut::with_capacity(buf.len());
                message_buf.extend(buf);

                if let Err(e) = stream.send_stream.write_all(&message_buf).await {
                    tracing::warn!("Failed to write to stream: {:?}", e);
                    bail!(e);
                }

                // Judge whether ids are reached to end of the range if the filter type is AbsoluteRange
                if filter_type == FilterType::AbsoluteRange {
                    let is_end = (object.group_id() == end_group.unwrap()
                        && object.object_id() == end_object.unwrap())
                        || (object.group_id() > end_group.unwrap());

                    if is_end {
                        break;
                    }
                }

                Some(id)
            }
            // Send the object to the client if the next cache is exist as subgroup
            Ok(Some((id, CacheObject::Subgroup(object)))) => {
                let mut buf = BytesMut::new();
                object.packetize(&mut buf);

                let mut message_buf = BytesMut::with_capacity(buf.len());
                message_buf.extend(buf);

                if let Err(e) = stream.send_stream.write_all(&message_buf).await {
                    tracing::warn!("Failed to write to stream: {:?}", e);
                    bail!(e);
                }

                // Judge whether ids are reached to end of the range if the filter type is AbsoluteRange
                let header = subgroup_header.as_ref().unwrap();
                if filter_type == FilterType::AbsoluteRange {
                    let is_end = (header.group_id() == end_group.unwrap()
                        && object.object_id() == end_object.unwrap())
                        || (header.group_id() > end_group.unwrap());

                    if is_end {
                        break;
                    }
                }

                Some(id)
            }
            // Will be retried if the next cache is not exist
            Ok(None) => {
                thread::sleep(sleep_time);
                object_cache_id
            }
            _ => {
                let msg = "cache is not exist";
                senders
                    .close_session_tx()
                    .send((
                        u8::from(TerminationErrorCode::InternalError) as u64,
                        msg.to_string(),
                    ))
                    .await?;
                break;
            }
        };
    }

    senders
        .buffer_tx()
        .send(BufferCommand::ReleaseStream {
            session_id: downstream_session_id,
            stream_id: downstream_stream_id,
        })
        .await?;

    Ok(())
}
