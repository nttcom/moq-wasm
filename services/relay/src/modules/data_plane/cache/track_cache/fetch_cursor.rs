use std::sync::Arc;

use tokio::time::Instant;

use crate::modules::{
    data_plane::cache::{cached_object::CachedObject, track_cache::NextObject},
    domain::object_cursor::ObjectCursor,
};

use super::{FetchInterrupted, TrackCache, location};

pub(crate) struct FetchCursor<'a> {
    cache: &'a TrackCache,
    groups: std::vec::IntoIter<u64>,
    start: moqt::object::Location,
    end: moqt::object::Location,
    current: Option<GroupCursor>,
}

struct GroupCursor {
    group_id: u64,
    reopen_count: usize,
    fully_known: bool,
    frontier: Option<u64>,
    next: ObjectCursor,
    end_exclusive: Option<u64>,
}

impl GroupCursor {
    fn reached_end(&self, cursor: ObjectCursor) -> bool {
        cursor.object_id().is_none_or(|object_id| {
            self.end_exclusive
                .is_some_and(|end_object_id| object_id >= end_object_id)
        })
    }

    fn is_known(&self, cursor: ObjectCursor) -> bool {
        self.fully_known
            || self
                .frontier
                .is_some_and(|frontier| cursor < ObjectCursor::At(frontier))
    }
}

impl<'a> FetchCursor<'a> {
    pub(crate) fn new(
        cache: &'a TrackCache,
        start: moqt::object::Location,
        end: moqt::object::Location,
        group_order: moqt::object::GroupOrder,
    ) -> Self {
        let mut groups = cache.read().groups_in_range(start.group_id, end.group_id);
        if matches!(group_order, moqt::object::GroupOrder::Descending) {
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
            if group.reached_end(group.next) {
                self.current = None;
                continue;
            }
            let in_known = group.is_known(group.next);
            let found = if in_known {
                match self
                    .cache
                    .read()
                    .next_group_object(group.group_id, group.next)
                {
                    Some(object) => NextObject::Object(object),
                    None => NextObject::Finished,
                }
            } else {
                self.cache
                    .next_group_object_or_wait(group.group_id, group.reopen_count, group.next)
                    .await?
            };
            match found {
                // draft-14 §9.2.1.3: an expired object must not start to be
                // forwarded, so the FETCH cannot be completed from cache.
                NextObject::Object(object) if object.is_expired(Instant::now()) => {
                    return Err(FetchInterrupted::Incomplete);
                }
                NextObject::Object(object) => {
                    let object_id = object.object.location.object_id;
                    group.next = ObjectCursor::after(object_id);
                    if group.reached_end(ObjectCursor::At(object_id)) {
                        self.current = None;
                        continue;
                    }
                    return Ok(Some(object));
                }
                NextObject::Aborted => return Err(FetchInterrupted::Incomplete),
                // Group reads return only cached objects: FETCH never serves
                // an object whose payload is still arriving.
                NextObject::Partial(_) => unreachable!("a group read returned a partial object"),
                NextObject::Finished if group.fully_known => self.current = None,
                NextObject::Finished if in_known => {
                    group.next = group.frontier.map_or(group.next, ObjectCursor::At);
                }
                NextObject::Finished => self.current = None,
            }
        }
    }

    fn enter_group(&self, group_id: u64) -> GroupCursor {
        let ledger = self.cache.read();
        let reopen_count = ledger.reopen_count_in_group(group_id);
        let known_prefix_end = ledger
            .known_ranges
            .end_of_range_containing(location(group_id, 0));
        let fully_known = matches!(known_prefix_end, Some(end) if end.group_id > group_id);
        let frontier = match known_prefix_end {
            Some(end) if end.group_id == group_id => Some(end.object_id),
            _ => None,
        };
        let next = if group_id == self.start.group_id {
            ObjectCursor::At(self.start.object_id)
        } else {
            ObjectCursor::At(0)
        };
        let end_exclusive = if group_id == self.end.group_id && self.end.object_id != 0 {
            Some(self.end.object_id)
        } else {
            None
        };
        GroupCursor {
            group_id,
            reopen_count,
            fully_known,
            frontier,
            next,
            end_exclusive,
        }
    }
}
