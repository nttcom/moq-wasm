use crate::modules::session_handlers::bi_stream::BiStreamTrait;

struct SessionHandler {
    control_stream: Box<dyn BiStreamTrait>,
}

impl SessionHandler {
    pub(crate) fn new(control_stream: Box<dyn BiStreamTrait>) -> Self {
        Self { control_stream }
    }
}
