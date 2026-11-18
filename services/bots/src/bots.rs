use anyhow::Result;
use aws_lc_rs::{
    constant_time::verify_slices_are_equal,
    digest::{SHA256, digest},
    rand::{SecureRandom, SystemRandom},
};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{
    bot_types::BotTypes,
    id_token::IdTokenVerifier,
    signer::BotTokenSigner,
    store::{BotRecord, BotState, BotStore},
};

/// A bot that has not reported for this long is taken as gone: three missed
/// heartbeats of a bot reporting every 30 s.
const HEARTBEAT_TIMEOUT_MS: u64 = 90_000;
/// The environment variable a locally launched bot finds its key in.
const BOT_KEY_ENV: &str = "BOTS_BOT_KEY";

#[derive(Debug)]
pub(crate) enum StartError {
    UnknownType,
    /// The prefix is not `<app>/<room>...` with non-empty elements.
    InvalidRoom,
    /// The room runs a bot of that type.
    Conflict,
    /// The type runs `max_running` bots.
    Full,
    Internal(anyhow::Error),
}

#[derive(Debug)]
pub(crate) enum BotError {
    NotFound,
    Unauthorized,
    /// The bot finished, failed or stopped reporting; it may not go on.
    Ended,
    Internal(anyhow::Error),
}

impl From<anyhow::Error> for BotError {
    fn from(error: anyhow::Error) -> Self {
        Self::Internal(error)
    }
}

/// A bot as the app server sees it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct BotView {
    pub(crate) id: String,
    #[serde(rename = "type")]
    pub(crate) bot_type: String,
    pub(crate) namespace_prefix: String,
    pub(crate) state: BotState,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) error: Option<String>,
    pub(crate) created_at_ms: u64,
    pub(crate) updated_at_ms: u64,
}

#[derive(Debug, Deserialize)]
pub(crate) struct Heartbeat {
    pub(crate) state: BotState,
    #[serde(default)]
    pub(crate) error: Option<String>,
}

pub(crate) struct Bots {
    types: BotTypes,
    store: BotStore,
    signer: BotTokenSigner,
    /// `None` accepts only locally launched bots, which present their key.
    id_tokens: Option<IdTokenVerifier>,
    /// The URL a bot calls back, passed to it as `--bots-url`.
    public_url: String,
}

impl Bots {
    pub(crate) fn new(
        types: BotTypes,
        store: BotStore,
        signer: BotTokenSigner,
        id_tokens: Option<IdTokenVerifier>,
        public_url: String,
    ) -> Self {
        Self {
            types,
            store,
            signer,
            id_tokens,
            public_url,
        }
    }

    /// Takes the room for `bot_type` and launches the bot. The caller has
    /// checked that the app's launch token covers `namespace_prefix`.
    pub(crate) async fn start(
        &self,
        app_id: &str,
        bot_type: &str,
        namespace_prefix: &str,
        options: Option<&Value>,
        now_ms: u64,
    ) -> Result<BotView, StartError> {
        let Some(kind) = self.types.get(bot_type) else {
            return Err(StartError::UnknownType);
        };
        let elements: Vec<&str> = namespace_prefix.split('/').collect();
        if elements.len() < 2 || elements.iter().any(|e| e.is_empty()) {
            return Err(StartError::InvalidRoom);
        }
        let holder = self
            .store
            .room_holder(bot_type, namespace_prefix)
            .await
            .map_err(StartError::Internal)?;
        if let Some(holder) = &holder {
            let current = self
                .store
                .read(&holder.id)
                .await
                .map_err(StartError::Internal)?;
            if current.is_some_and(|record| is_active(&record, now_ms)) {
                return Err(StartError::Conflict);
            }
        }
        if self
            .running(bot_type, now_ms)
            .await
            .map_err(StartError::Internal)?
            >= kind.max_running
        {
            return Err(StartError::Full);
        }
        let id = random_hex();
        if !self
            .store
            .take_room(bot_type, namespace_prefix, &id, holder)
            .await
            .map_err(StartError::Internal)?
        {
            return Err(StartError::Conflict);
        }
        let key = kind.is_local().then(random_hex);
        let record = BotRecord {
            id: id.clone(),
            bot_type: bot_type.to_string(),
            app_id: app_id.to_string(),
            namespace_prefix: namespace_prefix.to_string(),
            state: BotState::Starting,
            error: None,
            created_at_ms: now_ms,
            updated_at_ms: now_ms,
            key_sha256: key.as_deref().map(sha256_hex),
        };
        self.store
            .write(&record)
            .await
            .map_err(StartError::Internal)?;
        let mut args = kind.args.clone();
        args.extend([
            "--bot-type".to_string(),
            bot_type.to_string(),
            "--bot-id".to_string(),
            id.clone(),
            "--bots-url".to_string(),
            self.public_url.clone(),
            "--namespace-prefix".to_string(),
            namespace_prefix.to_string(),
            "--grace-seconds".to_string(),
            kind.grace.as_secs().to_string(),
            "--max-seconds".to_string(),
            kind.max_run.as_secs().to_string(),
        ]);
        if let Some(options) = options {
            args.extend(["--options".to_string(), options.to_string()]);
        }
        let env = key
            .map(|key| vec![(BOT_KEY_ENV.to_string(), key)])
            .unwrap_or_default();
        if let Err(error) = kind.launcher.launch(args, env).await {
            let failed = BotRecord {
                state: BotState::Failed,
                error: Some(format!("cannot launch the bot: {error:#}")),
                ..record
            };
            self.store
                .write(&failed)
                .await
                .map_err(StartError::Internal)?;
            return Err(StartError::Internal(error));
        }
        self.view(&record, now_ms)
            .await
            .map_err(StartError::Internal)
    }

