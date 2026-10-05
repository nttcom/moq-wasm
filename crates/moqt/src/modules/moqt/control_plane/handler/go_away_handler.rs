use crate::modules::moqt::control_plane::control_messages::messages::go_away::GoAway;

#[derive(Clone, Debug)]
pub struct GoAwayHandler {
    new_session_uri: String,
}

impl GoAwayHandler {
    pub(crate) fn new(go_away: GoAway) -> Self {
        Self {
            new_session_uri: go_away.new_session_uri,
        }
    }

    pub fn new_session_uri(&self) -> &str {
        &self.new_session_uri
    }
}
