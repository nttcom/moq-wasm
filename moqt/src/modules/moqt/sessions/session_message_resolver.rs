use anyhow::bail;
use bytes::BytesMut;

use crate::modules::moqt::{
    enums::SessionEvent,
    messages::{
        control_message_type::ControlMessageType,
        control_messages::{
            publish_namespace::PublishNamespace, subscribe_namespace::SubscribeNamespace,
            util::get_message_type,
        },
        moqt_message::MOQTMessage,
    },
};

pub(crate) struct SessionMessageResolver;

impl SessionMessageResolver {
    pub(crate) fn resolve_message(binary_message: Vec<u8>) -> anyhow::Result<SessionEvent> {
        let mut bytes_mut = BytesMut::from(binary_message.as_slice());
        let message_type = get_message_type(&mut bytes_mut)?;
        match message_type {
            ControlMessageType::SubscribeUpdate => todo!(),
            ControlMessageType::Subscribe => todo!(),
            ControlMessageType::PublishNamespace => {
                let result = PublishNamespace::depacketize(&mut bytes_mut)?;
                Ok(SessionEvent::PublishNameSpace(
                    result.request_id,
                    result.track_namespace,
                ))
            }
            ControlMessageType::UnAnnounce => todo!(),
            ControlMessageType::UnSubscribe => todo!(),
            ControlMessageType::SubscribeDone => todo!(),
            ControlMessageType::AnnounceCancel => todo!(),
            ControlMessageType::TrackStatusRequest => todo!(),
            ControlMessageType::TrackStatus => todo!(),
            ControlMessageType::GoAway => todo!(),
            ControlMessageType::SubscribeNamespace => {
                let result = SubscribeNamespace::depacketize(&mut bytes_mut)?;
                Ok(SessionEvent::SubscribeNameSpace(
                    result.request_id,
                    result.track_namespace_prefix,
                ))
            }
            ControlMessageType::UnSubscribeAnnounces => todo!(),
            ControlMessageType::MaxSubscribeId => todo!(),
            ControlMessageType::Fetch => todo!(),
            ControlMessageType::FetchCancel => todo!(),
            _ => {
                tracing::error!("unknown message: {}", message_type as u8);
                bail!("unknown message")
            },
        }
    }
}
