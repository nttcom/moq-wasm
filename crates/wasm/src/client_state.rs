use std::collections::{BTreeSet, HashMap, HashSet};

use anyhow::{Result, anyhow};
use moqt::wire::{
    ContentExists, Fetch, FetchOk, FetchParams, GroupOrder, Location, Subscribe, TrackStatus,
    TrackStatusOk,
};

use crate::{
    incoming_fetch::{FetchRange, FetchTarget, IncomingFetchRequest, location_after},
    messages::SubgroupState,
    request_rejection::RequestRejection,
};

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct TrackKey {
    pub(crate) namespace: Vec<String>,
    pub(crate) name: String,
}

impl TrackKey {
    pub(crate) fn new(namespace: Vec<String>, name: String) -> Self {
        Self { namespace, name }
    }
}

#[derive(Debug, Clone)]
struct IncomingSubscribeRequest {
    track_key: TrackKey,
    track_alias: Option<u64>,
    largest_location: Option<Location>,
}

#[derive(Debug, Clone)]
struct IncomingTrackStatusRequest {
    track_key: TrackKey,
    group_order: GroupOrder,
}

/// Publisher-side bookkeeping the session layer does not know about: which
/// namespaces this client announced, which tracks it is sending on and how far
/// each has progressed. SUBSCRIBE_OK, TRACK_STATUS_OK and FETCH_OK are answered
/// from it.
#[derive(Debug, Default)]
pub(crate) struct ClientState {
    published_namespaces: HashSet<Vec<String>>,
    subscribed_namespace_prefixes: HashSet<Vec<String>>,
    publish_namespace_requests: HashMap<u64, Vec<String>>,
    subscribe_namespace_requests: HashMap<u64, Vec<String>>,
    incoming_subscriptions: HashMap<u64, IncomingSubscribeRequest>,
    publishing_track_aliases: HashMap<TrackKey, BTreeSet<u64>>,
    subgroup_states: HashMap<u64, SubgroupState>,
    largest_published_locations: HashMap<TrackKey, Location>,
    incoming_track_statuses: HashMap<u64, IncomingTrackStatusRequest>,
    incoming_fetches: HashMap<u64, IncomingFetchRequest>,
}

impl ClientState {
    pub(crate) fn contains_published_namespace(&self, namespace: &[String]) -> bool {
        self.published_namespaces.contains(namespace)
    }

    pub(crate) fn register_publish_namespace_request(
        &mut self,
        request_id: u64,
        namespace: Vec<String>,
    ) {
        self.published_namespaces.insert(namespace.clone());
        self.publish_namespace_requests
            .insert(request_id, namespace);
    }

    pub(crate) fn finish_publish_namespace_request(&mut self, request_id: u64, success: bool) {
        if let Some(namespace) = self.publish_namespace_requests.remove(&request_id)
            && !success
        {
            self.published_namespaces.remove(&namespace);
        }
    }

    pub(crate) fn contains_subscribed_namespace_prefix(&self, namespace_prefix: &[String]) -> bool {
        self.subscribed_namespace_prefixes
            .contains(namespace_prefix)
    }

    pub(crate) fn register_subscribe_namespace_request(
        &mut self,
        request_id: u64,
        namespace_prefix: Vec<String>,
    ) {
        self.subscribed_namespace_prefixes
            .insert(namespace_prefix.clone());
        self.subscribe_namespace_requests
            .insert(request_id, namespace_prefix);
    }

    pub(crate) fn finish_subscribe_namespace_request(&mut self, request_id: u64, success: bool) {
        if let Some(namespace_prefix) = self.subscribe_namespace_requests.remove(&request_id)
            && !success
        {
            self.subscribed_namespace_prefixes.remove(&namespace_prefix);
        }
    }

    pub(crate) fn add_publishing_alias(&mut self, track_key: TrackKey, track_alias: u64) {
        self.publishing_track_aliases
            .entry(track_key)
            .or_default()
            .insert(track_alias);
    }

    fn remove_publishing_alias(&mut self, track_key: &TrackKey, track_alias: u64) {
        self.subgroup_states.remove(&track_alias);
        if let Some(aliases) = self.publishing_track_aliases.get_mut(track_key) {
            aliases.remove(&track_alias);
            if aliases.is_empty() {
                self.publishing_track_aliases.remove(track_key);
            }
        }
    }

