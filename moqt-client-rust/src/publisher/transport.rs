use anyhow::{anyhow, Context, Result};
use bytes::Buf;
use bytes::BytesMut;
use moqt_core::{
    control_message_type::ControlMessageType,
    data_stream_type::DataStreamType,
    messages::{
        control_messages::{
            announce::Announce,
            announce_error::AnnounceError,
            announce_ok::AnnounceOk,
            client_setup::ClientSetup,
            group_order::GroupOrder,
            setup_parameters::{MaxSubscribeID, SetupParameter},
            subscribe::Subscribe,
            subscribe_ok::SubscribeOk,
            version_specific_parameters::{AuthorizationInfo, VersionSpecificParameter},
        },
        data_streams::DataStreams,
        data_streams::{
            datagram, extension_header::ExtensionHeader, object_status::ObjectStatus,
            subgroup_stream,
        },
        moqt_payload::MOQTPayload,
    },
    variable_integer::{read_variable_integer, write_variable_integer},
};
use std::collections::HashSet;
use tokio::io::AsyncWriteExt;
use url::Url;
use wtransport::{
    stream::{RecvStream, SendStream},
    ClientConfig, Connection, Endpoint,
};

use super::state::{PublisherState, TrackKey};

use std::sync::atomic::{AtomicU64, Ordering};

pub struct MoqtPublisher {
    connection: Connection,
    control_send: SendStream,
    control_recv: RecvStream,
    control_buf: BytesMut,
    pub(crate) state: PublisherState,
    data_uni_counter: AtomicU64,
}

impl MoqtPublisher {
    pub async fn connect(url: &str) -> Result<Self> {
        let url = Url::parse(url).context("parse moqt url")?;

        let endpoint = Endpoint::client(ClientConfig::default())?;
        let connection = endpoint
            .connect(url.as_str())
            .await
            .context("connect webtransport")?;

        let (control_send, control_recv) = connection
            .open_bi()
            .await
            .context("open control bidirectional stream")?
            .await
            .context("wait control stream")?;

        println!("MoQ connected: control stream (bi) established to {url}");
        Ok(Self {
            connection,
            control_send,
            control_recv,
            control_buf: BytesMut::new(),
            state: PublisherState::default(),
            data_uni_counter: AtomicU64::new(0),
        })
    }

    pub async fn open_data_uni(&self) -> Result<SendStream> {
        let stream = self
            .connection
            .open_uni()
            .await
            .context("open data uni stream")?
            .await
            .context("wait data uni stream")?;
        let id = self.data_uni_counter.fetch_add(1, Ordering::Relaxed);
        println!("[moqt-publisher] data uni stream opened (local_id={id})");
        Ok(stream)
    }

