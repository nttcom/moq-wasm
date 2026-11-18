use std::{collections::HashMap, sync::Mutex};

use anyhow::{Context, Result, bail, ensure};
use aws_lc_rs::signature::{RSA_PKCS1_2048_8192_SHA256, RsaPublicKeyComponents};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use serde::Deserialize;

pub(crate) const GOOGLE_CERTS_URL: &str = "https://www.googleapis.com/oauth2/v3/certs";
const GOOGLE_ISSUERS: [&str; 2] = ["https://accounts.google.com", "accounts.google.com"];

/// The modulus and exponent of an RSA key, big-endian.
type RsaKey = (Vec<u8>, Vec<u8>);

/// Verifies the Google ID tokens a bot's job gets from its metadata server:
/// RS256 signed by one of Google's published keys, issued by Google for
/// `audience`, not expired, and naming one of `service_accounts` as a verified
/// email.
pub(crate) struct IdTokenVerifier {
    client: reqwest::Client,
    certs_url: String,
    audience: String,
    service_accounts: Vec<String>,
    /// Google's keys by key id, refetched when a token names a key id it does
    /// not hold.
    keys: Mutex<HashMap<String, RsaKey>>,
}

#[derive(Deserialize)]
struct Header {
    alg: String,
    kid: String,
}

#[derive(Deserialize)]
struct Payload {
    iss: String,
    aud: String,
    exp: u64,
    email: Option<String>,
    #[serde(default)]
    email_verified: bool,
}

#[derive(Deserialize)]
struct Certs {
    keys: Vec<Jwk>,
}

#[derive(Deserialize)]
struct Jwk {
    kid: String,
    n: String,
    e: String,
}

impl IdTokenVerifier {
    pub(crate) fn new(certs_url: String, audience: String, service_accounts: Vec<String>) -> Self {
        Self {
            client: reqwest::Client::new(),
            certs_url,
            audience,
            service_accounts,
            keys: Mutex::new(HashMap::new()),
        }
    }

    /// `now` is Unix seconds.
    pub(crate) async fn verify(&self, token: &str, now: u64) -> Result<()> {
        let mut parts = token.split('.');
        let (Some(header), Some(payload), Some(signature), None) =
            (parts.next(), parts.next(), parts.next(), parts.next())
        else {
            bail!("not a JWT");
        };
        let header: Header = decode_json(header)?;
        ensure!(header.alg == "RS256", "unexpected alg {}", header.alg);
        let (n, e) = self.key(&header.kid).await?;
        let signed = &token[..header_and_payload_len(token)];
        RsaPublicKeyComponents { n: &n, e: &e }
            .verify(
                &RSA_PKCS1_2048_8192_SHA256,
                signed.as_bytes(),
                &URL_SAFE_NO_PAD.decode(signature)?,
            )
            .map_err(|_| anyhow::anyhow!("invalid signature"))?;
        let payload: Payload = decode_json(payload)?;
        ensure!(
            GOOGLE_ISSUERS.contains(&payload.iss.as_str()),
            "not issued by Google"
        );
        ensure!(payload.aud == self.audience, "issued for another audience");
        ensure!(payload.exp > now, "expired");
        ensure!(
            payload.email_verified
                && payload
                    .email
                    .is_some_and(|email| self.service_accounts.contains(&email)),
            "not a bots' service account"
        );
        Ok(())
    }

    async fn key(&self, kid: &str) -> Result<RsaKey> {
        if let Some(key) = self.cached_key(kid) {
            return Ok(key);
        }
        let certs: Certs = self
            .client
            .get(&self.certs_url)
            .send()
            .await
            .and_then(reqwest::Response::error_for_status)
            .with_context(|| format!("cannot read {}", self.certs_url))?
            .json()
            .await?;
        let mut keys = HashMap::new();
        for jwk in certs.keys {
            keys.insert(
                jwk.kid,
                (
                    URL_SAFE_NO_PAD.decode(jwk.n)?,
                    URL_SAFE_NO_PAD.decode(jwk.e)?,
                ),
            );
        }
        *self
            .keys
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = keys;
        self.cached_key(kid)
            .with_context(|| format!("no Google key {kid}"))
    }

    fn cached_key(&self, kid: &str) -> Option<RsaKey> {
        self.keys
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .get(kid)
            .cloned()
    }
}

fn header_and_payload_len(token: &str) -> usize {
    token.rfind('.').unwrap_or(token.len())
}

fn decode_json<T: serde::de::DeserializeOwned>(part: &str) -> Result<T> {
    Ok(serde_json::from_slice(&URL_SAFE_NO_PAD.decode(part)?)?)
}

#[cfg(test)]
mod tests {
    use std::net::SocketAddr;

