use crate::modules::core::{
    data_sender::DataSender,
    published_resource::PublishedResource,
    publisher::Publisher,
};

pub(crate) struct StreamAllocator<'a> {
    publisher: &'a dyn Publisher,
    published_resource: &'a PublishedResource,
}

impl<'a> StreamAllocator<'a> {
    pub(crate) fn new(
        publisher: &'a dyn Publisher,
        published_resource: &'a PublishedResource,
    ) -> Self {
        Self {
            publisher,
            published_resource,
        }
    }

    pub(crate) async fn create_stream(
        &self,
        subscriber_track_alias: u64,
    ) -> anyhow::Result<Box<dyn DataSender>> {
        self.publisher
            .new_stream(self.published_resource, subscriber_track_alias)
            .await
    }

    pub(crate) fn create_datagram(&self) -> Box<dyn DataSender> {
        self.publisher.new_datagram(self.published_resource)
    }
}