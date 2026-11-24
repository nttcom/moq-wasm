//! Records, tracks and the robot side of robot teleoperation, cloud
//! policies and data collection over MoQT
//! (`docs/architecture/robotics/architecture.md`).

mod arbiter;
mod driver;
mod record;
mod robot;
mod tracks;

pub use arbiter::{Applied, Arbiter, ArbiterConfig, HoldReason, Output};
pub use driver::Driver;
pub use record::{
    Action, Control, ControlReason, Episode, Record, State, decode_object, encode_object,
};
pub use robot::{Command, Robot};
pub use tracks::TrackKind;
