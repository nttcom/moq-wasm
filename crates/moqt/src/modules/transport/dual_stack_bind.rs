use std::net::{Ipv6Addr, SocketAddr};

/// Runs `bind` on `[::]:port`.
pub(crate) fn bind_dual_stack<T>(
    port: u16,
    bind: impl FnOnce(SocketAddr) -> anyhow::Result<T>,
) -> anyhow::Result<T> {
    bind(SocketAddr::from((Ipv6Addr::UNSPECIFIED, port)))
}