    pub(crate) fn validate_incoming_subscribe(&self, message: &Subscribe) -> u64 {
        if !self.contains_published_namespace(&message.track_namespace) {
            return 404;
        }
        if self
            .incoming_subscriptions
            .contains_key(&message.request_id)
        {
            return 409;
        }
        0
    }

    pub(crate) fn register_incoming_subscribe(&mut self, message: &Subscribe) {
        self.incoming_subscriptions.insert(
            message.request_id,
            IncomingSubscribeRequest {
                track_key: TrackKey::new(
                    message.track_namespace.clone(),
                    message.track_name.clone(),
                ),
                track_alias: None,
                largest_location: None,
            },
        );
    }

    pub(crate) fn activate_incoming_subscribe(
        &mut self,
        request_id: u64,
        track_alias: u64,
    ) -> Result<ContentExists> {
        let track_key = {
            let entry = self
                .incoming_subscriptions
                .get_mut(&request_id)
                .ok_or_else(|| anyhow!("unknown subscribe request: {request_id}"))?;
            entry.track_alias = Some(track_alias);
            entry.largest_location = self
                .largest_published_locations
                .get(&entry.track_key)
                .copied();
            entry.track_key.clone()
        };
        let content_exists = self.published_content(&track_key);
        self.add_publishing_alias(track_key, track_alias);
        Ok(content_exists)
    }

    pub(crate) fn remove_incoming_subscribe(&mut self, request_id: u64) -> Option<u64> {
        let removed = self.incoming_subscriptions.remove(&request_id)?;
        let track_alias = removed.track_alias?;
        self.remove_publishing_alias(&removed.track_key, track_alias);
        Some(track_alias)
    }

    pub(crate) fn get_track_subscribers(
        &self,
        namespace: Vec<String>,
        track_name: String,
    ) -> Vec<u64> {
        self.publishing_track_aliases
            .get(&TrackKey::new(namespace, track_name))
            .map(|aliases| aliases.iter().copied().collect())
            .unwrap_or_default()
    }

    pub(crate) fn record_published_object(&mut self, track_alias: u64, location: Location) {
        let Some(track_key) = self
            .publishing_track_aliases
            .iter()
            .find(|(_, aliases)| aliases.contains(&track_alias))
            .map(|(track_key, _)| track_key.clone())
        else {
            return;
        };
        let largest = self
            .largest_published_locations
            .entry(track_key)
            .or_insert(location);
        *largest = (*largest).max(location);
    }

    fn published_content(&self, track_key: &TrackKey) -> ContentExists {
        match self.largest_published_locations.get(track_key) {
            Some(location) => ContentExists::True {
                location: *location,
            },
            None => ContentExists::False,
        }
    }

    pub(crate) fn accept_incoming_track_status(
        &mut self,
        message: &TrackStatus,
    ) -> Result<(), RequestRejection> {
        if !self.contains_published_namespace(&message.track_namespace) {
            return Err(RequestRejection::TrackDoesNotExist);
        }
        self.incoming_track_statuses.insert(
            message.request_id,
            IncomingTrackStatusRequest {
                track_key: TrackKey::new(
                    message.track_namespace.clone(),
                    message.track_name.clone(),
                ),
                group_order: message.group_order,
            },
        );
        Ok(())
    }

    pub(crate) fn answer_incoming_track_status(
        &mut self,
        request_id: u64,
    ) -> Result<TrackStatusOk> {
        let request = self
            .incoming_track_statuses
            .remove(&request_id)
            .ok_or_else(|| anyhow!("unknown track status request: {request_id}"))?;
        Ok(TrackStatusOk {
            request_id,
            track_alias: 0,
            expires: 0,
            group_order: request.group_order.delivered(),
            content_exists: self.published_content(&request.track_key),
            delivery_timeout: None,
            max_duration: None,
        })
    }

    pub(crate) fn reject_incoming_track_status(&mut self, request_id: u64) -> Result<()> {
        self.incoming_track_statuses
            .remove(&request_id)
            .map(|_| ())
            .ok_or_else(|| anyhow!("unknown track status request: {request_id}"))
    }

