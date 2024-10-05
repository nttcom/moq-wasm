#[derive(Debug, Clone)]
pub enum ForwardingPreference {
    Datagram,
    Track,
    Subgroup,
}

#[derive(Debug, Clone)]
pub struct Track {
    track_alias: u64,
    track_namespace: Vec<String>,
    track_name: String,
    forwarding_preference: Option<ForwardingPreference>,
}

impl Track {
    pub fn new(
        track_alias: u64,
        track_namespace: Vec<String>,
        track_name: String,
        forwarding_preference: Option<ForwardingPreference>,
    ) -> Self {
        Self {
            track_alias,
            track_namespace,
            track_name,
            forwarding_preference,
        }
    }

    pub fn set_forwarding_preference(&mut self, forwarding_preference: ForwardingPreference) {
        self.forwarding_preference = Some(forwarding_preference);
    }

    pub fn get_track_namespace_and_name(&self) -> (Vec<String>, String) {
        (self.track_namespace.clone(), self.track_name.to_string())
    }

    pub fn get_track_alias(&self) -> u64 {
        self.track_alias
    }
}
