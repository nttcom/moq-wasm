use std::collections::HashMap;
use std::sync::Arc;

use anyhow::{Context, Result, bail};
use moqt_client_rust::publisher::MoqtPublisher;
use moqt_core::messages::data_streams::object_status::ObjectStatus;
use tokio::sync::Mutex;
use wtransport::stream::SendStream;

#[derive(Clone)]
pub struct MoqtManager {
    url: Option<String>,
    inner: Arc<Mutex<ManagerState>>,
}

#[derive(Default)]
struct ManagerState {
    publisher: Option<MoqtPublisher>,
    tracks: HashMap<(String, String), TrackInfo>,
    created_once: bool,
    announced_namespaces: std::collections::HashSet<String>,
}

#[derive(Default)]
struct TrackInfo {
    subscriber_alias: Option<u64>,
    announced: bool,
    header_sent: bool,
    subscribed: bool,
    stream: Option<SendStream>,
    object_id: u64,
    group_id: u64,
}

impl MoqtManager {
    pub fn new(url: Option<String>) -> Self {
        Self {
            url,
            inner: Arc::new(Mutex::new(ManagerState::default())),
        }
    }

    /// Announce namespace (once) and prepare tracks (video/audio) by waiting for SubscribeOk.
    pub async fn setup_namespace(&self, namespace: &[String]) -> Result<()> {
        let url = match &self.url {
            Some(u) => u.clone(),
            None => return Ok(()), // MoQ 出力なし
        };

        self.ensure_publisher(&url).await?;

        // Publisher をロック外で扱うため一時的に取り出す
        let (mut publisher, announce_needed) = {
            let mut guard = self.inner.lock().await;
            let publisher = guard
                .publisher
                .take()
                .ok_or_else(|| anyhow::anyhow!("publisher not initialized"))?;
            let ns = namespace.join("/");
            let announce_needed = !guard.announced_namespaces.contains(&ns);
            (publisher, announce_needed)
        };

        if announce_needed {
            publisher
                .announce(namespace, "auth".to_string())
                .await
                .context("announce")?;
        }

        // track (video, audio) を順不同で SubscribeOk 応答し、uni stream を保持
        let pending: std::collections::HashSet<String> =
            ["video", "audio"].iter().map(|s| s.to_string()).collect();
        let acquired = publisher
            .wait_subscribes_and_accept(namespace, pending)
            .await
            .context("wait subscribes and accept")?;
        for (track, subscriber_alias) in acquired {
            let mut guard = self.inner.lock().await;
            let key = (namespace.join("/"), track.clone());
            let entry = guard.tracks.entry(key.clone()).or_default();
            entry.subscribed = true;
            entry.subscriber_alias = Some(subscriber_alias);
            // uni stream はここでは開かない
        }

        let mut guard = self.inner.lock().await;
        if announce_needed {
            guard.announced_namespaces.insert(namespace.join("/"));
        }
        guard.publisher = Some(publisher);
        Ok(())
    }

    pub async fn send_object(
        &self,
        namespace: &[String],
        track_name: &str,
        is_keyframe: bool,
        payload: &[u8],
    ) -> Result<()> {
        let url = match &self.url {
            Some(u) => u.clone(),
            None => return Ok(()),
        };

        self.ensure_publisher(&url).await?;

        let (publisher, subscriber_alias, key, mut object_id, mut prev_stream, mut group_id) = {
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
            if !entry.subscribed {
                return Err(anyhow::anyhow!("subscribe_ok not completed yet"));
            }
            let subscriber_alias = entry.subscriber_alias.ok_or_else(|| {
                anyhow::anyhow!("subscriber alias not ready (subscribe_ok pending)")
            })?;
            let object_id = entry.object_id;
            let prev_stream = if is_keyframe {
                entry.stream.take()
            } else {
                None
            };
            let group_id = entry.group_id;
            (publisher, subscriber_alias, key, object_id, prev_stream, group_id)
        };

        let mut guard = self.inner.lock().await;
        let entry = guard
            .tracks
            .get_mut(&key)
            .ok_or_else(|| anyhow::anyhow!("track not set up"))?;
        // 直前の GoP を閉じる
        if let Some(mut s) = prev_stream {
            // 前のストリームで EndOfGroup を送信
            publisher
                .write_subgroup_object(
                    &mut s,
                    subscriber_alias,
                    group_id,
                    0,
                    object_id,
                    Some(ObjectStatus::EndOfGroup),
                    &[],
                )
                .await
                .context("send end-of-group")?;
            object_id = object_id.saturating_add(1);
            if let Err(e) = s.finish().await {
                eprintln!("[moqt] finish previous uni stream failed: {e:?}");
            }
            // 次の GoP: group_id を進め、object_id をリセット
            group_id = group_id.saturating_add(1);
            object_id = 0;
        }

        if is_keyframe {
            entry.header_sent = false;
        }

        if entry.stream.is_none() {
            entry.stream = Some(
                publisher
                    .open_data_uni()
                    .await
                    .context("open data uni stream")?,
            );
            entry.header_sent = false;
        }
        let stream = entry
            .stream
            .as_mut()
            .ok_or_else(|| anyhow::anyhow!("data uni stream not initialized"))?;

        // Header 未送信なら同じストリームに送る
        if !entry.header_sent {
            publisher
                .write_subgroup_header(stream, subscriber_alias, group_id, 0, 0)
                .await
                .context("send subgroup header")?;
            println!(
                "[moqt] subgroup header sent namespace={:?} track={} alias={} group_id={}",
                namespace, track_name, subscriber_alias, group_id
            );
            entry.header_sent = true;
        }

        publisher
            .write_subgroup_object(stream, subscriber_alias, group_id, 0, object_id, None, payload)
            .await
            .context("send subgroup object")?;
        println!(
            "[moqt] subgroup object sent namespace={:?} track={} alias={} group_id={} object_id={}",
            namespace, track_name, subscriber_alias, group_id, object_id
        );

        object_id = object_id.saturating_add(1);
        entry.object_id = object_id;
        entry.group_id = group_id;
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
