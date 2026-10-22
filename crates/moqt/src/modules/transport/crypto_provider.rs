use quinn::rustls;

pub(crate) fn install_default_crypto_provider() {
    // install_default fails when a provider is already installed (ours or the application's); keep that one.
    let _ = rustls::crypto::ring::default_provider().install_default();
}
