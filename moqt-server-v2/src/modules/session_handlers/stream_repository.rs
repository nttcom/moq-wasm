use std::sync::{Arc, Mutex};

use crate::modules::session_handlers::bi_stream::BiStreamTrait;

pub struct StreamRepository {
    streams: Mutex<Vec<Arc<Mutex<dyn BiStreamTrait>>>>,
}

impl StreamRepository {
    pub fn new() -> Self {
        Self {
            streams: Mutex::new(Vec::new()),
        }
    }

    pub fn add_stream(&self, stream: Arc<Mutex<dyn BiStreamTrait>>) -> bool {
        match self.streams.lock() {
            Ok(mut s) => {
                 s.push(stream);
                 true
            },
            Err(_) => false
        }
    }

    pub fn remove_stream(&self, stream: Arc<Mutex<dyn BiStreamTrait>>) {
    }
}