    use aws_lc_rs::{
        rand::SystemRandom,
        rsa::KeySize,
        signature::{KeyPair, RSA_PKCS1_SHA256, RsaKeyPair},
    };
    use http_server::{HttpServer, json_response};
    use hyper::StatusCode;
    use serde_json::{Value, json};
    use tokio::net::TcpListener;

    use super::*;

    const NOW: u64 = 1_800_000_000;
    const AUDIENCE: &str = "https://bots.example";
    const ACCOUNT: &str = "bot@project.iam.gserviceaccount.com";

    struct Google {
        key: RsaKeyPair,
        certs_url: String,
        _server: HttpServer,
    }

    /// Serves the JWKS of a new key as Google's certs endpoint does.
    async fn google() -> Google {
        let key = RsaKeyPair::generate(KeySize::Rsa2048).unwrap();
        let public = key.public_key();
        let jwks = json!({ "keys": [{
            "kid": "k1",
            "kty": "RSA",
            "alg": "RS256",
            "n": URL_SAFE_NO_PAD.encode(public.modulus().big_endian_without_leading_zero()),
            "e": URL_SAFE_NO_PAD.encode(public.exponent().big_endian_without_leading_zero()),
        }]});
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address: SocketAddr = listener.local_addr().unwrap();
        let server = HttpServer::run(listener, move |_, _| {
            let jwks = jwks.clone();
            async move { json_response(StatusCode::OK, &jwks) }
        });
        Google {
            key,
            certs_url: format!("http://{address}/certs"),
            _server: server,
        }
    }

    fn sign(key: &RsaKeyPair, kid: &str, payload: &Value) -> String {
        let header = URL_SAFE_NO_PAD.encode(json!({ "alg": "RS256", "kid": kid }).to_string());
        let payload = URL_SAFE_NO_PAD.encode(payload.to_string());
        let signed = format!("{header}.{payload}");
        let mut signature = vec![0; key.public_modulus_len()];
        key.sign(
            &RSA_PKCS1_SHA256,
            &SystemRandom::new(),
            signed.as_bytes(),
            &mut signature,
        )
        .unwrap();
        format!("{signed}.{}", URL_SAFE_NO_PAD.encode(signature))
    }

    fn claims() -> Value {
        json!({
            "iss": "https://accounts.google.com",
            "aud": AUDIENCE,
            "exp": NOW + 600,
            "email": ACCOUNT,
            "email_verified": true,
        })
    }

    fn verifier(google: &Google) -> IdTokenVerifier {
        IdTokenVerifier::new(
            google.certs_url.clone(),
            AUDIENCE.into(),
            vec![
                "recorder@project.iam.gserviceaccount.com".into(),
                ACCOUNT.into(),
            ],
        )
    }

    #[tokio::test]
    async fn a_token_of_any_listed_service_account_for_the_audience_passes() {
        // Arrange
        let google = google().await;
        let token = sign(&google.key, "k1", &claims());

        // Act / Assert
        verifier(&google).verify(&token, NOW).await.unwrap();
    }

    #[tokio::test]
    async fn a_token_differing_in_any_checked_claim_is_refused() {
        // Arrange
        let google = google().await;
        let verifier = verifier(&google);
        let changes = [
            ("iss", json!("https://evil.example")),
            ("aud", json!("https://other.example")),
            ("exp", json!(NOW)),
            ("email", json!("other@project.iam.gserviceaccount.com")),
            ("email_verified", json!(false)),
        ];

        for (claim, value) in changes {
            let mut payload = claims();
            payload[claim] = value;
            let token = sign(&google.key, "k1", &payload);

            // Act / Assert
            assert!(verifier.verify(&token, NOW).await.is_err(), "{claim}");
        }
    }

    #[tokio::test]
    async fn a_token_signed_by_another_key_or_tampered_with_is_refused() {
        // Arrange
        let google = google().await;
        let verifier = verifier(&google);
        let other = RsaKeyPair::generate(KeySize::Rsa2048).unwrap();
        let forged = sign(&other, "k1", &claims());
        let token = sign(&google.key, "k1", &claims());
        let (signed, signature) = token.rsplit_once('.').unwrap();
        let (header, _) = signed.split_once('.').unwrap();
        let mut payload = claims();
        payload["email"] = json!("other@project.iam.gserviceaccount.com");
        let tampered = format!(
            "{header}.{}.{signature}",
            URL_SAFE_NO_PAD.encode(payload.to_string())
        );

        // Act / Assert
        assert!(verifier.verify(&forged, NOW).await.is_err());
        assert!(verifier.verify(&tampered, NOW).await.is_err());
        assert!(
            verifier
                .verify(&sign(&google.key, "k2", &claims()), NOW)
                .await
                .is_err()
        );
    }
}
