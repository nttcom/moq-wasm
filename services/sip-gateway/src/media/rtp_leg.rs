use std::net::IpAddr;

use anyhow::{Context, Result, bail};
use rustrtc::{
    AudioCapability, Keying, MediaCapabilities, MediaKind, PeerConnection, PeerConnectionEvent,
    RtcConfiguration, SdpType, SessionDescription, TransceiverDirection,
    media::{MediaSample, MediaStreamTrack},
    rtp::{RtpHeader, RtpPacket},
};

use crate::media::{
    dtmf::EventPacket,
    g711::{G711, SAMPLES_PER_PACKET},
};

/// The phone side of a call's media: one SDES-SRTP audio stream
/// (RTP/SAVP, RFC 4568) with the trunk, without ICE or DTLS.
pub(crate) struct RtpLeg {
    peer_connection: PeerConnection,
}

/// What the answer settled.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Negotiated {
    pub(crate) codec: G711,
    /// The payload type of RFC 4733 events, when the answer accepts them.
    pub(crate) telephone_event: Option<u8>,
}

impl RtpLeg {
    /// Binds the media socket and returns the SDP offer: PCMU, then PCMA,
    /// then `telephone-event`, with an SRTP key.
    pub(crate) async fn offer(media_address: Option<IpAddr>) -> Result<(Self, String)> {
        let peer_connection = PeerConnection::new(configuration(media_address));
        peer_connection.add_transceiver(MediaKind::Audio, TransceiverDirection::SendRecv);
        let offer = peer_connection
            .create_offer()
            .await
            .context("create SDP offer")?;
        peer_connection
            .set_local_description(offer.clone())
            .context("apply SDP offer")?;
        Ok((Self { peer_connection }, offer.to_sdp_string()))
    }

    /// Binds the media socket and answers the trunk's offer with the first
    /// G.711 format it lists. Fails when the offer is not SRTP keyed with
    /// SDES or lists no G.711 codec, which the caller refuses with 488.
    pub(crate) async fn answer_offer(
        offer: &str,
        media_address: Option<IpAddr>,
    ) -> Result<(Self, String, Negotiated)> {
        let description =
            SessionDescription::parse(SdpType::Offer, offer).context("parse SDP offer")?;
        negotiated_codec(&description)?;
        let peer_connection = PeerConnection::new(configuration(media_address));
        peer_connection
            .set_remote_description(description)
            .await
            .context("apply SDP offer")?;
        let answer = peer_connection
            .create_answer()
            .await
            .context("create SDP answer")?;
        peer_connection
            .set_local_description(answer.clone())
            .context("apply SDP answer")?;
        let negotiated = Negotiated {
            codec: negotiated_codec(&answer)?,
            telephone_event: telephone_event(&answer),
        };
        Ok((Self { peer_connection }, answer.to_sdp_string(), negotiated))
    }

    /// Applies the trunk's answer. Fails when the answer is not SRTP keyed
    /// with SDES or picks no G.711 codec.
    pub(crate) async fn answer(&self, answer: &str) -> Result<Negotiated> {
        let description =
            SessionDescription::parse(SdpType::Answer, answer).context("parse SDP answer")?;
        let negotiated = Negotiated {
            codec: negotiated_codec(&description)?,
            telephone_event: telephone_event(&description),
        };
        self.peer_connection
            .set_remote_description(description)
            .await
            .context("apply SDP answer")?;
        Ok(negotiated)
    }

    pub(crate) fn sender(&self, codec: G711) -> RtpSender {
        RtpSender {
            peer_connection: self.peer_connection.clone(),
            payload_type: codec.payload_type(),
            ssrc: random_u32(),
            sequence_number: random_u32() as u16,
            timestamp: random_u32(),
            event_timestamp: 0,
        }
    }

    /// The payloads of the trunk's audio packets in `codec`, until the
    /// connection closes. Packets of another payload type (telephone-event,
    /// comfort noise) are skipped.
    pub(crate) async fn receive(&self, codec: G711, mut on_payload: impl FnMut(&[u8])) {
        while let Some(event) = self.peer_connection.recv().await {
            let PeerConnectionEvent::Track(transceiver) = event else {
                continue;
            };
            let Some(receiver) = transceiver.receiver() else {
                continue;
            };
            let track = receiver.track();
            while let Ok(sample) = track.recv().await {
                if let MediaSample::Audio(frame) = sample
                    && frame.payload_type == Some(codec.payload_type())
                {
                    on_payload(&frame.data);
                }
            }
        }
    }

