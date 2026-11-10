use quinn::rustls;

pub(crate) fn install_default_crypto_provider() {
    // install_default fails when a provider is already installed (ours or the application's); keep that one.
    let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
}

#[cfg(test)]
mod tests {
    use quinn::rustls::{NamedGroup, crypto::aws_lc_rs::default_provider};

    #[test]
    fn the_provider_offers_the_post_quantum_key_share_first() {
        // Arrange
        let provider = default_provider();

        // Act
        let first_group = provider.kx_groups[0].name();

        // Assert
        assert_eq!(first_group, NamedGroup::X25519MLKEM768);
    }
}
