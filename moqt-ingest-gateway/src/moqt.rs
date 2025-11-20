use std::collections::HashMap;
use std::sync::Arc;

use anyhow::{Context, Result, bail};
use moqt_client_rust::publisher::MoqtPublisher;
use tokio::sync::Mutex;

#[derive(Clone)]
pub struct MoqtManager {
    url: Option<String>,
    inner: Arc<Mutex<ManagerState>>,
}

#[derive(Default)]
struct ManagerState {
    publisher: Option<MoqtPublisher>,
    next_alias: u64,
    tracks: HashMap<(String, String), TrackInfo>,
    created_once: bool,
}

#[derive(Default)]
struct TrackInfo {
    alias: u64,
    subscriber_alias: Option<u64>,
    announced: bool,
    header_sent: bool,
    subscribed: bool,
    stream: Option<wtransport::stream::SendStream>,
    object_id: u64,
}

impl MoqtManager {
    pub fn new(url: Option<String>) -> Self {
        Self {
            url,
            inner: Arc::new(Mutex::new(ManagerState::default())),
        }
    }

    /// Setup MoQ publisher and return after SubscribeOk is sent.
    pub async fn setup_publisher(&self, namespace: &[String], track_name: &str) -> Result<()> {
        let url = match &self.url {
            Some(u) => u.clone(),
            None => return Ok(()), // MoQ 出力なし
        };

        self.ensure_publisher(&url).await?;

        // Publisher をロック外で扱うため一時的に取り出す
        let (mut publisher, base_alias, announce_needed, subscribed, key) = {
            let mut guard = self.inner.lock().await;
            let publisher = guard
                .publisher
                .take()
                .ok_or_else(|| anyhow::anyhow!("publisher not initialized"))?;
            let key = (namespace.join("/"), track_name.to_string());
            if !guard.tracks.contains_key(&key) {
                let alias = guard.next_alias;
                guard.next_alias += 1;
                guard.tracks.insert(
                    key.clone(),
                    TrackInfo {
                        alias,
                        subscriber_alias: None,
                        ..Default::default()
                    },
                );
            }
            let entry = guard.tracks.get_mut(&key).expect("entry exists");
            let base_alias = entry.alias;
            let announce_needed = !entry.announced;
            let subscribed = entry.subscribed;
            (publisher, base_alias, announce_needed, subscribed, key)
        };

        if announce_needed {
            publisher
                .announce(namespace, "auth".to_string())
                .await
                .context("announce")?;
        }

        // SUBSCRIBE を待って SubscribeOk を返し、サーバ側 alias を採用
        let subscriber_alias = if subscribed {
            base_alias
        } else {
            publisher
                .wait_subscribe_and_accept(base_alias, namespace, track_name)
                .await
                .context("wait subscribe and send ok")?
        };

        // state を更新して publisher を戻す
        let mut guard = self.inner.lock().await;
        if let Some(entry) = guard.tracks.get_mut(&key) {
            if announce_needed {
                entry.announced = true;
            }
            entry.subscribed = true;
            entry.subscriber_alias = Some(subscriber_alias);
        }
        guard.publisher = Some(publisher);
        Ok(())
    }

    pub async fn send_object(
        &self,
        namespace: &[String],
        track_name: &str,
        payload: &[u8],
    ) -> Result<()> {
        let url = match &self.url {
            Some(u) => u.clone(),
            None => return Ok(()),
        };

        self.ensure_publisher(&url).await?;

        let (publisher, subscriber_alias, header_needed, key, object_id) = {
            let mut guard = self.inner.lock().await;
            let publisher = guard
                .publisher
                .take()
                .ok_or_else(|| anyhow::anyhow!("publisher not initialized"))?;
            let key = (namespace.join("/"), track_name.to_string());
            let entry = guard
                .tracks
                .get_mut(&key)
                .ok_or_else(|| anyhow::anyhow!("track not set up"))?;
            let subscriber_alias = entry.subscriber_alias.ok_or_else(|| {
                anyhow::anyhow!("subscriber alias not ready (subscribe_ok pending)")
            })?;
            let header_needed = !entry.header_sent;
            entry.object_id = entry.object_id.saturating_add(1);
            let object_id = entry.object_id;
            (publisher, subscriber_alias, header_needed, key, object_id)
        };

        let mut guard = self.inner.lock().await;
        let entry = guard
            .tracks
            .get_mut(&key)
            .ok_or_else(|| anyhow::anyhow!("track not set up"))?;
        if entry.stream.is_none() {
            let stream = publisher
                .open_data_uni()
                .await
                .context("open data uni stream")?;
            entry.stream = Some(stream);
        }
        let stream = entry.stream.as_mut().expect("stream exists");

        // Header 未送信なら同じストリームに送る
        if header_needed {
            publisher
                .write_subgroup_header(stream, subscriber_alias, 0, 0, 0)
                .await
                .context("send subgroup header")?;
            entry.header_sent = true;
        }

        publisher
            .write_subgroup_object(stream, subscriber_alias, 0, 0, object_id, None, payload)
            .await
            .context("send subgroup object")?;

        entry.object_id = object_id;
        guard.publisher = Some(publisher);
        Ok(())
    }

    async fn ensure_publisher(&self, url: &str) -> Result<()> {
        let need_create = {
            let guard = self.inner.lock().await;
            guard.publisher.is_none()
        };

        if need_create {
            let created = {
                let guard = self.inner.lock().await;
                guard.created_once
            };
            if created {
                bail!("MoQ publisher disconnected; reconnection is disabled");
            }

            let mut publisher = MoqtPublisher::connect(url)
                .await
                .context("connect moqt publisher")?;
            publisher
                .setup(vec![0xff00000a], 100)
                .await
                .context("moqt setup")?;

            let mut guard = self.inner.lock().await;
            if guard.publisher.is_none() {
                guard.publisher = Some(publisher);
                guard.created_once = true;
            }
        }

        Ok(())
    }
}
