use crate::modules::enums::MOQTEvent;

pub(crate) struct Manager {
    join_handle: tokio::task::JoinHandle<()>,
}

impl Manager {
    pub fn run(receiver: tokio::sync::mpsc::Receiver<(moqt::Publisher, moqt::Subscriber)>) {}

    fn create_session_event_watcher(
        mut receiver: tokio::sync::mpsc::Receiver<(moqt::Publisher, moqt::Subscriber)>,
    ) -> tokio::task::JoinHandle<()> {
        tokio::task::Builder::new()
            .name("Session Event Watcher")
            .spawn(async move {
                loop {
                    if let Some((publisher, subscriber)) = receiver.recv().await {

                    } else {
                        tracing::error!("Failed to receive session event");
                        break;
                    }
                }
            })
            .unwrap()
    }

    fn create_pub_sub_event_watcher(
        mut receiver: tokio::sync::mpsc::UnboundedReceiver<MOQTEvent>,
    ) -> tokio::task::JoinHandle<()> {
        tokio::task::Builder::new()
            .name("Session Event Watcher")
            .spawn(async move {
                loop {
                    if let Some(event) = receiver.recv().await {

                    } else {
                        tracing::error!("Failed to receive session event");
                        break;
                    }
                }
            })
            .unwrap()
    }

    fn solve_event() {
        
    }
}
