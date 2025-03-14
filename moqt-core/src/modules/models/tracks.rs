use std::collections::HashMap;

type GroupId = u64;
type StreamId = u64;
type SubgroupId = u64;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ForwardingPreference {
    Datagram,
    Subgroup,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Track {
    track_alias: u64,
    track_namespace: Vec<String>,
    track_name: String,
    forwarding_preference: Option<ForwardingPreference>,
    group_subgroup_stream_map: HashMap<GroupId, HashMap<SubgroupId, StreamId>>,
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
            group_subgroup_stream_map: HashMap::new(),
        }
    }

    pub fn set_forwarding_preference(&mut self, forwarding_preference: ForwardingPreference) {
        self.forwarding_preference = Some(forwarding_preference);
    }

    pub fn get_forwarding_preference(&self) -> Option<ForwardingPreference> {
        self.forwarding_preference.clone()
    }

    pub fn get_track_namespace_and_name(&self) -> (Vec<String>, String) {
        (self.track_namespace.clone(), self.track_name.to_string())
    }

    pub fn get_track_alias(&self) -> u64 {
        self.track_alias
    }

    pub fn set_stream_id(&mut self, group_id: u64, subgroup_id: u64, stream_id: u64) {
        self.group_subgroup_stream_map
            .entry(group_id)
            .or_default()
            .insert(subgroup_id, stream_id);
    }

    pub fn get_all_group_ids(&self) -> Vec<u64> {
        self.group_subgroup_stream_map.keys().cloned().collect()
    }

    pub fn get_subgroup_ids_for_group(&self, group_id: u64) -> Vec<u64> {
        self.group_subgroup_stream_map
            .get(&group_id)
            .map(|subgroup_stream_map| subgroup_stream_map.keys().cloned().collect())
            .unwrap_or_default()
    }

    pub fn get_stream_id_for_subgroup(&self, group_id: u64, subgroup_id: u64) -> Option<u64> {
        self.group_subgroup_stream_map
            .get(&group_id)
            .and_then(|subgroup_stream_map| subgroup_stream_map.get(&subgroup_id))
            .cloned()
    }
}

#[cfg(test)]
mod succsess {
    use crate::models::tracks::{ForwardingPreference, Track};

    #[test]
    fn new() {
        let track_alias = 0;
        let track_namespace = Vec::from(["test".to_string(), "test".to_string()]);
        let track_name = "track_name".to_string();
        let forwarding_preference = Some(ForwardingPreference::Datagram);

        let track = Track::new(
            track_alias,
            track_namespace.clone(),
            track_name.clone(),
            forwarding_preference.clone(),
        );

        assert_eq!(track.track_alias, track_alias);
        assert_eq!(track.track_namespace, track_namespace);
        assert_eq!(track.track_name, track_name);
        assert_eq!(track.forwarding_preference, forwarding_preference);
    }

    #[test]
    fn set_forwarding_preference() {
        let track_alias = 0;
        let track_namespace = Vec::from(["test".to_string(), "test".to_string()]);
        let track_name = "track_name".to_string();
        let expected_forwarding_preference = Some(ForwardingPreference::Subgroup);

        let mut track = Track::new(
            track_alias,
            track_namespace.clone(),
            track_name.clone(),
            None,
        );
        track.set_forwarding_preference(ForwardingPreference::Subgroup);

        assert_eq!(track.track_alias, track_alias);
        assert_eq!(track.track_namespace, track_namespace);
        assert_eq!(track.track_name, track_name);
        assert_eq!(track.forwarding_preference, expected_forwarding_preference);
    }

    #[test]
    fn get_track_namespace_and_name() {
        let track_alias = 0;
        let track_namespace = Vec::from(["test".to_string(), "test".to_string()]);
        let track_name = "track_name".to_string();
        let forwarding_preference = Some(ForwardingPreference::Datagram);

        let track = Track::new(
            track_alias,
            track_namespace.clone(),
            track_name.clone(),
            forwarding_preference,
        );

        assert_eq!(
            track.get_track_namespace_and_name(),
            (track_namespace, track_name)
        );
    }

    #[test]
    fn get_track_alias() {
        let track_alias = 0;
        let track_namespace = Vec::from(["test".to_string(), "test".to_string()]);
        let track_name = "track_name".to_string();
        let forwarding_preference = Some(ForwardingPreference::Datagram);

        let track = Track::new(
            track_alias,
            track_namespace.clone(),
            track_name.clone(),
            forwarding_preference,
        );

        assert_eq!(track.get_track_alias(), track_alias);
    }

    #[test]
    fn set_and_get_forwarding_preference() {
        let track_alias = 0;
        let track_namespace = Vec::from(["test".to_string(), "test".to_string()]);
        let track_name = "track_name".to_string();
        let forwarding_preference = ForwardingPreference::Datagram;

        let mut track = Track::new(
            track_alias,
            track_namespace.clone(),
            track_name.clone(),
            None,
        );

        track.set_forwarding_preference(forwarding_preference.clone());

        let result_forwarding_preference = track.get_forwarding_preference().unwrap();

        assert_eq!(result_forwarding_preference, forwarding_preference);
    }
}
