use anyhow::{bail, Ok, Result};
use moqt_core::messages::control_messages::setup_parameters::RoleCase;

#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum MOQTClientStatus {
    Connected,
    SetUp,
}

#[derive(Debug)]
pub struct MOQTClient {
    id: usize,
    status: MOQTClientStatus,
    role: Option<RoleCase>,
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
