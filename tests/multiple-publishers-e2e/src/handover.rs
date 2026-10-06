//! Scenarios where a subscriber must keep receiving a track while its
//! publishers come and go (draft-14 §8.4).
//!
//! Every namespace publisher here sends the same objects at the same time: one
//! group every `GROUP_INTERVAL`, its id taken from the wall clock, so any two
//! publishers of a track are redundant copies the relay can merge.

use std::{
    process::Stdio,
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use crate::{connect, object_payload};
use anyhow::{Context as _, bail};
use moqt::{
    ContentExists, DataReceiver, ExtensionHeaders, FilterType, GroupOrder, Session, SessionEvent,
    Subgroup, SubgroupId, SubgroupObject, SubscribeOption, Subscription,
};
use tokio::{
    io::{AsyncBufReadExt, BufReader},
    sync::mpsc,
    task::JoinHandle,
};

pub(crate) const SERVE_NAMESPACE_ARG: &str = "--serve-namespace";
const READY_LINE: &str = "namespace publisher ready";
const TRACK_NAME: &str = "clock";
const GROUP_INTERVAL: Duration = Duration::from_millis(200);
const OBJECTS_PER_GROUP: u64 = 3;
// The relay times out an unanswered upstream SUBSCRIBE after 10 s; answering
// well within that proves the SUBSCRIBE did not wait for the killed publisher.
const SUBSCRIBE_DEADLINE: Duration = Duration::from_secs(3);
const GROUPS_AFTER_HANDOVER: usize = 5;

fn current_group_id() -> u64 {
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("the clock is after the epoch")
        .as_millis() as u64;
    millis / GROUP_INTERVAL.as_millis() as u64
}

pub(crate) struct NamespacePublisher {
    session: Arc<Session>,
    event_loop: JoinHandle<anyhow::Result<()>>,
}

impl NamespacePublisher {
    pub(crate) async fn announce(relay_url: &str, track_namespace: &str) -> anyhow::Result<Self> {
        let session = Arc::new(connect(relay_url).await?);
        session
            .publisher()
            .publish_namespace(track_namespace.to_string())
            .await
            .context("PUBLISH_NAMESPACE was not accepted")?;
        let event_loop = tokio::spawn(Self::answer_subscribes(session.clone()));
        Ok(Self {
            session,
            event_loop,
        })
    }

    async fn answer_subscribes(session: Arc<Session>) -> anyhow::Result<()> {
        loop {
            match session.receive_event().await? {
                SessionEvent::Subscribe(handler) => {
                    let track_alias = handler.ok(1_000_000, ContentExists::False).await?;
                    let subscription = handler.into_subscription(track_alias);
                    tokio::spawn(send_clock_groups(session.clone(), subscription));
                }
                SessionEvent::Disconnected() => return Ok(()),
                SessionEvent::ProtocolViolation() => bail!("publisher protocol violation"),
                _ => {}
            }
        }
    }

    pub(crate) async fn leave(self) {
        self.event_loop.abort();
        drop(self.session);
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
}

async fn send_clock_groups(
    session: Arc<Session>,
    subscription: Subscription,
) -> anyhow::Result<()> {
    let stream_factory = session.publisher().create_stream(&subscription);
    let mut ticker = tokio::time::interval(GROUP_INTERVAL);
    let mut last_group_id = None;
    loop {
        ticker.tick().await;
        let group_id = current_group_id();
        if last_group_id >= Some(group_id) {
            continue;
        }
        last_group_id = Some(group_id);
        let uninitialized = stream_factory.next().await?;
        let header = uninitialized.create_header(group_id, SubgroupId::None, 128, false, false);
        let mut stream = uninitialized.send_header(header).await?;
        for object_id in 0..OBJECTS_PER_GROUP {
            let object = stream.create_object_field(
                if object_id == 0 { 0 } else { 1 },
                ExtensionHeaders::default(),
                SubgroupObject::new_payload(object_payload(group_id, object_id).into()),
            );
            stream.send(object).await?;
        }
        stream.close().await?;
    }
}

/// Runs in the child process of the killed-publisher scenario.
pub(crate) async fn serve_namespace_forever(
    relay_url: &str,
    track_namespace: &str,
) -> anyhow::Result<()> {
    let _publisher = NamespacePublisher::announce(relay_url, track_namespace).await?;
    println!("{READY_LINE}");
    std::future::pending().await
}

struct ClockSubscriber {
    _session: Arc<Session>,
    group_receiver: mpsc::UnboundedReceiver<u64>,
    _receiver_task: JoinHandle<anyhow::Result<()>>,
}

impl ClockSubscriber {
    async fn subscribe(relay_url: &str, track_namespace: &str) -> anyhow::Result<Self> {
        let session = Arc::new(connect(relay_url).await?);
        let mut subscriber = session.subscriber();
        let subscription = tokio::time::timeout(
            SUBSCRIBE_DEADLINE,
            subscriber.subscribe(
                track_namespace.to_string(),
                TRACK_NAME.to_string(),
                SubscribeOption {
                    subscriber_priority: 128,
                    group_order: GroupOrder::Ascending,
                    forward: true,
                    filter_type: FilterType::LargestObject,
                },
            ),
        )
        .await
        .context("SUBSCRIBE was not answered in time")??;
        let DataReceiver::Stream(mut factory) =
            subscriber.accept_data_receiver(&subscription).await?
        else {
            bail!("expected a stream data receiver");
        };
        let (group_sender, group_receiver) = mpsc::unbounded_channel();
        let receiver_task = tokio::spawn(async move {
            loop {
                let mut stream = factory.next().await?;
                let group_sender = group_sender.clone();
                tokio::spawn(async move {
                    let mut objects = 0;
                    let mut group_id = None;
                    while let Ok(Some(subgroup)) = stream.receive().await {
                        match subgroup {
                            Subgroup::Header(header) => group_id = Some(header.group_id),
                            Subgroup::Object(_) => objects += 1,
                        }
                    }
                    if let Some(group_id) = group_id
                        && objects == OBJECTS_PER_GROUP
                    {
                        let _ = group_sender.send(group_id);
                    }
                });
            }
        });
        Ok(Self {
            _session: session,
            group_receiver,
            _receiver_task: receiver_task,
        })
    }

    async fn expect_groups_after(
        &mut self,
        after_group_id: u64,
        count: usize,
        within: Duration,
    ) -> anyhow::Result<()> {
        let deadline = tokio::time::Instant::now() + within;
        let mut received = 0;
        while received < count {
            let group_id = tokio::time::timeout_at(deadline, self.group_receiver.recv())
                .await
                .with_context(|| format!("received only {received} of {count} complete groups"))?
                .context("the subscription ended")?;
            if group_id > after_group_id {
                received += 1;
            }
        }
        Ok(())
    }
}

fn scenario_namespace(scenario: &str) -> String {
    format!(
        "anon/multiple-publishers-e2e/{scenario}-{}",
        current_group_id()
    )
}

/// The bug this pins: a publisher killed without CONNECTION_CLOSE stays
/// registered until its QUIC idle timeout, and the relay used to send new
/// SUBSCRIBEs to it alone.
pub(crate) async fn run_killed_publisher_scenario(relay_url: &str) -> anyhow::Result<()> {
    let track_namespace = scenario_namespace("killed-publisher");
    let mut killed = tokio::process::Command::new(std::env::current_exe()?)
        .args([SERVE_NAMESPACE_ARG, &track_namespace])
        .env("MOQT_E2E_RELAY_URL", relay_url)
        .stdout(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .context("failed to start the publisher process")?;
    let mut stdout = BufReader::new(killed.stdout.take().expect("stdout is piped")).lines();
    while stdout.next_line().await?.as_deref() != Some(READY_LINE) {}
    killed.start_kill()?;
    killed.wait().await?;
    tracing::info!("[killed-publisher] publisher process killed with SIGKILL");

    let _restarted = NamespacePublisher::announce(relay_url, &track_namespace).await?;
    let mut subscriber = ClockSubscriber::subscribe(relay_url, &track_namespace)
        .await
        .context("SUBSCRIBE must be served by the restarted publisher")?;
    subscriber
        .expect_groups_after(0, GROUPS_AFTER_HANDOVER, Duration::from_secs(5))
        .await?;
    tracing::info!("[killed-publisher] OK: the restarted publisher served the subscriber");
    Ok(())
}

/// A second publisher announcing the namespace joins the subscribed track, so
/// the first one leaving does not end the subscription.
pub(crate) async fn run_handover_scenario(
    first_publisher_relay_url: &str,
    second_publisher_relay_url: &str,
    scenario: &str,
) -> anyhow::Result<()> {
    let track_namespace = scenario_namespace(scenario);
    let first = NamespacePublisher::announce(first_publisher_relay_url, &track_namespace).await?;
    let mut subscriber =
        ClockSubscriber::subscribe(first_publisher_relay_url, &track_namespace).await?;
    subscriber
        .expect_groups_after(0, 2, Duration::from_secs(5))
        .await
        .context("the first publisher should serve the subscriber")?;

    let _second =
        NamespacePublisher::announce(second_publisher_relay_url, &track_namespace).await?;
    // Leave time for the relay to subscribe the announcing publisher.
    tokio::time::sleep(Duration::from_secs(1)).await;
    first.leave().await;
    let left_at_group_id = current_group_id();
    tracing::info!("[{scenario}] first publisher left");

    subscriber
        .expect_groups_after(
            left_at_group_id,
            GROUPS_AFTER_HANDOVER,
            Duration::from_secs(5),
        )
        .await
        .context("the second publisher should keep the subscription alive")?;
    tracing::info!("[{scenario}] OK: the subscription survived the first publisher leaving");
    Ok(())
}
