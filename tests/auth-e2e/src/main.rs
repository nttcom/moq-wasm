//! Authentication e2e against two relays backed by the VTS and the anon issuer.
//!
//! Driven by `scripts/auth-e2e.sh`, which starts the compose stack with the
//! `auth` profile, mints the tokens and passes them through the environment:
//!
//! - `AUTH_E2E_APP_ID`      appId of the client app (`publish`/`subscribe` = `site1`)
//! - `AUTH_E2E_APP_TOKEN`   long-lived token for that app
//! - `AUTH_E2E_SHORT_TOKEN` same claims, expires within a minute
//! - `AUTH_E2E_RELAY_TOKEN` a relay token (must be rejected on the client port)
//! - `AUTH_E2E_ANON_TOKEN`  token obtained from the anon issuer
//!
//! `--scenario vts-down` runs only the check that connecting fails while the
//! VTS is unreachable.

use std::{env, sync::Arc, time::Duration};

use anyhow::{Context, bail};
use bytes::Bytes;
use moqt::{
    ClientConfig, DataReceiver, Endpoint, ExtensionHeaders, FilterType, GroupOrder, PublishOption,
    QUIC, Session, SessionEvent, Subgroup, SubgroupId, SubgroupObject, SubscribeOption,
    Subscription, wire::RequestError,
};

const DEFAULT_RELAY_A_URL: &str = "moqt://127.0.0.1:4433";
const DEFAULT_RELAY_B_URL: &str = "moqt://127.0.0.1:4434";
const TEST_PAYLOAD: &[u8] = b"auth e2e payload";
const UNAUTHORIZED: u64 = 0x1;
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);
const EXPIRY_TIMEOUT: Duration = Duration::from_secs(90);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Scenario {
    All,
    VtsDown,
}

struct Config {
    relay_a_url: String,
    relay_b_url: String,
    scenario: Scenario,
}

struct Tokens {
    app_id: String,
    app: String,
    short: String,
    relay: String,
    anon: String,
}

impl Config {
    fn from_args() -> anyhow::Result<Self> {
        let mut config = Self {
            relay_a_url: DEFAULT_RELAY_A_URL.to_string(),
            relay_b_url: DEFAULT_RELAY_B_URL.to_string(),
            scenario: Scenario::All,
        };
        let mut args = env::args().skip(1);
        while let Some(arg) = args.next() {
            let value = args
                .next()
                .ok_or_else(|| anyhow::anyhow!("missing value for {arg}"))?;
            match arg.as_str() {
                "--relay-a-url" => config.relay_a_url = value,
                "--relay-b-url" => config.relay_b_url = value,
                "--scenario" => {
                    config.scenario = match value.as_str() {
                        "all" => Scenario::All,
                        "vts-down" => Scenario::VtsDown,
                        other => bail!("unknown scenario: {other}"),
                    }
                }
                _ => bail!("unknown argument: {arg}"),
            }
        }
        Ok(config)
    }
}

impl Tokens {
    fn from_env() -> anyhow::Result<Self> {
        fn required(name: &str) -> anyhow::Result<String> {
            env::var(name).with_context(|| format!("{name} is not set"))
        }
        Ok(Self {
            app_id: required("AUTH_E2E_APP_ID")?,
            app: required("AUTH_E2E_APP_TOKEN")?,
            short: required("AUTH_E2E_SHORT_TOKEN")?,
            relay: required("AUTH_E2E_RELAY_TOKEN")?,
            anon: required("AUTH_E2E_ANON_TOKEN")?,
        })
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info,moqt=warn".into()),
        )
        .init();
    let config = Config::from_args()?;
    let app_token = env::var("AUTH_E2E_APP_TOKEN").context("AUTH_E2E_APP_TOKEN is not set")?;

    match config.scenario {
        Scenario::VtsDown => {
            expect_handshake_rejected(&config.relay_a_url, Some(&app_token), "vts down").await?;
        }
        Scenario::All => run_all(&config, Tokens::from_env()?).await?,
    }

    tracing::info!("auth e2e passed");
    println!("auth e2e passed");
    Ok(())
}