    pub(crate) fn accept_incoming_fetch(
        &mut self,
        fetch: &Fetch,
    ) -> Result<IncomingFetchRequest, RequestRejection> {
        let target = self.fetch_target(&fetch.fetch_params)?;
        if !self.contains_published_namespace(&target.track_key.namespace) {
            return Err(RequestRejection::TrackDoesNotExist);
        }
        let range = FetchRange::resolve(
            target.start,
            target.requested_end,
            self.largest_published_locations
                .get(&target.track_key)
                .copied(),
        )?;
        let request = IncomingFetchRequest {
            track_key: target.track_key,
            group_order: fetch.group_order.delivered(),
            range,
        };
        self.incoming_fetches
            .insert(fetch.request_id, request.clone());
        Ok(request)
    }

    fn fetch_target(&self, fetch_params: &FetchParams) -> Result<FetchTarget, RequestRejection> {
        match fetch_params {
            FetchParams::Standalone {
                track_namespace,
                track_name,
                start_location,
                end_location,
            } => Ok(FetchTarget {
                track_key: TrackKey::new(track_namespace.clone(), track_name.clone()),
                start: *start_location,
                requested_end: *end_location,
            }),
            FetchParams::RelativeJoining {
                joining_request_id,
                joining_start,
            } => self.joining_fetch_target(*joining_request_id, |largest| {
                largest.group_id.saturating_sub(*joining_start)
            }),
            FetchParams::AbsoluteJoining {
                joining_request_id,
                joining_start,
            } => self.joining_fetch_target(*joining_request_id, |_| *joining_start),
        }
    }

    /// draft-14 §9.16.2.1: a Joining Fetch starts at Object 0 of a group and
    /// ends right after the Largest Location of the joined subscription.
    fn joining_fetch_target(
        &self,
        joining_request_id: u64,
        start_group: impl FnOnce(Location) -> u64,
    ) -> Result<FetchTarget, RequestRejection> {
        let subscription = self
            .incoming_subscriptions
            .get(&joining_request_id)
            .filter(|subscription| subscription.track_alias.is_some())
            .ok_or(RequestRejection::InvalidJoiningRequestId)?;
        let largest = subscription
            .largest_location
            .ok_or(RequestRejection::InvalidRange)?;
        Ok(FetchTarget {
            track_key: subscription.track_key.clone(),
            start: Location {
                group_id: start_group(largest),
                object_id: 0,
            },
            requested_end: location_after(largest),
        })
    }

    pub(crate) fn answer_incoming_fetch(&self, request_id: u64) -> Result<FetchOk> {
        let request = self
            .incoming_fetches
            .get(&request_id)
            .ok_or_else(|| anyhow!("unknown fetch request: {request_id}"))?;
        Ok(FetchOk {
            request_id,
            group_order: request.group_order,
            end_of_track: false,
            end_location: request.range.end,
            max_cache_duration: None,
        })
    }

    pub(crate) fn contains_incoming_fetch(&self, request_id: u64) -> bool {
        self.incoming_fetches.contains_key(&request_id)
    }

    pub(crate) fn remove_incoming_fetch(
        &mut self,
        request_id: u64,
    ) -> Result<IncomingFetchRequest> {
        self.incoming_fetches
            .remove(&request_id)
            .ok_or_else(|| anyhow!("unknown fetch request: {request_id}"))
    }

    fn subgroup_state_entry(&mut self, track_alias: u64) -> &mut SubgroupState {
        self.subgroup_states
            .entry(track_alias)
            .or_insert_with(|| SubgroupState::with_track(track_alias))
    }

    pub(crate) fn current_subgroup_state(&mut self, track_alias: u64) -> SubgroupState {
        self.subgroup_state_entry(track_alias).clone()
    }

    pub(crate) fn mark_subgroup_header_sent(&mut self, track_alias: u64) {
        self.subgroup_state_entry(track_alias).mark_header_sent();
    }

    pub(crate) fn increment_subgroup_object(&mut self, track_alias: u64) {
        self.subgroup_state_entry(track_alias).increment_object_id();
    }