    /// The room's latest bot of `bot_type`.
    pub(crate) async fn latest(
        &self,
        bot_type: &str,
        namespace_prefix: &str,
        now_ms: u64,
    ) -> Result<Option<BotView>> {
        let Some(holder) = self.store.room_holder(bot_type, namespace_prefix).await? else {
            return Ok(None);
        };
        let Some(record) = self.store.read(&holder.id).await? else {
            return Ok(None);
        };
        Ok(Some(self.view(&record, now_ms).await?))
    }

    /// The bot as the app server sees it: a bot asked to stop shows as
    /// stopping before its next heartbeat says so, and one that stopped
    /// reporting as failed.
    pub(crate) async fn view(&self, record: &BotRecord, now_ms: u64) -> Result<BotView> {
        let (state, error) = if record.state.has_ended() {
            (record.state, record.error.clone())
        } else if !is_active(record, now_ms) {
            (
                BotState::Failed,
                Some("the bot stopped reporting".to_string()),
            )
        } else if self.store.stop_requested(&record.id).await? {
            (BotState::Stopping, None)
        } else {
            (record.state, record.error.clone())
        };
        Ok(BotView {
            id: record.id.clone(),
            bot_type: record.bot_type.clone(),
            namespace_prefix: record.namespace_prefix.clone(),
            state,
            error,
            created_at_ms: record.created_at_ms,
            updated_at_ms: record.updated_at_ms,
        })
    }

    /// `None` for an id of another type.
    pub(crate) async fn record(&self, bot_type: &str, id: &str) -> Result<Option<BotRecord>> {
        Ok(self
            .store
            .read(id)
            .await?
            .filter(|record| record.bot_type == bot_type))
    }

    /// The bot stops at its next heartbeat.
    pub(crate) async fn stop(&self, id: &str) -> Result<()> {
        self.store.request_stop(id).await
    }

    /// A new token for the bot to join its room with.
    pub(crate) async fn token(
        &self,
        bot_type: &str,
        id: &str,
        bearer: Option<&str>,
        now_ms: u64,
    ) -> Result<String, BotError> {
        let record = self.authenticated(bot_type, id, bearer, now_ms).await?;
        if !is_active(&record, now_ms) {
            return Err(BotError::Ended);
        }
        Ok(self
            .signer
            .sign(
                &record.app_id,
                &record.namespace_prefix,
                &record.bot_type,
                now_ms / 1000,
            )
            .await?)
    }

    /// Records what the bot reports; `true` asks it to stop, because the app
    /// asked or it reached its type's longest run.
    pub(crate) async fn heartbeat(
        &self,
        bot_type: &str,
        id: &str,
        bearer: Option<&str>,
        heartbeat: Heartbeat,
        now_ms: u64,
    ) -> Result<bool, BotError> {
        let record = self.authenticated(bot_type, id, bearer, now_ms).await?;
        if !is_active(&record, now_ms) {
            return Err(BotError::Ended);
        }
        let max_run_ms = self
            .types
            .get(&record.bot_type)
            .map(|kind| kind.max_run.as_millis() as u64)
            .unwrap_or_default();
        let stop =
            now_ms >= record.created_at_ms + max_run_ms || self.store.stop_requested(id).await?;
        let updated = BotRecord {
            state: heartbeat.state,
            error: heartbeat.error,
            updated_at_ms: now_ms,
            ..record
        };
        self.store.write(&updated).await?;
        Ok(stop)
    }

