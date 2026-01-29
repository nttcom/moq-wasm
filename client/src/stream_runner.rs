use std::pin::Pin;

pub(crate) struct StreamTaskRunner {
    sender: tokio::sync::mpsc::Sender<Pin<Box<dyn Future<Output = ()> + Send + 'static>>>,
    join_handle: tokio::task::JoinHandle<()>,
}

impl StreamTaskRunner {
    pub(crate) fn new() -> Self {
        let (sender, receiver) = tokio::sync::mpsc::channel(10);
        let join_handle = Self::run(receiver);
        Self {
            sender,
            join_handle,
        }
    }

    fn run(
        mut receiver: tokio::sync::mpsc::Receiver<
            Pin<Box<dyn Future<Output = ()> + Send + 'static>>,
        >,
    ) -> tokio::task::JoinHandle<()> {
        tokio::task::Builder::new()
            .spawn(async move {
                let mut join_set = tokio::task::JoinSet::new();
                loop {
                    tokio::select! {
                        Some(task) = receiver.recv() => {
                            tracing::info!("receive task");
                            join_set.spawn(task);
                        }
                        Some(_) = join_set.join_next() => {
                            tracing::info!("Task finished");
                        }
                    }
                }
            })
            .unwrap()
    }

    pub(crate) async fn add_task(&self, task: Pin<Box<dyn Future<Output = ()> + Send + 'static>>) {
        tracing::info!("add task");
        let _ = self.sender.send(task).await;
    }
}

impl Drop for StreamTaskRunner {
    fn drop(&mut self) {
        tracing::info!("Runner dropped.");
        self.join_handle.abort();
    }
}