async fn run_all(config: &Config, tokens: Tokens) -> anyhow::Result<()> {
    let run_id = run_id();

    wait_until_relay_accepts(&config.relay_a_url, &tokens.app).await?;
    wait_until_relay_accepts(&config.relay_b_url, &tokens.app).await?;

    let expiry = tokio::spawn(expect_session_closed_on_expiry(
        config.relay_a_url.clone(),
        tokens.short.clone(),
    ));

    expect_handshake_rejected(&config.relay_a_url, None, "no token").await?;
    expect_handshake_rejected(&config.relay_a_url, Some("not-a-jwt"), "garbage token").await?;
    expect_handshake_rejected(
        &config.relay_a_url,
        Some(&tokens.relay),
        "relay token on client port",
    )
    .await?;

    cross_relay_pub_sub(config, &tokens, &run_id).await?;
    app_token_is_scoped(&config.relay_a_url, &tokens).await?;
    anon_token_reaches_only_anon(&config.relay_a_url, &tokens, &run_id).await?;

    expiry.await??;
    Ok(())
}

async fn wait_until_relay_accepts(url: &str, token: &str) -> anyhow::Result<()> {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    let mut last_error = None;
    while tokio::time::Instant::now() < deadline {
        match connect(url, Some(token)).await {
            Ok(_) => return Ok(()),
            Err(error) => {
                tracing::info!(?error, url, "relay is not ready yet");
                last_error = Some(error);
                tokio::time::sleep(Duration::from_millis(500)).await;
            }
        }
    }
    Err(last_error.unwrap_or_else(|| anyhow::anyhow!("failed to connect to {url}")))
}

async fn connect(url: &str, token: Option<&str>) -> anyhow::Result<Session<QUIC>> {
    let endpoint = Endpoint::<QUIC>::create_client(&ClientConfig {
        port: 0,
        verify_certificate: false,
        authorization_token: token.map(str::to_string),
    })?;
    let connecting = endpoint.connect(url).await?;
    tokio::time::timeout(HANDSHAKE_TIMEOUT, connecting)
        .await
        .context("handshake timed out")?
}

async fn expect_handshake_rejected(
    url: &str,
    token: Option<&str>,
    label: &str,
) -> anyhow::Result<()> {
    match connect(url, token).await {
        Ok(_) => bail!("{label}: handshake unexpectedly succeeded"),
        Err(error) => {
            tracing::info!(%error, label, "handshake rejected as expected");
            Ok(())
        }
    }
}

async fn cross_relay_pub_sub(config: &Config, tokens: &Tokens, run_id: &str) -> anyhow::Result<()> {
    let namespace = format!("{}/site1/{run_id}", tokens.app_id);
    let track_name = "video".to_string();

    let publisher_session = Arc::new(connect(&config.relay_a_url, Some(&tokens.app)).await?);
    let publisher = publisher_session.publisher();
    publisher
        .publish_namespace(namespace.clone())
        .await
        .context("publisher failed to publish namespace")?;
    let published = publisher
        .publish(
            namespace.clone(),
            track_name.clone(),
            PublishOption::default(),
        )
        .await
        .context("publisher failed to publish track")?;
    tokio::time::sleep(Duration::from_millis(500)).await;

    let subscriber_session = Arc::new(connect(&config.relay_b_url, Some(&tokens.app)).await?);
    let received = tokio::time::timeout(
        Duration::from_secs(15),
        subscribe_and_receive_one_object(subscriber_session, namespace, track_name, move || {
            send_test_object(publisher_session, published)
        }),
    )
    .await
    .context("timed out waiting for the object across relays")??;
    if received != TEST_PAYLOAD {
        bail!("unexpected payload across relays: {received:?}");
    }
    tracing::info!("cross-relay pub/sub with app tokens passed");
    Ok(())
}

async fn app_token_is_scoped(url: &str, tokens: &Tokens) -> anyhow::Result<()> {
    let session = connect(url, Some(&tokens.app)).await?;

    let error = session
        .subscriber()
        .subscribe(
            "OTHER/x".to_string(),
            "video".to_string(),
            SubscribeOption::default(),
        )
        .await
        .err()
        .context("SUBSCRIBE to another appId unexpectedly succeeded")?;
    expect_unauthorized(error, "SUBSCRIBE OTHER/x")?;

    let error = session
        .publisher()
        .publish_namespace(format!("{}/site2", tokens.app_id))
        .await
        .err()
        .context("PUBLISH_NAMESPACE outside the granted path unexpectedly succeeded")?;
    expect_unauthorized(error, "PUBLISH_NAMESPACE APP/site2")?;
    tracing::info!("app token scope enforced");
    Ok(())
}

