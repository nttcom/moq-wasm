use std::{collections::HashMap, sync::Arc};

use crate::modules::session_handlers::{
    moqt_bi_stream::MOQTBiStream,
    message_controller::MessageController,
    messages::message_process_result::MessageProcessResult::*
};

pub(crate) struct MessageJoinHandleManager {
    message_controller: Arc<MessageController>,
    join_handlers: std::sync::Mutex<HashMap<u64, tokio::task::JoinHandle<()>>>,
}

impl Drop for MessageJoinHandleManager {
    fn drop(&mut self) {
        self.join_handlers
            .lock()
            .unwrap()
            .iter()
            .for_each(|(_, j)| j.abort());
    }
}

impl MessageJoinHandleManager {
    pub fn new(message_controller: MessageController) -> Self {
        Self {
            message_controller: Arc::new(message_controller),
            join_handlers: std::sync::Mutex::new(HashMap::new()),
        }
    }

    pub fn start_receive(&self, stream: Box<dyn MOQTBiStream>) {
        let stream_id = stream.get_stream_id();
        let join_handle = self.create_join_handle(stream);
        self.join_handlers
            .lock()
            .unwrap()
            .insert(stream_id, join_handle);
    }

    fn create_join_handle(&self, mut stream: Box<dyn MOQTBiStream>) -> tokio::task::JoinHandle<()> {
        let message_controller = self.message_controller.clone();
        tokio::task::Builder::new()
            .name("Messsage Receiver")
            .spawn(async move {
                loop {
                    let buffer = stream.receive().await;
                    if let Err(e) = buffer {
                        tracing::error!("failed to receive message: {}", e.to_string());
                        break;
                    }
                    let message = message_controller.handle(buffer.unwrap());
                    if let Err(e) = message {
                        tracing::error!("failed to handle message: {}", e.to_string());
                        break;
                    }
                    match message.unwrap() {
                        Success(bytes_mut) => {
                            if let Err(e) = stream.send(&bytes_mut).await {
                                tracing::error!("sending Message has failed: {}", e.to_string());
                                // dispatch event
                            } else {
                                tracing::info!("sending Message has succeeded.");
                            }
                        },
                        SuccessWithoutResponse => continue,
                        Failure(termination_error_code, text) => {
                            tracing::error!("failed... code: {:?}, message: {}", termination_error_code, text);
                            break;
                        },
                        Fragment => {
                            tracing::warn!("fragment");
                        },
                    };
                }
            })
            .unwrap()
    }
}
