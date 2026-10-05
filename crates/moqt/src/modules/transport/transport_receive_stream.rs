use std::{fmt::Debug, task::Poll};

use async_trait::async_trait;
use tokio::io::ReadBuf;

use crate::modules::transport::read_error::ReadError;

#[cfg_attr(test, mockall::automock)]
#[async_trait]
pub(crate) trait TransportReceiveStream: Send + Sync + 'static + Debug + Unpin {
    fn poll_read<'a, 'b>(
        &mut self,
        cx: &mut std::task::Context<'a>,
        buf: &mut ReadBuf<'b>,
    ) -> Poll<Result<(), ReadError>>;
}
