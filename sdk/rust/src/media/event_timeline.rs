use bytes::Bytes;
use moqt::{client::TrackWriter, object::ExtensionHeaders};
use serde_json::Value;

/// Writes a draft-ietf-moq-msf-02 §8 event timeline as one group: it opens
/// with an empty record list, and each record is one more object, so a late
/// joiner's joining FETCH brings the records the track still keeps. A record
/// is the §8 JSON object, such as `{"T": milliseconds, "data": …}` or
/// `{"L": [group, object], "data": …}`.
pub struct EventTimeline {
    writer: TrackWriter,
}

impl EventTimeline {
    pub(super) async fn open(writer: TrackWriter) -> anyhow::Result<Self> {
        writer
            .write(encode(&[])?, true, ExtensionHeaders::default())
            .await?;
        Ok(Self { writer })
    }

    pub async fn append(&self, record: &Value) -> anyhow::Result<()> {
        self.writer
            .write(encode(&[record])?, false, ExtensionHeaders::default())
            .await?;
        Ok(())
    }
}

fn encode(records: &[&Value]) -> anyhow::Result<Bytes> {
    Ok(Bytes::from(serde_json::to_vec(records)?))
}
