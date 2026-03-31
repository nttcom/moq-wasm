struct TrackState {
    track_key: u128,
}

pub(crate) struct StateHandler {
    join_handle: tokio::task::JoinHandle<()>,
}

impl StateHandler {
    pub(crate) fn run(receiver: tokio::sync::mpsc::Receiver<TrackState>) -> Self {
        let join_handle = tokio::spawn(async move {
            let mut joinset = tokio::task::JoinSet::new();
            loop {
                tokio::select! {

                    Some(task) = joinset.join_next() => {
                        if let Err(e) = task {
                            tracing::error!("task failed: {:?}", e);
                        }
                    }
                }
            }
        });
        Self { join_handle }
    }
}
