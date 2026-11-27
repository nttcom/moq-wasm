use std::{sync::Arc, time::Duration};

use anyhow::Result;
use moq_wasm::{
    Session,
    media::{WatchOptions, watch_broadcasts},
};
use tokio::sync::Semaphore;

use crate::{
    broadcast_namespace::BroadcastNamespaces, broadcast_translation,
    target_language::TargetLanguage, translation_engine::TranslationEngine,
};

/// When a language is translated. `always_on` languages start with the
/// broadcast and run while it lasts, so the first viewer of one does not wait
/// for Gemini's first answer; the others run only while subscribed, and stop
/// after `language_time_limit` of continuous running, so a tab left open does
/// not keep a Gemini session running.
pub(crate) struct TranslationPolicy {
    pub(crate) language_time_limit: Duration,
    pub(crate) always_on: Vec<&'static TargetLanguage>,
}

#[derive(Clone)]
pub(crate) struct TranslationResources {
    pub(crate) engine: TranslationEngine,
    /// One permit per Gemini session that may run at once.
    pub(crate) capacity: Arc<Semaphore>,
    pub(crate) policy: Arc<TranslationPolicy>,
}

/// Translates every broadcast under `namespaces`' root over `session`.
/// Returns once the session ends or, with `idle_timeout`, once no broadcast
/// has been under the root for that long. Fails when the relay refuses
/// SUBSCRIBE_NAMESPACE.
pub(crate) async fn translate_root(
    session: Arc<Session>,
    namespaces: BroadcastNamespaces,
    resources: TranslationResources,
    idle_timeout: Option<Duration>,
) -> Result<()> {
    let root: Vec<String> = namespaces.root().split('/').map(str::to_string).collect();
    tracing::info!(
        track_namespace_prefix = namespaces.root(),
        "waiting for broadcasts"
    );
    let options = WatchOptions {
        filter: Some(Arc::new(move |namespace: &[String]| {
            namespaces.is_broadcast(&namespace.join("/"))
        })),
        empty_timeout: idle_timeout,
    };
    let watching = session.clone();
    let watch = watch_broadcasts(&watching, &root, options, move |source| {
        let track_namespace = source.namespace().join("/");
        tracing::info!(track_namespace, "broadcast started");
        session.scoped(broadcast_translation::serve(
            session.clone(),
            resources.clone(),
            source,
        ))
    });
    watching.scoped(watch).await
}
