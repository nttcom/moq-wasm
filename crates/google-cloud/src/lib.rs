mod cloud_storage;
mod metadata_token;
#[cfg(any(test, feature = "testing"))]
pub mod testing;

pub use cloud_storage::{CloudStorageBucket, Endpoint, Uploaded, Versioned};
pub use metadata_token::{MetadataToken, identity_token};
