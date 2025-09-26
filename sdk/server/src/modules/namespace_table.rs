use std::collections::{HashMap, HashSet};

type ParticipantId = usize;
type Namespace = String;

pub(crate) struct NamespaceTable {
    table: HashMap<Namespace, HashSet<ParticipantId>>
}

impl NamespaceTable {
    pub(crate) fn add(id: usize) {

    }
}