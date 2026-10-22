use std::time::Duration;

use anyhow::anyhow;
use app_registry::{App, RegistryStore};
use auth_token::{Claims, sign_token};
use base64::{Engine, engine::general_purpose::STANDARD};
use ring::rand::{SecureRandom, SystemRandom};

pub(crate) const MAX_TOKEN_TTL_SECONDS: u64 = 24 * 60 * 60;
pub(crate) const MAX_APPS_PER_OWNER: usize = 20;

pub(crate) enum Creation {
    Created(App),
    LimitReached,
}

pub(crate) struct TokenScope {
    pub(crate) publish: Option<String>,
    pub(crate) subscribe: Option<String>,
    pub(crate) ttl_seconds: u64,
}

pub(crate) struct IssuedToken {
    pub(crate) token: String,
    pub(crate) expires_at: u64,
}

pub(crate) async fn create_app(
    store: &RegistryStore,
    sub: &str,
    name: String,
    now: u64,
) -> anyhow::Result<Creation> {
    let app = App {
        app_id: random_app_id()?,
        name: Some(name),
        secret: random_secret()?,
        is_relay: false,
        sub: Some(sub.to_owned()),
        created_at: Some(now),
        rotated_at: None,
    };
    store
        .update(|apps| {
            if apps
                .iter()
                .filter(|existing| owned_by(existing, sub))
                .count()
                >= MAX_APPS_PER_OWNER
            {
                return Ok(Creation::LimitReached);
            }
            apps.push(app.clone());
            Ok(Creation::Created(app.clone()))
        })
        .await
}

pub(crate) async fn issue_token(
    store: &RegistryStore,
    sub: &str,
    app_id: &str,
    scope: TokenScope,
    now: u64,
) -> anyhow::Result<Option<IssuedToken>> {
    let apps = store.load().await?.apps;
    let Some(app) = apps.get(app_id).filter(|app| owned_by(app, sub)) else {
        return Ok(None);
    };
    let claims = Claims::issued(
        app.app_id.clone(),
        scope.publish,
        scope.subscribe,
        now,
        Duration::from_secs(scope.ttl_seconds),
    );
    let expires_at = now + scope.ttl_seconds;
    Ok(Some(IssuedToken {
        token: sign_token(&claims, &app.secret)?,
        expires_at,
    }))
}

pub(crate) async fn rotate_secret(
    store: &RegistryStore,
    sub: &str,
    app_id: &str,
    now: u64,
) -> anyhow::Result<Option<App>> {
    let secret = random_secret()?;
    store
        .update(|apps| {
            let Some(app) = apps
                .iter_mut()
                .find(|app| app.app_id == app_id && owned_by(app, sub))
            else {
                return Ok(None);
            };
            app.secret = secret.clone();
            app.rotated_at = Some(now);
            Ok(Some(app.clone()))
        })
        .await
}

pub(crate) async fn delete_app(
    store: &RegistryStore,
    sub: &str,
    app_id: &str,
) -> anyhow::Result<Option<()>> {
    store
        .update(|apps| {
            let Some(index) = apps
                .iter()
                .position(|app| app.app_id == app_id && owned_by(app, sub))
            else {
                return Ok(None);
            };
            apps.remove(index);
            Ok(Some(()))
        })
        .await
}

pub(crate) fn owned_by(app: &App, sub: &str) -> bool {
    !app.is_relay && app.sub.as_deref() == Some(sub)
}

fn random_app_id() -> anyhow::Result<String> {
    let mut bytes = random_bytes::<16>()?;
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    let hex: String = bytes.iter().map(|byte| format!("{byte:02x}")).collect();
    Ok(format!(
        "{}-{}-{}-{}-{}",
        &hex[0..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..32]
    ))
}

fn random_secret() -> anyhow::Result<String> {
    Ok(STANDARD.encode(random_bytes::<32>()?))
}

fn random_bytes<const N: usize>() -> anyhow::Result<[u8; N]> {
    let mut bytes = [0; N];
    SystemRandom::new()
        .fill(&mut bytes)
        .map_err(|_| anyhow!("the system random source failed"))?;
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn app_ids_are_random_version_4_uuids() {
        // Act
        let first = random_app_id().unwrap();
        let second = random_app_id().unwrap();

        // Assert
        assert_ne!(first, second);
        let groups: Vec<&str> = first.split('-').collect();
        assert_eq!(
            groups.iter().map(|group| group.len()).collect::<Vec<_>>(),
            [8, 4, 4, 4, 12]
        );
        assert!(groups[2].starts_with('4'), "{first}");
        assert!(matches!(&groups[3][..1], "8" | "9" | "a" | "b"), "{first}");
    }

    #[test]
    fn secrets_are_32_random_bytes_in_base64() {
        // Act
        let first = random_secret().unwrap();
        let second = random_secret().unwrap();

        // Assert
        assert_eq!(STANDARD.decode(&first).unwrap().len(), 32);
        assert_ne!(first, second);
    }
}
