//! Spawning and clocks that work both natively, on tokio, and on wasm32 in a
//! browser, where `spawn` needs no `Send` and `std::time` panics.
pub use crate::modules::executor::JoinHandle;
pub use crate::modules::executor::now;
pub use crate::modules::executor::spawn;
pub use crate::modules::executor::system_time;
