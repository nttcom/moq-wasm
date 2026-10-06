use std::fmt::Debug;

#[cfg(target_arch = "wasm32")]
use crate::modules::transport::browser::browser_connection_creator::BrowserConnectionCreator;
use crate::modules::transport::transport_connection_creator::TransportConnectionCreator;
#[cfg(not(target_arch = "wasm32"))]
use crate::modules::transport::{
    dual::dual_connection_creator::DualProtocolCreator,
    quic::quic_connection_creator::QUICConnectionCreator,
    webtransport::wt_connection_creator::WtConnectionCreator,
};

// Prevent `TransportConnectionCreator` from public
#[allow(warnings)]
pub trait TransportProtocol: 'static + Debug {
    type ConnectionCreator: TransportConnectionCreator;
}

// The protocol name should be all upper case.
#[cfg(not(target_arch = "wasm32"))]
#[allow(warnings)]
#[derive(Debug)]
pub struct QUIC;

#[cfg(not(target_arch = "wasm32"))]
impl TransportProtocol for QUIC {
    type ConnectionCreator = QUICConnectionCreator;
}

#[cfg(not(target_arch = "wasm32"))]
#[allow(warnings)]
#[derive(Debug)]
pub struct WEBTRANSPORT;

#[cfg(not(target_arch = "wasm32"))]
impl TransportProtocol for WEBTRANSPORT {
    type ConnectionCreator = WtConnectionCreator;
}

#[cfg(not(target_arch = "wasm32"))]
#[allow(warnings)]
#[derive(Debug)]
pub struct DUAL;

#[cfg(not(target_arch = "wasm32"))]
impl TransportProtocol for DUAL {
    type ConnectionCreator = DualProtocolCreator;
}

#[cfg(target_arch = "wasm32")]
#[allow(warnings)]
#[derive(Debug)]
pub struct BROWSER;

#[cfg(target_arch = "wasm32")]
impl TransportProtocol for BROWSER {
    type ConnectionCreator = BrowserConnectionCreator;
}
