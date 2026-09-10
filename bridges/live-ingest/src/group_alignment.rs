use std::{collections::BTreeMap, sync::Mutex};

/// A rendition that missed a source keyframe, and so has no id for that
/// presentation time, can only find ids that are still recent.
const REMEMBERED_KEYFRAMES: usize = 64;

/// draft-ietf-moq-cmsf-01 §3.2: the tracks of a switching set start their
/// groups at the same presentation times with the same group ids. The source
/// track assigns an id to each of its keyframes here, and renditions, whose
/// keyframes the transcoder forces onto the same presentation times, look the
/// id up instead of numbering on their own.
pub(crate) struct GroupAlignment {
    inner: Mutex<Inner>,
}

struct Inner {
    next_group_id: u64,
    by_presentation_time: BTreeMap<u64, u64>,
}

impl GroupAlignment {
    pub(crate) fn new(first_group_id: u64) -> Self {
        Self {
            inner: Mutex::new(Inner {
                next_group_id: first_group_id,
                by_presentation_time: BTreeMap::new(),
            }),
        }
    }

    pub(crate) fn keyframe(&self, presentation_us: u64) -> u64 {
        let mut inner = self.inner.lock().expect("group alignment lock");
        if let Some(group_id) = inner.by_presentation_time.get(&presentation_us) {
            return *group_id;
        }
        let group_id = inner.next_group_id;
        inner.next_group_id += 1;
        inner.by_presentation_time.insert(presentation_us, group_id);
        while inner.by_presentation_time.len() > REMEMBERED_KEYFRAMES {
            inner.by_presentation_time.pop_first();
        }
        group_id
    }

    pub(crate) fn aligned(&self, presentation_us: u64) -> Option<u64> {
        self.inner
            .lock()
            .expect("group alignment lock")
            .by_presentation_time
            .get(&presentation_us)
            .copied()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numbers_source_keyframes_consecutively_and_once() {
        // Arrange
        let alignment = GroupAlignment::new(700);

        // Act
        let first = alignment.keyframe(0);
        let second = alignment.keyframe(2_000_000);
        let repeated = alignment.keyframe(0);

        // Assert
        assert_eq!((first, second, repeated), (700, 701, 700));
    }

    #[test]
    fn renditions_only_find_ids_the_source_assigned() {
        // Arrange
        let alignment = GroupAlignment::new(700);
        alignment.keyframe(2_000_000);

        // Act / Assert
        assert_eq!(alignment.aligned(2_000_000), Some(700));
        assert_eq!(alignment.aligned(2_000_001), None);
    }

    #[test]
    fn forgets_the_oldest_keyframes_past_the_window() {
        // Arrange
        let alignment = GroupAlignment::new(0);

        // Act
        for index in 0..=REMEMBERED_KEYFRAMES as u64 {
            alignment.keyframe(index * 2_000_000);
        }

        // Assert
        assert_eq!(alignment.aligned(0), None);
        assert_eq!(alignment.aligned(2_000_000), Some(1));
    }
}
