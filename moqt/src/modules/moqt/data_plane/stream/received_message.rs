use crate::modules::moqt::control_plane::control_messages::messages::{
    client_setup::ClientSetup, fetch::Fetch, fetch_ok::FetchOk, namespace_ok::NamespaceOk,
    publish::Publish, publish_namespace::PublishNamespace,
    publish_namespace_done::PublishNamespaceDone, publish_ok::PublishOk,
    request_error::RequestError, server_setup::ServerSetup, subscribe::Subscribe,
    subscribe_namespace::SubscribeNamespace, subscribe_ok::SubscribeOk, unsubscribe::Unsubscribe,
    unsubscribe_namespace::UnsubscribeNamespace,
};

pub(crate) enum ReceivedMessage {
    ClientSetup(ClientSetup),
    ServerSetup(ServerSetup),
    PublishNamespace(PublishNamespace),
    PublishNamespaceOk(NamespaceOk),
    PublishNamespaceError(RequestError),
    PublishNamespaceDone(PublishNamespaceDone),
    SubscribeNamespace(SubscribeNamespace),
    SubscribeNamespaceOk(NamespaceOk),
    SubscribeNamespaceError(RequestError),
    UnsubscribeNamespace(UnsubscribeNamespace),
    Publish(Publish),
    PublishOk(PublishOk),
    PublishError(RequestError),
    Subscribe(Subscribe),
    SubscribeOk(SubscribeOk),
    SubscribeError(RequestError),
    Unsubscribe(Unsubscribe),
    Fetch(Fetch),
    FetchOk(FetchOk),
    FetchError(RequestError),
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
            ReceivedMessage::PublishNamespaceDone(_) => "PublishNamespaceDone",
            ReceivedMessage::SubscribeNamespace(_) => "SubscribeNamespace",
            ReceivedMessage::SubscribeNamespaceOk(_) => "SubscribeNamespaceOk",
            ReceivedMessage::SubscribeNamespaceError(_) => "SubscribeNamespaceError",
            ReceivedMessage::UnsubscribeNamespace(_) => "UnsubscribeNamespace",
            ReceivedMessage::Publish(_) => "Publish",
            ReceivedMessage::PublishOk(_) => "PublishOk",
            ReceivedMessage::PublishError(_) => "PublishError",
            ReceivedMessage::Subscribe(_) => "Subscribe",
            ReceivedMessage::SubscribeOk(_) => "SubscribeOk",
            ReceivedMessage::SubscribeError(_) => "SubscribeError",
            ReceivedMessage::Unsubscribe(_) => "Unsubscribe",
            ReceivedMessage::Fetch(_) => "Fetch",
            ReceivedMessage::FetchOk(_) => "FetchOk",
            ReceivedMessage::FetchError(_) => "FetchError",
            ReceivedMessage::FatalError() => "FatalError",
        };

        f.write_str(name)
    }
}
