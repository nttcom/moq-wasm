//! QUIC stream identifiers (RFC 9000 §2.1): bit 0 names the initiator, bit 1
//! the direction, the rest is the stream's index within that kind.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Role {
    Client,
    Server,
}

impl Role {
    fn initiator_bit(self) -> u64 {
        match self {
            Role::Client => 0,
            Role::Server => 1,
        }
    }

    pub(crate) fn opened_by_me(self, stream_id: u64) -> bool {
        stream_id & 0x1 == self.initiator_bit()
    }

    /// The id of the `index`-th stream of `kind` this side opens.
    pub(crate) fn stream_id(self, kind: StreamKind, index: u64) -> u64 {
        (index << 2) | kind.direction_bit() | self.initiator_bit()
    }

    /// The id of the `index`-th stream of `kind` the peer opens.
    pub(crate) fn peer_stream_id(self, kind: StreamKind, index: u64) -> u64 {
        let peer = match self {
            Role::Client => Role::Server,
            Role::Server => Role::Client,
        };
        peer.stream_id(kind, index)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum StreamKind {
    Bidirectional,
    Unidirectional,
}

impl StreamKind {
    fn direction_bit(self) -> u64 {
        match self {
            StreamKind::Bidirectional => 0,
            StreamKind::Unidirectional => 0x2,
        }
    }

    pub(crate) fn of(stream_id: u64) -> Self {
        if stream_id & 0x2 == 0 {
            StreamKind::Bidirectional
        } else {
            StreamKind::Unidirectional
        }
    }
}

pub(crate) fn stream_index(stream_id: u64) -> u64 {
    stream_id >> 2
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stream_ids_follow_the_quic_numbering() {
        // Act / Assert
        assert_eq!(Role::Client.stream_id(StreamKind::Bidirectional, 0), 0);
        assert_eq!(Role::Server.stream_id(StreamKind::Bidirectional, 0), 1);
        assert_eq!(Role::Client.stream_id(StreamKind::Unidirectional, 0), 2);
        assert_eq!(Role::Server.stream_id(StreamKind::Unidirectional, 1), 7);
        assert!(Role::Client.opened_by_me(4));
        assert!(!Role::Client.opened_by_me(5));
        assert_eq!(StreamKind::of(6), StreamKind::Unidirectional);
        assert_eq!(stream_index(7), 1);
    }
}
