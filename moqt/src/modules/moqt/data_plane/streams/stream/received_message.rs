use crate::modules::moqt::control_plane::control_messages::messages::{
    client_setup::ClientSetup, namespace_ok::NamespaceOk, publish::Publish,
    publish_namespace::PublishNamespace, publish_ok::PublishOk, request_error::RequestError,
    server_setup::ServerSetup, subscribe::Subscribe, subscribe_namespace::SubscribeNamespace,
    subscribe_ok::SubscribeOk,
};

#[derive(Debug)]
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
    FatalError(),
}