    async fn authenticated(
        &self,
        bot_type: &str,
        id: &str,
        bearer: Option<&str>,
        now_ms: u64,
    ) -> Result<BotRecord, BotError> {
        let record = self.record(bot_type, id).await?.ok_or(BotError::NotFound)?;
        let bearer = bearer.ok_or(BotError::Unauthorized)?;
        let authentic = match (&record.key_sha256, &self.id_tokens) {
            (Some(key_sha256), _) => {
                verify_slices_are_equal(key_sha256.as_bytes(), sha256_hex(bearer).as_bytes())
                    .is_ok()
            }
            (None, Some(id_tokens)) => match id_tokens.verify(bearer, now_ms / 1000).await {
                Ok(()) => true,
                Err(error) => {
                    tracing::info!(bot_id = id, %error, "bot identity refused");
                    false
                }
            },
            (None, None) => false,
        };
        if authentic {
            Ok(record)
        } else {
            Err(BotError::Unauthorized)
        }
    }

    async fn running(&self, bot_type: &str, now_ms: u64) -> Result<usize> {
        let mut running = 0;
        for id in self.store.room_holders(bot_type).await? {
            if self
                .store
                .read(&id)
                .await?
                .is_some_and(|record| is_active(&record, now_ms))
            {
                running += 1;
            }
        }
        Ok(running)
    }
}

fn is_active(record: &BotRecord, now_ms: u64) -> bool {
    !record.state.has_ended() && now_ms.saturating_sub(record.updated_at_ms) < HEARTBEAT_TIMEOUT_MS
}

fn random_hex() -> String {
    let mut bytes = [0u8; 16];
    SystemRandom::new()
        .fill(&mut bytes)
        .expect("the system random number generator works");
    hex(&bytes)
}

