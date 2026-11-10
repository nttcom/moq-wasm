use web_transport_quinn::proto::{ConnectRequest, ConnectResponse};

use crate::modules::transport::{
    application_protocol::ApplicationProtocols, connect_target::ConnectTarget,
};

pub(crate) async fn connect_session(
    connection: quinn::Connection,
    target: &ConnectTarget,
    protocols: &ApplicationProtocols,
) -> anyhow::Result<web_transport_quinn::Session> {
    let request = ConnectRequest::new(target.url.clone()).with_protocols(
        protocols
            .web_transport
            .iter()
            .map(|protocol| protocol.to_string()),
    );
    Ok(web_transport_quinn::Session::connect(connection, request).await?)
}

pub(crate) async fn accept_session(
    request: web_transport_quinn::Request,
    protocols: &ApplicationProtocols,
) -> anyhow::Result<web_transport_quinn::Session> {
    let mut response = ConnectResponse::OK;
    response.protocol = protocols
        .select_web_transport(&request.protocols)
        .map(str::to_owned);
    Ok(request.respond(response).await?)
}
