use super::server_processes::senders::Senders;
use std::sync::Arc;

#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum MOQTClientStatus {
    Connected,
    SetUp,
}

#[derive(Debug)]
pub struct MOQTClient {
    id: usize,
    status: MOQTClientStatus,
    senders: Arc<Senders>,
}

impl MOQTClient {
    pub fn new(id: usize, senders: Senders) -> Self {
        let senders = Arc::new(senders);
        MOQTClient {
            id,
            status: MOQTClientStatus::Connected,
            senders,
        }
    }
    pub fn id(&self) -> usize {
        self.id
    }
    pub fn status(&self) -> MOQTClientStatus {
        self.status
    }

    pub fn update_status(&mut self, new_status: MOQTClientStatus) {
        self.status = new_status;
    }

    pub fn senders(&self) -> Arc<Senders> {
        self.senders.clone()
    }
}