fn sha256_hex(value: &str) -> String {
    hex(digest(&SHA256, value.as_bytes()).as_ref())
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use auth_token::{decode_unverified_payload, verify_signature};

    use super::*;
    use crate::test_support::{TestBots, test_bots};

    const NOW: u64 = 1_800_000_000_000;
    const KEY: &str = "bot-key";

    /// Starts a recorder in `room` whose key is `KEY`.
    async fn started(test: &TestBots, room: &str) -> BotView {
        let bot = test
            .bots
            .start("app", "recorder", room, None, NOW)
            .await
            .unwrap();
        let mut record = test.store.read(&bot.id).await.unwrap().unwrap();
        record.key_sha256 = Some(sha256_hex(KEY));
        test.store.write(&record).await.unwrap();
        bot
    }

    fn heartbeat(state: BotState) -> Heartbeat {
        Heartbeat { state, error: None }
    }

    #[tokio::test]
    async fn a_room_runs_one_bot_of_a_type_and_the_type_its_maximum() {
        // Arrange
        let test = test_bots();
        started(&test, "app/a").await;

        // Act
        let same_room = test.bots.start("app", "recorder", "app/a", None, NOW).await;
        started(&test, "app/b").await;
        let third = test.bots.start("app", "recorder", "app/c", None, NOW).await;

        // Assert
        assert!(matches!(same_room, Err(StartError::Conflict)));
        assert!(matches!(third, Err(StartError::Full)));
    }

    #[tokio::test]
    async fn only_a_room_of_the_app_with_a_known_type_is_started() {
        // Arrange
        let test = test_bots();

        // Act / Assert
        for room in ["app", "app//a"] {
            let result = test.bots.start("app", "recorder", room, None, NOW).await;
            assert!(matches!(result, Err(StartError::InvalidRoom)), "{room}");
        }
        let unknown = test.bots.start("app", "chat", "app/a", None, NOW).await;
        assert!(matches!(unknown, Err(StartError::UnknownType)));
    }

    #[tokio::test]
    async fn a_room_is_free_again_once_its_bot_ends_or_stops_reporting() {
        // Arrange
        let test = test_bots();
        let finished = started(&test, "app/a").await;
        started(&test, "app/b").await;
        test.bots
            .heartbeat(
                "recorder",
                &finished.id,
                Some(KEY),
                heartbeat(BotState::Finished),
                NOW,
            )
            .await
            .unwrap();

        // Act
        let after_finish = test.bots.start("app", "recorder", "app/a", None, NOW).await;
        let after_silence = test
            .bots
            .start("app", "recorder", "app/b", None, NOW + HEARTBEAT_TIMEOUT_MS)
            .await;

        // Assert
        assert!(after_finish.is_ok());
        assert!(after_silence.is_ok());
    }

    #[tokio::test]
    async fn a_silent_bot_shows_as_failed() {
        // Arrange
        let test = test_bots();
        let bot = started(&test, "app/a").await;

        // Act
        let latest = test
            .bots
            .latest("recorder", "app/a", NOW + HEARTBEAT_TIMEOUT_MS)
            .await
            .unwrap()
            .unwrap();

        // Assert
        assert_eq!((latest.id, latest.state), (bot.id, BotState::Failed));
    }

    #[tokio::test]
    async fn a_bot_with_its_key_gets_a_token_for_its_room() {
        // Arrange
        let test = test_bots();
        let bot = started(&test, "app/a").await;

        // Act
        let token = test
            .bots
            .token("recorder", &bot.id, Some(KEY), NOW)
            .await
            .unwrap();

        // Assert
        verify_signature(&token, "app-secret").unwrap();
        let payload = decode_unverified_payload(&token).unwrap();
        assert_eq!(payload["subscribe"], "a");
        assert_eq!(payload["publish"], "a/bots/recorder");
    }

    #[tokio::test]
    async fn a_bot_is_found_only_under_its_own_type() {
        // Arrange
        let test = test_bots();
        let bot = started(&test, "app/a").await;

        // Act
        let other_type = test.bots.token("chat", &bot.id, Some(KEY), NOW).await;

        // Assert
        assert!(matches!(other_type, Err(BotError::NotFound)));
        assert!(test.bots.record("chat", &bot.id).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn a_wrong_key_an_unknown_bot_or_an_ended_one_gets_no_token() {
        // Arrange
        let test = test_bots();
        let bot = started(&test, "app/a").await;
        let ended = started(&test, "app/b").await;
        test.bots
            .heartbeat(
                "recorder",
                &ended.id,
                Some(KEY),
                heartbeat(BotState::Failed),
                NOW,
            )
            .await
            .unwrap();

        // Act / Assert
        assert!(matches!(
            test.bots
                .token("recorder", &bot.id, Some("other"), NOW)
                .await,
            Err(BotError::Unauthorized)
        ));
        assert!(matches!(
            test.bots.token("recorder", &bot.id, None, NOW).await,
            Err(BotError::Unauthorized)
        ));
        assert!(matches!(
            test.bots.token("recorder", "missing", Some(KEY), NOW).await,
            Err(BotError::NotFound)
        ));
        assert!(matches!(
            test.bots.token("recorder", &ended.id, Some(KEY), NOW).await,
            Err(BotError::Ended)
        ));
    }

    #[tokio::test]
    async fn a_bot_asked_to_stop_shows_as_stopping_before_its_heartbeat() {
        // Arrange
        let test = test_bots();
        let bot = started(&test, "app/a").await;

        // Act
        test.bots.stop(&bot.id).await.unwrap();

        // Assert
        let latest = test.bots.latest("recorder", "app/a", NOW).await.unwrap();
        assert_eq!(latest.unwrap().state, BotState::Stopping);
    }

    #[tokio::test]
    async fn a_heartbeat_asks_to_stop_once_stopped_or_at_the_longest_run() {
        // Arrange
        let test = test_bots();
        let stopped = started(&test, "app/a").await;
        let long = started(&test, "app/b").await;
        test.bots.stop(&stopped.id).await.unwrap();

        // Act
        let stop_requested = test
            .bots
            .heartbeat(
                "recorder",
                &stopped.id,
                Some(KEY),
                heartbeat(BotState::Running),
                NOW,
            )
            .await
            .unwrap();
        let before_max = test
            .bots
            .heartbeat(
                "recorder",
                &long.id,
                Some(KEY),
                heartbeat(BotState::Running),
                NOW + 59_000,
            )
            .await
            .unwrap();
        let at_max = test
            .bots
            .heartbeat(
                "recorder",
                &long.id,
                Some(KEY),
                heartbeat(BotState::Running),
                NOW + 60_000,
            )
            .await
            .unwrap();

        // Assert
        assert!(stop_requested && !before_max && at_max);
        let latest = test
            .bots
            .latest("recorder", "app/b", NOW + 60_000)
            .await
            .unwrap();
        assert_eq!(latest.unwrap().state, BotState::Running);
    }
}
