use std::{
    collections::HashMap,
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use moq_wasm::{CertificateVerification, ConnectOptions, Session, connect};
use serde::Deserialize;
use tokio::{sync::Mutex, task::JoinHandle};

use crate::{
    broadcast_namespace::BroadcastNamespaces,
    root_translation::{TranslationResources, translate_root},
};

const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
/// An app whose last broadcast ended this long ago, or that never had one,
/// gives its place to another app.
const IDLE_TIMEOUT: Duration = Duration::from_secs(60);
/// The `appId` prefix services/temporary-apps gives its apps.
const TEMPORARY_APP_ID_PREFIX: &str = "temp-";

pub(crate) enum Refusal {
    NotATemporaryAppToken,
    Expired,
    /// `max_apps` other apps are being translated.
    Full,
    Relay(anyhow::Error),
}

/// The temporary apps (services/temporary-apps) being translated. An app is an
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
        // The signature is left to the relay, which refuses the session of a
        // token it cannot verify.
        let app_id = temporary_app_id(token, unix_now())?;
        let mut apps = self.apps.lock().await;
        if !apps.contains_key(&app_id) && apps.len() >= self.max_apps {
            return Err(Refusal::Full);
        }
        // The app's running translation is replaced only once the relay has
        // accepted `token`, so a forged token cannot end it.
        let session = self.connect(token).await.map_err(Refusal::Relay)?;
        tracing::info!(app_id, "translation of a temporary app started");
        let namespaces = BroadcastNamespaces::new(format!("{app_id}/live-translate"));
        apps.insert(
            app_id.clone(),
            AppTranslation::run(
                session,
                namespaces,
                self.resources.clone(),
                app_id,
                self.apps.clone(),
            ),
        );
        Ok(())
    }

    async fn connect(&self, token: &str) -> Result<Session> {
        let options = ConnectOptions {
            token: Some(token.to_string()),
            certificate_verification: CertificateVerification::native_roots_or_disabled(
                !self.insecure,
            ),
            ..ConnectOptions::default()
        };
        tokio::time::timeout(CONNECT_TIMEOUT, connect(&self.relay, options))
            .await
            .context("the relay did not answer in time")?
    }
}

#[derive(Deserialize)]
struct Claims {
    #[serde(rename = "appId")]
    app_id: String,
    exp: Option<u64>,
}

/// The `appId` of a temporary app's token, read without checking its
/// signature.
fn temporary_app_id(token: &str, now: u64) -> Result<String, Refusal> {
    let claims: Claims = token
        .split('.')
        .nth(1)
        .and_then(|payload| URL_SAFE_NO_PAD.decode(payload).ok())
        .and_then(|payload| serde_json::from_slice(&payload).ok())
        .filter(|claims: &Claims| claims.app_id.starts_with(TEMPORARY_APP_ID_PREFIX))
        .ok_or(Refusal::NotATemporaryAppToken)?;
    if claims.exp.is_none_or(|exp| exp <= now) {
        return Err(Refusal::Expired);
    }
    Ok(claims.app_id)
}

pub(crate) fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs())
}

struct AppTranslation {
    join_handle: JoinHandle<()>,
}

impl AppTranslation {
    fn run(
        session: Session,
        namespaces: BroadcastNamespaces,
        resources: TranslationResources,
        app_id: String,
        apps: Arc<Mutex<HashMap<String, AppTranslation>>>,
    ) -> Self {
        let session = Arc::new(session);
        let join_handle = tokio::spawn(session.clone().scoped(async move {
            let result =
                translate_root(session.clone(), namespaces, resources, Some(IDLE_TIMEOUT)).await;
            match result {
                Ok(()) => tracing::info!(app_id, "translation of a temporary app ended"),
                Err(error) => {
                    tracing::info!(app_id, %error, "translation of a temporary app ended")
                }
            }
            session.close().await;
            // `start` replaces an entry only by dropping it, which aborts this
            // task, so the entry found here is this task's own.
            apps.lock().await.remove(&app_id);
        }));
        Self { join_handle }
    }
}

impl Drop for AppTranslation {
    fn drop(&mut self) {
        self.join_handle.abort();
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A token whose payload holds `appId` and `exp`, with a placeholder
    /// signature: `temporary_app_id` does not check it.
    pub(crate) fn unsigned_token(app_id: &str, exp: u64) -> String {
        let payload = serde_json::json!({ "appId": app_id, "exp": exp }).to_string();
        format!(
            "{}.{}.signature",
            URL_SAFE_NO_PAD.encode(br#"{"alg":"HS256"}"#),
            URL_SAFE_NO_PAD.encode(payload)
        )
    }

    #[test]
    fn a_temporary_app_token_names_its_app() {
        // Arrange
        let token = unsigned_token("temp-3f9a", 2_000);

        // Act
        let app_id = temporary_app_id(&token, 1_000);

        // Assert
        assert!(matches!(app_id, Ok(app_id) if app_id == "temp-3f9a"));
    }

    #[test]
    fn a_registered_app_token_or_a_malformed_one_is_not_a_temporary_app_token() {
        // Act / Assert
        assert!(matches!(
            temporary_app_id(&unsigned_token("app-1", 2_000), 1_000),
            Err(Refusal::NotATemporaryAppToken)
        ));
        assert!(matches!(
            temporary_app_id("not-a-jwt", 1_000),
            Err(Refusal::NotATemporaryAppToken)
        ));
    }

    #[test]
    fn an_expired_temporary_app_token_is_refused() {
        // Act / Assert
        assert!(matches!(
            temporary_app_id(&unsigned_token("temp-3f9a", 1_000), 1_000),
            Err(Refusal::Expired)
        ));
    }
}
