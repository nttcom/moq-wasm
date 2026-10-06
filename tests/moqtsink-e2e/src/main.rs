use std::{collections::BTreeSet, env, time::Duration};

use anyhow::{Context, Result, bail, ensure};
use moqt::{
    ClientConfig, ContentExists, DataReceiver, Endpoint, Fetch, FetchObject, FetchOption,
    FilterType, GroupOrder, Location, QUIC, Session, Subgroup, SubgroupObject, SubscribeOption,
    Subscriber, Subscription,
};
use msf::Catalog;
use tokio::process::{Child, Command};

const DEFAULT_RELAY_URL: &str = "moqt://127.0.0.1:4433";
const NAMESPACE: &str = "anon/moqtsink/e2e";
const CATALOG_TRACK: &str = "catalog";
const PUBLISHED_TRACKS: [&str; 2] = ["video", "audio"];
const SUBSCRIBED_TRACK: &str = "video_cmaf";
const STARTUP_TIMEOUT: Duration = Duration::from_secs(30);
const RECEIVE_TIMEOUT: Duration = Duration::from_secs(20);
const RETRY_INTERVAL: Duration = Duration::from_secs(1);

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::INFO)
        .try_init()
        .ok();
    let relay_url = env::var("MOQT_E2E_RELAY_URL").unwrap_or_else(|_| DEFAULT_RELAY_URL.into());
    let plugin_file =
        env::var("MOQT_E2E_PLUGIN_FILE").context("MOQT_E2E_PLUGIN_FILE is not set")?;
    let mut sink = spawn_moqtsink(&plugin_file, &relay_url)?;
    tokio::select! {
        result = verify(&relay_url) => result,
        status = sink.wait() => Err(anyhow::anyhow!("gst-launch exited early: {:?}", status?)),
    }?;
    tracing::info!("moqtsink e2e passed");
    Ok(())
}

fn spawn_moqtsink(plugin_file: &str, relay_url: &str) -> Result<Child> {
    let pipeline = format!(
        "moqtsink name=moqt relay-url={relay_url} namespace={NAMESPACE} \
         videotestsrc is-live=true ! video/x-raw,width=320,height=180,framerate=30/1 \
         ! x264enc tune=zerolatency speed-preset=ultrafast key-int-max=30 \
         ! h264parse config-interval=-1 ! moqt.video \
         audiotestsrc is-live=true ! audioconvert ! audioresample ! avenc_aac \
         ! aacparse ! moqt.audio"
    );
    Command::new("gst-launch-1.0")
        .arg(format!("--gst-plugin-load={plugin_file}"))
        .args(pipeline.split_whitespace())
        .kill_on_drop(true)
        .spawn()
        .context("spawn gst-launch-1.0")
}

async fn verify(relay_url: &str) -> Result<()> {
    let session = connect(relay_url).await?;
    let mut subscriber = session.subscriber();

    let catalog = subscribe_to_published_track(&mut subscriber, CATALOG_TRACK).await?;
    let tracks = fetch_catalog_track_names(&mut subscriber, &catalog).await?;
    for track in PUBLISHED_TRACKS.iter().chain([&SUBSCRIBED_TRACK]) {
        ensure!(tracks.contains(*track), "catalog lacks {track}: {tracks:?}");
    }

    for track in PUBLISHED_TRACKS {
        let subscription = subscribe_to_published_track(&mut subscriber, track).await?;
        expect_groups(&mut subscriber, &subscription, 2).await?;
    }

    let subscription = subscribe(&mut subscriber, SUBSCRIBED_TRACK).await?;
    expect_groups(&mut subscriber, &subscription, 1).await
}

async fn connect(relay_url: &str) -> Result<Session> {
    let endpoint = Endpoint::<QUIC>::create_client(&ClientConfig {
        port: 0,
        verify_certificate: false,
        authorization_token: None,
    })?;
    endpoint.connect(relay_url).await?.await
}

