//! Records and tracks for robot teleoperation, cloud policies and data
//! collection over MoQT (`docs/architecture/robotics/architecture.md`).

mod record;
mod tracks;

pub use record::{
    Action, Control, ControlReason, Episode, Record, State, decode_object, encode_object,
};
pub use tracks::TrackKind;
