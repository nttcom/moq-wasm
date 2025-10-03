use anyhow::bail;
use bytes::BytesMut;

use crate::modules::moqt::{
    enums::{Authorization, DeliveryTimeout, MaxCacheDuration, SessionEvent},
    messages::{
        control_message_type::ControlMessageType,
        control_messages::{
            publish_namespace::PublishNamespace,
            subscribe_namespace::SubscribeNamespace, util::get_message_type,
            version_specific_parameters::VersionSpecificParameter,
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
            ControlMessageType::GoAway => todo!(),
            ControlMessageType::Subscribe => todo!(),
            ControlMessageType::SubscribeUpdate => todo!(),
            ControlMessageType::UnSubscribe => todo!(),
            ControlMessageType::PublishDone => todo!(),
            ControlMessageType::Publish => todo!(),
            ControlMessageType::Fetch => todo!(),
            ControlMessageType::FetchCancel => todo!(),
            ControlMessageType::TrackStatusRequest => todo!(),
            ControlMessageType::TrackStatus => todo!(),
            ControlMessageType::PublishNamespace => {
                let result = PublishNamespace::depacketize(&mut bytes_mut)?;
                let params = Self::resolve_param(result.parameters);
                Ok(SessionEvent::PublishNameSpace(
                    result.request_id,
                    result.track_namespace,
                    params.0,
                ))
            }
            ControlMessageType::PublishNamespaceDone => todo!(),
            ControlMessageType::PublishNamespaceCancel => todo!(),
            ControlMessageType::SubscribeNamespace => {
                let result = SubscribeNamespace::depacketize(&mut bytes_mut)?;
                let params = Self::resolve_param(result.parameters);
                Ok(SessionEvent::SubscribeNameSpace(
                    result.request_id,
                    result.track_namespace_prefix,
                    params.0,
                ))
            }
            ControlMessageType::UnSubscribeNamespace => todo!(),
            _ => {
                tracing::error!("unknown message: {}", message_type as u8);
                bail!("unknown message")
            }
        }
    }

    fn resolve_param(
        mut params: Vec<VersionSpecificParameter>,
    ) -> (
        Vec<Authorization>,
        Vec<DeliveryTimeout>,
        Vec<MaxCacheDuration>,
    ) {
        let authorization_info = params
            .iter_mut()
            .filter_map(|e| {
                if let VersionSpecificParameter::AuthorizationInfo(param) = e {
                    Some(param.get_value())
                } else {
                    None
                }
            })
            .collect();

        let delivery_timeout = params
            .iter_mut()
            .filter_map(|e| {
                if let VersionSpecificParameter::DeliveryTimeout(param) = e {
                    Some(param.get_value())
                } else {
                    None
                }
            })
            .collect();

        let max_cache_duration = params
            .iter_mut()
            .filter_map(|e| {
                if let VersionSpecificParameter::MaxCacheDuration(param) = e {
                    Some(param.get_value())
                } else {
                    None
                }
            })
            .collect();

        (authorization_info, delivery_timeout, max_cache_duration)
    }
}
