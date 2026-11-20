mod app;
mod cloud_storage;
mod file;
mod location;
mod store;
#[cfg(test)]
mod test_support;
#[cfg(any(test, feature = "testing"))]
pub mod testing;

pub use app::{App, Apps, Phone, is_e164};
pub use location::{Location, LocationVariables};
pub use store::RegistryStore;
