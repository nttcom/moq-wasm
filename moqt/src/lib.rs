use crate::modules::moqt::sessions::session_creator::SessionCreator;
use crate::modules::transport::transport_connection_creator::TransportConnectionCreator;
use std::net::SocketAddr;

mod modules;

pub use crate::modules::moqt::messages::control_messages::enums::FilterType;
pub use crate::modules::moqt::messages::control_messages::group_order::GroupOrder;
pub use modules::moqt::enums::SessionEvent;
pub use modules::moqt::enums::{
    Authorization, ContentExists, DeliveryTimeout, Forward, MaxCacheDuration, RequestId,
    SubscriberPriority, TrackNamespace,
};
pub use modules::moqt::handler::publish_handler::PublishHandler;
pub use modules::moqt::handler::publish_namespace_handler::PublishNamespaceHandler;
pub use modules::moqt::handler::subscribe_handler::SubscribeHandler;
pub use modules::moqt::handler::subscribe_namespace_handler::SubscribeNamespaceHandler;
pub use modules::moqt::messages::control_messages::location::Location;
pub use modules::moqt::messages::object::datagram_object::DatagramObject;
pub use modules::moqt::options::PublishOption;
pub use modules::moqt::options::SubscribeOption;
pub use modules::moqt::protocol::QUIC;
pub use modules::moqt::protocol::TransportProtocol;
pub use modules::moqt::sessions::publication::Publication;
pub use modules::moqt::sessions::publisher::Publisher;
pub use modules::moqt::sessions::session::Session;
pub use modules::moqt::sessions::subscriber::Subscriber;
pub use modules::moqt::sessions::subscription::Acceptance;
pub use modules::moqt::sessions::subscription::Subscription;
pub use modules::moqt::streams::datagram::datagram_receiver::DatagramReceiver;
pub use modules::moqt::streams::datagram::datagram_sender::DatagramHeader;
pub use modules::moqt::streams::datagram::datagram_sender::DatagramSender;

pub struct ServerConfig {
    pub port: u16,
    pub cert_path: String,
    pub key_path: String,
    pub keep_alive_interval_sec: u64,
    // log_level: String,
}

pub struct Endpoint<T: TransportProtocol> {
    session_creator: SessionCreator<T>,
}

impl<T: TransportProtocol> Endpoint<T> {
    pub fn create_client(port_num: u16) -> anyhow::Result<Self> {
        let client = T::ConnectionCreator::client(port_num)?;
        let session_creator = SessionCreator {
            transport_creator: client,
        };
        Ok(Self { session_creator })
    }

    pub fn create_client_with_custom_cert(
        port_num: u16,
        custom_cert_path: &str,
    ) -> anyhow::Result<Self> {
        let client = T::ConnectionCreator::client_with_custom_cert(port_num, custom_cert_path)?;
        let session_creator = SessionCreator {
            transport_creator: client,
        };
        Ok(Self { session_creator })
    }

    pub fn create_server(server_config: ServerConfig) -> anyhow::Result<Self> {
        let server = T::ConnectionCreator::server(
            server_config.cert_path,
            server_config.key_path,
            server_config.port,
            server_config.keep_alive_interval_sec,
        )?;
        let session_creator = SessionCreator {
            transport_creator: server,
        };
        Ok(Self { session_creator })
    }

    pub async fn connect(
        &self,
        remote_address: SocketAddr,
        host: &str,
    ) -> anyhow::Result<Session<T>> {
        self.session_creator
            .create_new_connection(remote_address, host)
            .await
    }

    pub async fn accept(&mut self) -> anyhow::Result<Session<T>> {
        self.session_creator.accept_new_connection().await
    }
}
