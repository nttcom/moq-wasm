use anyhow::{Context, Result};
use base64::{engine::general_purpose, Engine as _};
use rand::{rngs::OsRng, RngCore};
use sha1::{Digest, Sha1};
use time::{macros::format_description, OffsetDateTime};

const PASSWORD_DIGEST_TYPE: &str = "http://docs.oasis-open.org/wss/2004/01/oasis-200401-wss-username-token-profile-1.0#PasswordDigest";
const NONCE_ENCODING: &str = "http://docs.oasis-open.org/wss/2004/01/oasis-200401-wss-soap-message-security-1.0#Base64Binary";

pub fn build_wsse_header(username: &str, password: &str) -> Result<String> {
    let created = created_timestamp()?;
    let nonce = generate_nonce(20);
    let nonce_b64 = general_purpose::STANDARD.encode(&nonce);
    let digest_b64 = password_digest(&nonce, &created, password);
    let username = xml_escape(username);

    Ok(format!(
        r#"  <s:Header>
    <Security xmlns="http://docs.oasis-open.org/wss/2004/01/oasis-200401-wss-wssecurity-secext-1.0.xsd" s:mustUnderstand="1">
      <UsernameToken>
        <Username>{}</Username>
        <Password Type="{}">{}</Password>
        <Nonce EncodingType="{}">{}</Nonce>
        <Created xmlns="http://docs.oasis-open.org/wss/2004/01/oasis-200401-wss-wssecurity-utility-1.0.xsd">{}</Created>
      </UsernameToken>
    </Security>
  </s:Header>
"#,
        username, PASSWORD_DIGEST_TYPE, digest_b64, NONCE_ENCODING, nonce_b64, created
    ))
}

fn created_timestamp() -> Result<String> {
    let format =
        format_description!("[year]-[month]-[day]T[hour]:[minute]:[second].000Z");
    OffsetDateTime::now_utc()
        .format(&format)
        .context("failed to format wsse created timestamp")
}

fn generate_nonce(len: usize) -> Vec<u8> {
    let mut bytes = vec![0u8; len];
    OsRng.fill_bytes(&mut bytes);
    bytes
}

fn password_digest(nonce: &[u8], created: &str, password: &str) -> String {
    let mut hasher = Sha1::new();
    hasher.update(nonce);
    hasher.update(created.as_bytes());
    hasher.update(password.as_bytes());
    let digest = hasher.finalize();
    general_purpose::STANDARD.encode(digest)
}

fn xml_escape(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len());
    for ch in value.chars() {
        match ch {
            '&' => escaped.push_str("&amp;"),
            '<' => escaped.push_str("&lt;"),
            '>' => escaped.push_str("&gt;"),
            '"' => escaped.push_str("&quot;"),
            '\'' => escaped.push_str("&apos;"),
            _ => escaped.push(ch),
        }
    }
    escaped
}
