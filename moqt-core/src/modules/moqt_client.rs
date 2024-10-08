use super::messages::setup_parameters::RoleCase;
use anyhow::{bail, Ok, Result};

#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum MOQTClientStatus {
    Connected,
    SetUp,
    Closed,
}

#[derive(Debug)]
pub struct MOQTClient {
    pub id: usize,
    pub status: MOQTClientStatus,
    pub role: Option<RoleCase>,
}

impl MOQTClient {
    pub fn new(id: usize) -> Self {
        MOQTClient {
            id,
            status: MOQTClientStatus::Connected,
            role: None,
        }
    }
    pub fn id(&self) -> usize {
        self.id
    }
    pub fn status(&self) -> MOQTClientStatus {
        self.status
    }
    pub fn role(&self) -> Option<RoleCase> {
        self.role
    }

    pub fn update_status(&mut self, new_status: MOQTClientStatus) {
        self.status = new_status;
    }
    pub fn set_role(&mut self, new_role: RoleCase) -> Result<()> {
        if self.role.is_some() {
            bail!("Client's role is already set.");
        }
        self.role = Some(new_role);

        Ok(())
    }
}
