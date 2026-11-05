use std::time::Duration;

use anyhow::{Context, Result, bail, ensure};
use mediapack::loc::{LocObject, from_extension_headers};
use moqt::{
    ClientConfig, Endpoint, QUIC, Session, TrackReader, object::DataReceiver,
    subscribe::SubscribeOption,
};
use msf::Catalog;

use crate::{source_streamer::SOURCE_AUDIO_TRACK, unix_micros};

const TRANSLATED_OBJECTS_TO_CHECK: usize = 100;
const ECHO_DELAY_MICROS: i64 = 3_700_000;
const DELAY_TOLERANCE_MICROS: i64 = 400_000;
const FRAME_MICROS: u64 = 20_000;
const GROUP_MICROS: u64 = 2_000_000;
const STEP_TIMEOUT: Duration = Duration::from_secs(30);

pub(crate) async fn run(relay_url: &str, broadcast_namespace: &str) -> Result<()> {
    let endpoint = Endpoint::<QUIC>::create_client(&ClientConfig {
        verify_certificate: false,
        ..ClientConfig::default()
    })?;
    let session = endpoint.connect(relay_url).await?.await?;
    let translated_namespace = format!("{broadcast_namespace}/translated");
    let catalog = tokio::time::timeout(
        STEP_TIMEOUT,
        read_catalog_with_retry(&session, &translated_namespace),
    )
    .await
    .context("translated catalog did not arrive")??;
    check_catalog(&catalog, broadcast_namespace)?;
    check_unknown_language_is_rejected(&session, &translated_namespace).await?;
    tokio::time::timeout(
        STEP_TIMEOUT,
        check_translated_timestamps(&session, &translated_namespace),
    )
    .await
    .context("translated audio did not arrive")??;
    Ok(())
}

async fn read_catalog_with_retry(session: &Session, namespace: &str) -> Result<Catalog> {
    loop {
        match first_object(session, namespace, "catalog").await {
            Ok(object) => {
                return serde_json::from_slice(&object.payload).context("parse translated catalog");
            }
            Err(error) => {
                tracing::info!(?error, "translated catalog not ready yet");
                tokio::time::sleep(Duration::from_secs(1)).await;
            }
        }
    }
}

async fn first_object(
    session: &Session,
    namespace: &str,
    track: &str,
) -> Result<moqt::TrackObject> {
    let mut subscriber = session.subscriber();
    let subscription = subscriber
        .subscribe(
            namespace.to_string(),
            track.to_string(),
            SubscribeOption::default(),
        )
        .await?;
    let DataReceiver::Stream(factory) = subscriber.accept_data_receiver(&subscription).await?
    else {
        bail!("{track} arrived as datagrams");
    };
    let object = TrackReader::new(factory)
        .next_object()
        .await?
        .context("track ended without an object")?;
    let _ = subscriber.unsubscribe(&subscription).await;
    Ok(object)
}

fn check_catalog(catalog: &Catalog, broadcast_namespace: &str) -> Result<()> {
    let tracks = catalog.tracks.as_deref().unwrap_or_default();
    let find = |name: &str| tracks.iter().find(|track| track.name == name);
    let video = find("video").context("catalog lists the source video")?;
    ensure!(
        video.namespace.as_deref() == Some(broadcast_namespace),
        "video points at the broadcast"
    );
    let source = find(SOURCE_AUDIO_TRACK).context("catalog lists the source audio")?;
    ensure!(
        source.lang.as_deref() == Some("ja"),
        "source audio is Japanese"
    );
    let english = find("audio_en").context("catalog lists English")?;
    ensure!(
        english.namespace.is_none(),
        "translated tracks live in the catalog's namespace"
    );
    ensure!(
        english.alt_group == source.alt_group,
        "languages share one alt group"
    );
    tracing::info!(tracks = tracks.len(), "translated catalog is correct");
    Ok(())
}

async fn check_unknown_language_is_rejected(session: &Session, namespace: &str) -> Result<()> {
    let result = session
        .subscriber()
        .subscribe(
            namespace.to_string(),
            "audio_fr".to_string(),
            SubscribeOption::default(),
        )
        .await;
    ensure!(
        result.is_err(),
        "a language outside the list must be rejected"
    );
    tracing::info!("unknown language is rejected");
    Ok(())
}

async fn check_translated_timestamps(session: &Session, namespace: &str) -> Result<()> {
    let mut subscriber = session.subscriber();
    let subscription = subscriber
        .subscribe(
            namespace.to_string(),
            "audio_en".to_string(),
            SubscribeOption::default(),
        )
        .await?;
    let DataReceiver::Stream(factory) = subscriber.accept_data_receiver(&subscription).await?
    else {
        bail!("translated audio arrived as datagrams");
    };
    let mut reader = TrackReader::new(factory);
    let mut previous = None;
    let mut delays = Vec::with_capacity(TRANSLATED_OBJECTS_TO_CHECK);
    while delays.len() < TRANSLATED_OBJECTS_TO_CHECK {
        let object = reader
            .next_object()
            .await?
            .context("translated audio ended")?;
        let received_micros = unix_micros();
        let group_id = object.group_id;
        let loc = LocObject {
            extensions: from_extension_headers(&object.extension_headers),
            payload: object.payload,
        };
        let timestamp = loc
            .capture_timestamp()
            .context("translated object has a timestamp")?
            .micros();
        ensure!(
            group_id == timestamp / GROUP_MICROS,
            "group {group_id} matches timestamp {timestamp}"
        );
        if let Some(previous) = previous {
            ensure!(
                timestamp >= previous + FRAME_MICROS,
                "timestamps advance by at least one frame"
            );
        }
        previous = Some(timestamp);
        delays.push(received_micros as i64 - timestamp as i64);
    }
    let _ = subscriber.unsubscribe(&subscription).await;
    delays.sort_unstable();
    let median = delays[delays.len() / 2];
    tracing::info!(
        median_micros = median,
        "translated audio arrives this long after its timestamp"
    );
    ensure!(
        (median - ECHO_DELAY_MICROS).abs() <= DELAY_TOLERANCE_MICROS,
        "translated timestamps sit at the source time: median arrival lag {median} µs, expected about {ECHO_DELAY_MICROS} µs"
    );
    Ok(())
}
