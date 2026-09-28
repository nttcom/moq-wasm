use std::sync::Arc;

use crate::modules::relay::cache::{cached_object::CachedObject, track_cache::NextObject};

use super::{FetchInterrupted, TrackCache, location};

/// Walks a FETCH range in delivery order, one object per `next` call, so the
/// caller can send each object as soon as the cache can prove its position.
pub(crate) struct FetchCursor<'a> {
    cache: &'a TrackCache,
    groups: std::vec::IntoIter<u64>,
    start: moqt::Location,
    end: moqt::Location,
    current: Option<GroupCursor>,
}

struct GroupCursor {
    group_id: u64,
    fully_known: bool,
    frontier: Option<u64>,
    next_object_id: u64,
    end_exclusive: Option<u64>,
}

impl GroupCursor {
    fn reached_end(&self, object_id: u64) -> bool {
        self.end_exclusive
            .is_some_and(|end_object_id| object_id >= end_object_id)
    }

    fn is_known(&self, object_id: u64) -> bool {
        self.fully_known || self.frontier.is_some_and(|frontier| object_id < frontier)
    }
}

impl<'a> FetchCursor<'a> {
    pub(crate) fn new(
        cache: &'a TrackCache,
        start: moqt::Location,
        end: moqt::Location,
        group_order: moqt::GroupOrder,
    ) -> Self {
        let mut groups = cache.read().groups_in_range(start.group_id, end.group_id);
        if matches!(group_order, moqt::GroupOrder::Descending) {
            groups.reverse();
        }
        Self {
            cache,
            groups: groups.into_iter(),
            start,
            end,
            current: None,
        }
    }

    pub(crate) async fn next(&mut self) -> Result<Option<Arc<CachedObject>>, FetchInterrupted> {
        loop {
            let Some(group) = self.current.as_mut() else {
                let Some(group_id) = self.groups.next() else {
                    return Ok(None);
                };
                self.current = Some(self.enter_group(group_id));
                continue;
            };
            if group.reached_end(group.next_object_id) {
                self.current = None;
                continue;
            }
            let in_known = group.is_known(group.next_object_id);
            let found = if in_known {
                match self
                    .cache
                    .read()
                    .next_group_object(group.group_id, group.next_object_id)
                {
                    Some(object) => NextObject::Object(object),
                    None => NextObject::Finished,
                }
            } else {
                self.cache
                    .next_group_object_or_wait(group.group_id, group.next_object_id)
                    .await?
            };
            match found {
                NextObject::Object(object) => {
                    let object_id = object.location.object_id;
                    group.next_object_id = object_id.saturating_add(1);
                    if group.reached_end(object_id) {
                        self.current = None;
                        continue;
                    }
                    return Ok(Some(object));
                }
                NextObject::Aborted => return Err(FetchInterrupted::Incomplete),
                NextObject::Finished if group.fully_known => self.current = None,
                NextObject::Finished if in_known => {
                    group.next_object_id = group.frontier.unwrap_or(group.next_object_id);
                }
                NextObject::Finished => self.current = None,
            }
        }
    }

    fn enter_group(&self, group_id: u64) -> GroupCursor {
        let known_prefix_end = self
            .cache
            .read()
            .known_ranges
            .end_of_range_containing(location(group_id, 0));
        let fully_known = matches!(known_prefix_end, Some(end) if end.group_id > group_id);
        let frontier = match known_prefix_end {
            Some(end) if end.group_id == group_id => Some(end.object_id),
            _ => None,
        };
        let next_object_id = if group_id == self.start.group_id {
            self.start.object_id
        } else {
            0
        };
        let end_exclusive = if group_id == self.end.group_id && self.end.object_id != 0 {
            Some(self.end.object_id)
        } else {
            None
        };
        GroupCursor {
            group_id,
            fully_known,
            frontier,
            next_object_id,
            end_exclusive,
        }
    }
}
