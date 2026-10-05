use std::{fmt::Debug, task::Poll};

use tokio::io::ReadBuf;

use crate::modules::{
    executor::{MaybeSend, MaybeSync},
    transport::read_error::ReadError,
};

#[cfg_attr(test, mockall::automock)]
pub(crate) trait TransportReceiveStream:
    MaybeSend + MaybeSync + 'static + Debug + Unpin
{
    fn poll_read<'a, 'b>(
        &mut self,
        cx: &mut std::task::Context<'a>,
        buf: &mut ReadBuf<'b>,
    ) -> Poll<Result<(), ReadError>>;
}
