use std::collections::BTreeSet;

use tokio::time::Instant;

use crate::modules::data_plane::cache::{cached_object::CachedObject, disk_cache::StoredPayload};

/// The cached objects eviction has to look at, so that it never walks the
/// whole track. An entry can outlive its object, or name a location that
/// now holds another object; the caller checks the object before acting.
#[derive(Default)]
pub(super) struct EvictionQueues {
    /// Objects not yet past the memory TTL, by `received_at`.
    arrivals: BTreeSet<(Instant, moqt::object::Location)>,
    /// By `CachedObject::expiry`.
    expiries: BTreeSet<(Instant, moqt::object::Location)>,
    /// The objects stored in each disk segment, so the ones of a deleted
    /// segment can be found. A segment leaves once deleted, so there are
    /// about `RETENTION_LIMIT / SEGMENT_SPAN` of them.
    segments: Vec<SegmentObjects>,
}

struct SegmentObjects {
    /// Any payload in the segment; they all share its deleted flag.
    payload: StoredPayload,
    locations: Vec<moqt::object::Location>,
}

impl EvictionQueues {
    pub(super) fn track(&mut self, object: &CachedObject) {
        let at = object.object.location;
        self.arrivals.insert((object.received_at, at));
        if let Some(expiry) = object.expiry() {
            self.expiries.insert((expiry, at));
        }
    }

    /// Re-enters `object` into the arrivals, so a payload stored after the
    /// memory TTL still leaves memory.
    pub(super) fn track_stored(&mut self, object: &CachedObject, stored: &StoredPayload) {
        self.track(object);
        let at = object.object.location;
        match self
            .segments
            .iter_mut()
            .rev()
            .find(|segment| segment.payload.is_in_segment_of(stored))
        {
            Some(segment) => segment.locations.push(at),
            None => self.segments.push(SegmentObjects {
                payload: stored.clone(),
                locations: vec![at],
            }),
        }
    }

    pub(super) fn take_arrived_before(
        &mut self,
        deadline: Instant,
    ) -> Vec<(Instant, moqt::object::Location)> {
        let mut arrived = Vec::new();
        while let Some(&entry) = self.arrivals.first()
            && entry.0 < deadline
        {
            self.arrivals.pop_first();
            arrived.push(entry);
        }
        arrived
    }

    pub(super) fn return_arrival(&mut self, entry: (Instant, moqt::object::Location)) {
        self.arrivals.insert(entry);
    }

    /// Takes the locations whose expiry is at or before `now`, and those of
    /// the deleted segments.
    pub(super) fn take_expired(&mut self, now: Instant) -> Vec<moqt::object::Location> {
        let mut expired = Vec::new();
        while let Some(&(expiry, at)) = self.expiries.first()
            && expiry <= now
        {
            self.expiries.pop_first();
            expired.push(at);
        }
        self.segments.retain_mut(|segment| {
            let lost = segment.payload.is_lost();
            if lost {
                expired.append(&mut segment.locations);
            }
            !lost
        });
        expired
    }

    /// The locations `take_expired(now)` would take, without taking them.
    pub(super) fn expired_candidates(
        &self,
        now: Instant,
    ) -> impl Iterator<Item = moqt::object::Location> {
        self.expiries
            .iter()
            .take_while(move |(expiry, _)| *expiry <= now)
            .map(|(_, at)| *at)
            .chain(
                self.segments
                    .iter()
                    .filter(|segment| segment.payload.is_lost())
                    .flat_map(|segment| segment.locations.iter().copied()),
            )
    }

    /// The track's segments were purged along with its stored objects.
    pub(super) fn forget_segments(&mut self) {
        self.segments.clear();
    }
}