    pub async fn write_subgroup_header(
        &self,
        stream: &mut SendStream,
        track_alias: u64,
        group_id: u64,
        subgroup_id: u64,
        publisher_priority: u8,
    ) -> Result<()> {
        let header =
            subgroup_stream::Header::new(track_alias, group_id, subgroup_id, publisher_priority)?;
        let mut payload = BytesMut::new();
        header.packetize(&mut payload);

        let mut buf = Vec::with_capacity(1 + payload.len());
        buf.extend(write_variable_integer(
            u8::from(DataStreamType::SubgroupHeader) as u64,
        ));
        buf.extend_from_slice(&payload);

        stream.write_all(&buf).await?;
        stream.flush().await?;
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn write_subgroup_object(
        &self,
        stream: &mut SendStream,
        track_alias: u64,
        _group_id: u64,
        _subgroup_id: u64,
        object_id: u64,
        object_status: Option<ObjectStatus>,
        payload: &[u8],
    ) -> Result<()> {
        if !self.state.subscribed_aliases.contains(&track_alias) {
            return Err(anyhow!(
                "subscribe_ok not sent yet for alias={}",
                track_alias
            ));
        }

        let object =
            subgroup_stream::Object::new(object_id, vec![], object_status, payload.to_vec())?;
        let mut buf = BytesMut::new();
        object.packetize(&mut buf);

        stream.write_all(&buf).await?;
        stream.flush().await?;
        Ok(())
    }

    pub async fn setup(&mut self, versions: Vec<u32>, max_subscribe_id: u64) -> Result<()> {
        let params = vec![SetupParameter::MaxSubscribeID(MaxSubscribeID::new(
            max_subscribe_id,
        ))];
        let msg = ClientSetup::new(versions, params);
        let mut payload = BytesMut::new();
        msg.packetize(&mut payload);
        self.write_control(ControlMessageType::ClientSetup, &payload)
            .await?;

        loop {
            let (msg_type, mut payload) = self.read_control_message().await?;
            if msg_type == ControlMessageType::ServerSetup {
                moqt_core::messages::control_messages::server_setup::ServerSetup::depacketize(
                    &mut payload,
                )
                .context("depacketize server_setup")?;
                println!("MoQ setup complete (server_setup received)");
                break;
            }
        }
        Ok(())
    }

    pub async fn announce(&mut self, namespace: &[String], auth_info: String) -> Result<()> {
        let msg = Announce::new(
            namespace.to_vec(),
            vec![VersionSpecificParameter::AuthorizationInfo(
                AuthorizationInfo::new(auth_info),
            )],
        );
        let mut payload = BytesMut::new();
        msg.packetize(&mut payload);
        self.write_control(ControlMessageType::Announce, &payload)
            .await?;

        loop {
            let (msg_type, mut payload) = self.read_control_message().await?;
            match msg_type {
                ControlMessageType::AnnounceOk => {
                    AnnounceOk::depacketize(&mut payload).context("depacketize announce_ok")?;
                    println!("MoQ announce ok for namespace={:?}", namespace);
                    break;
                }
                ControlMessageType::AnnounceError => {
                    let err = AnnounceError::depacketize(&mut payload)
                        .context("depacketize announce_error")?;
                    return Err(anyhow!("announce rejected: {:?}", err));
                }
                _ => continue,
            }
        }
        Ok(())
    }

    /// namespace 配下で期待する track すべての Subscribe を受信し、SubscribeOk を返す。
    /// data uni stream は開かず、(track_name, subscriber_alias) を返す。
    pub async fn wait_subscribes_and_accept(
        &mut self,
        namespace: &[String],
        mut expected_tracks: HashSet<String>,
    ) -> Result<Vec<(String, u64)>> {
        let mut acquired = Vec::new();
        while !expected_tracks.is_empty() {
            let (msg_type, mut payload) = self.read_control_message().await?;
            match msg_type {
                ControlMessageType::Subscribe => {
                    let sub: Subscribe = Subscribe::depacketize(&mut payload)?;
                    print!(
                        "MoQ subscribe received alias={} ns={:?} track={}",
                        sub.track_alias(),
                        sub.track_namespace(),
                        sub.track_name()
                    );
                    let ns = sub.track_namespace();
                    let track = sub.track_name();

                    if ns == namespace && expected_tracks.remove(track) {
                        let ok = SubscribeOk::new(
                            sub.subscribe_id(),
                            sub.track_alias(),
                            GroupOrder::Ascending,
                            false,
                            None,
                            None,
                            vec![],
                        );
                        let mut buf = BytesMut::new();
                        ok.packetize(&mut buf);
                        self.write_control(ControlMessageType::SubscribeOk, &buf)
                            .await?;
                        self.state.subscribed_aliases.insert(sub.track_alias());

                        let key = TrackKey::new(namespace.join("/"), track);
                        let entry = self.state.tracks.entry(key).or_default();
                        entry.alias = sub.track_alias();
                        entry.subscriber_alias = Some(sub.track_alias());

                        println!(
                            "MoQ subscribe ok sent alias={} ns={:?} track={}",
                            sub.track_alias(),
                            sub.track_namespace(),
                            sub.track_name()
                        );
                        acquired.push((track.to_string(), sub.track_alias()));
                    } else {
                        println!(
                            "MoQ subscribe ignored ns={:?} track={}",
                            sub.track_namespace(),
                            sub.track_name()
                        );
                    }
                }
                ControlMessageType::SubscribeError => {
                    return Err(anyhow!("received SubscribeError"));
                }
                ControlMessageType::AnnounceOk => {
                    println!("MoQ AnnounceOk received");
                }
                ControlMessageType::AnnounceError => {
                    let _ = AnnounceError::depacketize(&mut payload)?;
                    return Err(anyhow!("announce rejected"));
                }
                ControlMessageType::ServerSetup => {
                    let _ = moqt_core::messages::control_messages::server_setup::ServerSetup::depacketize(&mut payload)?;
                }
                _ => {
                    // それ以外は無視
                }
            }
        }

        Ok(acquired)
    }

    pub async fn send_subgroup_header(
        &mut self,
        track_alias: u64,
        group_id: u64,
        subgroup_id: u64,
        publisher_priority: u8,
    ) -> Result<()> {
        let header =
            subgroup_stream::Header::new(track_alias, group_id, subgroup_id, publisher_priority)?;
        let mut payload = BytesMut::new();
        header.packetize(&mut payload);

        let mut buf = Vec::with_capacity(1 + payload.len());
        buf.extend(write_variable_integer(
            u8::from(DataStreamType::SubgroupHeader) as u64,
        ));
        buf.extend_from_slice(&payload);

        let mut stream = self
            .connection
            .open_uni()
            .await
            .context("open subgroup header uni stream")?
            .await
            .context("wait subgroup header uni stream")?;
        stream.write_all(&buf).await?;
        stream.flush().await?;
        Ok(())
    }

    pub async fn send_subgroup_object(
        &mut self,
        track_alias: u64,
        _group_id: u64,
        _subgroup_id: u64,
        object_id: u64,
        object_status: Option<ObjectStatus>,
        payload: &[u8],
    ) -> Result<()> {
        if !self.state.subscribed_aliases.contains(&track_alias) {
            return Err(anyhow!(
                "subscribe_ok not sent yet for alias={}",
                track_alias
            ));
        }

        let object =
            subgroup_stream::Object::new(object_id, vec![], object_status, payload.to_vec())?;
        let mut buf = BytesMut::new();
        object.packetize(&mut buf);

        let mut stream = self
            .connection
            .open_uni()
            .await
            .context("open subgroup object uni stream")?
            .await
            .context("wait subgroup object uni stream")?;
        stream.write_all(&buf).await?;
        stream.flush().await?;
        Ok(())
    }

    pub async fn send_datagram_object(
        &self,
        track_alias: u64,
        group_id: u64,
        object_id: u64,
        publisher_priority: u8,
        payload: &[u8],
    ) -> Result<()> {
        let datagram_obj = datagram::Object::new(
            track_alias,
            group_id,
            object_id,
            publisher_priority,
            Vec::<ExtensionHeader>::new(),
            payload.to_vec(),
        )?;
        let mut buf = BytesMut::new();
        datagram_obj.packetize(&mut buf);

        let mut packet = Vec::with_capacity(1 + buf.len());
        packet.extend(write_variable_integer(
            u8::from(DataStreamType::ObjectDatagram) as u64,
        ));
        packet.extend_from_slice(&buf);

        self.connection
            .send_datagram(packet)
            .context("send datagram")?;
        Ok(())
    }

    async fn write_control(&mut self, msg_type: ControlMessageType, payload: &[u8]) -> Result<()> {
        let mut buf = Vec::with_capacity(8 + payload.len());
        buf.extend(write_variable_integer(u8::from(msg_type) as u64));
        buf.extend(write_variable_integer(payload.len() as u64));
        buf.extend_from_slice(payload);
        self.control_send.write_all(&buf).await?;
        self.control_send.flush().await?;
        Ok(())
    }

    async fn read_control_message(&mut self) -> Result<(ControlMessageType, BytesMut)> {
        loop {
            if let Some(parsed) = try_parse_control(&mut self.control_buf)? {
                return Ok(parsed);
            }
            let mut tmp = [0u8; 2048];
            let n = self.control_recv.read(&mut tmp).await?;
            match n {
                Some(0) => {
                    println!("[moqt-publisher] control stream closed by peer");
                    return Err(anyhow!("control stream closed"));
                }
                Some(n) => self.control_buf.extend_from_slice(&tmp[..n]),
                None => continue,
            }
        }
    }

    pub async fn close(mut self) -> Result<()> {
        println!("[moqt-publisher] closing control stream and connection");
        let _ = self.control_send.reset(0u8.into());
        self.connection.close(0u32.into(), b"done");
        Ok(())
    }
}

impl Drop for MoqtPublisher {
    fn drop(&mut self) {
        println!("[moqt-publisher] dropped without explicit close (streams will be reset)");
    }
}

fn try_parse_control(buf: &mut BytesMut) -> Result<Option<(ControlMessageType, BytesMut)>> {
    let mut cursor = std::io::Cursor::new(&buf[..]);
    let msg_type = match read_variable_integer(&mut cursor) {
        Ok(v) => v,
        Err(_) => return Ok(None),
    };
    let len = match read_variable_integer(&mut cursor) {
        Ok(v) => v,
        Err(_) => return Ok(None),
    };
    if cursor.remaining() < len as usize {
        return Ok(None);
    }

    let message_type = ControlMessageType::try_from(msg_type as u8)?;
    let consumed = cursor.position() as usize;
    buf.advance(consumed);
    let payload = buf.split_to(len as usize);
    Ok(Some((message_type, payload)))
}
