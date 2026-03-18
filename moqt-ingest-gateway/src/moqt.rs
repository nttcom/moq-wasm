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

        // Publisher をロック外で扱うため一時的に取り出すが、必ず戻す
        let mut publisher = {
            let mut guard = self.inner.lock().await;
            guard
                .publisher
                .take()
                .ok_or_else(|| anyhow::anyhow!("publisher not initialized"))?
        };

        let result: Result<()> = async {
            let announce_needed = {
                let guard = self.inner.lock().await;
                !guard.announced_namespaces.contains(&namespace.join("/"))
            };

            if announce_needed {
                publisher
                    .announce(namespace, "auth".to_string())
                    .await
                    .context("announce")?;
            }

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
            }

            if announce_needed {
                let mut guard = self.inner.lock().await;
                guard.announced_namespaces.insert(namespace.join("/"));
            }
            Ok(())
        }
        .await;

        let mut guard = self.inner.lock().await;
        guard.publisher = Some(publisher);
        result
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

        // Publisher を取り出す（必ず後で戻す）
        let publisher = {
            let mut guard = self.inner.lock().await;
            guard
                .publisher
                .take()
                .ok_or_else(|| anyhow::anyhow!("publisher not initialized"))?
        };

        // トラックの現在状態を取得してロックを外す
        let (key, subscriber_alias, object_id, group_id, stream, header_sent) = {
            let mut guard = self.inner.lock().await;
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
            let stream = entry.stream.take();
            let header_sent = entry.header_sent;
            (
                key,
                subscriber_alias,
                entry.object_id,
                entry.group_id,
                stream,
                header_sent,
            )
        };

        // 送信処理本体
        let mut object_id_local = object_id;
        let mut group_id_local = group_id;
        let mut stream_local = stream;
        let mut header_sent_local = header_sent;

        let send_result: Result<()> = async {
            // 直前の GoP を閉じる (keyframe の場合)
            if is_keyframe {
                if let Some(mut s) = stream_local.take() {
                    publisher
                        .write_subgroup_object(
                            &mut s,
                            subscriber_alias,
                            group_id_local,
                            0,
                            object_id_local,
                            Vec::new(),
                            Some(ObjectStatus::EndOfGroup),
                            &[],
                        )
                        .await
                        .context("send end-of-group")?;
                    object_id_local = object_id_local.saturating_add(1);
                    if let Err(e) = s.finish().await {
                        eprintln!("[moqt] finish previous uni stream failed: {e:?}");
                    }
                    group_id_local = group_id_local.saturating_add(1);
                    object_id_local = 0;
                }
                header_sent_local = false;
            }

            if stream_local.is_none() {
                stream_local = Some(
                    publisher
                        .open_data_uni()
                        .await
                        .context("open data uni stream")?,
                );
                header_sent_local = false;
            }
            let stream_ref = stream_local
                .as_mut()
                .ok_or_else(|| anyhow::anyhow!("data uni stream not initialized"))?;

            if !header_sent_local {
                publisher
                    .write_subgroup_header(stream_ref, subscriber_alias, group_id_local, 0, 0)
                    .await
                    .context("send subgroup header")?;
                println!(
                    "[moqt] subgroup header sent namespace={:?} track={} alias={} group_id={}",
                    namespace, track_name, subscriber_alias, group_id_local
                );
                header_sent_local = true;
            }

            publisher
                .write_subgroup_object(
                    stream_ref,
                    subscriber_alias,
                    group_id_local,
                    0,
                    object_id_local,
                    Vec::new(),
                    None,
                    payload,
                )
                .await
                .context("send subgroup object")?;
            println!(
                "[moqt] subgroup object sent namespace={:?} track={} alias={} group_id={} object_id={}",
                namespace, track_name, subscriber_alias, group_id_local, object_id_local
            );

            object_id_local = object_id_local.saturating_add(1);
            Ok(())
        }
        .await;

        // 戻す（成功時だけ状態更新）
        let mut guard = self.inner.lock().await;
        let result = match send_result {
            Ok(()) => {
                if let Some(entry) = guard.tracks.get_mut(&key) {
                    entry.object_id = object_id_local;
                    entry.group_id = group_id_local;
                    entry.stream = stream_local;
                    entry.header_sent = header_sent_local;
                }
                Ok(())
            }
            Err(err) => {
                if let Some(entry) = guard.tracks.get_mut(&key) {
                    entry.stream = stream_local;
                    entry.header_sent = header_sent_local;
                }
                Err(err)
            }
        };
        guard.publisher = Some(publisher);
        result
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
