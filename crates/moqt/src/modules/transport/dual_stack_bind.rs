use std::{
    io::ErrorKind,
    net::{Ipv6Addr, SocketAddr},
};

pub(crate) const EPHEMERAL_PORT_ATTEMPTS: u32 = 8;

/// Runs `bind` on `[::]:port`.
///
/// XNU binds `[::]:P` even while another socket holds `0.0.0.0:P` (port 0
/// can pick such a P), and IPv4 datagrams for P then reach that other socket.
/// On Apple targets the IPv4 port is therefore held while `bind` runs; once
/// the dual-stack socket exists, XNU refuses IPv4 binds to P.
///
/// XNU picks that IPv4 port without regard to sockets bound to a specific
/// IPv6 address, such as a browser's QUIC connections, so binding `[::]:P`
/// can fail with `AddrInUse`; for port 0, `bind` then runs again on a new number.
pub(crate) fn bind_dual_stack<T>(
    port: u16,
    mut bind: impl FnMut(SocketAddr) -> anyhow::Result<T>,
) -> anyhow::Result<T> {
    let mut attempts = 1;
    loop {
        match bind_once(port, &mut bind) {
            Err(error)
                if port == 0 && attempts < EPHEMERAL_PORT_ATTEMPTS && is_address_in_use(&error) =>
            {
                tracing::debug!(%error, "drawing another ephemeral port");
                attempts += 1;
            }
            result => return result,
        }
    }
}

fn bind_once<T>(
    port: u16,
    bind: &mut impl FnMut(SocketAddr) -> anyhow::Result<T>,
) -> anyhow::Result<T> {
    #[cfg(target_vendor = "apple")]
    let ipv4_reservation = std::net::UdpSocket::bind((std::net::Ipv4Addr::UNSPECIFIED, port))?;
    #[cfg(target_vendor = "apple")]
    let port = ipv4_reservation.local_addr()?.port();
    bind(SocketAddr::from((Ipv6Addr::UNSPECIFIED, port)))
}

fn is_address_in_use(error: &anyhow::Error) -> bool {
    error.chain().any(|cause| {
        cause
            .downcast_ref::<std::io::Error>()
            .is_some_and(|error| error.kind() == ErrorKind::AddrInUse)
    })
}

#[cfg(test)]
mod tests {
    use std::{
        io::ErrorKind,
        net::{Ipv4Addr, UdpSocket},
    };

    use super::{EPHEMERAL_PORT_ATTEMPTS, bind_dual_stack};
    use crate::modules::test_support::HANDSHAKE_TIMEOUT;

    fn in_use() -> anyhow::Error {
        std::io::Error::from(ErrorKind::AddrInUse).into()
    }

    #[test]
    fn port_zero_draws_another_number_while_the_first_is_in_use() {
        // Arrange
        let mut attempts = 0;

        // Act
        let socket = bind_dual_stack(0, |address| {
            attempts += 1;
            if attempts == 1 {
                return Err(in_use());
            }
            Ok(UdpSocket::bind(address)?)
        });

        // Assert
        assert!(socket.is_ok());
        assert_eq!(attempts, 2);
    }

    #[test]
    fn port_zero_gives_up_once_every_drawn_number_is_in_use() {
        // Arrange
        let mut attempts = 0;

        // Act
        let result = bind_dual_stack(0, |_| -> anyhow::Result<()> {
            attempts += 1;
            Err(in_use())
        });

        // Assert
        assert!(result.is_err());
        assert_eq!(attempts, EPHEMERAL_PORT_ATTEMPTS);
    }

    #[test]
    fn a_fixed_port_in_use_is_not_drawn_again() {
        // Arrange
        let port = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0))
            .unwrap()
            .local_addr()
            .unwrap()
            .port();
        let mut attempts = 0;

        // Act
        let result = bind_dual_stack(port, |_| -> anyhow::Result<()> {
            attempts += 1;
            Err(in_use())
        });

        // Assert
        assert!(result.is_err());
        assert_eq!(attempts, 1);
    }

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