    pub(crate) fn close(&self) {
        self.peer_connection.close();
    }
}

/// Sends 20 ms G.711 packets on the leg, numbering and timestamping them.
pub(crate) struct RtpSender {
    peer_connection: PeerConnection,
    payload_type: u8,
    ssrc: u32,
    sequence_number: u16,
    timestamp: u32,
    /// The timestamp of the event being sent: its first packet's.
    event_timestamp: u32,
}

impl RtpSender {
    pub(crate) async fn send(&mut self, payload: Vec<u8>) -> Result<()> {
        let header = RtpHeader::new(
            self.payload_type,
            self.sequence_number,
            self.timestamp,
            self.ssrc,
        );
        self.send_packet(header, payload).await
    }

    /// Sends one RFC 4733 event packet in the place of a 20 ms audio packet,
    /// in the same stream. Every packet of an event carries its first packet's
    /// timestamp, while the stream's timestamp still advances 20 ms per packet,
    /// so the audio after the event resumes at the time that passed.
    pub(crate) async fn send_event(&mut self, payload_type: u8, packet: EventPacket) -> Result<()> {
        if packet.starts {
            self.event_timestamp = self.timestamp;
        }
        let mut header = RtpHeader::new(
            payload_type,
            self.sequence_number,
            self.event_timestamp,
            self.ssrc,
        );
        header.marker = packet.starts;
        self.send_packet(header, packet.payload.to_vec()).await
    }

    async fn send_packet(&mut self, header: RtpHeader, payload: Vec<u8>) -> Result<()> {
        self.sequence_number = self.sequence_number.wrapping_add(1);
        self.timestamp = self.timestamp.wrapping_add(SAMPLES_PER_PACKET as u32);
        self.peer_connection
            .send_raw_rtp(RtpPacket::new(header, payload))
            .await
            .context("send RTP")
    }
}

/// SDES-SRTP with PCMU, PCMA and `telephone-event`, without ICE or DTLS.
fn configuration(media_address: Option<IpAddr>) -> RtcConfiguration {
    let mut config = RtcConfiguration::sip();
    config.keying = Keying::Sdes;
    config.media_capabilities = Some(MediaCapabilities {
        audio: vec![
            AudioCapability::pcmu(),
            AudioCapability::pcma(),
            AudioCapability::telephone_event(),
        ],
        video: vec![],
        application: None,
        image: vec![],
    });
    config.external_ips = media_address.into_iter().collect();
    // UPnP stays off: `.cargo/audit.toml` ignores igd's h2 advisory for it.
    config.enable_upnp = false;
    config
}

/// The first G.711 format of the description's audio section: an answer
/// lists the formats it accepts from the offer, preferred first (RFC 3264
/// §6.1), and an offer the formats it can take, preferred first.
fn negotiated_codec(description: &SessionDescription) -> Result<G711> {
    let audio = description
        .media_sections
        .iter()
        .find(|section| section.kind == MediaKind::Audio && section.port != 0)
        .context("no audio is accepted")?;
    if audio.protocol != "RTP/SAVP" || audio.get_crypto_attributes().is_empty() {
        bail!("the audio is not SRTP keyed with SDES");
    }
    audio
        .formats
        .iter()
        .filter_map(|format| format.parse().ok())
        .find_map(G711::from_payload_type)
        .context("no G.711 codec is listed")
}

/// The payload type `a=rtpmap` gives `telephone-event/8000` in the audio
/// section.
fn telephone_event(description: &SessionDescription) -> Option<u8> {
    let audio = description
        .media_sections
        .iter()
        .find(|section| section.kind == MediaKind::Audio && section.port != 0)?;
    audio.attributes.iter().find_map(|attribute| {
        let (payload_type, encoding) = attribute
            .value
            .as_deref()
            .filter(|_| attribute.key == "rtpmap")?
            .split_once(' ')?;
        let payload_type: u8 = payload_type.parse().ok()?;
        (encoding.eq_ignore_ascii_case("telephone-event/8000")
            && audio
                .formats
                .iter()
                .any(|format| format == &payload_type.to_string()))
        .then_some(payload_type)
    })
}

