use std::marker::PhantomData;

use crate::{
    TransportProtocol,
    modules::moqt::data_plane::{
        object::{
            extension_headers::ExtensionHeaders,
            subgroup::{SubgroupHeader, SubgroupId, SubgroupObject, SubgroupObjectField},
        },
        stream::sender::StreamSender,
    },
};

/// Typestate: header not yet sent.
pub struct Uninitialized;
/// Typestate: header has been sent.
pub struct HeaderSent;

pub type SubgroupHeaderSender<T> = StreamDataSender<T, Uninitialized>;
pub type SubgroupObjectSender<T> = StreamDataSender<T, HeaderSent>;

/// Handles sending data on a subgroup stream.
///
/// In the `S = Uninitialized` state a header can be created and sent.
/// Calling `send_header` consumes the sender and transitions it to `S = HeaderSent`.
/// Objects can only be sent in the `HeaderSent` state, so the invariant that
/// the header is always sent first is enforced at compile time.
pub struct StreamDataSender<T: TransportProtocol, S = Uninitialized> {
    stream_sender: StreamSender<T>,
    track_alias: u64,
    subgroup_header: Option<SubgroupHeader>,
    _state: PhantomData<S>,
}

// ─── Uninitialized State ───────────────────────────────────────────────────────

impl<T: TransportProtocol> StreamDataSender<T, Uninitialized> {
    pub(crate) fn new(track_alias: u64, send_stream: T::SendStream) -> Self {
        let stream_sender = StreamSender::new(send_stream);
        Self {
            stream_sender,
            track_alias,
            subgroup_header: None,
            _state: PhantomData,
        }
    }

    /// Creates a subgroup header associated with this stream.
    pub fn create_header(
        &self,
        group_id: u64,
        subgroup_id: SubgroupId,
        publisher_priority: u8,
        end_of_group: bool,
        has_extensions: bool,
    ) -> SubgroupHeader {
        SubgroupHeader::new(
            self.track_alias,
            group_id,
            subgroup_id,
            publisher_priority,
            end_of_group,
            has_extensions,
        )
    }

    pub async fn send_header(
        self,
        header: SubgroupHeader,
    ) -> anyhow::Result<StreamDataSender<T, HeaderSent>> {
        if header.track_alias != self.track_alias {
            anyhow::bail!(
                "track_alias mismatch: expected {}, got {}",
                self.track_alias,
                header.track_alias
            );
        }
        tracing::debug!("Sending new subgroup header: {:?}", header);
        let encoded_header = header.encode();
        self.stream_sender.send(&encoded_header).await?;
        Ok(StreamDataSender {
            stream_sender: self.stream_sender,
            track_alias: self.track_alias,
            subgroup_header: Some(header),
            _state: PhantomData,
        })
    }

    pub async fn close(&mut self) -> anyhow::Result<()> {
        self.stream_sender.close().await
    }
}

// ─── HeaderSent State ──────────────────────────────────────────────────────────

impl<T: TransportProtocol> StreamDataSender<T, HeaderSent> {
    pub fn create_object_field(
        &self,
        object_id_delta: u64,
        extension_headers: ExtensionHeaders,
        subgroup_object: SubgroupObject,
    ) -> SubgroupObjectField {
        let header = self.subgroup_header.as_ref().unwrap();
        SubgroupObjectField {
            message_type: header.message_type,
            object_id_delta,
            extension_headers,
            subgroup_object,
        }
    }

    pub async fn send(&mut self, data: SubgroupObjectField) -> anyhow::Result<()> {
        tracing::debug!("Sending subgroup object");
        let bytes = data.encode();
        self.stream_sender.send(&bytes).await
    }

    pub async fn close(&mut self) -> anyhow::Result<()> {
        self.stream_sender.close().await
    }
}
