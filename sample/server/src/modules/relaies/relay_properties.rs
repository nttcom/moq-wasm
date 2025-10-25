use std::{collections::VecDeque, sync::Arc};

use dashmap::DashMap;
use moqt::DatagramObject;

pub struct RelayProperties {
    pub(crate) sender_map: Arc<DashMap<u64, tokio::sync::broadcast::Sender<DatagramObject>>>,
    pub(crate) object_queue: Arc<DashMap<u64, VecDeque<DatagramObject>>>,
    pub(crate) joinset: tokio::task::JoinSet<()>,
}

impl RelayProperties {
    fn new() -> Self {
        Self {
            sender_map: Arc::new(DashMap::new()),
            object_queue: Arc::new(DashMap::new()),
            joinset: tokio::task::JoinSet::new(),
        }
    }
}
