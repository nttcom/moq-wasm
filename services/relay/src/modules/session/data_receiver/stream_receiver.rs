use moqt::object::SubgroupPart;

#[async_trait::async_trait]
pub(crate) trait StreamReceiver: Send + Sync + 'static {
    /// `Ok(None)` means the stream finished normally (FIN); errors keep the
    /// transport-closed / decode-failed distinction from `StreamReceiveError`.
    async fn receive_object(
        &mut self,
    ) -> Result<Option<SubgroupPart>, moqt::object::StreamReceiveError>;
}

#[async_trait::async_trait]
impl StreamReceiver for moqt::object::SubgroupPartReceiver {
    async fn receive_object(
        &mut self,
    ) -> Result<Option<SubgroupPart>, moqt::object::StreamReceiveError> {
        let part = self.receive().await?;
        if let Some(SubgroupPart::Header(start)) = &part {
            tracing::debug!(subgroup_header = ?start, "Received subgroup header");
        }
        Ok(part)
    }
}
