use std::{
    collections::HashMap,
    time::{Duration, Instant},
};

use anyhow::{Context, Result, anyhow, bail, ensure};
use aws_lc_rs::signature::{
    ECDSA_P256_SHA256_FIXED, RSA_PKCS1_2048_8192_SHA256, RsaPublicKeyComponents, UnparsedPublicKey,
};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use reqwest::{Client, Url};
use serde::{Deserialize, de::DeserializeOwned};
use tokio::sync::Mutex;

pub const GOOGLE_IAP_KEYS_URL: &str = "https://www.gstatic.com/iap/verify/public_key-jwk";
pub const GOOGLE_CERTS_URL: &str = "https://www.googleapis.com/oauth2/v3/certs";

const IAP_ISSUER: &str = "https://cloud.google.com/iap";
const GOOGLE_ISSUERS: [&str; 2] = ["https://accounts.google.com", "accounts.google.com"];
const IAP_CLOCK_SKEW_SECONDS: u64 = 30;
const MIN_KEY_REFRESH_INTERVAL: Duration = Duration::from_secs(60);
const KEYS_REQUEST_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IapCaller {
    pub sub: String,
    pub email: Option<String>,
}

/// Verifies Google Cloud IAP assertions: ES256 signed by a published IAP P-256
/// key, issued by IAP for `audience`, with non-empty `sub` and valid `iat`/`exp`
/// within a 30-second skew.
pub struct IapVerifier {
    audience: String,
    keys: JwksCache<Vec<u8>>,
}

/// Verifies Google ID tokens minted by a workload's metadata server: RS256
/// signed by one of Google's published RSA keys, issued by Google for
/// `audience`, not expired, and naming one of `service_accounts` as a verified
/// email.
pub struct IdTokenVerifier {
    audience: String,
    service_accounts: Vec<String>,
    keys: JwksCache<RsaKey>,
}

/// The modulus and exponent of an RSA key, big-endian.
#[derive(Clone)]
struct RsaKey {
    n: Vec<u8>,
    e: Vec<u8>,
}

#[derive(Deserialize)]
struct JwtHeader {
    alg: String,
    kid: String,
}

#[derive(Deserialize)]
struct IapClaims {
    iss: String,
    aud: String,
    sub: String,
    email: Option<String>,
    iat: u64,
    exp: u64,
}

#[derive(Deserialize)]
struct IdTokenClaims {
    iss: String,
    aud: String,
    exp: u64,
    email: Option<String>,
    #[serde(default)]
    email_verified: bool,
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
    n: Option<String>,
    e: Option<String>,
}

struct JwksCache<K> {
    http: Client,
    url: Url,
    cache: Mutex<KeyCache<K>>,
}

struct KeyCache<K> {
    keys: HashMap<String, K>,
    fetched_at: Option<Instant>,
}

type JwkExtractor<K> = fn(Jwk) -> Result<Option<(String, K)>>;

impl<K: Clone> JwksCache<K> {
    fn new(url: Url) -> Result<Self> {
        Ok(Self {
            http: Client::builder().timeout(KEYS_REQUEST_TIMEOUT).build()?,
            url,
            cache: Mutex::new(KeyCache {
                keys: HashMap::new(),
                fetched_at: None,
            }),
        })
    }

    async fn get(&self, kid: &str, extract: JwkExtractor<K>) -> Result<K> {
        let mut cache = self.cache.lock().await;
        if let Some(key) = cache.keys.get(kid) {
            return Ok(key.clone());
        }
        if cache
            .fetched_at
            .is_some_and(|fetched_at| fetched_at.elapsed() < MIN_KEY_REFRESH_INTERVAL)
        {
            bail!("the key {kid:?} is unknown");
        }
        cache.keys = self.fetch(extract).await?;
        cache.fetched_at = Some(Instant::now());
        cache
            .keys
            .get(kid)
            .cloned()
            .with_context(|| format!("the key {kid:?} is unknown"))
    }