async fn subscribe(subscriber: &mut Subscriber, track: &str) -> Result<Subscription> {
    subscriber
        .subscribe(
            NAMESPACE.to_string(),
            track.to_string(),
            SubscribeOption {
                subscriber_priority: 128,
                group_order: GroupOrder::Ascending,
                forward: true,
                filter_type: FilterType::LargestObject,
            },
        )
        .await
        .with_context(|| format!("subscribe {track}"))
}

/// Content Exists in the relay's SUBSCRIBE_OK proves the sink published the
/// track before any subscriber asked for it.
async fn subscribe_to_published_track(
    subscriber: &mut Subscriber,
    track: &str,
) -> Result<Subscription> {
    let wait = async {
        loop {
            match subscribe(subscriber, track).await {
                Ok(subscription)
                    if matches!(subscription.content_exists(), ContentExists::True { .. }) =>
                {
                    tracing::info!(track, "relay holds the track before any subscriber");
                    return Ok(subscription);
                }
                Ok(subscription) => {
                    subscriber.unsubscribe(subscription.request_id()).await?;
                }
                Err(error) => tracing::info!(track, ?error, "track not published yet"),
            }
            tokio::time::sleep(RETRY_INTERVAL).await;
        }
    };
    tokio::time::timeout(STARTUP_TIMEOUT, wait)
        .await
        .with_context(|| format!("{track} was not published within {STARTUP_TIMEOUT:?}"))?
}

async fn fetch_catalog_track_names(
    subscriber: &mut Subscriber,
    catalog: &Subscription,
) -> Result<BTreeSet<String>> {
    let ContentExists::True { location } = catalog.content_exists() else {
        bail!("catalog has no content");
    };
    let handle = subscriber
        .fetch(
            NAMESPACE.to_string(),
            CATALOG_TRACK.to_string(),
            Location {
                group_id: location.group_id,
                object_id: 0,
            },
            Location {
                group_id: location.group_id,
                object_id: location.object_id + 1,
            },
            FetchOption::default(),
        )
        .await?;
    let mut receiver = subscriber.accept_fetch_receiver(&handle).await?;
    loop {
        match tokio::time::timeout(RECEIVE_TIMEOUT, receiver.receive())
            .await
            .context("catalog FETCH timed out")??
        {
            Fetch::Object(object) => {
                if let FetchObject::Payload(payload) = object.fetch_object {
                    let catalog: Catalog = serde_json::from_slice(&payload)?;
                    return Ok(catalog
                        .tracks
                        .unwrap_or_default()
                        .into_iter()
                        .map(|track| track.name)
                        .collect());
                }
            }
            Fetch::Header(_) => {}
            Fetch::End => bail!("catalog FETCH returned no object"),
        }
    }
}

async fn expect_groups(
    subscriber: &mut Subscriber,
    subscription: &Subscription,
    groups: usize,
) -> Result<()> {
    let track = subscription.track_name();
    let receive = async {
        let DataReceiver::Stream(mut streams) =
            subscriber.accept_data_receiver(subscription).await?
        else {
            bail!("{track} arrived as datagrams");
        };
        let mut received = BTreeSet::new();
        while received.len() < groups {
            let mut stream = streams.next().await?;
            let mut group_id = None;
            while let Some(object) = stream.receive().await? {
                match object {
                    Subgroup::Header(header) => group_id = Some(header.group_id),
                    Subgroup::Object(field) => {
                        if matches!(field.subgroup_object, SubgroupObject::Payload { .. }) {
                            received.extend(group_id);
                            break;
                        }
                    }
                }
            }
        }
        tracing::info!(track, ?received, "received objects");
        Ok(())
    };
    tokio::time::timeout(RECEIVE_TIMEOUT, receive)
        .await
        .with_context(|| {
            format!("{track}: fewer than {groups} groups within {RECEIVE_TIMEOUT:?}")
        })??;
    subscriber.unsubscribe(subscription.request_id()).await
}
