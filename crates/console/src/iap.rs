use std::{
    collections::HashMap,
    time::{Duration, Instant},
};

use anyhow::{Context, anyhow, bail};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use reqwest::{Client, Url};
use ring::signature::{ECDSA_P256_SHA256_FIXED, UnparsedPublicKey};
use serde::{Deserialize, de::DeserializeOwned};
use tokio::sync::Mutex;

pub const GOOGLE_IAP_KEYS_URL: &str = "https://www.gstatic.com/iap/verify/public_key-jwk";
const ISSUER: &str = "https://cloud.google.com/iap";
const CLOCK_SKEW_SECONDS: u64 = 30;
const MIN_KEY_REFRESH_INTERVAL: Duration = Duration::from_secs(60);

pub struct IapVerifier {
    audience: String,
    keys: IapKeys,
}

#[derive(Deserialize)]
struct Header {
    alg: String,
    kid: String,
}

#[derive(Deserialize)]
struct Claims {
    iss: String,
    aud: String,
    sub: String,
    iat: u64,
    exp: u64,
}

impl IapVerifier {
    pub fn new(audience: String, keys_url: Url) -> Self {
        Self {
            audience,
            keys: IapKeys {
                http: Client::new(),
                url: keys_url,
                cache: Mutex::new(KeyCache::default()),
            },
        }
    }

    /// Returns the `sub` of an IAP assertion. Fails when the assertion is malformed, is not
    /// ES256-signed by a key IAP currently publishes, names another issuer or audience, or lies
    /// outside its `iat` / `exp` window by more than 30 seconds.
    pub async fn verify(&self, assertion: &str, now: u64) -> anyhow::Result<String> {
        let [header_segment, payload_segment, signature_segment] = split(assertion)?;
        let header: Header = decode_json(header_segment)?;
        if header.alg != "ES256" {
            bail!(
                "the IAP assertion is signed with {:?}, not ES256",
                header.alg
            );
        }
        let key = self.keys.get(&header.kid).await?;
        let signature = URL_SAFE_NO_PAD
            .decode(signature_segment)
            .context("the IAP assertion signature is not base64url")?;
        let signing_input = &assertion[..header_segment.len() + 1 + payload_segment.len()];
        UnparsedPublicKey::new(&ECDSA_P256_SHA256_FIXED, &key)
            .verify(signing_input.as_bytes(), &signature)
            .map_err(|_| anyhow!("the IAP assertion signature does not verify"))?;
        let claims: Claims = decode_json(payload_segment)?;
        if claims.iss != ISSUER {
            bail!("the IAP assertion is issued by {:?}", claims.iss);
        }
        if claims.aud != self.audience {
            bail!("the IAP assertion is for {:?}", claims.aud);
        }
        if claims.exp + CLOCK_SKEW_SECONDS < now {
            bail!("the IAP assertion expired at {}", claims.exp);
        }
        if claims.iat > now + CLOCK_SKEW_SECONDS {
            bail!(
                "the IAP assertion is issued in the future at {}",
                claims.iat
            );
        }
        if claims.sub.is_empty() {
            bail!("the IAP assertion has an empty sub");
        }
        Ok(claims.sub)
    }
}

struct IapKeys {
    http: Client,
    url: Url,
    cache: Mutex<KeyCache>,
}

#[derive(Default)]
struct KeyCache {
    keys: HashMap<String, Vec<u8>>,
    fetched_at: Option<Instant>,
}

#[derive(Deserialize)]
struct KeySet {
    keys: Vec<Jwk>,
}

#[derive(Deserialize)]
struct Jwk {
    kid: String,
    kty: String,
    crv: Option<String>,
    x: Option<String>,
    y: Option<String>,
}

impl IapKeys {
    async fn get(&self, kid: &str) -> anyhow::Result<Vec<u8>> {
        let mut cache = self.cache.lock().await;
        if let Some(key) = cache.keys.get(kid) {
            return Ok(key.clone());
        }
        if cache
            .fetched_at
            .is_some_and(|fetched_at| fetched_at.elapsed() < MIN_KEY_REFRESH_INTERVAL)
        {
            bail!("the IAP key {kid:?} is unknown");
        }
        cache.keys = self.fetch().await?;
        cache.fetched_at = Some(Instant::now());
        cache
            .keys
            .get(kid)
            .cloned()
            .with_context(|| format!("the IAP key {kid:?} is unknown"))
    }

    async fn fetch(&self) -> anyhow::Result<HashMap<String, Vec<u8>>> {
        let key_set: KeySet = self
            .http
            .get(self.url.clone())
            .send()
            .await?
            .error_for_status()?
            .json()
            .await
            .context("the IAP keys are not a JWK set")?;
        key_set
            .keys
            .into_iter()
            .filter_map(
                |jwk| match (jwk.kty.as_str(), jwk.crv.as_deref(), jwk.x, jwk.y) {
                    ("EC", Some("P-256"), Some(x), Some(y)) => Some((jwk.kid, x, y)),
                    _ => None,
                },
            )
            .map(|(kid, x, y)| Ok((kid, uncompressed_point(&x, &y)?)))
            .collect()
    }
}

fn uncompressed_point(x: &str, y: &str) -> anyhow::Result<Vec<u8>> {
    let mut point = vec![0x04];
    point.extend(
        URL_SAFE_NO_PAD
            .decode(x)
            .context("an IAP key x is not base64url")?,
    );
    point.extend(
        URL_SAFE_NO_PAD
            .decode(y)
            .context("an IAP key y is not base64url")?,
    );
    if point.len() != 65 {
        bail!("an IAP key is not a P-256 point");
    }
    Ok(point)
}

