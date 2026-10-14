use std::path::Path;

use rcgen::{CertifiedKey, generate_simple_self_signed};

#[derive(Debug)]
pub struct CertificateFiles {
    pub cert_path: String,
    pub key_path: String,
}

/// Writes a self-signed certificate for `subject_alt_names` to `dir/cert.pem`
/// and its private key to `dir/key.pem`, creating `dir`. Fails when a name is
/// not a valid subject alt name or `dir` cannot be written.
pub fn write_self_signed_cert(
    dir: &Path,
    subject_alt_names: &[&str],
) -> anyhow::Result<CertificateFiles> {
    let subject_alt_names = subject_alt_names
        .iter()
        .map(|name| name.to_string())
        .collect::<Vec<_>>();
    let CertifiedKey { cert, signing_key } = generate_simple_self_signed(subject_alt_names)?;
    std::fs::create_dir_all(dir)?;
    let cert_path = dir.join("cert.pem");
    let key_path = dir.join("key.pem");
    std::fs::write(&cert_path, cert.pem())?;
    std::fs::write(&key_path, signing_key.serialize_pem())?;
    Ok(CertificateFiles {
        cert_path: cert_path.to_string_lossy().into_owned(),
        key_path: key_path.to_string_lossy().into_owned(),
    })
}
