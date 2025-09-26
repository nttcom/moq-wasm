use async_trait::async_trait;

#[async_trait]
pub(crate) trait RepositoryTrait {
    async fn add(&mut self, publisher: moqt::Publisher);
    fn remove();
}