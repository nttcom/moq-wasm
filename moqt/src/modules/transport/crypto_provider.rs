use std::sync::Once;

use quinn::rustls;

static INSTALL_DEFAULT_PROVIDER: Once = Once::new();

pub(crate) fn install_default_crypto_provider() {
    INSTALL_DEFAULT_PROVIDER.call_once(|| {
        let _ = rustls::crypto::ring::default_provider().install_default();
    });
}
