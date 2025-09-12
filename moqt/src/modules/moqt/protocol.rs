use crate::modules::transport::{
    quic::{quic_connection::QUICConnection, quic_connection_creator::QUICConnectionCreator, quic_receive_stream::QUICReceiveStream, quic_send_stream::QUICSendStream},
    transport_connection::TransportConnection,
    transport_connection_creator::TransportConnectionCreator,
    transport_receive_stream::TransportReceiveStream, transport_send_stream::TransportSendStream,
};

pub trait TransportProtocol {
    type ConnectionCreator: TransportConnectionCreator;
    type Connection: TransportConnection;
    type SendStream: TransportSendStream;
    type ReceiveStream: TransportReceiveStream;
}

pub struct QUIC;

impl TransportProtocol for QUIC {
    type ConnectionCreator = QUICConnectionCreator;
    type Connection = QUICConnection;
    type SendStream = QUICSendStream;
    type ReceiveStream = QUICReceiveStream;
}
