use std::{sync::Arc, time::Duration};

use tokio::sync::Notify;

use super::{new_group_delay_task::NewGroupDelayTask, object_numbering::Placement};

/// A group this old is long enough to be ended on request
/// (NEW_GROUP_REQUEST), as is one that holds `MIN_GROUP_OBJECTS` objects.
pub const MIN_GROUP_DURATION: Duration = Duration::from_secs(2);
/// A group of this many objects is long enough to be ended on request, as is
/// one `MIN_GROUP_DURATION` old.
pub const MIN_GROUP_OBJECTS: u64 = 60;

/// The requests for a new group (NEW_GROUP_REQUEST, draft-18 §10.2.13) of a
/// track with dynamic groups. A request is passed on at once when the open
/// group is `MIN_GROUP_DURATION` old or holds `MIN_GROUP_OBJECTS` objects,
/// and otherwise as soon as the group reaches either: the delay §10.2.13
/// allows an Original Publisher for a minimum group duration.
pub(super) struct NewGroupRequests {
    /// Notified to have the track's writer start a new group.
    requested: Arc<Notify>,
    open_group: Option<OpenGroup>,
    /// The request for a group after the open one, until that group begins.
    outstanding: Option<Outstanding>,
}

struct OpenGroup {
    began_at: Duration,
    objects: u64,
}

enum Outstanding {
    PassedOn,
    Delayed(NewGroupDelayTask),
}

impl NewGroupRequests {
    pub(super) fn new(requested: Arc<Notify>) -> Self {
        Self {
            requested,
            open_group: None,
            outstanding: None,
        }
    }

    /// Call at `now` (`executor::now`) for a request the open group does not
    /// answer. A request that follows another for the same group adds nothing.
    pub(super) fn request(&mut self, now: Duration) {
        if self.outstanding.is_some() {
            return;
        }
        let delay = self
            .open_group
            .as_ref()
            .map_or(Duration::ZERO, |group| group.delay(now));
        self.outstanding = Some(if delay.is_zero() {
            self.requested.notify_one();
            Outstanding::PassedOn
        } else {
            Outstanding::Delayed(NewGroupDelayTask::run(self.requested.clone(), delay))
        });
    }

    /// Call at `now` (`executor::now`) for every object placed in the track.
    pub(super) fn object_placed(&mut self, placement: Placement, now: Duration) {
        if placement.starts_group {
            self.open_group = Some(OpenGroup {
                began_at: now,
                objects: 1,
            });
            self.outstanding = None;
            return;
        }
        let Some(group) = &mut self.open_group else {
            return;
        };
        group.objects = placement.location.object_id + 1;
        if group.objects >= MIN_GROUP_OBJECTS
            && let Some(Outstanding::Delayed(task)) = &self.outstanding
        {
            task.end_delay();
        }
    }
}

