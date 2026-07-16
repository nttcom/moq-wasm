use std::{fmt::Debug, task::Poll};

use async_trait::async_trait;
use bytes::BytesMut;

use crate::modules::transport::read_error::ReadError;

#[cfg_attr(test, mockall::automock)]
#[async_trait]
pub(crate) trait TransportReceiveStream: Send + Sync + 'static + Debug + Unpin {
    fn poll_read<'a>(
        &mut self,
        cx: &mut std::task::Context<'a>,
        buf: &mut BytesMut,
    ) -> Poll<Result<usize, ReadError>>;
}