    pub(crate) fn reset_subgroup_state(&mut self, track_alias: u64) {
        self.subgroup_states.remove(&track_alias);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::incoming_fetch::location;
    use moqt::wire::FilterType;

    const NAMESPACE: &str = "live";
    const TRACK: &str = "video";

    fn subscribe(request_id: u64, group_order: GroupOrder) -> Subscribe {
        Subscribe {
            request_id,
            track_namespace: vec![NAMESPACE.to_string()],
            track_name: TRACK.to_string(),
            subscriber_priority: 0,
            group_order,
            forward: true,
            filter_type: FilterType::LargestObject,
            authorization_tokens: vec![],
            delivery_timeout: None,
        }
    }

    fn publishing_state() -> ClientState {
        let mut state = ClientState::default();
        state.register_publish_namespace_request(0, vec![NAMESPACE.to_string()]);
        state
    }

    fn answered_subscription(state: &mut ClientState, request_id: u64) -> (u64, ContentExists) {
        let track_alias = request_id + 100;
        state.register_incoming_subscribe(&subscribe(request_id, GroupOrder::Ascending));
        let content_exists = state
            .activate_incoming_subscribe(request_id, track_alias)
            .unwrap();
        (track_alias, content_exists)
    }

    fn fetch(request_id: u64, group_order: GroupOrder, fetch_params: FetchParams) -> Fetch {
        Fetch {
            request_id,
            subscriber_priority: 0,
            group_order,
            fetch_params,
            authorization_tokens: vec![],
        }
    }

    fn standalone_fetch(request_id: u64, start: Location, end: Location) -> Fetch {
        fetch(
            request_id,
            GroupOrder::Publisher,
            FetchParams::Standalone {
                track_namespace: vec![NAMESPACE.to_string()],
                track_name: TRACK.to_string(),
                start_location: start,
                end_location: end,
            },
        )
    }

    fn relative_joining_fetch(
        request_id: u64,
        joining_request_id: u64,
        joining_start: u64,
    ) -> Fetch {
        fetch(
            request_id,
            GroupOrder::Ascending,
            FetchParams::RelativeJoining {
                joining_request_id,
                joining_start,
            },
        )
    }

    fn state_with_largest(largest: Location) -> ClientState {
        let mut state = publishing_state();
        let (track_alias, _) = answered_subscription(&mut state, 2);
        state.record_published_object(track_alias, largest);
        state
    }

    fn answer_track_status(state: &mut ClientState, request: &TrackStatus) -> TrackStatusOk {
        state.accept_incoming_track_status(request).unwrap();
        state
            .answer_incoming_track_status(request.request_id)
            .unwrap()
    }

    fn track_key() -> TrackKey {
        TrackKey::new(vec![NAMESPACE.to_string()], TRACK.to_string())
    }

    #[test]
    fn objects_sent_on_a_publish_alias_are_the_published_content_of_its_track() {
        // Arrange
        let mut state = publishing_state();
        state.add_publishing_alias(track_key(), 7);

        // Act
        state.record_published_object(7, location(3, 1));

        // Assert
        assert_eq!(
            state.published_content(&track_key()),
            ContentExists::True {
                location: location(3, 1)
            }
        );
    }

    #[test]
    fn track_status_ok_reports_the_largest_object_sent_on_the_track() {
        // Arrange
        let mut state = publishing_state();
        let (track_alias, _) = answered_subscription(&mut state, 2);
        state.record_published_object(track_alias, location(3, 1));
        state.record_published_object(track_alias, location(2, 9));

        // Act
        let track_status_ok = answer_track_status(&mut state, &subscribe(4, GroupOrder::Ascending));

        // Assert
        assert_eq!(track_status_ok.track_alias, 0);
        assert_eq!(
            track_status_ok.content_exists,
            ContentExists::True {
                location: location(3, 1)
            }
        );
    }

    #[test]
    fn track_status_ok_reports_no_content_before_an_object_is_sent() {
        // Arrange
        let mut state = publishing_state();
        answered_subscription(&mut state, 2);

        // Act
        let track_status_ok = answer_track_status(&mut state, &subscribe(4, GroupOrder::Ascending));

        // Assert
        assert_eq!(track_status_ok.content_exists, ContentExists::False);
    }

    #[test]
    fn largest_location_outlives_the_subscription_it_was_sent_on() {
        // Arrange
        let mut state = publishing_state();
        let (track_alias, _) = answered_subscription(&mut state, 2);
        state.record_published_object(track_alias, location(7, 0));
        state.remove_incoming_subscribe(2);

        // Act
        let track_status_ok = answer_track_status(&mut state, &subscribe(4, GroupOrder::Ascending));

        // Assert
        assert_eq!(
            track_status_ok.content_exists,
            ContentExists::True {
                location: location(7, 0)
            }
        );
    }

    #[test]
    fn track_status_ok_states_ascending_when_the_order_is_left_to_the_publisher() {
        // Arrange
        let mut state = publishing_state();

        // Act
        let track_status_ok = answer_track_status(&mut state, &subscribe(4, GroupOrder::Publisher));

        // Assert
        assert_eq!(track_status_ok.group_order, GroupOrder::Ascending);
    }

    #[test]
    fn track_status_for_an_unpublished_namespace_is_rejected() {
        // Arrange
        let mut state = ClientState::default();

        // Act
        let accepted = state.accept_incoming_track_status(&subscribe(4, GroupOrder::Ascending));

        // Assert
        assert_eq!(accepted, Err(RequestRejection::TrackDoesNotExist));
    }

    #[test]
    fn subscribe_ok_reports_the_largest_object_already_sent_on_the_track() {
        // Arrange
        let mut state = state_with_largest(location(7, 3));

        // Act
        let (_, content_exists) = answered_subscription(&mut state, 4);

        // Assert
        assert_eq!(
            content_exists,
            ContentExists::True {
                location: location(7, 3)
            }
        );
    }

    #[test]
    fn subscribe_ok_reports_no_content_before_an_object_is_sent() {
        // Arrange
        let mut state = publishing_state();

        // Act
        let (_, content_exists) = answered_subscription(&mut state, 2);

        // Assert
        assert_eq!(content_exists, ContentExists::False);
    }

    #[test]
    fn fetch_ok_ends_after_the_largest_object_when_the_range_reaches_past_it() {
        // Arrange
        let mut state = state_with_largest(location(5, 3));
        state
            .accept_incoming_fetch(&standalone_fetch(6, location(4, 0), location(9, 0)))
            .unwrap();

        // Act
        let fetch_ok = state.answer_incoming_fetch(6).unwrap();

        // Assert
        assert_eq!(fetch_ok.end_location, location(5, 4));
        assert_eq!(fetch_ok.group_order, GroupOrder::Ascending);
    }

    #[test]
    fn a_fetch_for_an_unpublished_namespace_is_rejected() {
        // Arrange
        let mut state = ClientState::default();

        // Act
        let accepted =
            state.accept_incoming_fetch(&standalone_fetch(6, location(0, 0), location(1, 0)));

        // Assert
        assert_eq!(accepted.unwrap_err(), RequestRejection::TrackDoesNotExist);
    }

    #[test]
    fn a_relative_joining_fetch_covers_the_groups_before_the_joined_largest_location() {
        // Arrange
        let mut state = state_with_largest(location(7, 3));
        let (track_alias, _) = answered_subscription(&mut state, 4);
        state.record_published_object(track_alias, location(8, 0));

        // Act
        let request = state
            .accept_incoming_fetch(&relative_joining_fetch(6, 4, 2))
            .unwrap();

        // Assert
        assert_eq!(request.track_key.name, TRACK);
        assert_eq!(request.range.start, location(5, 0));
        assert_eq!(request.range.end, location(7, 4));
    }

    #[test]
    fn a_joining_fetch_for_an_unknown_subscription_is_rejected() {
        // Arrange
        let mut state = state_with_largest(location(5, 3));

        // Act
        let accepted = state.accept_incoming_fetch(&relative_joining_fetch(6, 40, 1));

        // Assert
        assert_eq!(
            accepted.unwrap_err(),
            RequestRejection::InvalidJoiningRequestId
        );
    }

    #[test]
    fn a_joining_fetch_for_an_unanswered_subscription_is_rejected() {
        // Arrange
        let mut state = state_with_largest(location(5, 3));
        state.register_incoming_subscribe(&subscribe(8, GroupOrder::Ascending));

        // Act
        let accepted = state.accept_incoming_fetch(&relative_joining_fetch(6, 8, 1));

        // Assert
        assert_eq!(
            accepted.unwrap_err(),
            RequestRejection::InvalidJoiningRequestId
        );
    }

    #[test]
    fn a_joining_fetch_for_a_subscription_answered_without_content_is_an_invalid_range() {
        // Arrange
        let mut state = state_with_largest(location(5, 3));

        // Act
        let accepted = state.accept_incoming_fetch(&relative_joining_fetch(6, 2, 1));

        // Assert
        assert_eq!(accepted.unwrap_err(), RequestRejection::InvalidRange);
    }
}
