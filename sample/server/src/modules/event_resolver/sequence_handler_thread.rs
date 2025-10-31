use std::pin::Pin;

pub(crate) struct SequenceHandlerThread {
    sender: tokio::sync::mpsc::Sender<Pin<Box<dyn Future<Output = ()> + Send + 'static>>>,
}

impl SequenceHandlerThread {
    fn new() -> Self {
        let (sender, _) = tokio::sync::mpsc::channel(10);
        Self { sender }
    }

    fn run(
        mut receiver: tokio::sync::mpsc::Receiver<
            Pin<Box<dyn Future<Output = ()> + Send + 'static>>,
        >,
    ) {
        let _ = tokio::task::Builder::new().spawn(async move {
            let mut join_set = tokio::task::JoinSet::new();
            loop {
                tokio::select! {
                    Some(task) = receiver.recv() => {
                        join_set.spawn(task);
                    }
                    _ = join_set.join_next() => {
                        tracing::info!("Task finished");
                    }
                }
            }
        });
    }

    fn add_task(task: Pin<Box<dyn Future<Output = ()> + Send + 'static>>) {}
}