fn random_u32() -> u32 {
    let mut bytes = [0u8; 4];
    aws_lc_rs::rand::fill(&mut bytes).expect("the system random number generator works");
    u32::from_be_bytes(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn answer(protocol: &str, formats: &str, crypto: bool) -> SessionDescription {
        let mut sdp = format!(
            "v=0\r\no=- 1 1 IN IP4 192.0.2.1\r\ns=-\r\nc=IN IP4 192.0.2.1\r\nt=0 0\r\n\
             m=audio 4000 {protocol} {formats}\r\na=rtpmap:0 PCMU/8000\r\n\
             a=rtpmap:8 PCMA/8000\r\na=rtpmap:101 telephone-event/8000\r\na=sendrecv\r\n"
        );
        if crypto {
            sdp += "a=crypto:1 AES_CM_128_HMAC_SHA1_80 inline:a976SJLwniPcMiUP27gdcLYYcPm0bHZcghV84DsK\r\n";
        }
        SessionDescription::parse(SdpType::Answer, &sdp).unwrap()
    }

    #[test]
    fn the_answer_s_first_g711_format_is_used() {
        // Act / Assert
        assert_eq!(
            negotiated_codec(&answer("RTP/SAVP", "101 8 0", true)).unwrap(),
            G711::Pcma
        );
        assert_eq!(
            negotiated_codec(&answer("RTP/SAVP", "0 101", true)).unwrap(),
            G711::Pcmu
        );
    }

    #[test]
    fn telephone_event_takes_the_payload_type_the_answer_lists() {
        // Act / Assert
        assert_eq!(
            telephone_event(&answer("RTP/SAVP", "0 101", true)),
            Some(101)
        );
        assert_eq!(telephone_event(&answer("RTP/SAVP", "0", true)), None);
    }

    #[test]
    fn an_answer_without_sdes_srtp_or_g711_is_refused() {
        // Act / Assert
        assert!(negotiated_codec(&answer("RTP/AVP", "0", false)).is_err());
        assert!(negotiated_codec(&answer("RTP/SAVP", "0", false)).is_err());
        assert!(negotiated_codec(&answer("RTP/SAVP", "101", true)).is_err());
    }

    #[tokio::test]
    async fn a_trunk_s_offer_is_answered_with_sdes_srtp_and_its_first_g711_format() {
        // Arrange
        let offer = answer("RTP/SAVP", "8 0 101", true).to_sdp_string();

        // Act
        let (leg, answer, negotiated) =
            RtpLeg::answer_offer(&offer, Some("192.0.2.7".parse().unwrap()))
                .await
                .unwrap();

        // Assert
        leg.close();
        assert_eq!(negotiated.codec, G711::Pcma);
        let audio = answer
            .lines()
            .find(|line| line.starts_with("m=audio"))
            .unwrap();
        assert!(audio.contains("RTP/SAVP 8"), "{answer}");
        assert!(answer.contains("a=crypto:1 AES_CM_128_HMAC_SHA1_80 inline:"));
        assert!(answer.contains("c=IN IP4 192.0.2.7"), "{answer}");
    }

    #[tokio::test]
    async fn an_offer_without_sdes_srtp_is_not_answered() {
        // Arrange
        let offer = answer("RTP/AVP", "0", false).to_sdp_string();

        // Act / Assert
        assert!(RtpLeg::answer_offer(&offer, None).await.is_err());
    }

    #[tokio::test]
    async fn the_offer_is_sdes_srtp_with_g711_and_telephone_event() {
        // Act
        let (leg, offer) = RtpLeg::offer(Some("192.0.2.7".parse().unwrap()))
            .await
            .unwrap();

        // Assert
        leg.close();
        let audio = offer
            .lines()
            .find(|line| line.starts_with("m=audio"))
            .unwrap();
        assert!(audio.ends_with("RTP/SAVP 0 8 101"), "{offer}");
        assert!(offer.contains("a=crypto:1 AES_CM_128_HMAC_SHA1_80 inline:"));
        assert!(offer.contains("c=IN IP4 192.0.2.7"), "{offer}");
        assert!(!offer.contains("a=fingerprint"));
    }
}
