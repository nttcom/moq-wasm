use crate::modules::moqt::control_plane::control_messages::messages::{
    client_setup::ClientSetup, namespace_ok::NamespaceOk, publish::Publish,
    publish_namespace::PublishNamespace, publish_ok::PublishOk, request_error::RequestError,
    server_setup::ServerSetup, subscribe::Subscribe, subscribe_namespace::SubscribeNamespace,
    subscribe_ok::SubscribeOk, unsubscribe::Unsubscribe,
};

pub(crate) enum ReceivedMessage {
    ClientSetup(ClientSetup),
    ServerSetup(ServerSetup),
    PublishNamespace(PublishNamespace),
    PublishNamespaceOk(NamespaceOk),
    PublishNamespaceError(RequestError),
    SubscribeNamespace(SubscribeNamespace),
    SubscribeNamespaceOk(NamespaceOk),
    SubscribeNamespaceError(RequestError),
    Publish(Publish),
    PublishOk(PublishOk),
    PublishError(RequestError),
    Subscribe(Subscribe),
    SubscribeOk(SubscribeOk),
    SubscribeError(RequestError),
    Unsubscribe(Unsubscribe),
    FatalError(),
}

impl std::fmt::Debug for ReceivedMessage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let name = match self {
            ReceivedMessage::ClientSetup(_) => "ClientSetup",
            ReceivedMessage::ServerSetup(_) => "ServerSetup",
            ReceivedMessage::PublishNamespace(_) => "PublishNamespace",
            ReceivedMessage::PublishNamespaceOk(_) => "PublishNamespaceOk",
            ReceivedMessage::PublishNamespaceError(_) => "PublishNamespaceError",
            ReceivedMessage::SubscribeNamespace(_) => "SubscribeNamespace",
            ReceivedMessage::SubscribeNamespaceOk(_) => "SubscribeNamespaceOk",
            ReceivedMessage::SubscribeNamespaceError(_) => "SubscribeNamespaceError",
            ReceivedMessage::Publish(_) => "Publish",
            ReceivedMessage::PublishOk(_) => "PublishOk",
            ReceivedMessage::PublishError(_) => "PublishError",
            ReceivedMessage::Subscribe(_) => "Subscribe",
            ReceivedMessage::SubscribeOk(_) => "SubscribeOk",
            ReceivedMessage::SubscribeError(_) => "SubscribeError",
            ReceivedMessage::Unsubscribe(_) => "Unsubscribe",
            ReceivedMessage::FatalError() => "FatalError",
        };

        f.write_str(name)
    }
}