fn split(assertion: &str) -> anyhow::Result<[&str; 3]> {
    assertion
        .split('.')
        .collect::<Vec<_>>()
        .try_into()
        .map_err(|_| anyhow!("the IAP assertion is not a compact JWS"))
}

fn decode_json<T: DeserializeOwned>(segment: &str) -> anyhow::Result<T> {
    let bytes = URL_SAFE_NO_PAD
        .decode(segment)
        .context("an IAP assertion segment is not base64url")?;
    serde_json::from_slice(&bytes).context("an IAP assertion segment is not the expected JSON")
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::test_support::{AUDIENCE, FakeIapKeys, NOW, TestIapKey, iap_claims};

    fn verifier(keys: &FakeIapKeys) -> IapVerifier {
        IapVerifier::new(AUDIENCE.into(), keys.url())
    }

    #[tokio::test]
    async fn returns_the_sub_of_a_valid_assertion() {
        // Arrange
        let key = TestIapKey::new("key-1");
        let keys = FakeIapKeys::run(&[&key]).await;
        let assertion = key.sign(&iap_claims("accounts.google.com:1"));

        // Act
        let sub = verifier(&keys).verify(&assertion, NOW).await.unwrap();

        // Assert
        assert_eq!(sub, "accounts.google.com:1");
    }

    #[tokio::test]
    async fn rejects_an_assertion_for_another_audience() {
        // Arrange
        let key = TestIapKey::new("key-1");
        let keys = FakeIapKeys::run(&[&key]).await;
        let mut claims = iap_claims("accounts.google.com:1");
        claims["aud"] = json!("/projects/1/locations/r/services/other");

        // Act / Assert
        assert!(
            verifier(&keys)
                .verify(&key.sign(&claims), NOW)
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn rejects_an_assertion_from_another_issuer() {
        // Arrange
        let key = TestIapKey::new("key-1");
        let keys = FakeIapKeys::run(&[&key]).await;
        let mut claims = iap_claims("accounts.google.com:1");
        claims["iss"] = json!("https://accounts.google.com");

        // Act / Assert
        assert!(
            verifier(&keys)
                .verify(&key.sign(&claims), NOW)
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn accepts_clock_skew_up_to_30_seconds_and_no_more() {
        // Arrange
        let key = TestIapKey::new("key-1");
        let keys = FakeIapKeys::run(&[&key]).await;
        let verifier = verifier(&keys);
        let mut claims = iap_claims("accounts.google.com:1");
        claims["exp"] = json!(NOW);
        let assertion = key.sign(&claims);

        // Act / Assert
        assert!(verifier.verify(&assertion, NOW + 30).await.is_ok());
        assert!(verifier.verify(&assertion, NOW + 31).await.is_err());
    }

    #[tokio::test]
    async fn rejects_an_assertion_issued_in_the_future() {
        // Arrange
        let key = TestIapKey::new("key-1");
        let keys = FakeIapKeys::run(&[&key]).await;
        let mut claims = iap_claims("accounts.google.com:1");
        claims["iat"] = json!(NOW + 31);
        claims["exp"] = json!(NOW + 600);

        // Act / Assert
        assert!(
            verifier(&keys)
                .verify(&key.sign(&claims), NOW)
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn rejects_an_assertion_signed_by_a_key_iap_does_not_publish() {
        // Arrange
        let published = TestIapKey::new("key-1");
        let impostor = TestIapKey::new("key-1");
        let keys = FakeIapKeys::run(&[&published]).await;

        // Act / Assert
        let assertion = impostor.sign(&iap_claims("accounts.google.com:1"));
        assert!(verifier(&keys).verify(&assertion, NOW).await.is_err());
    }

    #[tokio::test]
    async fn rejects_an_assertion_that_is_not_es256() {
        // Arrange
        let key = TestIapKey::new("key-1");
        let keys = FakeIapKeys::run(&[&key]).await;
        let assertion = key.sign_with_header(
            &json!({ "alg": "HS256", "kid": "key-1" }),
            &iap_claims("accounts.google.com:1"),
        );

        // Act / Assert
        assert!(verifier(&keys).verify(&assertion, NOW).await.is_err());
    }

    #[tokio::test]
    async fn skips_keys_of_other_kinds_in_the_key_set() {
        // Arrange
        let key = TestIapKey::new("key-1");
        let keys = FakeIapKeys::run_with_extra(
            &[&key],
            json!({ "kid": "rsa", "kty": "RSA", "n": "AQAB", "e": "AQAB" }),
        )
        .await;
        let assertion = key.sign(&iap_claims("accounts.google.com:1"));

        // Act / Assert
        assert!(verifier(&keys).verify(&assertion, NOW).await.is_ok());
    }

    #[tokio::test]
    async fn fetches_the_keys_once_for_repeated_assertions() {
        // Arrange
        let key = TestIapKey::new("key-1");
        let keys = FakeIapKeys::run(&[&key]).await;
        let verifier = verifier(&keys);
        let assertion = key.sign(&iap_claims("accounts.google.com:1"));

        // Act
        for _ in 0..3 {
            verifier.verify(&assertion, NOW).await.unwrap();
        }

        // Assert
        assert_eq!(keys.fetches(), 1);
    }
}
