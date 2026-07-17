use anyhow::Result;
use bytes::Bytes;
use moqt::{QUIC, StreamDataReceiver, StreamDataReceiverFactory, Subgroup, SubgroupObject};

/// Reads object payloads from one subscribed track, hiding group boundaries and
/// header/status objects. The only place that touches moqt's object-reading API.
pub struct TrackReader {
    factory: StreamDataReceiverFactory<QUIC>,
    group: Option<StreamDataReceiver<QUIC>>,
}

impl TrackReader {
    pub fn new(factory: StreamDataReceiverFactory<QUIC>) -> Self {
        Self {
            factory,
            group: None,
        }
    }

    /// The next object payload, or `None` when the track ends.
    pub async fn next_object(&mut self) -> Result<Option<Bytes>> {
        loop {
            if self.group.is_none() {
                match self.factory.next().await {
                    Ok(group) => self.group = Some(group),
                    Err(e) => {
                        tracing::debug!("track ended: {e}");
                        return Ok(None);
                    }
                }
            }
            let group = self.group.as_mut().expect("group opened above");
            match group.receive().await {
                Ok(Some(Subgroup::Object(field))) => match field.subgroup_object {
                    SubgroupObject::Payload { data, .. } => return Ok(Some(data)),
                    SubgroupObject::Status { .. } => continue,
                },
                Ok(Some(Subgroup::Header(_))) => continue,
                Ok(None) => {
                    // group stream finished (FIN); advance to the next group
                    self.group = None;
                    continue;
                }
                Err(e) => {
                    tracing::warn!("group read error, advancing to next group: {e}");
                    self.group = None;
                    continue;
                }
            }
        }
    }
}
