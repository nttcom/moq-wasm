use std::{
    collections::HashMap,
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result};
use auth_token::{TEMPORARY_APP_ID_PREFIX, decode_unverified_payload};
use moqt::{CertificateVerification, ClientConfig};
use tokio::{sync::Mutex, task::JoinHandle};

use crate::{
    broadcast_namespace::BroadcastNamespaces,
    root_translation::{RootTranslation, TranslationResources},
};

const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
/// An app whose last broadcast ended this long ago, or that never had one,
/// gives its place to another app.
const IDLE_TIMEOUT: Duration = Duration::from_secs(60);

/// Why an app's translation was not started.
pub(crate) enum Refusal {
    NotATemporaryAppToken,
    Expired,
    /// `max_apps` other apps are being translated.
    Full,
    Relay(anyhow::Error),
}

/// The temporary apps (services/token-mint) being translated. An app is an
/// isolated namespace root that only its own tokens reach, so each is watched
/// over a relay session of its own, opened with the token a streamer hands
/// over. Nobody renews that token: the relay closes the session when it
/// expires, which ends the app's translation.
pub(crate) struct TemporaryApps {
    relay: String,
    insecure: bool,
    resources: TranslationResources,
    max_apps: usize,
    apps: Arc<Mutex<HashMap<String, AppTranslation>>>,
}

impl TemporaryApps {
    pub(crate) fn new(
        relay: String,
        insecure: bool,
        resources: TranslationResources,
        max_apps: usize,
    ) -> Self {
        Self {
            relay,
            insecure,
            resources,
            max_apps,
            apps: Arc::default(),
        }
    }

    /// Translates the broadcasts under `{appId}/live-translate` of the app
    /// `token` belongs to. An app already being translated starts over with
    /// `token`, so a streamer that starts again gets the token's whole
    /// lifetime; the languages its viewers were hearing end.
    pub(crate) async fn start(&self, token: &str) -> Result<(), Refusal> {
        let app_id = temporary_app_id(token, unix_now())?;
        let mut apps = self.apps.lock().await;
        if !apps.contains_key(&app_id) && apps.len() >= self.max_apps {
            return Err(Refusal::Full);
        }
        // The app's running translation is replaced only once the relay has
        // accepted `token`, so a forged token cannot end it.
        let root_translation = self.connect(&app_id, token).await.map_err(Refusal::Relay)?;
        tracing::info!(app_id, "translation of a temporary app started");
        apps.insert(
            app_id.clone(),
            AppTranslation::run(root_translation, app_id, self.apps.clone()),
        );
        Ok(())
    }

    async fn connect(&self, app_id: &str, token: &str) -> Result<RootTranslation> {
        let config = ClientConfig {
            port: 0,
            certificate_verification: CertificateVerification::native_roots_or_disabled(
                !self.insecure,
            ),
            authorization_token: Some(token.to_string().into()),
            ..ClientConfig::default()
        };
        let namespaces = BroadcastNamespaces::new(format!("{app_id}/live-translate"));
        tokio::time::timeout(
            CONNECT_TIMEOUT,
            RootTranslation::connect(&self.relay, &config, namespaces, self.resources.clone()),
        )
        .await
        .context("the relay did not answer in time")?
    }
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs())
}

/// The signature is left to the relay, which refuses the session of a token
/// it cannot verify.
fn temporary_app_id(token: &str, now: u64) -> Result<String, Refusal> {
    let payload = decode_unverified_payload(token).ok_or(Refusal::NotATemporaryAppToken)?;
    let app_id = payload
        .get("appId")
        .and_then(|app_id| app_id.as_str())
        .filter(|app_id| app_id.starts_with(TEMPORARY_APP_ID_PREFIX))
        .ok_or(Refusal::NotATemporaryAppToken)?;
    let expires_at = payload.get("exp").and_then(|exp| exp.as_u64());
    if expires_at.is_none_or(|expires_at| expires_at <= now) {
        return Err(Refusal::Expired);
    }
    Ok(app_id.to_string())
}

struct AppTranslation {
    join_handle: JoinHandle<()>,
}

impl AppTranslation {
    fn run(
        mut root_translation: RootTranslation,
        app_id: String,
        apps: Arc<Mutex<HashMap<String, AppTranslation>>>,
    ) -> Self {
        let join_handle = tokio::spawn(async move {
            match root_translation.route_events(Some(IDLE_TIMEOUT)).await {
                Ok(()) => tracing::info!(app_id, "translation of a temporary app ended: idle"),
                Err(error) => {
                    tracing::info!(app_id, %error, "translation of a temporary app ended")
                }
            }
            drop(root_translation);
            // `start` replaces an entry only by dropping it, which aborts this
            // task, so the entry found here is this task's own.
            apps.lock().await.remove(&app_id);
        });
        Self { join_handle }
    }
}

impl Drop for AppTranslation {
    fn drop(&mut self) {
        self.join_handle.abort();
    }
}

#[cfg(test)]
mod tests {
    use auth_token::{Claims, sign_token};

    use super::*;

    const NOW: u64 = 1_800_000_000;

    fn token(app_id: &str, exp: Option<u64>) -> String {
        let claims = Claims {
            app_id: app_id.to_string(),
            publish: Some(String::new()),
            subscribe: Some(String::new()),
            iat: None,
            exp,
            relay_id: None,
        };
        sign_token(&claims, "s3cret").unwrap()
    }

    #[test]
    fn a_live_token_of_a_temporary_app_names_the_app() {
        // Act
        let app_id = temporary_app_id(&token("temp-3f9a", Some(NOW + 1)), NOW);

        // Assert
        assert!(matches!(app_id, Ok(app_id) if app_id == "temp-3f9a"));
    }

    #[test]
    fn a_token_of_a_registered_app_is_refused() {
        // Act
        let app_id = temporary_app_id(&token("app-1", Some(NOW + 1)), NOW);

        // Assert
        assert!(matches!(app_id, Err(Refusal::NotATemporaryAppToken)));
    }

    #[test]
    fn text_that_is_not_a_token_is_refused() {
        // Act / Assert
        assert!(matches!(
            temporary_app_id("not-a-token", NOW),
            Err(Refusal::NotATemporaryAppToken)
        ));
    }

    #[test]
    fn an_expired_token_or_one_without_an_expiry_is_refused() {
        // Act / Assert
        for exp in [Some(NOW), None] {
            assert!(matches!(
                temporary_app_id(&token("temp-3f9a", exp), NOW),
                Err(Refusal::Expired)
            ));
        }
    }
}