async fn anon_token_reaches_only_anon(
    url: &str,
    tokens: &Tokens,
    run_id: &str,
) -> anyhow::Result<()> {
    let session = connect(url, Some(&tokens.anon)).await?;
    session
        .publisher()
        .publish_namespace(format!("anon/demo/{run_id}"))
        .await
        .context("anon token failed to publish under anon/")?;

    let error = session
        .publisher()
        .publish_namespace(format!("{}/site1", tokens.app_id))
        .await
        .err()
        .context("anon token unexpectedly published under another appId")?;
    expect_unauthorized(error, "PUBLISH_NAMESPACE APP/site1 with anon token")?;
    tracing::info!("anon token confined to anon/");
    Ok(())
}

async fn expect_session_closed_on_expiry(url: String, short_token: String) -> anyhow::Result<()> {
    let session = connect(&url, Some(&short_token)).await?;
    let started = tokio::time::Instant::now();
    tokio::time::timeout(EXPIRY_TIMEOUT, async {
        loop {
            match session.receive_event().await? {
                SessionEvent::Disconnected() | SessionEvent::ProtocolViolation() => {
                    return Ok::<(), anyhow::Error>(());
                }
                _ => {}
            }
        }
    })
    .await
    .context("session was not closed after the token expired")??;
    tracing::info!(
        elapsed_secs = started.elapsed().as_secs(),
        "session closed after token expiry"
    );
    Ok(())
}

fn expect_unauthorized(error: anyhow::Error, label: &str) -> anyhow::Result<()> {
    let request_error = error
        .downcast_ref::<RequestError>()
        .with_context(|| format!("{label}: expected a request error, got {error:?}"))?;
    if request_error.error_code != UNAUTHORIZED {
        bail!(
            "{label}: expected error code {UNAUTHORIZED}, got {}",
            request_error.error_code
        );
    }
    Ok(())
}

async fn send_test_object(
    session: Arc<Session<QUIC>>,
    subscription: Subscription,
) -> anyhow::Result<()> {
    let stream_factory = session.publisher().create_stream(&subscription);
    let uninitialized = stream_factory.next().await?;
    let header = uninitialized.create_header(0, SubgroupId::None, 128, false, false);
    let mut stream = uninitialized.send_header(header).await?;
    let object = stream.create_object_field(
        0,
        ExtensionHeaders::default(),
        SubgroupObject::new_payload(Bytes::from_static(TEST_PAYLOAD)),
    );
    stream.send(object).await?;
    stream.close().await?;
    Ok(())
}

async fn subscribe_and_receive_one_object<F, Fut>(
    session: Arc<Session<QUIC>>,
    track_namespace: String,
    track_name: String,
    after_subscribe: F,
) -> anyhow::Result<Vec<u8>>
where
    F: FnOnce() -> Fut,
    Fut: Future<Output = anyhow::Result<()>>,
{
    let option = SubscribeOption {
        subscriber_priority: 128,
        group_order: GroupOrder::Ascending,
        forward: true,
        filter_type: FilterType::LargestObject,
    };
    let subscription = session
        .subscriber()
        .subscribe(track_namespace, track_name, option)
        .await
        .context("subscriber failed to subscribe")?;
    after_subscribe().await?;
    let receiver = session
        .subscriber()
        .accept_data_receiver(&subscription)
        .await
        .context("subscriber failed to accept data receiver")?;
    let DataReceiver::Stream(mut factory) = receiver else {
        bail!("expected a subgroup stream receiver");
    };
    let mut stream = factory.next().await?;
    loop {
        let Some(subgroup) = stream.receive().await? else {
            bail!("stream ended before receiving an object");
        };
        if let Subgroup::Object(field) = subgroup
            && let SubgroupObject::Payload { data, .. } = field.subgroup_object
        {
            return Ok(data.to_vec());
        }
    }
}

fn run_id() -> String {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis().to_string())
        .unwrap_or_else(|_| "0".to_string())
}
