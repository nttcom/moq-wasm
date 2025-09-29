mod modules;

pub struct ServerSDK {
    protocol_handler: tokio::task::JoinHandle<()>,
    manager: tokio::task::JoinHandle<()>,
    repository: tokio::task::JoinHandle<()>,
}

impl ServerSDK {
    pub fn run() {
        let (manager_sender, manager_receiver) = tokio::sync::mpsc::unbounded_channel::<SessionEvent>();
        
    }
}
