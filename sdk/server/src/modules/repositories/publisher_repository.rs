use crate::modules::repositories::repository_trait::RepositoryTrait;
use crate::modules::publisher::Publisher;

struct PublisherRepository {
    publishers: tokio::sync::Mutex<Vec<Publisher>>,
}

impl RepositoryTrait for PublisherRepository {
    async fn add(&mut self, publisher: Publisher) {
        self.publishers.lock().await.push(publisher);
    }

    fn remove() {
        todo!()
    }
}