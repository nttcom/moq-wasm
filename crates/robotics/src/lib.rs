//! Records and tracks for robot teleoperation, cloud policies and data
//! collection over MoQT, and the robot's arbitration
//! (`docs/architecture/robotics/architecture.md`).

mod arbiter;
mod record;
mod tracks;

pub use arbiter::{Applied, Arbiter, ArbiterConfig, HoldReason, Output};
pub use record::{
    Action, Control, ControlReason, Episode, Record, State, decode_object, encode_object,
};
pub use tracks::TrackKind;
