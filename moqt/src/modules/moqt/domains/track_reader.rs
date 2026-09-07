use bytes::Bytes;

use crate::{
    ExtensionHeaders, StreamDataReceiver, StreamDataReceiverFactory, Subgroup, SubgroupObject,
    TransportProtocol,
};

#[derive(Debug, Clone)]
pub struct TrackObject {
    pub group_id: u64,
    pub object_id: u64,
    pub extension_headers: ExtensionHeaders,
    pub payload: Bytes,
}

pub struct TrackReader<T: TransportProtocol> {
    factory: StreamDataReceiverFactory<T>,
    subgroup: Option<OpenSubgroup<T>>,
}

impl<T: TransportProtocol> TrackReader<T> {
    pub fn new(factory: StreamDataReceiverFactory<T>) -> Self {
        Self {
            factory,
            subgroup: None,
        }
    }

    /// Returns `Ok(None)` once the track has no more subgroup streams.
    /// A failure inside one subgroup is returned as `Err` and that subgroup is
    /// dropped, so the next call continues with the following subgroup.
    pub async fn next_object(&mut self) -> anyhow::Result<Option<TrackObject>> {
        loop {
            let subgroup = match self.subgroup.as_mut() {
                Some(subgroup) => subgroup,
                None => match self.factory.next().await {
                    Ok(receiver) => self.subgroup.insert(OpenSubgroup::open(receiver).await?),
                    Err(error) => {
                        tracing::debug!(%error, "track ended");
                        return Ok(None);
                    }
                },
            };
            match subgroup.next_object().await {
                Ok(Some(object)) => return Ok(Some(object)),
                Ok(None) => self.subgroup = None,
                Err(error) => {
                    self.subgroup = None;
                    return Err(error);
                }
            }
        }
    }
}

struct OpenSubgroup<T: TransportProtocol> {
    receiver: StreamDataReceiver<T>,
    group_id: u64,
    prev_object_id: Option<u64>,
}

impl<T: TransportProtocol> OpenSubgroup<T> {
    async fn open(mut receiver: StreamDataReceiver<T>) -> anyhow::Result<Self> {
        match receiver.receive().await? {
            Some(Subgroup::Header(header)) => Ok(Self {
                receiver,
                group_id: header.group_id,
                prev_object_id: None,
            }),
            other => anyhow::bail!("subgroup stream did not start with a header: {other:?}"),
        }
    }

    async fn next_object(&mut self) -> anyhow::Result<Option<TrackObject>> {
        loop {
            match self.receiver.receive().await? {
                Some(Subgroup::Object(field)) => {
                    let object_id = field.resolve_object_id(self.prev_object_id);
                    self.prev_object_id = Some(object_id);
                    match field.subgroup_object {
                        SubgroupObject::Payload { data, .. } => {
                            return Ok(Some(TrackObject {
                                group_id: self.group_id,
                                object_id,
                                extension_headers: field.extension_headers,
                                payload: data,
                            }));
                        }
                        SubgroupObject::Status { .. } => continue,
                    }
                }
                Some(Subgroup::Header(_)) => continue,
                None => return Ok(None),
            }
        }
    }
}
