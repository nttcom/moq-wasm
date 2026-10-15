use std::net::{Ipv6Addr, SocketAddr};

/// Runs `bind` on `[::]:port`.
///
/// XNU binds `[::]:P` even while another socket holds `0.0.0.0:P` (port 0
/// can pick such a P), and IPv4 datagrams for P then reach that other socket.
/// On Apple targets the IPv4 port is therefore held while `bind` runs; once
/// the dual-stack socket exists, XNU refuses IPv4 binds to P.
pub(crate) fn bind_dual_stack<T>(
    port: u16,
    bind: impl FnOnce(SocketAddr) -> anyhow::Result<T>,
) -> anyhow::Result<T> {
    #[cfg(target_vendor = "apple")]
    let ipv4_reservation = std::net::UdpSocket::bind((std::net::Ipv4Addr::UNSPECIFIED, port))?;
    #[cfg(target_vendor = "apple")]
    let port = ipv4_reservation.local_addr()?.port();
    bind(SocketAddr::from((Ipv6Addr::UNSPECIFIED, port)))
}

#[cfg(test)]
mod tests {
    use std::net::{Ipv4Addr, UdpSocket};

    use super::bind_dual_stack;
    use crate::modules::test_support::HANDSHAKE_TIMEOUT;

    #[test]
    fn a_port_held_by_an_ipv4_socket_is_not_bound() {
        // Arrange
        let ipv4_holder = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0)).unwrap();
        let port = ipv4_holder.local_addr().unwrap().port();

        // Act
        let result = bind_dual_stack(port, |address| Ok(UdpSocket::bind(address)?));

        // Assert
        assert!(result.is_err());
    }

    #[test]
    fn a_free_port_receives_ipv4_datagrams() {
        // Arrange
        let server = bind_dual_stack(0, |address| Ok(UdpSocket::bind(address)?)).unwrap();
        server.set_read_timeout(Some(HANDSHAKE_TIMEOUT)).unwrap();
        let port = server.local_addr().unwrap().port();
        let client = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0)).unwrap();

        // Act
        client
            .send_to(b"ping", (Ipv4Addr::LOCALHOST, port))
            .unwrap();

        // Assert
        let mut buffer = [0; 4];
        assert_eq!(server.recv(&mut buffer).unwrap(), 4);
    }
}
