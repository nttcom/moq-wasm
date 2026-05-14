use std::fmt::Debug;

use crate::modules::transport::{
    dual::{
        dual_connection::DualConnection, dual_connection_creator::DualProtocolCreator,
        dual_receive_stream::DualReceiveStream, dual_send_stream::DualSendStream,
    },
    quic::{
        quic_connection::QUICConnection, quic_connection_creator::QUICConnectionCreator,
        quic_receive_stream::QUICReceiveStream, quic_send_stream::QUICSendStream,
    },
    transport_connection::TransportConnection,
    transport_connection_creator::TransportConnectionCreator,
    transport_receive_stream::TransportReceiveStream,
    transport_send_stream::TransportSendStream,
    webtransport::{
        wt_connection::WtConnection, wt_connection_creator::WtConnectionCreator,
        wt_receive_stream::WtReceiveStream, wt_send_stream::WtSendStream,
    },
};

// Prevent `TransportConnectionCreator` from public
#[allow(warnings)]
pub trait TransportProtocol: 'static + Debug {
    type ConnectionCreator: TransportConnectionCreator<Connection = Self::Connection>;
    type Connection: TransportConnection<SendStream = Self::SendStream, ReceiveStream = Self::ReceiveStream>;
    type SendStream: TransportSendStream;
    type ReceiveStream: TransportReceiveStream;
}

// The protocol name should be all upper case.
#[allow(warnings)]
#[derive(Debug)]
pub struct QUIC;

impl TransportProtocol for QUIC {
    type ConnectionCreator = QUICConnectionCreator;
    type Connection = QUICConnection;
    type SendStream = QUICSendStream;
    type ReceiveStream = QUICReceiveStream;
}

#[allow(warnings)]
#[derive(Debug)]
pub struct WEBTRANSPORT;

impl TransportProtocol for WEBTRANSPORT {
    type ConnectionCreator = WtConnectionCreator;
    type Connection = WtConnection;
    type SendStream = WtSendStream;
    type ReceiveStream = WtReceiveStream;
}

#[allow(warnings)]
#[derive(Debug)]
pub struct DUAL;

impl TransportProtocol for DUAL {
    type ConnectionCreator = DualProtocolCreator;
    type Connection = DualConnection;
    type SendStream = DualSendStream;
    type ReceiveStream = DualReceiveStream;
}