    async fn fetch(&self, extract: JwkExtractor<K>) -> Result<HashMap<String, K>> {
        let key_set: KeySet = self
            .http
            .get(self.url.clone())
            .send()
            .await
            .and_then(reqwest::Response::error_for_status)
            .with_context(|| format!("cannot read {}", self.url))?
            .json()
            .await
            .context("the keys are not a JWK set")?;
        let mut keys = HashMap::new();
        for jwk in key_set.keys {
            if let Some((kid, key)) = extract(jwk)? {
                keys.insert(kid, key);
            }
        }
        Ok(keys)
    }
}

impl IapVerifier {
    pub fn new(audience: String, keys_url: Url) -> Result<Self> {
        Ok(Self {
            audience,
            keys: JwksCache::new(keys_url)?,
        })
    }

    /// `now` is Unix seconds.
    pub async fn verify(&self, assertion: &str, now: u64) -> Result<IapCaller> {
        let [header_segment, payload_segment, signature_segment] = split_jws(assertion)?;
        let header: JwtHeader = decode_jws_json(header_segment)?;
        if header.alg != "ES256" {
            bail!(
                "the IAP assertion is signed with {:?}, not ES256",
                header.alg
            );
        }
        let key = self.keys.get(&header.kid, extract_p256_jwk).await?;
        let signature = URL_SAFE_NO_PAD
            .decode(signature_segment)
            .context("the IAP assertion signature is not base64url")?;
        let signing_input = &assertion[..header_segment.len() + 1 + payload_segment.len()];
        UnparsedPublicKey::new(&ECDSA_P256_SHA256_FIXED, &key)
            .verify(signing_input.as_bytes(), &signature)
            .map_err(|_| anyhow!("the IAP assertion signature does not verify"))?;
        let claims: IapClaims = decode_jws_json(payload_segment)?;
        if claims.iss != IAP_ISSUER {
            bail!("the IAP assertion is issued by {:?}", claims.iss);
        }
        if claims.aud != self.audience {
            bail!("the IAP assertion is for {:?}", claims.aud);
        }
        if claims.exp + IAP_CLOCK_SKEW_SECONDS < now {
            bail!("the IAP assertion expired at {}", claims.exp);
        }
        if claims.iat > now + IAP_CLOCK_SKEW_SECONDS {
            bail!(
                "the IAP assertion is issued in the future at {}",
                claims.iat
            );
        }
        if claims.sub.is_empty() {
            bail!("the IAP assertion has an empty sub");
        }
        Ok(IapCaller {
            sub: claims.sub,
            email: claims.email,
        })
    }
}

impl IdTokenVerifier {
    pub fn new(certs_url: Url, audience: String, service_accounts: Vec<String>) -> Result<Self> {
        Ok(Self {
            audience,
            service_accounts,
            keys: JwksCache::new(certs_url)?,
        })
    }

    /// `now` is Unix seconds.
    pub async fn verify(&self, token: &str, now: u64) -> Result<()> {
        let [header_segment, payload_segment, signature_segment] = split_jws(token)?;
        let header: JwtHeader = decode_jws_json(header_segment)?;
        ensure!(header.alg == "RS256", "unexpected alg {}", header.alg);
        let key = self.keys.get(&header.kid, extract_rsa_jwk).await?;
        let signing_input = &token[..header_segment.len() + 1 + payload_segment.len()];
        let signature = URL_SAFE_NO_PAD
            .decode(signature_segment)
            .context("the ID token signature is not base64url")?;
        RsaPublicKeyComponents {
            n: &key.n,
            e: &key.e,
        }
        .verify(
            &RSA_PKCS1_2048_8192_SHA256,
            signing_input.as_bytes(),
            &signature,
        )
        .map_err(|_| anyhow!("invalid signature"))?;
        let claims: IdTokenClaims = decode_jws_json(payload_segment)?;
        ensure!(
            GOOGLE_ISSUERS.contains(&claims.iss.as_str()),
            "not issued by Google"
        );
        ensure!(claims.aud == self.audience, "issued for another audience");
        ensure!(claims.exp > now, "expired");
        ensure!(
            claims.email_verified
                && claims
                    .email
                    .is_some_and(|email| self.service_accounts.contains(&email)),
            "not the service account of a bot job"
        );
        Ok(())
    }
}

