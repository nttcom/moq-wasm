use std::{net::IpAddr, str::FromStr};

use anyhow::{Context, Result, bail};

/// An IP address or a CIDR block (`192.0.2.0/24`, `2001:db8::/32`), as a
/// trunk lists the addresses it signals from.
pub(crate) struct AddressRange {
    network: IpAddr,
    prefix_length: u32,
}

impl AddressRange {
    pub(crate) fn contains(&self, address: IpAddr) -> bool {
        match (self.network, address.to_canonical()) {
            (IpAddr::V4(network), IpAddr::V4(address)) => masked(
                network.to_bits().into(),
                address.to_bits().into(),
                32,
                self.prefix_length,
            ),
            (IpAddr::V6(network), IpAddr::V6(address)) => masked(
                network.to_bits(),
                address.to_bits(),
                128,
                self.prefix_length,
            ),
            _ => false,
        }
    }
}

/// Whether the first `prefix_length` of the `width` bits are equal.
fn masked(network: u128, address: u128, width: u32, prefix_length: u32) -> bool {
    let ignored = width - prefix_length;
    network.checked_shr(ignored).unwrap_or(0) == address.checked_shr(ignored).unwrap_or(0)
}

impl FromStr for AddressRange {
    type Err = anyhow::Error;

    fn from_str(text: &str) -> Result<Self> {
        let (address, prefix_length) = match text.split_once('/') {
            Some((address, prefix_length)) => (address, Some(prefix_length)),
            None => (text, None),
        };
        let network: IpAddr = address
            .trim()
            .parse()
            .with_context(|| format!("{text} is not an IP address or CIDR block"))?;
        let width = if network.is_ipv4() { 32 } else { 128 };
        let prefix_length = match prefix_length {
            Some(prefix_length) => prefix_length
                .trim()
                .parse()
                .with_context(|| format!("{text} has no prefix length"))?,
            None => width,
        };
        if prefix_length > width {
            bail!("{text} has a prefix longer than its address");
        }
        Ok(Self {
            network,
            prefix_length,
        })
    }
}

/// Parses a comma-separated list, ignoring empty entries.
pub(crate) fn parse_ranges(text: &str) -> Result<Vec<AddressRange>> {
    text.split(',')
        .filter(|entry| !entry.trim().is_empty())
        .map(str::parse)
        .collect()
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::*;

    #[test]
    fn a_range_holds_the_addresses_sharing_its_prefix() {
        // Arrange
        let block: AddressRange = "54.172.60.0/30".parse().unwrap();
        let single: AddressRange = "192.0.2.7".parse().unwrap();
        let v6: AddressRange = "2001:db8::/32".parse().unwrap();

        // Act / Assert
        assert!(block.contains("54.172.60.3".parse().unwrap()));
        assert!(!block.contains("54.172.60.4".parse().unwrap()));
        assert!(single.contains("192.0.2.7".parse().unwrap()));
        assert!(single.contains("::ffff:192.0.2.7".parse().unwrap()));
        assert!(!single.contains("192.0.2.8".parse().unwrap()));
        assert!(v6.contains("2001:db8:1::1".parse().unwrap()));
        assert!(!v6.contains("2001:db9::1".parse().unwrap()));
        assert!(!v6.contains("192.0.2.7".parse().unwrap()));
    }

    #[test]
    fn a_list_rejects_what_is_not_an_address_or_block() {
        // Act / Assert
        assert_eq!(parse_ranges(" 192.0.2.0/24, ,::1").unwrap().len(), 2);
        assert!(parse_ranges("192.0.2.0/33").is_err());
        assert!(parse_ranges("192.0.2.0/").is_err());
        assert!(parse_ranges("trunk.example").is_err());
    }

    proptest! {
        /// The model: compare the bits of the prefix one by one.
        #[test]
        fn containment_compares_the_prefix_bits(
            network in any::<u32>(),
            // Mostly one flipped bit, so the prefix decides; else equal or unrelated.
            flipped in prop_oneof![
                4 => (0u32..32).prop_map(|bit| 1u32 << bit),
                1 => Just(0u32),
                1 => any::<u32>(),
            ],
            prefix_length in prop_oneof![Just(0u32), Just(1), Just(31), Just(32), 0u32..=32],
        ) {
            let address = network ^ flipped;
            let range = AddressRange {
                network: IpAddr::from(network.to_be_bytes()),
                prefix_length,
            };
            let expected = (0..prefix_length)
                .all(|bit| (network >> (31 - bit)) & 1 == (address >> (31 - bit)) & 1);
            prop_assert_eq!(range.contains(IpAddr::from(address.to_be_bytes())), expected);
        }
    }
}
