use std::collections::{HashMap, HashSet};

type ParticipantId = usize;
type Namespace = String;

pub(crate) struct NamespaceTable {
    pub(crate) table: tokio::sync::Mutex<HashMap<Namespace, HashSet<ParticipantId>>>,
}

impl NamespaceTable {
    pub(crate) async fn add_namespace(&self, namespace: Namespace) {
        let set = HashSet::new();
        self.table.lock().await.insert(namespace, set);
    }

    pub(crate) async fn add_id(&mut self, namespace: Namespace, id: ParticipantId) -> bool {
        let mut map = self.table.lock().await;
        let set = map.get_mut(&namespace);
        if let Some(set) = set {
            set.insert(id);
            true
        } else {
            tracing::error!("Namespace not found");
            false
        }
    }

    pub(crate) async fn remove_namespace(&self, namespace: Namespace) {
        self.table.lock().await.remove(&namespace);
    }

    pub(crate) async fn remove_id(&mut self, namespace: Namespace, id: ParticipantId) -> bool {
        let mut map = self.table.lock().await;
        let set = map.get_mut(&namespace);
        if let Some(set) = set {
            set.remove(&id);
            true
        } else {
            tracing::error!("Namespace not found");
            false
        }
    }
}