fn extract_p256_jwk(jwk: Jwk) -> Result<Option<(String, Vec<u8>)>> {
    match (jwk.kty.as_str(), jwk.crv.as_deref(), jwk.x, jwk.y) {
        ("EC", Some("P-256"), Some(x), Some(y)) => Ok(Some((jwk.kid, uncompressed_point(&x, &y)?))),
        _ => Ok(None),
    }
}

fn extract_rsa_jwk(jwk: Jwk) -> Result<Option<(String, RsaKey)>> {
    match (jwk.kty.as_str(), jwk.n, jwk.e) {
        ("RSA", Some(n), Some(e)) => Ok(Some((
            jwk.kid,
            RsaKey {
                n: URL_SAFE_NO_PAD
                    .decode(n)
                    .context("an RSA key n is not base64url")?,
                e: URL_SAFE_NO_PAD
                    .decode(e)
                    .context("an RSA key e is not base64url")?,
            },
        ))),
        _ => Ok(None),
    }
}

fn uncompressed_point(x: &str, y: &str) -> Result<Vec<u8>> {
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

fn split_jws(token: &str) -> Result<[&str; 3]> {
    token
        .split('.')
        .collect::<Vec<_>>()
        .try_into()
        .map_err(|_| anyhow!("not a compact JWS"))
}

fn decode_jws_json<T: DeserializeOwned>(segment: &str) -> Result<T> {
    let bytes = URL_SAFE_NO_PAD
        .decode(segment)
        .context("a JWS segment is not base64url")?;
    serde_json::from_slice(&bytes).context("a JWS segment is not the expected JSON")
}

#[cfg(test)]
mod tests {
    use aws_lc_rs::{
        rand::SystemRandom,
        rsa::KeySize,
        signature::{KeyPair, RSA_PKCS1_SHA256, RsaKeyPair},
    };
    use http_server::testing::StubServer;
    use hyper::StatusCode;
    use serde_json::{Value, json};

    use super::*;
    use crate::testing::{SilentServer, TestIapKey};

    const NOW: u64 = 1_800_000_000;
    const IAP_AUDIENCE: &str = "/projects/123/locations/region/services/console";
    const ID_AUDIENCE: &str = "https://bot-dispatcher.example";
    const ACCOUNT: &str = "bot@project.iam.gserviceaccount.com";

    async fn jwks_server(jwks: Vec<Value>) -> StubServer {
        StubServer::run(StatusCode::OK, json!({ "keys": jwks }).to_string()).await
    }

    fn keys_url(server: &StubServer) -> Url {
        Url::parse(&format!("{}/keys", server.url())).unwrap()
    }

    fn iap_claims(sub: &str) -> Value {
        json!({
            "iss": "https://cloud.google.com/iap",
            "aud": IAP_AUDIENCE,
            "sub": sub,
            "email": "someone@example.com",
            "iat": NOW - 10,
            "exp": NOW + 590,
        })
    }

    fn rsa_jwk(kid: &str, key: &RsaKeyPair) -> Value {
        let public = key.public_key();
        json!({
            "kid": kid,
            "kty": "RSA",
            "alg": "RS256",
            "n": URL_SAFE_NO_PAD.encode(public.modulus().big_endian_without_leading_zero()),
            "e": URL_SAFE_NO_PAD.encode(public.exponent().big_endian_without_leading_zero()),
        })
    }

    fn sign_rsa(key: &RsaKeyPair, kid: &str, payload: &Value) -> String {
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

    fn id_token_claims() -> Value {
        json!({
            "iss": "https://accounts.google.com",
            "aud": ID_AUDIENCE,
            "exp": NOW + 600,
            "email": ACCOUNT,
            "email_verified": true,
        })
    }

    fn iap_verifier(server: &StubServer) -> IapVerifier {
        IapVerifier::new(IAP_AUDIENCE.into(), keys_url(server)).unwrap()
    }

    fn id_verifier(server: &StubServer) -> IdTokenVerifier {
        IdTokenVerifier::new(
            keys_url(server),
            ID_AUDIENCE.into(),
            vec![
                "recorder@project.iam.gserviceaccount.com".into(),
                ACCOUNT.into(),
            ],
        )
        .unwrap()
    }

    #[tokio::test]
    async fn iap_returns_the_sub_and_email_of_a_valid_assertion() {
        // Arrange
        let key = TestIapKey::new("key-1");
        let keys = jwks_server(vec![key.jwk()]).await;
        let assertion = key.sign(&iap_claims("accounts.google.com:1"));

        // Act
        let caller = iap_verifier(&keys).verify(&assertion, NOW).await.unwrap();

        // Assert
        assert_eq!(caller.sub, "accounts.google.com:1");
        assert_eq!(caller.email.as_deref(), Some("someone@example.com"));
    }

    #[tokio::test]
    async fn iap_rejects_wrong_audience_issuer_alg_or_signer() {
        // Arrange
        let key = TestIapKey::new("key-1");
        let impostor = TestIapKey::new("key-1");
        let keys = jwks_server(vec![key.jwk()]).await;
        let verifier = iap_verifier(&keys);
        let mut wrong_aud = iap_claims("accounts.google.com:1");
        wrong_aud["aud"] = json!("/projects/1/locations/r/services/other");
        let mut wrong_iss = iap_claims("accounts.google.com:1");
        wrong_iss["iss"] = json!("https://accounts.google.com");
        let wrong_alg = key.sign_with_header(
            &json!({ "alg": "HS256", "kid": "key-1" }),
            &iap_claims("accounts.google.com:1"),
        );

        // Act / Assert
        assert!(verifier.verify(&key.sign(&wrong_aud), NOW).await.is_err());
        assert!(verifier.verify(&key.sign(&wrong_iss), NOW).await.is_err());
        assert!(verifier.verify(&wrong_alg, NOW).await.is_err());
        assert!(
            verifier
                .verify(&impostor.sign(&iap_claims("accounts.google.com:1")), NOW)
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn iap_accepts_clock_skew_up_to_30_seconds_and_rejects_future_iat() {
        // Arrange
        let key = TestIapKey::new("key-1");
        let keys = jwks_server(vec![key.jwk()]).await;
        let verifier = iap_verifier(&keys);
        let mut expiring = iap_claims("accounts.google.com:1");
        expiring["exp"] = json!(NOW);
        let expiring = key.sign(&expiring);
        let mut future = iap_claims("accounts.google.com:1");
        future["iat"] = json!(NOW + 31);
        future["exp"] = json!(NOW + 600);

        // Act / Assert
        assert!(verifier.verify(&expiring, NOW + 30).await.is_ok());
        assert!(verifier.verify(&expiring, NOW + 31).await.is_err());
        assert!(verifier.verify(&key.sign(&future), NOW).await.is_err());
    }

    #[tokio::test]
    async fn iap_skips_non_p256_keys_and_caches_the_key_set() {
        // Arrange
        let key = TestIapKey::new("key-1");
        let keys = jwks_server(vec![
            key.jwk(),
            json!({ "kid": "rsa", "kty": "RSA", "n": "AQAB", "e": "AQAB" }),
        ])
        .await;
        let verifier = iap_verifier(&keys);
        let assertion = key.sign(&iap_claims("accounts.google.com:1"));

        // Act
        for _ in 0..3 {
            verifier.verify(&assertion, NOW).await.unwrap();
        }

        // Assert
        assert_eq!(keys.requests().len(), 1);
    }

    #[tokio::test]
    async fn iap_gives_up_within_the_request_timeout_when_the_key_server_never_answers() {
        // Arrange
        let server = SilentServer::run().await;
        let verifier =
            IapVerifier::new(IAP_AUDIENCE.into(), server.url().join("keys").unwrap()).unwrap();
        let assertion = TestIapKey::new("key-1").sign(&iap_claims("accounts.google.com:1"));

        // Act
        let result = tokio::time::timeout(
            KEYS_REQUEST_TIMEOUT + Duration::from_secs(1),
            verifier.verify(&assertion, NOW),
        )
        .await;

        // Assert
        assert!(
            result
                .expect("verify must not wait past the keys timeout")
                .is_err()
        );
    }

    #[tokio::test]
    async fn id_token_of_any_listed_service_account_passes_and_skips_non_rsa_keys() {
        // Arrange
        let key = RsaKeyPair::generate(KeySize::Rsa2048).unwrap();
        let ec = TestIapKey::new("ec-1");
        let keys = jwks_server(vec![ec.jwk(), rsa_jwk("k1", &key)]).await;
        let token = sign_rsa(&key, "k1", &id_token_claims());

        // Act / Assert
        id_verifier(&keys).verify(&token, NOW).await.unwrap();
    }

    #[tokio::test]
    async fn id_token_differing_in_any_checked_claim_is_refused() {
        // Arrange
        let key = RsaKeyPair::generate(KeySize::Rsa2048).unwrap();
        let keys = jwks_server(vec![rsa_jwk("k1", &key)]).await;
        let verifier = id_verifier(&keys);
        let changes = [
            ("iss", json!("https://evil.example")),
            ("aud", json!("https://other.example")),
            ("exp", json!(NOW)),
            ("email", json!("other@project.iam.gserviceaccount.com")),
            ("email_verified", json!(false)),
        ];

        for (claim, value) in changes {
            let mut payload = id_token_claims();
            payload[claim] = value;
            let token = sign_rsa(&key, "k1", &payload);

            // Act / Assert
            assert!(verifier.verify(&token, NOW).await.is_err(), "{claim}");
        }
    }

    #[tokio::test]
    async fn id_token_signed_by_another_key_or_tampered_with_is_refused() {
        // Arrange
        let key = RsaKeyPair::generate(KeySize::Rsa2048).unwrap();
        let other = RsaKeyPair::generate(KeySize::Rsa2048).unwrap();
        let keys = jwks_server(vec![rsa_jwk("k1", &key)]).await;
        let verifier = id_verifier(&keys);
        let forged = sign_rsa(&other, "k1", &id_token_claims());
        let token = sign_rsa(&key, "k1", &id_token_claims());
        let (signed, signature) = token.rsplit_once('.').unwrap();
        let (header, _) = signed.split_once('.').unwrap();
        let mut payload = id_token_claims();
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
                .verify(&sign_rsa(&key, "k2", &id_token_claims()), NOW)
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn id_token_throttles_refetches_for_unknown_kids() {
        // Arrange
        let key = RsaKeyPair::generate(KeySize::Rsa2048).unwrap();
        let keys = jwks_server(vec![rsa_jwk("k1", &key)]).await;
        let verifier = id_verifier(&keys);

        // Act
        for kid in ["missing-1", "missing-2", "missing-3"] {
            let token = sign_rsa(&key, kid, &id_token_claims());
            assert!(verifier.verify(&token, NOW).await.is_err());
        }

        // Assert
        assert_eq!(keys.requests().len(), 1);
    }

    #[tokio::test]
    async fn id_token_gives_up_within_the_request_timeout_when_the_key_server_never_answers() {
        // Arrange
        let server = SilentServer::run().await;
        let verifier = IdTokenVerifier::new(
            server.url().join("certs").unwrap(),
            ID_AUDIENCE.into(),
            vec![ACCOUNT.into()],
        )
        .unwrap();
        let key = RsaKeyPair::generate(KeySize::Rsa2048).unwrap();
        let token = sign_rsa(&key, "k1", &id_token_claims());

        // Act
        let result = tokio::time::timeout(
            KEYS_REQUEST_TIMEOUT + Duration::from_secs(1),
            verifier.verify(&token, NOW),
        )
        .await;

        // Assert
        assert!(
            result
                .expect("verify must not wait past the keys timeout")
                .is_err()
        );
    }
}
