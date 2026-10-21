use std::{
    path::PathBuf,
    sync::atomic::{AtomicUsize, Ordering},
};

use crate::{location::Location, store::RegistryStore};

static NEXT_FILE: AtomicUsize = AtomicUsize::new(0);

pub struct TemporaryRegistryFile {
    path: PathBuf,
}

impl TemporaryRegistryFile {
    pub fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "app-registry-{}-{}.json",
            std::process::id(),
            NEXT_FILE.fetch_add(1, Ordering::Relaxed)
        ));
        Self { path }
    }

    pub fn store(&self) -> RegistryStore {
        RegistryStore::new(Location::File(self.path.clone())).unwrap()
    }

    pub fn write(&self, contents: &str) {
        std::fs::write(&self.path, contents).unwrap();
    }

    pub fn put(&self, body: &serde_json::Value) {
        self.write(&body.to_string());
    }

    pub fn body(&self) -> serde_json::Value {
        serde_json::from_slice(&std::fs::read(&self.path).unwrap()).unwrap()
    }
}

impl Default for TemporaryRegistryFile {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for TemporaryRegistryFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}
