use std::collections::{HashMap, HashSet};

use uuid::Uuid;
type Namespace = String;

pub(crate) struct Tables {
    publishers: tokio::sync::Mutex<HashMap<Namespace, HashSet<Uuid>>>,
    subscribers: tokio::sync::Mutex<HashMap<Namespace, HashSet<Uuid>>>,
    namespace_trackers: tokio::sync::Mutex<HashMap<Namespace, HashSet<Uuid>>>,
}

impl Tables {
    pub(crate) fn new() -> Self {
        Self {
            table: tokio::sync::Mutex::new(HashMap::new()),
        }
    }

    pub(crate) async fn add(&self, namespace: Namespace, uuid: Uuid) {
        let mut table = self.table.lock().await;
        if table.contains_key(&namespace) {
            table.get_mut(&namespace).unwrap().insert(uuid);
        } else {
            let mut set = HashSet::new();
            set.insert(uuid);
            table.insert(namespace, set);
        }
    }

    pub(crate) async fn get_by_namespace(&self, namespace: Namespace) -> HashSet<Uuid> {
        let mut table = self.table.lock().await;
        match table.get_mut(&namespace) {
            Some(set) => set.clone(),
            None => {
                tracing::error!("namespace not found");
                HashSet::new()
            }
        }
    }

    pub(crate) async fn remove(&self, namespace: Namespace, uuid: Uuid) {
        let mut table = self.table.lock().await;
        let set = match table.get_mut(&namespace) {
            Some(v) => v,
            None => {
                tracing::error!("namespace not found");
                return;
            }
        };
        set.remove(&uuid);

        if set.is_empty() {
            tracing::info!("Namespace has no session. Good bye! {}", &namespace);
            table.remove(&namespace);
        }
    }
}
