use crate::datagram_io::{recv_datagram, DatagramEvent};
use anyhow::{anyhow, Context, Result};
use bytes::{Buf, BytesMut};
use moqt_core::{
    control_message_type::ControlMessageType,
    messages::{
        control_messages::{
            client_setup::ClientSetup,
            group_order::GroupOrder,
            setup_parameters::{MaxSubscribeID, SetupParameter},
            subscribe::{FilterType, Subscribe},
            subscribe_error::SubscribeError,
            subscribe_ok::SubscribeOk,
            version_specific_parameters::{AuthorizationInfo, VersionSpecificParameter},
        },
        moqt_payload::MOQTPayload,
    },
    variable_integer::{read_variable_integer, write_variable_integer},
};
use std::io::Cursor;
use tokio::io::AsyncWriteExt;
use url::Url;
use wtransport::{
    stream::{RecvStream, SendStream},
    ClientConfig, Connection, Endpoint,
};

pub struct MoqtSubscriber {
    connection: Connection,
    control_send: SendStream,
    control_recv: RecvStream,
    control_buf: BytesMut,
}

impl MoqtSubscriber {
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
        })
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

    #[allow(clippy::too_many_arguments)]
    pub async fn subscribe(
        &mut self,
        subscribe_id: u64,
        track_alias: u64,
        namespace: Vec<String>,
        track_name: String,
        subscriber_priority: u8,
        filter_type: FilterType,
        start_group: Option<u64>,
        start_object: Option<u64>,
        end_group: Option<u64>,
        auth_info: String,
    ) -> Result<()> {
        let params = vec![VersionSpecificParameter::AuthorizationInfo(
            AuthorizationInfo::new(auth_info),
        )];
        let msg = Subscribe::new(
            subscribe_id,
            track_alias,
            namespace,
            track_name,
            subscriber_priority,
            GroupOrder::Ascending,
            filter_type,
            start_group,
            start_object,
            end_group,
            params,
        )?;
        let mut payload = BytesMut::new();
        msg.packetize(&mut payload);
        self.write_control(ControlMessageType::Subscribe, &payload)
            .await?;

        loop {
            let (msg_type, mut payload) = self.read_control_message().await?;
            match msg_type {
                ControlMessageType::SubscribeOk => {
                    let ok = SubscribeOk::depacketize(&mut payload)?;
                    if ok.subscribe_id() == subscribe_id {
                        println!("MoQ subscribe ok (alias={track_alias})");
                        return Ok(());
                    }
                }
                ControlMessageType::SubscribeError => {
                    let err = SubscribeError::depacketize(&mut payload)?;
                    return Err(anyhow!("subscribe rejected: {:?}", err));
                }
                _ => continue,
            }
        }
    }

    pub async fn recv_datagram(&self) -> Result<DatagramEvent> {
        recv_datagram(&self.connection).await
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
                    println!("[moqt-subscriber] control stream closed by peer");
                    return Err(anyhow!("control stream closed"));
                }
                Some(n) => self.control_buf.extend_from_slice(&tmp[..n]),
                None => continue,
            }
        }
    }
}

fn try_parse_control(buf: &mut BytesMut) -> Result<Option<(ControlMessageType, BytesMut)>> {
    let mut cursor = Cursor::new(&buf[..]);
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