impl OpenGroup {
    /// How long a request waits for the group to be long enough.
    fn delay(&self, now: Duration) -> Duration {
        if self.objects >= MIN_GROUP_OBJECTS {
            return Duration::ZERO;
        }
        MIN_GROUP_DURATION.saturating_sub(now.saturating_sub(self.began_at))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::modules::test_support::{is_notified, location, notified_within};

    const GROUP_ID: u64 = 7;
    const BEGAN_AT: Duration = Duration::from_secs(10);

    fn placed(group_id: u64, object_id: u64) -> Placement {
        Placement {
            location: location(group_id, object_id),
            starts_group: object_id == 0,
        }
    }

    /// The requests of a track whose group `GROUP_ID` began at `BEGAN_AT` and
    /// holds `objects` objects, and what they notify.
    fn requests_in_group_of(objects: u64) -> (NewGroupRequests, Arc<Notify>) {
        let requested = Arc::new(Notify::new());
        let mut requests = NewGroupRequests::new(requested.clone());
        for object_id in 0..objects {
            requests.object_placed(placed(GROUP_ID, object_id), BEGAN_AT);
        }
        (requests, requested)
    }

    #[tokio::test(start_paused = true)]
    async fn a_request_before_the_first_group_is_passed_on_at_once() {
        // Arrange
        let (mut requests, requested) = requests_in_group_of(0);

        // Act
        requests.request(BEGAN_AT);

        // Assert
        assert!(is_notified(&requested));
    }

    #[tokio::test(start_paused = true)]
    async fn a_request_in_a_group_of_the_minimum_duration_is_passed_on_at_once() {
        // Arrange
        let (mut requests, requested) = requests_in_group_of(1);

        // Act
        requests.request(BEGAN_AT + MIN_GROUP_DURATION);

        // Assert
        assert!(is_notified(&requested));
    }

    #[tokio::test(start_paused = true)]
    async fn a_request_in_a_group_of_the_minimum_objects_is_passed_on_at_once() {
        // Arrange
        let (mut requests, requested) = requests_in_group_of(MIN_GROUP_OBJECTS);

        // Act
        requests.request(BEGAN_AT);

        // Assert
        assert!(is_notified(&requested));
    }

    #[tokio::test(start_paused = true)]
    async fn a_request_in_a_short_group_is_passed_on_when_the_group_is_of_the_minimum_duration() {
        // Arrange
        let (mut requests, requested) = requests_in_group_of(MIN_GROUP_OBJECTS - 1);
        let age = Duration::from_millis(500);
        let remaining = MIN_GROUP_DURATION - age;

        // Act
        requests.request(BEGAN_AT + age);
        let before = notified_within(&requested, remaining - Duration::from_millis(1)).await;
        let after = notified_within(&requested, MIN_GROUP_DURATION).await;

        // Assert
        assert!(!before);
        assert!(after);
    }

    #[tokio::test(start_paused = true)]
    async fn a_request_in_a_short_group_is_passed_on_when_the_group_holds_the_minimum_objects() {
        // Arrange
        let (mut requests, requested) = requests_in_group_of(MIN_GROUP_OBJECTS - 2);
        requests.request(BEGAN_AT);

        // Act
        requests.object_placed(placed(GROUP_ID, MIN_GROUP_OBJECTS - 2), BEGAN_AT);
        let one_object_short = notified_within(&requested, Duration::from_millis(1)).await;
        requests.object_placed(placed(GROUP_ID, MIN_GROUP_OBJECTS - 1), BEGAN_AT);
        let long_enough = notified_within(&requested, Duration::from_millis(1)).await;

        // Assert
        assert!(!one_object_short);
        assert!(long_enough);
    }

    #[tokio::test(start_paused = true)]
    async fn a_request_held_back_is_answered_by_the_next_group() {
        // Arrange
        let (mut requests, requested) = requests_in_group_of(1);
        requests.request(BEGAN_AT);

        // Act: the writer starts a group on its own
        requests.object_placed(placed(GROUP_ID + 1, 0), BEGAN_AT + Duration::from_secs(1));
        let notified = notified_within(&requested, MIN_GROUP_DURATION * 2).await;

        // Assert
        assert!(!notified);
    }

    #[tokio::test(start_paused = true)]
    async fn requests_for_one_group_are_passed_on_once() {
        // Arrange: a request was passed on, and the group it asks for has not begun
        let (mut requests, requested) = requests_in_group_of(MIN_GROUP_OBJECTS);
        requests.request(BEGAN_AT);
        let first = is_notified(&requested);

        // Act
        requests.request(BEGAN_AT);
        let second = is_notified(&requested);
        requests.object_placed(placed(GROUP_ID + 1, 0), BEGAN_AT);
        requests.request(BEGAN_AT + MIN_GROUP_DURATION);
        let for_next_group = is_notified(&requested);

        // Assert
        assert!(first);
        assert!(!second);
        assert!(for_next_group);
    }
}
