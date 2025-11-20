#[derive(Default)]
pub struct PublisherState {
    pub subscribed_aliases: std::collections::HashSet<u64>,
}

impl PublisherState {
    pub fn new() -> Self {
        Self::default()
    }
}
