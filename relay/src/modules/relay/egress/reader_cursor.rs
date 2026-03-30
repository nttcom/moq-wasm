use std::collections::BTreeSet;

use crate::modules::{enums::GroupOrder, relay::types::CacheLocation};

#[derive(Clone, Debug)]
pub(crate) struct ReaderCursor {
    next_group_id: u64,
    next_index: u64,
    end_group: Option<u64>,
    group_order: GroupOrder,
    header_sent_groups: BTreeSet<u64>,
}

impl ReaderCursor {
    pub(crate) fn new(
        start_group_id: u64,
        start_index: u64,
        end_group: Option<u64>,
        group_order: GroupOrder,
    ) -> Self {
        Self {
            next_group_id: start_group_id,
            next_index: start_index,
            end_group,
            group_order,
            header_sent_groups: BTreeSet::new(),
        }
    }

    pub(crate) fn location(&self) -> CacheLocation {
        CacheLocation {
            group_id: self.next_group_id,
            index: self.next_index,
        }
    }

    pub(crate) fn advance_object(&mut self) {
        self.next_index = self.next_index.saturating_add(1);
    }

    pub(crate) fn jump_to_group(&mut self, group_id: u64) {
        self.next_group_id = group_id;
        self.next_index = 0;
    }

    pub(crate) fn mark_header_sent(&mut self, group_id: u64) {
        self.header_sent_groups.insert(group_id);
    }

    pub(crate) fn reset_header_sent(&mut self, group_id: u64) {
        self.header_sent_groups.remove(&group_id);
    }

    pub(crate) fn is_header_sent(&self, group_id: u64) -> bool {
        self.header_sent_groups.contains(&group_id)
    }

    pub(crate) fn group_order(&self) -> &GroupOrder {
        &self.group_order
    }

    pub(crate) fn has_passed_end_range(&self) -> bool {
        let Some(end_group) = self.end_group else {
            return false;
        };

        match self.group_order {
            GroupOrder::Descending => self.next_group_id < end_group,
            GroupOrder::Ascending | GroupOrder::Publisher => self.next_group_id > end_group,
        }
    }
}
