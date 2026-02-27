use std::{fmt::Debug, task::Poll};

use async_trait::async_trait;
use bytes::BytesMut;
use mockall::automock;

use crate::modules::transport::read_error::ReadError;

#[automock]
#[async_trait]
pub(crate) trait TransportReceiveStream: Send + Sync + 'static + Debug + Unpin {
    async fn receive(&mut self, buffer: &mut BytesMut) -> anyhow::Result<Option<usize>>;
    fn poll_read<'a>(
        &mut self,
        cx: &mut std::task::Context<'a>,
        buf: &mut BytesMut,
    ) -> Poll<Result<usize, ReadError>>;
}
